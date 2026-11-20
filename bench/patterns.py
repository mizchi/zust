#!/usr/bin/env python3
"""Benchmarks over the program shapes of bench/gen_patterns.py, one row per pattern: does zustc build it and print
what rustc's build prints, and how long (or how many instructions) do a cold build, `check` and a build after editing
one function take.

usage: python3 bench/patterns.py [--n 4] [--patterns interp,records,..|all] [--reps 3] [--instructions]
                                 [--json out.json] [--no-rustc]
(from the repository root, after `cargo build --release`: it runs target/release/zustc)

- n: groups per pattern (each pattern's code repeated n times under its own names; `mix` has every pattern n times).
- The times are the median of --reps runs (wall clock: they move by 10-30% on a shared machine). With
  --instructions, `check` and the edit build are also counted by cachegrind (deterministic to ~0.1%; needs valgrind).
- rustc's output is the reference (rustc --edition 2021 -O), and rustc must be on PATH; --no-rustc only builds and
  runs with zustc (then nothing checks the output).
- The table and --json are all it writes: bench/kpi_track.py is what appends KPI rows to the log.
The exit status is 1 when a pattern does not build (or an edit of it does not), a program exits with an error, or it
prints something else than rustc's build.
"""
import argparse, json, os, shutil, statistics, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "bench"))
from ci_bench import ZUSTC, instructions  # noqa: E402
import gen_patterns  # noqa: E402


def write(path, pattern, n, variant):
    with open(path, "w") as f:
        f.write(gen_patterns.program(pattern, n, variant))


def measure(pattern, n, reps, vg, rustc, work):
    d = os.path.join(work, pattern)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(d)
    src, exe = os.path.join(d, "p.rs"), os.path.join(d, "p")
    write(src, pattern, n, 0)
    row = {"pattern": pattern, "lines": sum(1 for _ in open(src))}
    build = [ZUSTC, src, "-o", exe]
    r = subprocess.run(build + ["--no-cache"], capture_output=True, text=True)
    if r.returncode != 0:
        row["error"] = (r.stderr.strip().splitlines() or ["build failed"])[0]
        return row
    run = subprocess.run([exe], capture_output=True, text=True)
    if run.returncode != 0:
        row["error"] = f"zustc's build exited with {run.returncode}"
        return row
    if rustc:
        ref = os.path.join(d, "ref")
        rc = subprocess.run(["rustc", "--edition", "2021", "-Awarnings", "-O", src, "-o", ref], capture_output=True, text=True)
        if rc.returncode != 0:
            row["error"] = "rustc: " + (rc.stderr.strip().splitlines() or ["build failed"])[0]
            return row
        want = subprocess.run([ref], capture_output=True, text=True)
        if want.returncode != 0:
            row["error"] = f"rustc's build exited with {want.returncode}"
            return row
        row["same_as_rustc"] = run.stdout == want.stdout
    # (a phase whose command fails is a failed row naming the phase, with that run's own stderr; the samples taken
    # before it stay in the row, and the rows of the other patterns are kept)
    def timed(phase, cmd):
        t = time.perf_counter()
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode != 0:
            raise PhaseError(f"{phase}: " + (r.stderr.strip().splitlines() or [f"exit {r.returncode}"])[0])
        return (time.perf_counter() - t) * 1e3

    def counted(phase, cmd, env=None):
        try:
            return instructions(cmd, env=env)
        except RuntimeError as e:
            raise PhaseError(f"{phase}: {str(e).splitlines()[0]}")

    # (key -> the samples so far; a key's value is their median, or for counts their least)
    samples = {}

    def take(key, phase, cmd, env=None):
        v = counted(phase, cmd, env) if key.endswith("_M") else timed(phase, cmd)
        samples.setdefault(key, []).append(v)

    try:
        for _ in range(reps):
            take("cold_ms", "cold build", build + ["--no-cache"])
        for _ in range(reps):
            take("check_ms", "check", [ZUSTC, "check", src])
        timed("build", build)
        for k in range(reps):
            write(src, pattern, n, k % 2 + 1)
            take("edit_ms", "edit", build)
        if vg:
            write(src, pattern, n, 0)
            timed("build", build)
            for _ in range(2):
                take("check_M", "check (counted)", [ZUSTC, "check", src])
            # (as kpi_track.py: a build under valgrind does not take the stamps' shortcut, and leaves the cache it reads)
            counted("build (counted)", build, env={"ZUSTC_NO_STAMP": "1"})
            write(src, pattern, n, 3)
            take("edit_M", "edit (counted)", build)
    except PhaseError as e:
        row["error"] = str(e)
    for k, v in samples.items():
        row[k] = min(v) if k.endswith("_M") else statistics.median(v)
    return row


class PhaseError(Exception):
    pass


def table(rows, vg):
    cols = ["pattern", "lines", "same as rustc", "cold build ms", "check ms", "edit 1 fn ms"] + (["check M instr", "edit M instr"] if vg else [])
    out = ["| " + " | ".join(cols) + " |", "|---|" + "---:|" * (len(cols) - 1)]
    for r in rows:
        if "error" in r:
            out.append(f"| {r['pattern']} | {r['lines']} | build failed: {r['error']} |" + " |" * (len(cols) - 3))
            continue
        same = {True: "yes", False: "**no**"}.get(r.get("same_as_rustc"), "-")
        cells = [r["pattern"], str(r["lines"]), same, f"{r['cold_ms']:.0f}", f"{r['check_ms']:.0f}", f"{r['edit_ms']:.0f}"]
        if vg:
            cells += [f"{r['check_M']:.1f}", f"{r['edit_M']:.1f}"]
        out.append("| " + " | ".join(cells) + " |")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser()
    def at_least_1(s):
        v = int(s)
        if v < 1:
            raise argparse.ArgumentTypeError(f"must be at least 1, not {v}")
        return v
    ap.add_argument("--n", type=at_least_1, default=4)
    ap.add_argument("--patterns", default="all")
    ap.add_argument("--reps", type=at_least_1, default=3)
    ap.add_argument("--instructions", action="store_true")
    ap.add_argument("--json")
    ap.add_argument("--no-rustc", action="store_true")
    a = ap.parse_args()
    names = list(gen_patterns.PATTERNS) + ["mix"] if a.patterns == "all" else a.patterns.split(",")
    for n in names:
        if n not in gen_patterns.PATTERNS and n != "mix":
            sys.exit(f"unknown pattern {n} (one of {', '.join(gen_patterns.PATTERNS)}, mix)")
    rustc = not a.no_rustc
    if rustc and shutil.which("rustc") is None:
        sys.exit("rustc is not on PATH: it gives the output to compare with (pass --no-rustc to skip the comparison)")
    work = tempfile.mkdtemp(prefix="zust-patterns-")
    try:
        rows = []
        for n in names:
            rows.append(measure(n, a.n, a.reps, a.instructions, rustc, work))
            print(f"{n}: {rows[-1]}", file=sys.stderr)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    print(table(rows, a.instructions))
    if a.json:
        json.dump({"n": a.n, "rows": rows}, open(a.json, "w"), indent=1)
    bad = [r["pattern"] for r in rows if "error" in r or r.get("same_as_rustc") is False]
    if bad:
        print(f"failed: {', '.join(bad)}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
