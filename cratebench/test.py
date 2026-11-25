"""Runs the tests of the cratebench crates (the upstream repositories in vendor/) with `cargo test` and `zurgo test`, and compares them test by test.

usage: python3 cratebench/test.py [NAME ..] [--check] [--write]   (at the repository root, after cargo build --release -p zurgo and
       git submodule update --init cratebench/vendor)
  NAME: a name in vendor/. All when omitted (with --check, those in test_passing.txt)
  --check: exits with 1 if even one crate in test_passing.txt disagrees with cargo (CI uses this)
  --write: writes the result table to test_result.md (the rows of crates not run are kept) and adds the crates that matched cargo to test_passing.txt (never removes any)

Both run the same things: the tests of lib, bin and tests/ (`--lib --bins --tests`; doc tests and benches are outside the scope of #96).
The crates are copied to out/cratebench-test/<NAME>/, with locks/<NAME>.lock put in as Cargo.lock (`--locked`; cargo and zurgo
use the same versions; for those not in locks/, the one cargo made is kept in locks/). What is compared is the result per (target, test name):
ok / FAILED / ignored. `[workspace]` is added to the copy's Cargo.toml (since out/ is inside the zust workspace). Targets are looked up from the src_path of `cargo metadata` (cargo's `Running <path>` lines).
Tests that zurgo does not run (#[ignore]) are counted by the `(ignored)` of `zurgo test -- --list`.
"""
import json
import os
import re
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ZURGO = os.path.join(ROOT, "target/release/zurgo")
OUT = os.path.join(ROOT, "out/cratebench-test")
LOCKS = os.path.join(HERE, "locks")
PASSING = os.path.join(HERE, "test_passing.txt")
RESULT = os.path.join(HERE, "test_result.md")
SELECT = ["--lib", "--bins", "--tests"]
TIMEOUT = 1800


def crates():
    return sorted(n for n in os.listdir(os.path.join(HERE, "vendor")) if os.path.isfile(os.path.join(HERE, "vendor", n, "Cargo.toml")))


def passing():
    if not os.path.exists(PASSING):
        return []
    return [l.strip() for l in open(PASSING) if l.strip() and not l.startswith("#")]


def run(cmd, cwd, env=None, merge=False):
    """(exit code or None on a timeout, stdout, stderr). merge: stderr into stdout, in the order written"""
    # (no colors: CI sets CARGO_TERM_COLOR=always, and the lines read here are matched as text)
    e = dict(os.environ, CARGO_TERM_COLOR="never", **(env or {}))
    try:
        p = subprocess.run(cmd, cwd=cwd, env=e, stdout=subprocess.PIPE, stderr=subprocess.STDOUT if merge else subprocess.PIPE, text=True, timeout=TIMEOUT)
        return p.returncode, p.stdout, p.stderr or ""
    except subprocess.TimeoutExpired as x:
        # (what was captured before the timeout is bytes, even with text=True)
        text = lambda b: b.decode(errors="replace") if isinstance(b, bytes) else (b or "")
        return None, text(x.stdout), text(x.stderr) + f"\n(timeout after {TIMEOUT}s)"


def label(t):
    """zurgo's name of a target's part: `(lib)`, `(bin x)`, `(test x)`"""
    k = t["kind"][0]
    return "(lib)" if k in ("lib", "rlib", "proc-macro") else f"({k} {t['name']})"


def cargo_outcomes(out, paths):
    """`cargo test`'s results: (target, test) -> ok / FAILED / ignored. A `Running <path> (..)` line names the target"""
    m, cur = {}, None
    for l in out.splitlines():
        s = l.strip()
        r = re.match(r"Running (?:unittests )?(\S+) \(", s)
        if r:
            cur = paths.get(os.path.normpath(r.group(1)), r.group(1))
            continue
        if s.startswith("Doc-tests "):
            cur = None
            continue
        r = re.match(r"test (.+?)(?: - should panic)? \.\.\. (ok|FAILED|ignored)(?:, .*)?$", s)
        if r and cur is not None:
            m[(cur, r.group(1))] = r.group(2)
    return m


def zurgo_outcomes(err, listed):
    """`zurgo test`'s results: its `PASS [..] (k/n) <pkg> (<target>) <test>` lines, and the ignored ones it lists"""
    m = {}
    for l in err.splitlines():
        r = re.match(r"\s*(PASS|FAIL|TIMEOUT|SIGNAL)\b.*?\] \(\d+/\d+\) \S+ (\((?:lib|bin [^)]*|test [^)]*)\)) (\S+)$", l)
        if r:
            m[(r.group(2), r.group(3))] = "ok" if r.group(1) == "PASS" else "FAILED"
    for l in listed.splitlines():
        r = re.match(r"\S+ (\((?:lib|bin [^)]*|test [^)]*)\)): (\S+) \(ignored\)$", l)
        if r:
            m[(r.group(1), r.group(2))] = "ignored"
    return m


def first_error(text):
    """the first `error` line (the home directory written `~`, as the result is kept in the repository)"""
    lines = [l.strip() for l in text.splitlines() if l.strip()]
    e = next((l for l in lines if l.startswith("error")), lines[-1] if lines else "(no output)")
    return e.replace(os.path.expanduser("~"), "~")


def one(name):
    """(status, detail, cargo's outcomes, zurgo's outcomes) of one crate. status: match / differs / zurgo-fails / cargo-fails"""
    d = os.path.join(OUT, name)
    # (removed only inside OUT: main takes only vendor/'s names, this checks it again before deleting)
    if os.path.dirname(os.path.realpath(d)) != os.path.realpath(OUT):
        sys.exit(f"internal: {d} is not in {OUT}")
    if os.path.isdir(d):
        shutil.rmtree(d)
    shutil.copytree(os.path.join(HERE, "vendor", name), os.path.join(d, "src"), ignore=shutil.ignore_patterns(".git"))
    pkg = os.path.join(d, "src")
    # (out/ is inside zust's workspace: the copy is a workspace of its own, for cargo and zurgo alike)
    man = os.path.join(pkg, "Cargo.toml")
    text = open(man).read()
    if not re.search(r"^\[workspace\]", text, re.M):
        open(man, "w").write(text.rstrip("\n") + "\n\n[workspace]\n")
    lock = os.path.join(LOCKS, f"{name}.lock")
    if os.path.exists(lock):
        shutil.copy(lock, os.path.join(pkg, "Cargo.lock"))
        locked = ["--locked"]
    else:
        locked = []
    # (cargo writes `Running <path>` to stderr, and the tests' results to stdout)
    code, out, _ = run(["cargo", "test", "--no-fail-fast", *locked, *SELECT], pkg, {"CARGO_TARGET_DIR": os.path.join(d, "cargo")}, merge=True)
    if code is None or "test result:" not in out:
        return "cargo-fails", first_error(out), {}, {}
    if not locked:
        os.makedirs(LOCKS, exist_ok=True)
        shutil.copy(os.path.join(pkg, "Cargo.lock"), lock)
    code_m, meta, _ = run(["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"], pkg)
    paths = {}
    if code_m == 0:
        for p in json.loads(meta)["packages"]:
            for t in p["targets"]:
                paths[os.path.normpath(os.path.relpath(t["src_path"], pkg))] = label(t)
    want = cargo_outcomes(out, paths)
    env = {"CARGO_TARGET_DIR": os.path.join(d, "zurgo")}
    code, _, zerr = run([ZURGO, "test", "--no-fail-fast", *SELECT], pkg, env)
    if code is None or (code != 0 and "Summary [" not in zerr):
        return "zurgo-fails", first_error(zerr), want, {}
    _, listed, _ = run([ZURGO, "test", *SELECT, "--", "--list"], pkg, env)
    got = zurgo_outcomes(zerr, listed)
    if got == want:
        return "match", f"テスト {len(want)} 個", want, got
    diff = [f"{t} {n}: cargo {want.get((t, n), '-')}, zurgo {got.get((t, n), '-')}" for (t, n) in sorted(set(want) | set(got)) if want.get((t, n)) != got.get((t, n))]
    return "differs", "; ".join(diff[:6]) + (f" (+{len(diff) - 6})" if len(diff) > 6 else ""), want, got


def old_rows():
    """test_result.md's rows, by crate"""
    if not os.path.exists(RESULT):
        return {}
    rows = {}
    for l in open(RESULT):
        r = re.match(r"\| (\S+) \| (match|differs|zurgo-fails|cargo-fails) \|", l)
        if r:
            rows[r.group(1)] = l.rstrip("\n")
    return rows


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    check, write = "--check" in sys.argv, "--write" in sys.argv
    if not os.path.exists(ZURGO):
        sys.exit(f"no {ZURGO}: cargo build --release -p zurgo first")
    names = args or (passing() if check else crates())
    # (a name is one of vendor/'s directories, never a path: its copy in out/ is removed before each run)
    known = set(os.listdir(os.path.join(HERE, "vendor")))
    unknown = [n for n in names if n not in known or os.sep in n or n in (".", "..")]
    if unknown:
        sys.exit(f"not a crate of cratebench/vendor: {', '.join(unknown)}")
    missing = [n for n in names if n not in crates()]
    if missing:
        sys.exit(f"not checked out: {', '.join(missing)} (git submodule update --init cratebench/vendor)")
    rows = []
    for n in names:
        status, detail, want, got = one(n)
        counts = {k: sum(1 for v in want.values() if v == k) for k in ("ok", "FAILED", "ignored")}
        rows.append((n, status, counts, detail))
        print(f"{n}: {status}: {detail}", flush=True)
    if write:
        # (the crates not run this time keep their rows)
        table = old_rows()
        for n, status, c, detail in rows:
            cell = detail.replace("|", "\\|")
            table[n] = f"| {n} | {status} | {c['ok']} / {c['FAILED']} / {c['ignored']} | {cell} |"
        with open(RESULT, "w") as f:
            f.write("# cratebench: `zurgo test` と `cargo test`\n\n`python3 cratebench/test.py --write` の結果 "
                    "(lib・bin・tests/ のテスト。doc テストは数えない)。cargo の列は cargo での結果の数。\n\n")
            f.write("| クレート | 結果 | cargo: ok / FAILED / ignored | 詳細 |\n|---|---|---:|---|\n")
            f.writelines(table[n] + "\n" for n in sorted(table))
        keep = sorted(set(passing()) | {n for n, s, _, _ in rows if s == "match"})
        with open(PASSING, "w") as f:
            f.write("# cratebench のクレートのうち、zurgo test が cargo test とテストごとに一致するもの "
                    "(cratebench/test.py --check が守る。--write は足すだけ)\n")
            f.writelines(f"{n}\n" for n in keep)
    if check:
        bad = [n for n, s, _, _ in rows if s != "match"]
        if bad:
            sys.exit(f"no longer like cargo test: {', '.join(bad)}")


if __name__ == "__main__":
    main()
