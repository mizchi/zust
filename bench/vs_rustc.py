"""Compares zustc and the real rustc on the same programs with the same steps, and makes a table (Markdown).
usage: python3 bench/vs_rustc.py [NAME ...]   (run in this directory; cargo build --release beforehand)
  NAME is gen1000 (the 1000 fns of bench/gen.py) or bench/rt/NAME.rs. All of them if omitted.

For each program, measures the following (times are the median of 3 runs):
  cold    a build from a state with neither a cache nor an incremental directory
  no-op   build again without changing anything
  edit    build after changing one spot (the body of f500 for gen1000, a constant in the fn appended at the end for bench/rt)
  disk    what the build leaves behind (zustc: cache + .o, rustc: the incremental directory)
  binary  the size of the output (after strip); run: the run time (the output is checked to match rustc -O)
Configurations compared: zustc (debug, the default linker; wild if wild is on PATH), zustc -O,
rustc -Cincremental (debug), rustc -O (non-incremental)"""
import os, shutil, statistics, subprocess, sys, time

ZUSTC = os.path.abspath("target/release/zustc")
W = os.path.abspath("out/vs_rustc")


def run(cmd):
    t = time.perf_counter()
    r = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if r.returncode != 0:
        raise SystemExit(f"failed: {' '.join(cmd)}\n{r.stderr.decode()[:2000]}")
    return (time.perf_counter() - t) * 1e3, r.stdout


def du(*paths):
    n = 0
    for p in paths:
        if os.path.isfile(p):
            n += os.path.getsize(p)
        elif os.path.isdir(p):
            n += sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(p) for f in fs)
    return n


def source(name, variant):
    if name.startswith("gen"):
        n = int(name[3:])
        return subprocess.run([sys.executable, "bench/gen.py", str(n), str(variant)],
                              capture_output=True, text=True, check=True).stdout
    src = open(f"bench/rt/{name}.rs").read()
    # (edit: change the constant of the fn at the end that nothing calls)
    return src + f"\n#[allow(dead_code)]\nfn __bench_edit() -> u64 {{ {variant} }}\n"


def median3(f):
    return statistics.median(f() for _ in range(3))


def bench(name):
    d = os.path.join(W, name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src = os.path.join(d, "p.rs")
    rows = []

    def scenario(label, build, exe, store, incremental=True):
        def write(v):
            with open(src, "w") as f:
                f.write(source(name, v))

        def cold():
            for p in store:
                shutil.rmtree(p, ignore_errors=True) if os.path.isdir(p) else (os.path.exists(p) and os.remove(p))
            write(0)
            return run(build)[0]

        c = median3(cold)
        noop = median3(lambda: run(build)[0]) if incremental else None
        k = [0]

        def edit():
            k[0] += 1
            write(k[0])
            return run(build)[0]

        e = median3(edit) if incremental else None
        write(0)
        run(build)
        r = statistics.median(run([exe])[0] for _ in range(3))
        out = run([exe])[1]
        # (zustc's output has no symbols; rustc's output includes std's symbols and debug info, so compare the stripped sizes)
        subprocess.run(["strip", "-o", exe + ".strip", exe], check=True)
        rows.append((label, c, noop, e, du(*store), os.path.getsize(exe + ".strip"), r, out))

    exe = os.path.join(d, "c")
    scenario("zustc", [ZUSTC, src, "-o", exe], exe, [exe + ".cache", exe + ".o"])
    exe = os.path.join(d, "cO")
    scenario("zustc -O", [ZUSTC, src, "-o", exe, "-O"], exe, [exe + ".cache", exe + ".o"])
    exe, inc = os.path.join(d, "r"), os.path.join(d, "r-incr")
    scenario("rustc -Cincremental", ["rustc", "--edition", "2021", "-Awarnings", "-Cincremental=" + inc, src, "-o", exe],
             exe, [inc])
    exe = os.path.join(d, "rO")
    scenario("rustc -O", ["rustc", "--edition", "2021", "-Awarnings", "-O", src, "-o", exe], exe, [], incremental=False)
    want = rows[-1][7]
    base = rows[-1][6]
    f = lambda x: "-" if x is None else f"{x:.0f}"
    print(f"\n### {name}\n")
    print("| 設定 | cold ms | no-op ms | edit ms | disk KiB | binary KiB (strip) | run ms | rustc -O 比 | 出力 |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---|")
    for label, c, noop, e, disk, size, r, out in rows:
        print(f"| {label} | {f(c)} | {f(noop)} | {f(e)} | {disk / 1024:.1f} | {size / 1024:.1f} | {r:.0f} | {r / base:.2f}x | {'一致' if out == want else '不一致'} |")
    sys.stdout.flush()


if __name__ == "__main__":
    linker = next((l for l in ["wild", "mold", "ld.lld", "ld"] if shutil.which(l)), "?")
    print(f"zustc のリンカ: {linker} / {subprocess.run(['rustc', '--version'], capture_output=True, text=True).stdout.strip()}")
    names = sys.argv[1:] or ["gen1000"] + sorted(f[:-3] for f in os.listdir("bench/rt") if f.endswith(".rs"))
    for n in names:
        bench(n)
