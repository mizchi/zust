"""Build times of a multi-crate workspace with many dependencies (zustc build compared with cargo).

usage: python3 bench/deps_bench.py [--mids N] [--fns F] [--deps K] [--reps R] [--no-cargo]

Layout (generated in out/depsbench):
  leaf : real crates from cratebench/vendor (fnv / adler2 / hex / itoa / rustc-hash / cfg-if)
  mid  : generated crates m0..m{N-1}. Each depends on 2 to 3 real crates and on up to K earlier mid crates (a DAG).
         Each has F fns, structs and methods, generic fns, and trait impls, and calls the fns of the crates it depends on
  app  : a bin that depends on all the mid crates that nothing else uses
Scenarios:
  cold      everything from nothing
  noop      once more without changing anything
  low-body  changes one fn body of the lowest mid crate m0 (which almost everything depends on transitively)
  low-sig   changes the signature of a pub fn of m0 (the users are fixed too)
  app-body  changes app's main
cargo is a workspace of path dependencies (dev profile: incremental on, crates in parallel). zustc is the default (Cranelift, no -O)
"""
import os
import random
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ZUSTC = os.path.join(ROOT, "target/release/zustc")
VENDOR = os.path.join(ROOT, "cratebench/vendor")


def copy_crate_src(src, dst):
    """Copies the crate's sources from the vendor submodule (the upstream repository as is) to dst/src/.
    One whose lib.rs is at the top of the repository (fnv) is also put at src/lib.rs"""
    os.makedirs(dst, exist_ok=True)
    if os.path.isdir(os.path.join(src, "src")):
        shutil.copytree(os.path.join(src, "src"), os.path.join(dst, "src"))
    elif os.path.isfile(os.path.join(src, "lib.rs")):
        os.makedirs(os.path.join(dst, "src"))
        shutil.copy(os.path.join(src, "lib.rs"), os.path.join(dst, "src/lib.rs"))
    else:
        raise RuntimeError(f"{src} にクレートのソースがない (git submodule update --init cratebench/vendor)")
# (name, edition, features, crate name)
REAL = [("fnv", "2015", ["std"], "fnv"), ("adler2", "2021", ["std"], "adler2"), ("hex", "2018", ["std", "alloc"], "hex"),
        ("itoa", "2021", [], "itoa"), ("rustc-hash", "2021", ["std"], "rustc_hash"), ("cfg-if", "2018", [], "cfg_if")]

# An expression that uses the real crate (u64 from x: u64)
USES = {
    "fnv": "{ let mut h = fnv::FnvHasher::default(); h.write(&x.to_le_bytes()); h.finish() }",
    "rustc_hash": "{ let mut h = rustc_hash::FxHasher::with_seed(7); h.write_u64(x); h.finish() }",
    "hex": "(hex::encode(x.to_le_bytes()).len() as u64 + x)",
    "itoa": "({ let mut b = itoa::Buffer::new(); b.format(x).len() as u64 } ^ x)",
    "adler2": "(adler2::adler32_slice(&x.to_le_bytes()) as u64)",
    "cfg_if": "(x ^ m_mode())",
}


def gen(d, mids, fns, k):
    rnd = random.Random(42)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    ws = []
    members = []
    for name, ed, feats, cn in REAL:
        copy_crate_src(os.path.join(VENDOR, name), os.path.join(d, name))
        f = f" features={','.join(feats)}" if feats else ""
        ws.append(f"lib {cn} {name}/src/lib.rs{f}")
        fl = "".join(f'{x} = []\n' for x in feats)
        open(os.path.join(d, name, "Cargo.toml"), "w").write(
            f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "{ed}"\n\n[lib]\nname = "{cn}"\npath = "src/lib.rs"\n\n'
            f'[features]\ndefault = [{", ".join(chr(34) + x + chr(34) for x in feats)}]\n{fl}')
        members.append(name)
    used = set()
    deps_of = []
    for i in range(mids):
        reals = rnd.sample([r[3] for r in REAL], rnd.randint(2, 3))
        prev = sorted(rnd.sample(range(i), min(i, rnd.randint(1, k)))) if i > 0 else []
        used.update(prev)
        deps_of.append((reals, prev))
        name = f"m{i}"
        os.makedirs(os.path.join(d, name, "src"))
        src = ["use std::hash::Hasher;", ""]
        if "cfg_if" in reals:
            src += ["cfg_if::cfg_if! {", "    if #[cfg(debug_assertions)] {", "        fn m_mode() -> u64 { 1 }",
                    "    } else {", "        fn m_mode() -> u64 { 2 }", "    }", "}", ""]
        src += [f"pub struct Acc{i} {{", "    pub total: u64,", "    pub items: Vec<u64>,", "}", "",
                f"impl Acc{i} {{", f"    pub fn new() -> Acc{i} {{", f"        Acc{i} {{ total: 0, items: Vec::new() }}", "    }",
                "    pub fn push(&mut self, x: u64) {", "        self.total = self.total.wrapping_add(x);", "        self.items.push(x);", "    }",
                "    pub fn digest(&self) -> u64 {", f"        self.items.iter().fold(self.total, |a, &b| a.rotate_left(5) ^ f{i}_0(b))", "    }", "}", "",
                f"pub trait Score{i} {{", "    fn score(&self) -> u64;", "}", "",
                f"impl Score{i} for Acc{i} {{", "    fn score(&self) -> u64 {", "        self.digest() % 1000003", "    }", "}", "",
                f"pub fn sum_by<F: Fn(u64) -> u64>(xs: &[u64], f: F) -> u64 {{", "    xs.iter().map(|&x| f(x)).fold(0u64, |a, b| a.wrapping_add(b))", "}", ""]
        for j in range(fns):
            r = reals[j % len(reals)]
            body = [f"pub fn f{i}_{j}(x: u64) -> u64 {{", "    let mut s = x.wrapping_add(3);", f"    s ^= {USES[r]};"]
            if prev:
                p = prev[j % len(prev)]
                body.append(f"    s = s.wrapping_add(m{p}::f{p}_{j % fns}(x >> 1));")
            if j > 0:
                body.append(f"    if x % 3 == 0 {{ s = s.wrapping_mul(f{i}_{j - 1}(x / 3) | 1); }}")
            body += ["    s", "}", ""]
            src += body
        # (the entry point used from outside: also uses the structs / traits of the mid crates it depends on)
        src += [f"pub fn run{i}(n: u64) -> u64 {{", f"    let mut a = Acc{i}::new();", "    let mut k = 0;", "    while k < n {",
                f"        a.push(f{i}_{fns - 1}(k));", "        k += 1;", "    }", "    let mut s = a.score();"]
        for p in prev:
            src += [f"    let mut b = m{p}::Acc{p}::new();", "    b.push(s);", f"    s ^= m{p}::Score{p}::score(&b);"]
        src += [f"    s.wrapping_add(sum_by(&a.items, |v| v % 7))", "}", ""]
        open(os.path.join(d, name, "src/lib.rs"), "w").write("\n".join(src))
        dl = [f"m{p}" for p in prev]
        ws.append(f"lib {name} {name}/src/lib.rs " + " ".join(reals + dl))
        cdeps = "".join(f'{[r for r in REAL if r[3] == c][0][0]} = {{ path = "../{[r for r in REAL if r[3] == c][0][0]}" }}\n' for c in reals)
        cdeps += "".join(f'm{p} = {{ path = "../m{p}" }}\n' for p in prev)
        open(os.path.join(d, name, "Cargo.toml"), "w").write(f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n{cdeps}')
        members.append(name)
    tops = [i for i in range(mids) if i not in used]
    os.makedirs(os.path.join(d, "app/src"))
    main = ["fn main() {", "    let mut s = std::env::args().count() as u64;"]
    for t in tops:
        main.append(f"    s = s.wrapping_mul(31).wrapping_add(m{t}::run{t}(50));")
    main += ['    println!("{s}");', "}", ""]
    open(os.path.join(d, "app/src/main.rs"), "w").write("\n".join(main))
    ws.append("bin app app/src/main.rs " + " ".join(f"m{t}" for t in tops))
    open(os.path.join(d, "app/Cargo.toml"), "w").write(
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n' + "".join(f'm{t} = {{ path = "../m{t}" }}\n' for t in tops))
    members.append("app")
    open(os.path.join(d, "workspace.txt"), "w").write("\n".join(ws) + "\n")
    open(os.path.join(d, "Cargo.toml"), "w").write(
        "[workspace]\nresolver = \"2\"\nmembers = [" + ", ".join(f'"{m}"' for m in members) + "]\n\n[profile.dev]\ndebug = 0\n")
    nfn = mids * fns
    ndep = sum(len(a) + len(b) for a, b in deps_of)
    return f"{len(REAL)} 実在クレート + {mids} 中間クレート ({nfn} 関数、依存の辺 {ndep} 本) + app (直接の依存 {len(tops)})"


def run(cmd, cwd):
    t = time.perf_counter()
    r = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True)
    if r.returncode != 0:
        print(r.stdout[-3000:], r.stderr[-3000:])
        raise SystemExit(f"failed: {cmd}")
    return time.perf_counter() - t


def edit(path, old, new):
    s = open(path).read()
    assert old in s, (path, old)
    open(path, "w").write(s.replace(old, new, 1))


def main():
    a = sys.argv[1:]
    opt = {"--mids": 20, "--fns": 20, "--deps": 3, "--reps": 3}
    i = 0
    while i < len(a):
        if a[i] in opt:
            opt[a[i]] = int(a[i + 1])
            i += 2
        else:
            i += 1
    d = os.path.join(ROOT, "out/depsbench")
    tools = [("zustc", f"{ZUSTC} build workspace.txt -o target", os.path.join(d, "target/app"))]
    if "--no-cargo" not in a:
        tools.append(("cargo", "cargo build --offline -q", os.path.join(d, "target_cargo/debug/app")))
    env_note = ""
    steps = ["cold", "noop", "low-body", "low-sig", "app-body"]
    results = {}
    outs = {}
    desc = ""
    for name, cmd, exe in tools:
        res = {}
        for rep in range(opt["--reps"]):
            desc = gen(d, opt["--mids"], opt["--fns"], opt["--deps"])
            os.environ["CARGO_TARGET_DIR"] = os.path.join(d, "target_cargo")
            m0 = os.path.join(d, "m0/src/lib.rs")
            main_rs = os.path.join(d, "app/src/main.rs")
            fns = opt["--fns"]
            acts = {
                "cold": None,
                "noop": None,
                "low-body": lambda r=rep: edit(m0, "let mut s = x.wrapping_add(3);", f"let mut s = x.wrapping_add({1003 + r});"),
                # (add a parameter to m0's last fn, and fix the calls inside m0 and on the user side)
                "low-sig": lambda: sig_edit(d, opt["--mids"], fns),
                "app-body": lambda r=rep: edit(main_rs, "let mut s = std::env::args().count() as u64;",
                                               f"let mut s = std::env::args().count() as u64 + {r + 1};"),
            }
            for st in steps:
                if acts[st]:
                    acts[st]()
                res.setdefault(st, []).append(run(cmd, d))
            outs.setdefault(name, []).append(subprocess.run([exe], capture_output=True, text=True).stdout)
        results[name] = res
    print(f"workspace: {desc}{env_note}")
    if len(tools) > 1 and outs["zustc"] != outs["cargo"]:
        print(f"OUTPUT DIFFERS: zustc {outs['zustc'][:1]} cargo {outs['cargo'][:1]}")
    print("| step | " + " | ".join(n for n, _, _ in tools) + " |")
    print("|---|" + "---:|" * len(tools))
    for st in steps:
        cells = []
        for n, _, _ in tools:
            v = sorted(results[n][st])
            cells.append(f"{v[len(v) // 2] * 1000:.0f} ms")
        print(f"| {st} | " + " | ".join(cells) + " |")


def sig_edit(d, mids, fns):
    """Adds a parameter to m0's f0_{fns-1} (the callers: m0's run0, and the f*_{fns-1} of the mid crates that depend on m0)"""
    j = fns - 1
    for i in range(mids):
        p = os.path.join(d, f"m{i}/src/lib.rs")
        s = open(p).read()
        if i == 0:
            s = s.replace(f"pub fn f0_{j}(x: u64) -> u64 {{", f"pub fn f0_{j}(x: u64, _e: u8) -> u64 {{", 1)
            s = s.replace(f"f0_{j}(k)", f"f0_{j}(k, 0)")
        s = s.replace(f"m0::f0_{j}(x >> 1)", f"m0::f0_{j}(x >> 1, 0)")
        open(p, "w").write(s)


main()
