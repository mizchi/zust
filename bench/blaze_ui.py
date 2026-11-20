"""blaze (indexable-inc/blaze) on a sample of rustc's tests/ui, next to zustc on the same tests.

usage: python3 bench/blaze_ui.py [--per-kind N] [--seed S]
  (run from the repository root after `python3 bench/rust_ui.py` has written out/rust_ui_results.txt; blaze from the
   PATH or $BLAZE)

The tests are the ones bench/rust_ui.py counts as applicable (the same filter), N of each of run-pass, check-pass and
build-pass, picked with a fixed seed. Each is put as src/main.rs of one cargo package (the edition from its
`//@ edition:` line, else 2015 as compiletest's default) and built with `blaze build`, the daemon kept up between the
tests (a fresh package after a failed build: blaze's incremental link state breaks with it). A test that uses the C
helpers compiletest links into every test (tests/auxiliary/rust_test_helpers.c) gets them through a build script, as
zustc gets them through -L. run-pass must also exit 0 and print its `.run.stdout` if it has one; the others must build.
compile-fail is
left out: blaze reports rust-analyzer's diagnostics, whose lines this harness does not compare.
zustc's result on each test is read from out/rust_ui_results.txt (the last full run of bench/rust_ui.py).
"""
import argparse
import os
import random
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import rust_ui  # noqa: E402

ROOT = rust_ui.ROOT
BLAZE = os.environ.get("BLAZE") or shutil.which("blaze") or os.path.expanduser("~/.cargo/bin/blaze")
W = os.path.join(ROOT, "out/blaze_ui")
KINDS = ("run-pass", "check-pass", "build-pass")


def package(edition):
    shutil.rmtree(W, ignore_errors=True)
    os.makedirs(os.path.join(W, "src"))
    with open(os.path.join(W, "Cargo.toml"), "w") as f:
        f.write(f'[package]\nname = "t"\nversion = "0.0.0"\nedition = "{edition}"\n\n[workspace]\n')
        # (blaze's build-script sandbox: the helpers' directory may be linked from)
        if rust_ui.HELPERS:
            f.write(f'\n[workspace.metadata.blaze.sandbox]\nbuild-script-link-dirs = ["{rust_ui.HELPERS}"]\n')


def run_one(path, kind, edition_now):
    text = open(path, errors="replace").read()
    eds = rust_ui.directives(text).get("edition", [])
    # (`2015..2021`: a range of editions, the first one)
    edition = (eds[0].split("..")[0].strip() if eds else "2015") or "2015"
    helpers = "rust_test_helpers" in text and rust_ui.HELPERS
    if edition != edition_now or helpers or os.path.exists(os.path.join(W, "build.rs")):
        package(edition)
        # (the C helpers compiletest links into every test, as zustc gets them through -L; blaze build scripts run in a
        # sandbox, so the manifest names their directory)
        if helpers:
            with open(os.path.join(W, "build.rs"), "w") as f:
                f.write(f'fn main() {{ println!("cargo:rustc-link-search=native={rust_ui.HELPERS}"); println!("cargo:rustc-link-lib=static=rust_test_helpers"); }}\n')
    shutil.copy(path, os.path.join(W, "src/main.rs"))
    # (blaze, like git, does not trust a file whose stat stamp is within ~20 ms of its change: wait it out, so that the
    # build sees the new main.rs; and remove the last test's binary, so that a build that kept it cannot pass)
    time.sleep(0.05)
    exe = os.path.join(W, "target/debug/t")
    if os.path.exists(exe):
        os.remove(exe)
    try:
        r = subprocess.run([BLAZE, "build"], cwd=W, capture_output=True, text=True, timeout=120)
    except subprocess.TimeoutExpired:
        return "timeout", edition
    if r.returncode != 0:
        last = next((l for l in r.stderr.splitlines() if l.startswith("error")), "")
        # (a failed link leaves blaze's incremental link state broken: every later link of the package failed too.
        # The next test starts from a fresh package)
        return "fail(build: " + last[:80] + ")", None
    if kind != "run-pass":
        return "pass", edition
    try:
        p = subprocess.run([*rust_ui.LIMIT, exe], capture_output=True, timeout=20, cwd=W)
    except subprocess.TimeoutExpired:
        return "timeout", edition
    if p.returncode != 0:
        return f"fail(run: exit {p.returncode} {p.stderr[-80:]!r})", edition
    want = os.path.splitext(path)[0] + ".run.stdout"
    if os.path.exists(want) and p.stdout != open(want, "rb").read():
        return "fail(run: stdout)", edition
    return "pass", edition


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--per-kind", type=int, default=100)
    ap.add_argument("--seed", type=int, default=1)
    a = ap.parse_args()
    zres = {}
    for line in open(os.path.join(ROOT, "out/rust_ui_results.txt")):
        st, kind, rel = line.rstrip("\n").split("\t")
        zres[(kind, rel)] = st
    rust_ui.HELPERS = rust_ui.test_helpers()
    ui = os.path.join(rust_ui.TESTS, "tests/ui")
    rng = random.Random(a.seed)
    edition = None
    rows = []
    for kind in KINDS:
        pool = sorted(rel for (k, rel) in zres if k == kind)
        sample = rng.sample(pool, min(a.per_kind, len(pool)))
        nb = nz = 0
        for rel in sample:
            b, edition = run_one(os.path.join(ui, rel), kind, edition)
            z = zres[(kind, rel)]
            nb += b == "pass"
            nz += z == "pass"
            print(f"{kind}\t{rel}\tblaze={b}\tzustc={z}", flush=True)
        rows.append((kind, len(sample), nb, nz))
    v = subprocess.run([BLAZE, "--version"], capture_output=True, text=True).stdout.strip()
    zc = subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()
    lines = [
        f"{v} / zustc {zc} / tests/ui at rustc {rust_ui.TAG}: {a.per_kind} applicable tests of each kind (all of a kind with",
        f"fewer; seed {a.seed}), the same tests for both (`python3 bench/blaze_ui.py --per-kind {a.per_kind}`)",
        "",
        "| kind | sample | blaze | zustc |",
        "|---|---:|---:|---:|",
    ]
    for kind, n, nb, nz in rows:
        lines.append(f"| {kind} | {n} | {nb} ({100 * nb / max(n, 1):.1f}%) | {nz} ({100 * nz / max(n, 1):.1f}%) |")
    text = "\n".join(lines) + "\n"
    print("\n" + text)
    # (for README's comparison table)
    with open(os.path.join(ROOT, "bench/blaze_ui_result.md"), "w") as f:
        f.write(text)


if __name__ == "__main__":
    main()
