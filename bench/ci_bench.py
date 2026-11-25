#!/usr/bin/env python3
"""Continuous benchmarks (for the CI dashboard). Writes JSON in github-action-benchmark's `customSmallerIsBetter` /
`customBiggerIsBetter` format.

usage: python3 bench/ci_bench.py [--out-dir out/ci] [--reps 5] [--no-lsp] [--no-valgrind]
(at the repository root, after cargo build --release --workspace)

What it writes:
- zustc.json (smaller is better): the cold / no-op / one-fn-edit build times of a generated program of N fns (bench/gen.py) and
  peak RSS, the check time, the sizes of the output and the cache, the run times of bench/rt (-O). With valgrind, instruction counts too
  (times fluctuate on shared runners, so differences are easier to see with these)
- analyzer.json (smaller is better): the p50 of each zust-analyzer query of bench/lsp_parity.py, the time from startup until the index is built, RSS
- parity.json (bigger is better): the fraction of answers that were the same as rust-analyzer's (only when rust-analyzer is on PATH)
"""
import argparse, json, os, resource, shutil, statistics, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ZUSTC = os.path.join(ROOT, "target/release/zustc")


def run_measured(cmd, cwd=ROOT):
    """(time ms, peak RSS MiB) of one command. Takes RUSAGE_CHILDREN through one intermediate child process"""
    code = ("import resource,subprocess,sys,time;t=time.perf_counter();"
            "subprocess.run(sys.argv[1:],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);"
            "print((time.perf_counter()-t)*1e3, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)")
    out = subprocess.run([sys.executable, "-c", code, *cmd], check=True, capture_output=True, text=True, cwd=cwd).stdout
    ms, kib = out.split()
    return float(ms), int(kib) / 1024


def instructions(cmd, cwd=ROOT, env=None):
    """The instruction count (millions) counted with valgrind (cachegrind, no cache simulation). Child processes (the linker) are not counted"""
    r = subprocess.run(["valgrind", "--tool=cachegrind", "--cache-sim=no", "--cachegrind-out-file=/dev/null", *cmd],
                       capture_output=True, text=True, cwd=cwd, env=None if env is None else {**os.environ, **env})
    # (valgrind exits with the program's status: a count of a failed build or check is not a measurement)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)} exited with {r.returncode} under valgrind: {r.stderr[-500:]}")
    for line in r.stderr.splitlines():
        if "I" in line and "refs:" in line:
            return int(line.split(":")[1].strip().replace(",", "")) / 1e6
    raise RuntimeError(f"cachegrind の出力がない: {r.stderr[-500:]}")


def gen(n, variant, path):
    with open(path, "w") as f:
        subprocess.run([sys.executable, os.path.join(ROOT, "bench/gen.py"), str(n), str(variant)], stdout=f, check=True)


def du(path):
    return os.path.getsize(path) if os.path.exists(path) else 0


def bench_build(n, reps, work, vg, out):
    """A program of N fns: cold (no cache) / noop / edit builds, and check"""
    d = os.path.join(work, f"gen{n}")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src, exe = os.path.join(d, "p.rs"), os.path.join(d, "p")
    build = [ZUSTC, src, "-o", exe]
    gen(n, 0, src)
    colds, rss = [], []
    for _ in range(reps):
        for f in os.listdir(d):
            if f != "p.rs":
                os.remove(os.path.join(d, f))
        ms, mib = run_measured(build)
        colds.append(ms)
        rss.append(mib)
    noops = [run_measured(build)[0] for _ in range(reps)]
    edits = []
    for k in range(reps):
        gen(n, k % 2 + 1, src)
        edits.append(run_measured(build)[0])
    gen(n, 0, src)
    run_measured(build)
    checks = [run_measured([ZUSTC, "check", src])[0] for _ in range(reps)]
    name = f"gen{n}"
    out += [
        (f"{name} build cold", statistics.median(colds), "ms"),
        (f"{name} build noop", statistics.median(noops), "ms"),
        (f"{name} build edit 1 fn", statistics.median(edits), "ms"),
        (f"{name} check", statistics.median(checks), "ms"),
        (f"{name} build cold peak RSS", statistics.median(rss), "MiB"),
        (f"{name} binary", du(exe) / 1024, "KiB"),
        (f"{name} cache", du(exe + ".cache") / 1024, "KiB"),
    ]
    if vg:
        out.append((f"{name} build noop instructions", instructions(build), "M instr"))
        gen(n, 3, src)
        out.append((f"{name} build edit 1 fn instructions", instructions(build), "M instr"))
        gen(n, 0, src)
        run_measured(build)
        out.append((f"{name} check instructions", instructions([ZUSTC, "check", "-j", "1", src]), "M instr"))


def bench_runtime(reps, work, out):
    """The run times of the bench/rt programs built with -O"""
    rt = os.path.join(ROOT, "bench/rt")
    for f in sorted(os.listdir(rt)):
        if not f.endswith(".rs"):
            continue
        name = f[:-3]
        exe = os.path.join(work, "rt", name)
        os.makedirs(os.path.dirname(exe), exist_ok=True)
        subprocess.run([ZUSTC, os.path.join(rt, f), "-o", exe, "-O"], check=True, capture_output=True)
        ts = [run_measured([exe])[0] for _ in range(reps)]
        out.append((f"rt/{name} run (-O)", statistics.median(ts), "ms"))


def bench_lsp(work, out_small, out_big):
    """The zust-analyzer numbers of bench/lsp_parity.py (on zustc's own source)"""
    servers = "zustc,rust-analyzer" if shutil.which("rust-analyzer") else "zustc"
    j = os.path.join(work, "lsp_parity.json")
    subprocess.run([sys.executable, os.path.join(ROOT, "bench/lsp_parity.py"), "--servers", servers, "--targets", "zustc",
                    "--json", j, "--out", os.path.join(work, "lsp_parity_result.md")], check=True, cwd=ROOT)
    t = json.load(open(j))["zustc"]
    for op, (p50, _p90) in t["timing"].items():
        out_small.append((f"lsp {op} p50", p50, "ms"))
    out_small.append(("lsp ready (startup to indexed)", t["ready_ms"], "ms"))
    out_small.append(("lsp peak RSS", t["rss_mb"], "MiB"))
    if "rust-analyzer" in servers:
        for k, v in t["metrics"].items():
            if v is not None:
                out_big.append((f"parity {k}", 100 * v, "%"))


def write(path, rows):
    json.dump([{"name": n, "value": round(v, 3), "unit": u} for n, v, u in rows], open(path, "w"), ensure_ascii=False, indent=1)
    for n, v, u in rows:
        print(f"{n:45} {v:10.2f} {u}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", default=os.path.join(ROOT, "out/ci"))
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--no-lsp", action="store_true")
    ap.add_argument("--no-valgrind", action="store_true")
    a = ap.parse_args()
    os.makedirs(a.out_dir, exist_ok=True)
    work = os.path.join(a.out_dir, "work")
    os.makedirs(work, exist_ok=True)
    vg = not a.no_valgrind and shutil.which("valgrind") is not None
    build = []
    for n in (1000, 5000):
        bench_build(n, a.reps, work, vg, build)
    bench_runtime(a.reps, work, build)
    write(os.path.join(a.out_dir, "zustc.json"), build)
    if not a.no_lsp:
        small, big = [], []
        bench_lsp(work, small, big)
        write(os.path.join(a.out_dir, "analyzer.json"), small)
        if big:
            write(os.path.join(a.out_dir, "parity.json"), big)


if __name__ == "__main__":
    main()
