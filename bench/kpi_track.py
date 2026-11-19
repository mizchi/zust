#!/usr/bin/env python3
"""Measure the KPIs after one change and append them to bench/kpi_log.tsv, so that what each change did, and how far
the KPIs have drifted in total, stays visible (small increases add up).

usage: python3 bench/kpi_track.py "short label of the change" [--lsp] [--dry-run]
(run from the repository root after cargo build --release --workspace; with tests/ui numbers, run
 bench/rust_ui.py first: its out/rust_ui.json is read)

Only deterministic numbers are logged (wall-clock times move by 10-30% between runs on a shared machine):
- gen5000 (bench/gen.py, 5000 fns): instructions (of the zustc process; forked workers count the parent's work again) of
  a no-op build and `check` (each the least of 3 runs: one run moves by ~0.1%), a build after editing one function,
  counted by cachegrind; and the build cache size. The work directory is always ~/.cache/zust-kpi-track (whatever
  checkout runs it), so paths do not change the counts.
- traits: bench/gen_traits.py 50 (generic traits with associated types, where-clauses, iterator chains, closures,
  dyn with bindings, operators): `check` (the least of 3) and a build after editing one function. gen5000 reaches
  little of trait solving and inference; this does.
- crates: cratebench's crates (real code: each as a lib and the app that uses it), the sum over them of a no-op build
  (the least of 3) and of a build after editing the app.
- with --lsp: the instructions of one zust-analyzer session of bench/lsp_parity.py on zustc's sources as of REF_COMMIT
  (a snapshot unpacked once under the work directory: the checkout's own sources grow with the code, which would move
  the count without any change to the server). It moves by ~0.1% run to run (the client polls until the index is ready).
- tests/ui pass counts from out/rust_ui.json, when it is newer than target/release/zustc.

Each run prints the change against the previous row and against the first row (the reference), and rewrites
bench/kpi_log.md from the log.
"""
import argparse, datetime, json, os, shutil, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "bench"))
from ci_bench import ZUSTC, gen, instructions, run_measured  # noqa: E402

LOG = os.path.join(ROOT, "bench/kpi_log.tsv")
MD = os.path.join(ROOT, "bench/kpi_log.md")
# (a fixed place, whichever checkout runs this: the source path goes into the build and its counts)
WORK = os.environ.get("ZUST_KPI_WORK") or os.path.expanduser("~/.cache/zust-kpi-track")
COLS = ["date", "commit", "change", "noop_M", "edit_M", "check_M", "cache_KiB", "lsp_G", "run", "check", "build", "cfail",
        "tr_check_M", "tr_edit_M", "cr_noop_M", "cr_edit_M"]
# (columns added later: rows before them read as "-")


def build_kpis():
    d = os.path.join(WORK, "gen5000")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src, exe = os.path.join(d, "p.rs"), os.path.join(d, "p")
    build = [ZUSTC, src, "-o", exe]
    gen(5000, 0, src)
    run_measured(build)
    run_measured(build)
    noop = min(instructions(build) for _ in range(3))
    gen(5000, 3, src)
    edit = instructions(build)
    gen(5000, 0, src)
    run_measured(build)
    # (the least of 3 runs, as for the no-op build: one run moves by ~0.06%)
    check = min(instructions([ZUSTC, "check", src]) for _ in range(3))
    cache = os.path.getsize(exe + ".crcache2") / 1024
    return noop, edit, check, cache


def traits_kpis(n=50):
    """bench/gen_traits.py (traits, associated types, generic bounds, iterator chains, closures, dyn): `check` (the
    least of 3) and a build after editing one function"""
    d = os.path.join(WORK, "traits")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src, exe = os.path.join(d, "p.rs"), os.path.join(d, "p")
    gen_t = lambda v: open(src, "w").write(subprocess.run([sys.executable, os.path.join(ROOT, "bench/gen_traits.py"), str(n), str(v)],
                                                          capture_output=True, text=True, check=True).stdout)
    gen_t(0)
    build = [ZUSTC, src, "-o", exe]
    run_measured(build)
    check = min(instructions([ZUSTC, "check", src]) for _ in range(3))
    gen_t(3)
    edit = instructions(build)
    return check, edit


def crates_kpis():
    """cratebench's crates (fnv, adler2, hex, itoa, rustc-hash, cfg-if: real code, built as a lib and the app that uses
    it): the sum over them of a no-op build (the least of 3) and of a build after editing the app"""
    # (by its path: bench/ has a run.py of its own)
    import importlib.util
    spec = importlib.util.spec_from_file_location("cratebench_run", os.path.join(ROOT, "cratebench/run.py"))
    cb = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cb)
    noop = edit = 0.0
    for name, ed, feats in cb.crates():
        d = os.path.join(WORK, "crates", name)
        cb.setup(name, ed, feats, d)
        tgt = os.path.join(d, "ct")
        build = [ZUSTC, "build", "ws.txt", "-o", tgt]
        run_measured(build, cwd=d)
        run_measured(build, cwd=d)
        noop += min(instructions(build, cwd=d) for _ in range(3))
        cb.toggle_edit(d, 0)
        edit += instructions(build, cwd=d)
    return noop, edit


REF_COMMIT = "45579fc"


def lsp_tree():
    d = os.path.join(WORK, f"lsp-tree-{REF_COMMIT}")
    if not os.path.isdir(d):
        os.makedirs(d + ".tmp", exist_ok=True)
        a = subprocess.run(["git", "archive", REF_COMMIT], capture_output=True, check=True, cwd=ROOT).stdout
        subprocess.run(["tar", "-x", "-C", d + ".tmp"], input=a, check=True)
        os.replace(d + ".tmp", d)
    return d


def lsp_kpi():
    """instructions (G) of one zust-analyzer session: the server binary is wrapped in cachegrind for one run"""
    za = os.path.join(ROOT, "target/release/zust-analyzer")
    real = os.path.join(WORK, "zust-analyzer.real")
    out = os.path.join(WORK, "lsp.cg")
    shutil.copy(za, real)
    try:
        with open(za + ".wrap", "w") as f:
            f.write(f'#!/bin/sh\nexec valgrind --tool=cachegrind --cache-sim=no --cachegrind-out-file={out} {real} "$@"\n')
        os.chmod(za + ".wrap", 0o755)
        os.replace(za + ".wrap", za)
        env = dict(os.environ, ZUST_LSP_ROOT=lsp_tree())
        subprocess.run([sys.executable, os.path.join(ROOT, "bench/lsp_parity.py"), "--servers", "zustc", "--targets", "zustc",
                        "--timeout", "600", "--json", os.path.join(WORK, "lsp.json"), "--out", os.path.join(WORK, "lsp.md")],
                       check=True, capture_output=True, cwd=ROOT, env=env)
    finally:
        shutil.copy(real, za)
    for line in open(out):
        if line.startswith("summary:"):
            return int(line.split()[1]) / 1e9
    return None


def ui_counts():
    j = os.path.join(ROOT, "out/rust_ui.json")
    if not os.path.exists(j) or os.path.getmtime(j) < os.path.getmtime(ZUSTC):
        return [None] * 4
    k = json.load(open(j))["kinds"]
    return [k[x]["pass"] for x in ("run-pass", "check-pass", "build-pass", "compile-fail")]


def read_log():
    if not os.path.exists(LOG):
        return []
    rows = []
    for line in open(LOG).read().splitlines()[1:]:
        if line.strip():
            rows.append(dict(zip(COLS, line.split("\t"))))
    return rows


def num(r, k):
    v = r.get(k, "")
    return float(v) if v not in ("", "-", "None") else None


def pct(a, b):
    return "-" if a is None or b is None or b == 0 else f"{(a - b) / b * 100:+.2f}%"


def first_with(rows, k):
    """the first row that has a value for k (the reference for Σ: columns added later start later)"""
    return next((r for r in rows if num(r, k) is not None), rows[0] if rows else {})


def write_md(rows):
    out = ["# KPI log (one row per change)",
           "",
           "Written by `python3 bench/kpi_track.py` after each change (deterministic counts only; see the script). "
           "Δ is against the row before, Σ against the first row. The same binary measured again moves by up to "
           "~0.01% (builds) / ~0.1% (LSP), so differences below that are noise. The LSP column is measured on a fixed "
           "snapshot of the sources (45579fc) since 2026-11-19; the values the rows before had were taken on the "
           "checkout's own sources and are left out. gen5000 is simple functions and reaches little of trait solving and "
           "inference, so since 2026-11-19 two more workloads are measured: traits (bench/gen_traits.py 50: generic "
           "traits with associated types, bounds, iterator chains, closures, dyn) and crates (cratebench's six crates, "
           "real code, as a lib and its app; the sum over them).",
           "",
           "| date | commit | change | no-op build (M instr) | edit one fn (M instr) | check (M instr) | cache (KiB) | LSP session (G instr) | traits check (M) | traits edit (M) | crates no-op (M) | crates edit (M) | tests/ui run / check / build / compile-fail |",
           "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|"]
    prev = None
    for r in rows:
        cells = []
        for k in ("noop_M", "edit_M", "check_M", "cache_KiB", "lsp_G", "tr_check_M", "tr_edit_M", "cr_noop_M", "cr_edit_M"):
            v = num(r, k)
            if v is None:
                cells.append("-")
                continue
            s = f"{v:.1f}" if k != "lsp_G" else f"{v:.3f}"
            if prev is not None and num(prev, k) is not None:
                s += f" (Δ {pct(v, num(prev, k))}, Σ {pct(v, num(first_with(rows, k), k))})"
            cells.append(s)
        ui = " / ".join(r.get(k, "-") or "-" for k in ("run", "check", "build", "cfail"))
        out.append(f"| {r['date']} | {r['commit']} | {r['change']} | " + " | ".join(cells) + f" | {ui} |")
        prev = r
    open(MD, "w").write("\n".join(out) + "\n")


KPIS = (("noop_M", "no-op build"), ("edit_M", "edit one fn"), ("check_M", "check"), ("cache_KiB", "cache"), ("lsp_G", "LSP session"),
        ("tr_check_M", "traits check"), ("tr_edit_M", "traits edit"), ("cr_noop_M", "crates no-op"), ("cr_edit_M", "crates edit"))


def band_check(rows, row, last=7, floor=0.001):
    out = []
    for k, name in KPIS:
        v = num(row, k)
        hist = [num(r, k) for r in rows[-last:] if num(r, k) is not None]
        if v is None or len(hist) < 3:
            continue
        hist.sort()
        med = hist[len(hist) // 2]
        mad = sorted(abs(h - med) for h in hist)[len(hist) // 2]
        band = max(floor * med, 3 * 1.4826 * mad)
        if v > med + band:
            out.append(f"REGRESSION {name}: {v:.3f} against the median {med:.3f} of the last {len(hist)} (band {band / med * 100:.2f}%)")
        elif v < med - band:
            out.append(f"improved {name}: {v:.3f} against the median {med:.3f} of the last {len(hist)}")
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("change")
    ap.add_argument("--lsp", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    os.makedirs(WORK, exist_ok=True)
    noop, edit, check, cache = build_kpis()
    tr_check, tr_edit = traits_kpis()
    cr_noop, cr_edit = crates_kpis()
    lsp = lsp_kpi() if a.lsp else None
    ui = ui_counts()
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    # (the log itself does not count as a change)
    dirty = [l for l in subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"], capture_output=True, text=True,
                                       cwd=ROOT).stdout.splitlines() if not l.endswith(("bench/kpi_log.tsv", "bench/kpi_log.md"))]
    row = {"date": datetime.date.today().isoformat(), "commit": commit + ("+" if dirty else ""), "change": a.change.replace("\t", " "),
           "noop_M": f"{noop:.3f}", "edit_M": f"{edit:.3f}", "check_M": f"{check:.3f}", "cache_KiB": f"{cache:.1f}",
           "lsp_G": "-" if lsp is None else f"{lsp:.4f}", "run": str(ui[0] or "-"), "check": str(ui[1] or "-"),
           "build": str(ui[2] or "-"), "cfail": str(ui[3] or "-"),
           "tr_check_M": f"{tr_check:.3f}", "tr_edit_M": f"{tr_edit:.3f}", "cr_noop_M": f"{cr_noop:.3f}", "cr_edit_M": f"{cr_edit:.3f}"}
    rows = read_log()
    prev = rows[-1] if rows else None
    first = rows[0] if rows else row
    for k, name in KPIS:
        v = num(row, k)
        if v is None:
            continue
        ref = first_with(rows, k) if num(first_with(rows, k), k) is not None else row
        print(f"{name:14} {v:12.3f}   Δ {pct(v, num(prev, k)) if prev else '-':>8}   Σ {pct(v, num(ref, k)):>8}")
    # (as blaze's benchhistory check: against the median of the last 7 rows, a move past the noise band (0.1%, or
    # three times the spread the history shows, whichever is larger) is reported)
    flags = band_check(rows, row)
    for f in flags:
        print(f)
    if a.dry_run:
        return
    new = not os.path.exists(LOG)
    with open(LOG, "a") as f:
        if new:
            f.write("\t".join(COLS) + "\n")
        f.write("\t".join(row[c] for c in COLS) + "\n")
    write_md(read_log())


if __name__ == "__main__":
    main()
