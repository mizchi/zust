#!/usr/bin/env python3
"""Run zustc against rustc's own UI test suite (rust-lang/rust `tests/ui`) and report how much of it passes.

usage: python3 bench/rust_ui.py --fetch                 sparse-clone tests/ui at the pinned commit into out/rust-tests
       python3 bench/rust_ui.py [--kinds run-pass,check-pass,compile-fail] [--limit N] [--jobs N]
                                [--json out/rust_ui.json] [--results out/rust_ui_results.txt] [--filter SUBSTR]
(run from the repository root after cargo build --release)

Each test's kind comes from its header directives, as in rustc's compiletest:
- run-pass: zustc builds it, the program exits 0, and its stdout equals `<name>.run.stdout` when that file exists
- check-pass / build-pass: `zustc check` accepts it (build-pass also builds it)
- compile-fail (no pass/fail directive; errors marked with `//~ ERROR`): `zustc check` rejects it with an error on a
  line an annotation points at. Rejections that only report other lines are counted as "wrong line"
Each is compiled in its `//@ edition:` (`--edition=`), 2015 without one, as compiletest does.

Tests zustc cannot be asked to handle are counted as "not applicable", not as failures: nightly `#![feature(..)]`,
auxiliary crates (`aux-build`, `aux-crate`), `compile-flags` other than `--test` and `--cfg`, `revisions`, `needs-*` other than what Linux x86-64 has,
`only-*` / `ignore-*` that exclude Linux x86-64, and `ignore-test`. A zustc panic or signal is counted as a crash.
"""
import argparse, json, os, re, subprocess, sys, tempfile, time
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ZUSTC = os.path.join(ROOT, "target/release/zustc")
TESTS = os.path.join(ROOT, "out/rust-tests")
# (rust-lang/rust 1.100.0: the rustc this repository is developed with)
REPO = "https://github.com/rust-lang/rust"
TAG = "1.100.0"
COMMIT = "0ef1bc386059596dada8fb65b50425aab2ea637a"

PASS_KINDS = ("run-pass", "check-pass", "build-pass", "run-fail", "check-fail", "build-fail", "run-crash")


def fetch():
    if os.path.isdir(os.path.join(TESTS, ".git")):
        head = subprocess.run(["git", "-C", TESTS, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
        if head == COMMIT:
            print(f"{TESTS} is already at {TAG} ({COMMIT[:9]})")
            return
        raise SystemExit(f"{TESTS} is at {head}, not {COMMIT}; remove it and fetch again")
    os.makedirs(os.path.dirname(TESTS), exist_ok=True)
    subprocess.run(["git", "-c", "advice.detachedHead=false", "clone", "-q", "--depth", "1", "--filter=blob:none", "--sparse",
                    "--branch", TAG, REPO, TESTS], check=True)
    subprocess.run(["git", "-C", TESTS, "sparse-checkout", "set", "tests/ui", "tests/auxiliary"], check=True)
    head = subprocess.run(["git", "-C", TESTS, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    if head != COMMIT:
        raise SystemExit(f"tag {TAG} is {head}, expected {COMMIT}")
    print(f"fetched tests/ui at {TAG} ({COMMIT[:9]}) into {TESTS}")


def directives(text):
    """`//@ name` / `//@ name: value` lines at the top of the file"""
    out = {}
    for m in re.finditer(r"^\s*//@\s*([\w-]+)(?:\s*:\s*(.*))?$", text, re.M):
        out.setdefault(m.group(1), []).append((m.group(2) or "").strip())
    return out


def extra_flags(text):
    """The `//@ compile-flags:` zustc takes, as arguments to pass on: `--test` and `--cfg NAME`. None when there are
    others"""
    words = " ".join(directives(text).get("compile-flags", [])).split()
    out, i = [], 0
    while i < len(words):
        w = words[i]
        if w == "--test":
            out.append(w)
        elif w == "--cfg" and i + 1 < len(words):
            out += [w, words[i + 1]]
            i += 1
        else:
            return None
        i += 1
    return out


def classify(path):
    """(kind, reason-if-not-applicable)"""
    text = open(path, errors="replace").read()
    ds = directives(text)
    kind = next((k for k in PASS_KINDS if k in ds), "compile-fail")
    if "ignore-test" in ds:
        return kind, "ignore-test"
    if "#![feature(" in text:
        return kind, "nightly feature"
    for d in ("aux-build", "aux-crate", "aux-bin", "proc-macro"):
        if d in ds:
            return kind, "auxiliary crate"
    if "revisions" in ds:
        return kind, "revisions"
    if (ds.get("compile-flags") and extra_flags(text) is None) or "rustc-env" in ds or "exec-env" in ds:
        return kind, "compile flags / env"
    for d in ds:
        if d.startswith("only-") and d not in ("only-x86_64", "only-linux", "only-unix", "only-64bit", "only-gnu", "only-elf"):
            return kind, "only other targets"
        if d.startswith("ignore-") and d in ("ignore-x86_64", "ignore-linux", "ignore-unix", "ignore-64bit", "ignore-gnu", "ignore-elf"):
            return kind, "ignored on this target"
        if d.startswith("needs-") and d not in ("needs-unwind", "needs-threads", "needs-dynamic-linking"):
            return kind, "needs other support"
    if kind == "compile-fail" and "//~" not in text:
        return kind, "no error annotation"
    if kind in ("run-fail", "check-fail", "build-fail", "run-crash"):
        return kind, "other kind"
    return kind, None


# (memory limit per test, so that a runaway compile (e.g. endless monomorphization) cannot take the machine down)
LIMIT = ["prlimit", "--as=2147483648", "--"] if subprocess.run(["sh", "-c", "command -v prlimit"], capture_output=True).returncode == 0 else []


def error_lines(text):
    """Lines that `//~ ERROR` annotations point at: `//~` this line, `//~^` one line up per caret, `//~v` one line down
    per v, `//~|` the same line as the previous annotation (of any kind, as compiletest: `//~^ NOTE` then `//~| ERROR`
    is an error on the note's line)"""
    out, last = set(), None
    for i, line in enumerate(text.split("\n"), 1):
        m = re.search(r"//~(\^+|v+|\||)\s*([A-Za-z]+)", line)
        if not m:
            continue
        mark = m.group(1)
        if mark == "|":
            at = last
        elif mark.startswith("v"):
            at = i + len(mark)
        else:
            at = i - len(mark)
        last = at
        if at is not None and m.group(2) in ("ERROR", "error"):
            out.add(at)
    return out


def crashed(r):
    return r.returncode < 0 or "panicked at" in r.stderr or "RUST_BACKTRACE" in r.stderr


def test_helpers():
    """compiletest links every test with tests/auxiliary/rust_test_helpers.c (as librust_test_helpers.a, found
    through `-L`): build it once, into out/rust_test_helpers/"""
    c = os.path.join(TESTS, "tests/auxiliary/rust_test_helpers.c")
    if not os.path.exists(c):
        return None
    d = os.path.join(ROOT, "out/rust_test_helpers")
    a = os.path.join(d, "librust_test_helpers.a")
    if not os.path.exists(a) or os.path.getmtime(a) < os.path.getmtime(c):
        os.makedirs(d, exist_ok=True)
        o = os.path.join(d, "rust_test_helpers.o")
        subprocess.run(["cc", "-c", "-O2", "-fPIC", c, "-o", o], check=True)
        subprocess.run(["ar", "crs", a, o], check=True)
    return d


HELPERS = None


def edition(text):
    """The edition the test is compiled in, as compiletest passes it: `//@ edition: 2018` (the lower end of a range
    `2018..`), 2015 without one"""
    m = re.search(r"^\s*//@\s*edition\s*:\s*(\d{4})", text, re.M)
    return m.group(1) if m else "2015"


# (path -> why the test did not pass, for --reasons: zustc's first error, or how the run failed)
REASONS = {}


def first_error(stderr):
    """zustc's first error message, without its `error: <file>: <line>:<col>: ` prefix"""
    for line in stderr.splitlines():
        if line.startswith("error"):
            m = re.match(r"error(?:\[E\d+\])?: (?:\S+?\.rs:\s?)?(?:\d+:\d+:\s?)?(.*)", line)
            return (m.group(1) if m else line)[:200]
    return stderr.strip().splitlines()[0][:200] if stderr.strip() else "(no message)"


def run_failure(p):
    """How a built test's run failed: its panic message, else its first line of stderr, else its exit code"""
    err = p.stderr.decode(errors="replace").splitlines()
    for i, line in enumerate(err):
        if "panicked at" in line and i + 1 < len(err):
            return f"run: panicked: {err[i + 1][:160]}"
    first = next((l for l in err if l.strip()), "")
    return f"run: {first[:160]}" if first else f"run: exit code {p.returncode}"


def run_one(path, kind, tmp):
    r, why = run_one_inner(path, kind, tmp)
    if r != "pass":
        REASONS[path] = why
    return r


def run_one_inner(path, kind, tmp):
    """(status, why it is not "pass")"""
    name = os.path.splitext(os.path.basename(path))[0]
    exe = os.path.join(tmp, re.sub(r"[^\w]", "_", os.path.relpath(path, TESTS)))
    text = open(path, errors="replace").read()
    ed = "--edition=" + edition(text)
    # (`//@ compile-flags: --test`: the test harness; a run runs every test, as compiletest runs the test binary)
    flags = extra_flags(text) or []
    try:
        if kind in ("check-pass", "compile-fail"):
            r = subprocess.run([*LIMIT, ZUSTC, "check", "-j", "1", path, ed, *flags], capture_output=True, text=True, timeout=30)
            if crashed(r):
                return "crash", "zustc crashed"
            ok = r.returncode == 0
            if kind == "check-pass":
                return ("pass", "") if ok else ("fail", first_error(r.stderr))
            if ok:
                return "fail", "accepted: no error reported"
            # (rejected: count it only when an error is reported on a line an annotation points at)
            want = error_lines(open(path, errors="replace").read())
            got = {int(m.group(1)) for m in re.finditer(r"^error: [^\n]*?:(\d+):\d+:", r.stderr, re.M)}
            return ("pass", "") if want & got else ("wrong-line", first_error(r.stderr))
        libs = ["-L", HELPERS] if HELPERS else []
        r = subprocess.run([*LIMIT, ZUSTC, path, "-o", exe, "--no-cache", ed, *flags, *libs], capture_output=True, text=True, timeout=60)
        if crashed(r):
            return "crash", "zustc crashed"
        if r.returncode != 0:
            return "fail", first_error(r.stderr)
        if kind == "build-pass":
            return "pass", ""
        p = subprocess.run([*LIMIT, exe], capture_output=True, timeout=20, cwd=tmp)
        if p.returncode != 0:
            return "fail", run_failure(p)
        want = os.path.join(os.path.dirname(path), name + ".run.stdout")
        if os.path.exists(want) and p.stdout != open(want, "rb").read():
            return "fail", "run: stdout differs from the .run.stdout file"
        return "pass", ""
    except subprocess.TimeoutExpired:
        return "timeout", "timeout"
    finally:
        for suf in ("", ".o", ".cache", ".link"):
            try:
                os.remove(exe + suf)
            except OSError:
                pass


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fetch", action="store_true")
    ap.add_argument("--kinds", default="run-pass,check-pass,build-pass,compile-fail")
    ap.add_argument("--limit", type=int, default=0, help="run only the first N applicable tests of each kind")
    ap.add_argument("--filter", default="", help="only tests whose path contains this")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--json", default=os.path.join(ROOT, "out/rust_ui.json"))
    ap.add_argument("--results", default=os.path.join(ROOT, "out/rust_ui_results.txt"))
    ap.add_argument("--reasons", default=None, help="write why each test that does not pass fails (status, kind, path, reason)")
    ap.add_argument("--baseline", default=None, help="list of passing tests (kind<TAB>path) to compare against")
    ap.add_argument("--check", action="store_true", help="with --baseline: exit 1 if a test in the baseline no longer passes, or zustc crashes on any test")
    ap.add_argument("--write-baseline", action="store_true", help="with --baseline: write the passing tests to it")
    ap.add_argument("--bench-json", default=None, help="write pass rates in github-action-benchmark's customBiggerIsBetter format")
    a = ap.parse_args()
    if a.fetch:
        fetch()
        return
    ui = os.path.join(TESTS, "tests/ui")
    if not os.path.isdir(ui):
        raise SystemExit("tests/ui is missing: run python3 bench/rust_ui.py --fetch first")
    global HELPERS
    HELPERS = test_helpers()
    kinds = a.kinds.split(",")
    tests, skipped = [], {}
    for d, dirs, fs in os.walk(ui):
        dirs[:] = sorted(x for x in dirs if x != "auxiliary")
        # (as compiletest: a directory with `compiletest-ignore-dir` holds module files of another test, not tests)
        if "compiletest-ignore-dir" in fs:
            dirs[:] = []
            continue
        for f in sorted(fs):
            if not f.endswith(".rs"):
                continue
            p = os.path.join(d, f)
            if a.filter and a.filter not in p:
                continue
            k, why = classify(p)
            if k not in kinds:
                continue
            if why:
                skipped.setdefault(k, {}).setdefault(why, 0)
                skipped[k][why] += 1
            else:
                tests.append((p, k))
    if a.limit:
        per = {}
        tests = [t for t in tests if per.setdefault(t[1], 0) < a.limit and not per.__setitem__(t[1], per[t[1]] + 1)]
    t0 = time.time()
    with tempfile.TemporaryDirectory() as tmp, ThreadPoolExecutor(a.jobs) as ex:
        res = list(ex.map(lambda t: (t[0], t[1], run_one(t[0], t[1], tmp)), tests))
    summary = {}
    for _, k, r in res:
        s = summary.setdefault(k, {"pass": 0, "fail": 0, "wrong-line": 0, "crash": 0, "timeout": 0})
        s[r] += 1
    out = {"commit": COMMIT, "tag": TAG, "seconds": round(time.time() - t0, 1), "kinds": {}}
    for k in kinds:
        s = summary.get(k, {"pass": 0, "fail": 0, "wrong-line": 0, "crash": 0, "timeout": 0})
        n = sum(s.values())
        out["kinds"][k] = {**s, "applicable": n, "not_applicable": skipped.get(k, {}), "pass_rate": round(100 * s["pass"] / n, 2) if n else None}
    os.makedirs(os.path.dirname(a.json), exist_ok=True)
    json.dump(out, open(a.json, "w"), indent=1)
    with open(a.results, "w") as f:
        for p, k, r in sorted(res):
            f.write(f"{r}\t{k}\t{os.path.relpath(p, ui)}\n")
    if a.reasons:
        with open(a.reasons, "w") as f:
            for p, k, r in sorted(res):
                if r != "pass":
                    why = REASONS.get(p, "").replace("\t", " ").replace("\n", " ")
                    f.write(f"{r}\t{k}\t{os.path.relpath(p, ui)}\t{why}\n")
    if a.bench_json:
        rows = []
        for k, v in out["kinds"].items():
            if v["pass_rate"] is not None:
                rows.append({"name": f"tests/ui {k} pass rate", "value": v["pass_rate"], "unit": "%"})
                rows.append({"name": f"tests/ui {k} passing", "value": v["pass"], "unit": "tests"})
        json.dump(rows, open(a.bench_json, "w"), indent=1)
    status = 0
    if a.baseline:
        now = {(k, os.path.relpath(p, ui)): r for p, k, r in res}
        passing = {key for key, r in now.items() if r == "pass"}
        # (compile-fail tests rejected at the annotated line for another reason than the one they test: not counted
        #  as passing in the baseline, see bench/rust_ui_accidental.txt)
        acc = os.path.join(os.path.dirname(os.path.abspath(a.baseline)), "rust_ui_accidental.txt")
        if os.path.exists(acc):
            skip = {tuple(l.rstrip("\n").split("\t")[:2]) for l in open(acc) if l.strip() and not l.startswith("#")}
            passing -= skip
        # (the ratchet: the least number of passing tests of each kind. It only goes up (--write-baseline raises it
        #  to the counts of the run), and --check fails below it)
        ratchet_path = os.path.join(os.path.dirname(os.path.abspath(a.baseline)), "rust_ui_ratchet.txt")
        ratchet = {}
        if os.path.exists(ratchet_path):
            for l in open(ratchet_path):
                if l.strip() and not l.startswith("#"):
                    k, n = l.split("\t")[:2]
                    # (a kind twice, as .gitattributes' union merge leaves the file when both sides raised it: the
                    #  higher one)
                    ratchet[k] = max(ratchet.get(k, 0), int(n))
        counts = {k: sum(1 for kk, _ in passing if kk == k) for k in kinds}
        # (only a run of every test of a kind counts against it: not one with --filter or --limit)
        whole = not a.filter and not a.limit
        if a.write_baseline:
            # (the kinds this run did not cover keep their entries)
            new = dict(ratchet)
            if whole:
                for k in kinds:
                    new[k] = max(counts[k], ratchet.get(k, 0))
            with open(ratchet_path, "w") as f:
                f.write("# the least number of tests/ui tests of each kind zustc must pass (bench/rust_ui.py --check fails below it;\n")
                f.write("# --write-baseline raises it, never lowers it). Accidental passes (rust_ui_accidental.txt) are not counted\n")
                for k in PASS_KINDS + ("compile-fail",):
                    if k in new:
                        f.write(f"{k}\t{new[k]}\n")
        elif whole:
            for k in kinds:
                if k in ratchet and counts[k] < ratchet[k]:
                    print(f"ratchet: {k} passes {counts[k]}, below {ratchet[k]} (bench/rust_ui_ratchet.txt)")
                    if a.check:
                        status = 1
        if a.write_baseline:
            with open(a.baseline, "w") as f:
                f.write(f"# tests/ui tests zustc passes (rust-lang/rust {TAG}, {COMMIT[:9]}); bench/rust_ui.py --write-baseline\n")
                for k, p in sorted(passing, key=lambda x: (x[1], x[0])):
                    f.write(f"{k}\t{p}\n")
        else:
            base = set()
            for line in open(a.baseline):
                if line.strip() and not line.startswith("#"):
                    k, p = line.rstrip("\n").split("\t")
                    base.add((k, p))
            ran = set(now)
            lost = sorted(x for x in base & ran if now[x] not in ("pass", "timeout"))
            slow = sorted(x for x in base & ran if now[x] == "timeout")
            new = sorted(passing - base)
            for k, p in lost:
                print(f"regression: {k} {p} ({now[(k, p)]})")
            for k, p in slow:
                print(f"timeout (was passing): {k} {p}")
            print(f"baseline: {len(lost)} regressions, {len(slow)} timeouts, {len(new)} newly passing")
            if new and a.check:
                print("(newly passing tests: update the baseline with --write-baseline)")
            if lost and a.check:
                status = 1
            # (a compiler must not panic on any input: every crash fails the check)
            crashes = sorted((k, os.path.relpath(p, ui)) for p, k, r in res if r == "crash")
            for k, p in crashes:
                print(f"crash: {k} {p}")
            if crashes and a.check:
                status = 1
    print(f"rust-lang/rust tests/ui at {TAG} ({COMMIT[:9]}), {out['seconds']} s")
    print("| kind | applicable | pass | fail | wrong line | crash | timeout | pass rate | not applicable |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for k, v in out["kinds"].items():
        na = sum(v["not_applicable"].values())
        rate = "-" if v["pass_rate"] is None else f"{v['pass_rate']}%"
        wl = v["wrong-line"] if k == "compile-fail" else "-"
        print(f"| {k} | {v['applicable']} | {v['pass']} | {v['fail']} | {wl} | {v['crash']} | {v['timeout']} | {rate} | {na} |")
    sys.exit(status)


if __name__ == "__main__":
    main()
