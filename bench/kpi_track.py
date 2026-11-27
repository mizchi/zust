#!/usr/bin/env python3
"""Measure the KPIs after one change and append them to bench/kpi_log.tsv, so that what each change did, and how far
the KPIs have drifted in total, stays visible (small increases add up).

usage: python3 bench/kpi_track.py "short label of the change" [--lsp] [--dry-run] [--jobs N] [--only SECTIONS]
(in a pull request, measure with --dry-run and commit no rows: .github/workflows/kpi.yml compares the pull request
 with its base and keeps the result in a comment on it; rows go in with commits made on main)
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
- patterns: bench/gen_patterns.py mix 4 (the shapes of application code: an interpreter, record processing, error
  handling with `?`, state machines, trait-object plugins, zero-copy parsing with lifetimes, macro_rules!, nested
  modules, generic containers; 2600 lines): `check` (the least of 3) and a build after editing one function.
  bench/patterns.py measures each pattern alone.
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
        "tr_check_M", "tr_edit_M", "cr_noop_M", "cr_edit_M", "pt_check_M", "pt_edit_M"]
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
    # (the cache's compiler id is the stat of /proc/self/exe, which under valgrind is valgrind's: a no-op build under it,
    #  not short-circuited by the stamps, leaves the cache the measured edit build reads as its own)
    instructions(build, env={"ZUSTC_NO_STAMP": "1"})
    gen(5000, 3, src)
    edit = instructions(build)
    gen(5000, 0, src)
    run_measured(build)
    # (the least of 3 runs, as for the no-op build: one run moves by ~0.06%. At -j 1: valgrind does not count the
    # children a parallel check forks, #101)
    check = min(instructions([ZUSTC, "check", "-j", "1", src]) for _ in range(3))
    cache = os.path.getsize(exe + ".cache") / 1024
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
    check = min(instructions([ZUSTC, "check", "-j", "1", src]) for _ in range(3))
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


def patterns_kpis(n=4):
    """bench/gen_patterns.py mix (every pattern, n groups each): `check` (the least of 3) and a build after editing one
    function"""
    d = os.path.join(WORK, "patterns")
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src, exe = os.path.join(d, "p.rs"), os.path.join(d, "p")
    gen_p = lambda v: open(src, "w").write(subprocess.run([sys.executable, os.path.join(ROOT, "bench/gen_patterns.py"), "mix", str(n), str(v)],
                                                          capture_output=True, text=True, check=True).stdout)
    gen_p(0)
    build = [ZUSTC, src, "-o", exe]
    run_measured(build)
    check = min(instructions([ZUSTC, "check", "-j", "1", src]) for _ in range(3))
    # (a build under valgrind does not take the stamps' shortcut, and leaves the cache the edit build reads)
    instructions(build, env={"ZUSTC_NO_STAMP": "1"})
    gen_p(3)
    edit = instructions(build)
    return check, edit


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
    kinds = ("run-pass", "check-pass", "build-pass", "compile-fail")
    # (as the ratchet counts them: without the compile-fail tests rejected for another reason than the one they test,
    #  bench/rust_ui_accidental.txt; from the run's per-test results when they are as new as its counts)
    res = os.path.join(ROOT, "out/rust_ui_results.txt")
    acc = os.path.join(ROOT, "bench/rust_ui_accidental.txt")
    # (the same run writes both at its end, a moment apart: a filtered run with its own `--json` leaves the JSON older)
    if os.path.exists(res) and abs(os.path.getmtime(res) - os.path.getmtime(j)) < 5 and os.path.exists(acc):
        skip = {tuple(l.rstrip("\n").split("\t")[:2]) for l in open(acc) if l.strip() and not l.startswith("#")}
        n = dict.fromkeys(kinds, 0)
        for line in open(res):
            f = line.rstrip("\n").split("\t")
            if len(f) >= 3 and f[0] == "pass" and f[1] in n and (f[1], f[2]) not in skip:
                n[f[1]] += 1
        return [n[x] for x in kinds]
    return [k[x]["pass"] for x in kinds]


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
           "real code, as a lib and its app; the sum over them). Since 2026-11-20, patterns (bench/gen_patterns.py mix 4: "
           "the shapes of application code, check and an edit build).",
           "",
           "| date | commit | change | no-op build (M instr) | edit one fn (M instr) | check (M instr) | cache (KiB) | LSP session (G instr) | traits check (M) | traits edit (M) | crates no-op (M) | crates edit (M) | patterns check (M) | patterns edit (M) | tests/ui run / check / build / compile-fail |",
           "|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|"]
    prev = None
    for r in rows:
        cells = []
        for k in ("noop_M", "edit_M", "check_M", "cache_KiB", "lsp_G", "tr_check_M", "tr_edit_M", "cr_noop_M", "cr_edit_M", "pt_check_M", "pt_edit_M"):
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
        ("tr_check_M", "traits check"), ("tr_edit_M", "traits edit"), ("cr_noop_M", "crates no-op"), ("cr_edit_M", "crates edit"),
        ("pt_check_M", "patterns check"), ("pt_edit_M", "patterns edit"))


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


SECTIONS = ("gen5000", "traits", "crates", "patterns", "lsp")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("change")
    ap.add_argument("--lsp", action="store_true")
    ap.add_argument("--dry-run", action="store_true")
    # (the sections side by side, each in its own work directory: an instruction count does not depend on the load.
    # The LSP session runs alone after them, as its count moves with how often the client polls)
    ap.add_argument("--jobs", type=int, default=1, help="measure the gen5000, traits, crates and patterns sections side by side")
    # (some of the sections only, as kpi.yml measures them on runners side by side; the KPIs of the others are left
    # out of what is printed. With --dry-run only: a row with some of the KPIs is not logged)
    ap.add_argument("--only", help=f"comma-separated sections: {','.join(SECTIONS)}")
    a = ap.parse_args()
    only = SECTIONS if a.only is None else a.only.split(",")
    unknown = [x for x in only if x not in SECTIONS]
    if unknown or (a.only is not None and not a.dry_run):
        ap.error(f"--only takes {','.join(SECTIONS)}, with --dry-run" if not unknown else f"no section {unknown[0]}")
    os.makedirs(WORK, exist_ok=True)
    nones = {"gen5000": (None,) * 4, "traits": (None,) * 2, "crates": (None,) * 2, "patterns": (None,) * 2}
    sections = [(k, f) for k, f in (("gen5000", build_kpis), ("traits", traits_kpis), ("crates", crates_kpis),
                                    ("patterns", patterns_kpis)) if k in only]
    if a.jobs > 1:
        from concurrent.futures import ThreadPoolExecutor
        with ThreadPoolExecutor(a.jobs) as ex:
            got = dict(zip([k for k, _ in sections], [f.result() for f in [ex.submit(s) for _, s in sections]]))
    else:
        got = {k: s() for k, s in sections}
    (noop, edit, check, cache), (tr_check, tr_edit), (cr_noop, cr_edit), (pt_check, pt_edit) = (
        got.get(k, nones[k]) for k in ("gen5000", "traits", "crates", "patterns"))
    lsp = lsp_kpi() if (a.lsp and a.only is None) or "lsp" in (a.only or "").split(",") else None
    ui = ui_counts()
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    # (the log itself does not count as a change)
    dirty = [l for l in subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"], capture_output=True, text=True,
                                       cwd=ROOT).stdout.splitlines() if not l.endswith(("bench/kpi_log.tsv", "bench/kpi_log.md"))]
    f3 = lambda v, d=3: "-" if v is None else f"{v:.{d}f}"
    row = {"date": datetime.date.today().isoformat(), "commit": commit + ("+" if dirty else ""), "change": a.change.replace("\t", " "),
           "noop_M": f3(noop), "edit_M": f3(edit), "check_M": f3(check), "cache_KiB": f3(cache, 1),
           "lsp_G": f3(lsp, 4), "run": str(ui[0] or "-"), "check": str(ui[1] or "-"),
           "build": str(ui[2] or "-"), "cfail": str(ui[3] or "-"),
           "tr_check_M": f3(tr_check), "tr_edit_M": f3(tr_edit), "cr_noop_M": f3(cr_noop), "cr_edit_M": f3(cr_edit),
           "pt_check_M": f3(pt_check), "pt_edit_M": f3(pt_edit)}
    rows = read_log()
    prev = rows[-1] if rows else None
    first = rows[0] if rows else row
    for k, name in KPIS:
        v = num(row, k)
        if v is None:
            continue
        ref = first_with(rows, k) if num(first_with(rows, k), k) is not None else row
        # (the LSP session is in G: four decimals, as the log keeps it)
        print(f"{name:14} {v:12.{4 if k == 'lsp_G' else 3}f}   Δ {pct(v, num(prev, k)) if prev else '-':>8}   Σ {pct(v, num(ref, k)):>8}")
    # (as blaze's benchhistory check: against the median of the last 7 rows, a move past the noise band (0.1%, or
    # three times the spread the history shows, whichever is larger) is reported)
    flags = band_check(rows, row)
    for f in flags:
        print(f)
    if a.dry_run:
        return
    new = not os.path.exists(LOG)
    # (a log whose header is from before columns were added: the header gets them, and the rows before get "-")
    if not new:
        lines = open(LOG).read().splitlines()
        if lines and lines[0].split("\t") != COLS and COLS[:len(lines[0].split("\t"))] == lines[0].split("\t"):
            # (written beside it and renamed over it: a failed write leaves the log as it was)
            with open(LOG + ".tmp", "w") as f:
                f.write("\t".join(COLS) + "\n")
                for l in lines[1:]:
                    if l.strip():
                        cells = l.split("\t")
                        f.write("\t".join(cells + ["-"] * (len(COLS) - len(cells))) + "\n")
            os.replace(LOG + ".tmp", LOG)
    with open(LOG, "a") as f:
        if new:
            f.write("\t".join(COLS) + "\n")
        f.write("\t".join(row[c] for c in COLS) + "\n")
    write_md(read_log())


if __name__ == "__main__":
    main()
