#!/usr/bin/env python3
"""Can zustc build and run programs that use popular crates.io crates, with the same output as rustc?

usage: python3 cratebench/popular/run.py [CRATE ...] [--json PATH] [--markdown PATH] [--rustc-only]
       (from the repository root, after `cargo build --release --workspace`; needs the network for `cargo fetch`)

Each apps/<crate>/ is a small program that uses the crate's main API and prints deterministic text (Cargo.toml pins
the crate with `=`, Cargo.lock pins the rest). For each one:
  1. `cargo run --locked --release` builds it with rustc: its stdout is the expected output.
  2. `zurgo run` builds the same package with zustc (Cargo.toml, Cargo.lock, features and build scripts as cargo
     uses them) and runs it.
  3. status: pass (same stdout) | mismatch | run-fail (exit code) | build-fail | timeout | rustc-fail
     For a build failure, `failing_crate` is the crates.io package the first error points into (or the app itself).
--rustc-only checks step 1 alone (on any platform, e.g. to verify a new app).
Results: out/popular/<crate>/ (expected.txt, actual.txt, zurgo.log) and the --json / --markdown summaries.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
ZURGO = os.path.join(ROOT, "target/release/zurgo")
OUT = os.path.join(ROOT, "out/popular")
TIMEOUT = 900
REG = re.compile(r"index\.crates\.io-[0-9a-f]+/([A-Za-z0-9_.-]+?)-(\d+\.\d+\.\d+[^/]*)/")


def run(cmd, cwd, env=None, timeout=TIMEOUT):
    t = time.time()
    try:
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env={**os.environ, **(env or {})}, timeout=timeout)
        return p.returncode, p.stdout, p.stderr, time.time() - t
    except subprocess.TimeoutExpired as e:
        return None, e.stdout or "", f"timeout ({timeout} s)", time.time() - t


def pinned(app_dir):
    """the crate this app is about and its pinned version (the first `=` dependency)"""
    m = re.search(r'^([\w-]+) = (?:\{ version = )?"=([^"]+)"', open(os.path.join(app_dir, "Cargo.toml")).read(), re.M)
    return (m.group(1), m.group(2)) if m else (os.path.basename(app_dir), "?")


def first_error(log):
    lines = log.splitlines()
    for i, l in enumerate(lines):
        if l.startswith("error"):
            where = next((x for x in lines[i + 1:i + 4] if "-->" in x), "")
            return l.strip(), where.strip()
    return (lines[-1].strip() if lines else ""), ""


def check(app, rustc_only):
    d = os.path.join(HERE, "apps", app)
    name, version = pinned(d)
    out = os.path.join(OUT, app)
    os.makedirs(out, exist_ok=True)
    r = dict(crate=name, version=version, status="", failing_crate=None, first_error=None, error_at=None,
             rustc_seconds=None, zustc_seconds=None)
    code, _, err, _ = run(["cargo", "fetch", "--locked"], d)
    if code != 0:
        r.update(status="rustc-fail", first_error=f"cargo fetch: {err.strip()[-300:]}")
        return r
    code, expected, err, secs = run(["cargo", "run", "--locked", "--release", "-q"], d,
                                    {"CARGO_TARGET_DIR": os.path.join(OUT, "cargo-target")})
    r["rustc_seconds"] = round(secs, 1)
    open(os.path.join(out, "expected.txt"), "w").write(expected)
    if code != 0:
        r.update(status="rustc-fail", first_error=err.strip()[-300:])
        return r
    if rustc_only:
        r["status"] = "rustc-ok"
        return r
    code, actual, err, secs = run([ZURGO, "run"], d, {"CARGO_TARGET_DIR": os.path.join(out, "target")})
    r["zustc_seconds"] = round(secs, 1)
    open(os.path.join(out, "actual.txt"), "w").write(actual)
    open(os.path.join(out, "zurgo.log"), "w").write(err)
    built = os.path.exists(os.path.join(out, "target", "zust", "debug"))
    if code is None:
        r["status"] = "timeout"
    elif code == 0:
        r["status"] = "pass" if actual == expected else "mismatch"
        if r["status"] == "mismatch":
            diff = next((f"line {i + 1}: expected {a!r}, got {b!r}" for i, (a, b) in
                         enumerate(zip(expected.splitlines() + [""] * 999, actual.splitlines() + [""] * 999)) if a != b), "")
            r["first_error"] = diff
    else:
        msg, where = first_error(err)
        failed_build = not actual and ("error" in err.lower()) and not re.search(r"panicked at|Aborted|Segmentation", err)
        r["status"] = "build-fail" if failed_build or not built else "run-fail"
        r["first_error"], r["error_at"] = msg[:300], where[:200]
        m = REG.search(where) or REG.search(err)
        r["failing_crate"] = f"{m.group(1)} {m.group(2)}" if m else (app if r["status"] == "build-fail" else None)
    return r


def markdown(rows):
    n = sum(r["status"] == "pass" for r in rows)
    out = [f"## Popular crates: {n} / {len(rows)} run with the same output as rustc\n",
           "| crate | status | where it stops | first error | rustc s | zustc s |", "|---|---|---|---|---:|---:|"]
    for r in rows:
        e = (r["first_error"] or "").replace("|", "\\|")
        out.append(f'| {r["crate"]} {r["version"]} | {r["status"]} | {r["failing_crate"] or ""} | {e[:160]} | '
                   f'{r["rustc_seconds"] or ""} | {r["zustc_seconds"] or ""} |')
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("crates", nargs="*")
    ap.add_argument("--json")
    ap.add_argument("--markdown")
    ap.add_argument("--rustc-only", action="store_true")
    a = ap.parse_args()
    apps = a.crates or sorted(os.listdir(os.path.join(HERE, "apps")))
    if not a.rustc_only and not os.path.exists(ZURGO):
        sys.exit(f"{ZURGO} is missing: run cargo build --release --workspace first")
    rows = []
    for app in apps:
        r = check(app, a.rustc_only)
        print(f'{r["crate"]:12} {r["status"]:10} {r["failing_crate"] or ""} {(r["first_error"] or "")[:120]}', flush=True)
        rows.append(r)
    md = markdown(rows)
    print("\n" + md)
    if a.json:
        os.makedirs(os.path.dirname(os.path.abspath(a.json)), exist_ok=True)
        json.dump(rows, open(a.json, "w"), indent=1)
    if a.markdown:
        open(a.markdown, "w").write(md)


if __name__ == "__main__":
    main()
