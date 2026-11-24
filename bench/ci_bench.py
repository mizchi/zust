#!/usr/bin/env python3
"""継続的なベンチマーク (CI のダッシュボード用)。github-action-benchmark の `customSmallerIsBetter` /
`customBiggerIsBetter` の形の JSON を書く。

usage: python3 bench/ci_bench.py [--out-dir out/ci] [--reps 5] [--no-lsp] [--no-valgrind]
(リポジトリの根で。事前に cargo build --release --workspace)

書くもの:
- zustc.json (小さいほどよい): 生成した N 関数のプログラム (bench/gen.py) の cold / no-op / 1 関数編集のビルド時間と
  ピーク RSS、check の時間、出力とキャッシュの大きさ、bench/rt の実行時間 (-O)。valgrind があれば命令数も
  (共有のランナーでは時間が揺れるので、こちらの方が差を見やすい)
- analyzer.json (小さいほどよい): bench/lsp_parity.py の zust-analyzer の各問い合わせの p50、起動から索引ができるまで、RSS
- parity.json (大きいほどよい): rust-analyzer と同じ答えを返した割合 (rust-analyzer が PATH にあるときだけ)
"""
import argparse, json, os, resource, shutil, statistics, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ZUSTC = os.path.join(ROOT, "target/release/zustc")


def run_measured(cmd, cwd=ROOT):
    """1 つのコマンドの (時間 ms, ピーク RSS MiB)。子プロセスを 1 段挟んで RUSAGE_CHILDREN を取る"""
    code = ("import resource,subprocess,sys,time;t=time.perf_counter();"
            "subprocess.run(sys.argv[1:],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);"
            "print((time.perf_counter()-t)*1e3, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)")
    out = subprocess.run([sys.executable, "-c", code, *cmd], check=True, capture_output=True, text=True, cwd=cwd).stdout
    ms, kib = out.split()
    return float(ms), int(kib) / 1024


def instructions(cmd, cwd=ROOT, env=None):
    """valgrind (cachegrind、キャッシュの模擬なし) で数えた命令数 (百万)。子プロセス (リンカ) は数えない"""
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
    """N 関数のプログラム: cold (キャッシュなし) / noop / edit のビルドと check"""
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
        (f"{name} cache", du(exe + ".crcache2") / 1024, "KiB"),
    ]
    if vg:
        out.append((f"{name} build noop instructions", instructions(build), "M instr"))
        gen(n, 3, src)
        out.append((f"{name} build edit 1 fn instructions", instructions(build), "M instr"))
        gen(n, 0, src)
        run_measured(build)
        out.append((f"{name} check instructions", instructions([ZUSTC, "check", "-j", "1", src]), "M instr"))


def bench_runtime(reps, work, out):
    """bench/rt のプログラムを -O でビルドして実行した時間"""
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
    """bench/lsp_parity.py (zustc 自身のソースが対象) の zust-analyzer の数字"""
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
