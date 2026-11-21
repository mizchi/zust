"""Rust の UI フレームワーク (dioxus / leptos) を、wasm32 向けに依存するクレートの葉から順に zustc でビルドする。

usage: python3 cratebench/ui/run.py [APP ...] [--rustc]   (リポジトリの根で。事前に cargo build --release --workspace)
  APP: apps/ の名前 (dioxus / leptos)。省くとすべて
  --rustc: 先に cargo build --target wasm32-unknown-unknown で、rustc ならビルドできることを確かめる

クレートの一覧は `zurgo metadata --filter-platform wasm32-unknown-unknown` (Cargo.lock の版、解決された feature。
`cargo metadata` と同じもの)。cargo はソースを取ってくる (`cargo fetch`) のにだけ使う。
wasm に入るクレート (proc-macro を通らずに app から届くもの) だけをビルドする。proc-macro とそれが使うクレートは
ホストで動くもので、zustc には proc-macro を動かす仕組みがないので数えるだけ。
各クレートは、推移的な依存 (lib) と自分のワークスペース (ws.txt) にして `zustc build` する。依存のどれかが
通らなければ、そのクレートはビルドせず blocked とする。zustc のワークスペースのビルドは wasm32 をまだ受け付けない
ので、cfg は x86-64 のもの (wasm32 の cfg にしても、通る・通らないの境目は同じだった: レポート参照)。
結果は out/cratebench-ui/<APP>/ (クレートごとの ws.txt と err.txt、results.json)。
"""
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
ZUSTC = os.path.join(ROOT, "target/release/zustc")
ZURGO = os.path.join(ROOT, "target/release/zurgo")
OUT = os.path.join(ROOT, "out/cratebench-ui")
TARGET = "wasm32-unknown-unknown"


# (`rustc --print cfg --target wasm32-unknown-unknown`)
WASM_CFG = {("panic", "abort"), ("target_abi", ""), ("target_arch", "wasm32"), ("target_endian", "little"), ("target_env", ""),
            ("target_family", "wasm"), ("target_os", "unknown"), ("target_pointer_width", "32"), ("target_vendor", "unknown"),
            ("debug_assertions", None)} | {("target_feature", f) for f in ("bulk-memory", "multivalue", "mutable-globals",
            "nontrapping-fptoint", "reference-types", "sign-ext")} | {("target_has_atomic", a) for a in ("8", "16", "32", "64", "ptr")}


def on_target(platform):
    """whether a `[target.<platform>]` key applies to wasm32 (a triple, or `cfg(..)`)"""
    if platform is None:
        return True
    if not platform.startswith("cfg("):
        return platform == TARGET
    toks = re.findall(r'[A-Za-z_][A-Za-z0-9_]*|"[^"]*"|[(),=]', platform[4:-1])
    pos = [0]

    def ev():
        name = toks[pos[0]]
        pos[0] += 1
        if name in ("all", "any", "not") and pos[0] < len(toks) and toks[pos[0]] == "(":
            pos[0] += 1
            vals = []
            while toks[pos[0]] != ")":
                vals.append(ev())
                if toks[pos[0]] == ",":
                    pos[0] += 1
            pos[0] += 1
            return all(vals) if name == "all" else any(vals) if name == "any" else not vals[0]
        if pos[0] < len(toks) and toks[pos[0]] == "=":
            val = toks[pos[0] + 1][1:-1]
            pos[0] += 2
            return (name, val) in WASM_CFG
        return (name, None) in WASM_CFG

    return ev()


def metadata(app):
    d = os.path.join(HERE, "apps", app)
    # (cargo only downloads the sources; zurgo reads the graph, as cargo metadata would give it)
    subprocess.run(["cargo", "fetch", "--locked", "--target", TARGET], cwd=d, capture_output=True, text=True, check=True)
    r = subprocess.run([ZURGO, "metadata", "--format-version", "1", "--filter-platform", TARGET], cwd=d, capture_output=True, text=True,
                       check=True)
    return json.loads(r.stdout)


def lines_of(d):
    n = 0
    for dp, _, fs in os.walk(d):
        for f in fs:
            if f.endswith(".rs"):
                with open(os.path.join(dp, f), errors="ignore") as h:
                    n += sum(1 for _ in h)
    return n


def graph(meta):
    """クレート (id → 情報) と、wasm に入るもの (依存の順、app が最後) と、ホストで動くもの"""
    pk = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    C = {}
    for i, n in nodes.items():
        p = pk[i]
        t = next((t for t in p["targets"] if set(t["kind"]) & {"lib", "rlib", "proc-macro", "bin"}), p["targets"][0])
        # (a normal dependency on wasm32: a `kind: null` declaration whose platform applies. metadata lists every
        #  declaration of an edge it shows, also one for another platform beside a build dependency)
        deps = [d["pkg"] for d in n["deps"] if any(k["kind"] is None and on_target(k["target"]) for k in d["dep_kinds"])]
        C[i] = {"name": p["name"], "version": p["version"], "edition": t["edition"],
                "proc_macro": "proc-macro" in t["kind"], "build_rs": any("custom-build" in x["kind"] for x in p["targets"]),
                "features": n["features"], "deps": deps, "src": t["src_path"], "root": i == root}
    order, seen = [], set()

    def visit(i):
        if i in seen:
            return
        seen.add(i)
        for d in C[i]["deps"]:
            if not C[d]["proc_macro"]:
                visit(d)
        order.append(i)

    visit(root)
    host, st = set(), [d for i in order for d in C[i]["deps"] if C[d]["proc_macro"]]
    while st:
        k = st.pop()
        if k not in host:
            host.add(k)
            st.extend(C[k]["deps"])
    return C, order, host


def closure(C, i, tset):
    seen, st = [], [i]
    while st:
        for d in C[st.pop()]["deps"]:
            if d in tset and d not in seen:
                seen.append(d)
                st.append(d)
    return seen


def sweep(app):
    meta = metadata(app)
    C, order, host = graph(meta)
    tset = set(order)
    ext = lambda c: c["name"].replace("-", "_")
    out = os.path.join(OUT, app)
    os.makedirs(out, exist_ok=True)
    res = {}
    for i in order:
        c = C[i]
        deps = [j for j in order if j in closure(C, i, tset)]
        bad = [C[j]["name"] for j in deps if res[j]["status"] != "pass"]
        r = {"name": c["name"], "version": c["version"], "status": "blocked", "blocked_by": bad, "first": ""}
        res[i] = r
        if bad:
            continue
        # (2 つの版のあるクレートは、ワークスペースでは名前が 1 つなので、依存の順で後のもの (近いもの) を使う)
        ws = {}
        for j in deps + [i]:
            cj = C[j]
            dn = sorted({ext(C[d]) for d in cj["deps"] if d in tset})
            f = f' features={",".join(cj["features"])}' if cj["features"] else ""
            ws[ext(cj)] = f'{"bin" if cj["root"] else "lib"} {ext(cj)} {cj["src"]} {" ".join(dn)}{f}'.rstrip()
        d = os.path.join(out, f'{c["name"]}-{c["version"]}')
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "ws.txt"), "w") as h:
            h.write("\n".join(ws.values()) + "\n")
        try:
            p = subprocess.run([ZUSTC, "build", os.path.join(d, "ws.txt"), "-o", os.path.join(d, "t"), "--no-cache", "-j", "4"],
                               capture_output=True, text=True, timeout=300)
            err, ok = (p.stderr + p.stdout).strip(), p.returncode == 0
        except subprocess.TimeoutExpired:
            err, ok = "timeout (300 s)", False
        with open(os.path.join(d, "err.txt"), "w") as h:
            h.write(err)
        r["status"] = "pass" if ok else "fail"
        r["first"] = next((l for l in err.splitlines() if l.startswith("error")), err.splitlines()[0] if err else "")
    with open(os.path.join(out, "results.json"), "w") as h:
        json.dump(res, h, indent=1)
    # (通らないクレートの上にある (それに推移的に依存する) クレートの数: 直す順の目安)
    above = {i: 0 for i in order}
    for i in order:
        for j in closure(C, i, tset):
            above[j] += 1
    reg = os.path.expanduser("~/.cargo/registry/src/")
    n = {s: sum(1 for r in res.values() if r["status"] == s) for s in ("pass", "fail", "blocked")}
    pm = sorted({C[i]["name"] for i in host if C[i]["proc_macro"]})
    print(f"## {app}\n")
    print(f"- crates built for wasm32: {len(order)} ({sum(lines_of(os.path.dirname(C[i]['src'])) for i in order)} lines of .rs in their "
          f"directories), {sum(C[i]['build_rs'] for i in order)} with a build script")
    print(f"- host crates (proc macros and what they use): {len(host)}, of them {len(pm)} proc-macro crates: {' '.join(pm)}")
    print(f"- zustc: {n['pass']} pass, {n['fail']} fail on their own code, {n['blocked']} blocked by a dependency that fails")
    print(f"- {C[order[-1]]['name']} (the app): {res[order[-1]]['status']}\n")
    print("| crate | crates above it | first error |\n|---|---:|---|")
    for i in sorted((i for i in order if res[i]["status"] == "fail"), key=lambda i: -above[i]):
        e = res[i]["first"].replace(reg, "").replace("|", "\\|")
        e = e.split("index.crates.io-", 1)[-1].split("/", 1)[-1] if "index.crates.io-" in e else e
        print(f'| {C[i]["name"]} {C[i]["version"]} | {above[i]} | {e[:160]} |')
    print()


def main():
    a = sys.argv[1:]
    check_rustc = "--rustc" in a
    apps = [x for x in a if not x.startswith("--")] or sorted(os.listdir(os.path.join(HERE, "apps")))
    for app in apps:
        if check_rustc:
            t = time.time()
            env = {**os.environ, "CARGO_TARGET_DIR": os.path.join(OUT, "cargo-target")}
            p = subprocess.run(["cargo", "build", "--locked", "--target", TARGET], cwd=os.path.join(HERE, "apps", app),
                               capture_output=True, text=True, env=env)
            print(f"{app}: cargo build --target {TARGET}: {'ok' if p.returncode == 0 else 'FAILED'} ({time.time() - t:.0f} s)\n")
        sweep(app)


if __name__ == "__main__":
    main()
