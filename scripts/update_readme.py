#!/usr/bin/env python3
"""Rewrite the measured numbers in README.md, PERFORMANCE.md and SUPPORT.md from benchmark results, so that they need
no hand edits. bench.yml runs this after each push to main and commits the files that changed.

usage: python3 scripts/update_readme.py [--ci-dir out/ci] [--ui-json out/ci/rust_ui_summary.json]
                                        [--ui-results out/ci/rust_ui.tsv] [--commit SHA] [--check]

What it rewrites, in each of the files (anything else is left as written):
- blocks between `<!-- auto:NAME -->` and `<!-- /auto:NAME -->`:
  - `kpi`: the KPI table, from ci-dir/zustc.json and ci-dir/analyzer.json (bench/ci_bench.py)
  - `kpi-summary`: the times of builds and `check` only (the README's summary of `kpi`)
  - `ui`: the tests/ui table, from the rust_ui.py summary (--json)
- inline values right after a marker:
  - `<!-- ui:DIR,DIR -->72.5%`: run-pass + check-pass + build-pass of those tests/ui directories, from the per-test
    results (rust_ui.py --results)
  - `<!-- cf:DIR,DIR -->38.0%`: compile-fail of those directories
- `<!-- measured -->...<!-- /measured -->`: the date and commit the numbers come from (in a file whose numbers changed)

Inputs that are missing leave their values as they were. --check exits 1 if a file would change (nothing written).
"""
import argparse, collections, datetime, json, os, re, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FILES = [os.path.join(ROOT, f) for f in ("README.md", "PERFORMANCE.md", "SUPPORT.md")]
KINDS = ("run-pass", "check-pass", "build-pass", "compile-fail")
KIND_TEXT = {"run-pass": "run-pass (builds, runs, same output)", "check-pass": "check-pass (accepted by `zustc check`)",
             "build-pass": "build-pass", "compile-fail": "compile-fail (rejected at an annotated line)"}


def load(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def by_name(rows):
    return {r["name"]: r["value"] for r in rows or []}


def kpi_table(z, a):
    def v(d, k, fmt):
        return fmt.format(d[k]) if k in d else "-"
    lines = ["| KPI | gen1000 | gen5000 |", "|---|---:|---:|"]

    def cell(n, key, fmt, ins):
        # a time, and its instruction count (valgrind) when there is one: "21.8 ms (124.7 M)"
        t = v(z, f"{n} {key}", fmt)
        if ins and f"{n} {key} instructions" in z:
            t += f" ({z[f'{n} {key} instructions']:.1f} M)"
        return t
    for label, key, fmt, ins in (
        ("cold build", "build cold", "{:.0f} ms", False),
        ("no-op build", "build noop", "{:.1f} ms", True),
        ("build after editing one function", "build edit 1 fn", "{:.1f} ms", True),
        ("`check`", "check", "{:.0f} ms", True),
        ("build cache on disk", "cache", "{:.1f} KiB", False),
        ("binary", "binary", "{:.1f} KiB", False),
        ("cold build peak RSS", "build cold peak RSS", "{:.1f} MiB", False),
    ):
        lines.append(f"| {label} | {cell('gen1000', key, fmt, ins)} | {cell('gen5000', key, fmt, ins)} |")
    lsp = [f"{q} {v(a, f'lsp {q} p50', '{:.2f} ms')}" for q in ("definition", "hover", "completion", "references", "rename")]
    lines += ["", "| zust-analyzer (zustc's own sources) | |", "|---|---:|",
              "| p50: " + ", ".join(q.split()[0] for q in lsp) + " | " + " / ".join(q.split(" ", 1)[1] for q in lsp) + " |",
              f"| startup until indexed | {v(a, 'lsp ready (startup to indexed)', '{:.0f} ms')} |",
              f"| peak RSS | {v(a, 'lsp peak RSS', '{:.1f} MiB')} |"]
    return "\n".join(lines)


def kpi_summary(z):
    def v(k, fmt):
        return fmt.format(z[k]) if k in z else "-"
    lines = ["| | gen1000 | gen5000 |", "|---|---:|---:|"]
    for label, key, fmt in (
        ("cold build", "build cold", "{:.0f} ms"),
        ("no-op build", "build noop", "{:.1f} ms"),
        ("build after editing one function", "build edit 1 fn", "{:.1f} ms"),
        ("`check`", "check", "{:.0f} ms"),
    ):
        lines.append(f"| {label} | {v(f'gen1000 {key}', fmt)} | {v(f'gen5000 {key}', fmt)} |")
    return "\n".join(lines)


def ui_table(kinds):
    lines = ["| kind | applicable | pass |", "|---|---:|---:|"]
    for k in KINDS:
        d = kinds[k]
        lines.append(f"| {KIND_TEXT[k]} | {d['applicable']} | {d['pass']} ({d['pass'] / d['applicable'] * 100:.1f}%) |")
    return "\n".join(lines)


def per_dir(results):
    """(dir, compile-fail?) -> [pass, total] from rust_ui.py --results (status, kind, path)"""
    c = collections.defaultdict(lambda: [0, 0])
    for line in open(results):
        parts = line.rstrip("\n").split("\t")
        if len(parts) != 3:
            continue
        st, kind, path = parts
        k = (path.split("/")[0], kind == "compile-fail")
        c[k][1] += 1
        c[k][0] += st == "pass"
    return c


def rate(c, dirs, cf):
    p = sum(c[(d, cf)][0] for d in dirs if (d, cf) in c)
    t = sum(c[(d, cf)][1] for d in dirs if (d, cf) in c)
    return None if t == 0 else p / t * 100


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ci-dir", default=os.path.join(ROOT, "out/ci"))
    ap.add_argument("--ui-json")
    ap.add_argument("--ui-results")
    ap.add_argument("--commit")
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    ui_json = a.ui_json or os.path.join(a.ci_dir, "rust_ui_summary.json")
    ui_results = a.ui_results or os.path.join(a.ci_dir, "rust_ui.tsv")
    z, an = by_name(load(os.path.join(a.ci_dir, "zustc.json"))), by_name(load(os.path.join(a.ci_dir, "analyzer.json")))
    ui = load(ui_json)
    c = per_dir(ui_results) if os.path.exists(ui_results) else None
    stale = []
    for path in FILES:
        if not os.path.exists(path):
            continue
        s = old = open(path).read()

        def block(name, text):
            nonlocal s
            pat = re.compile(rf"(<!-- auto:{name} -->\n)(?:.*?\n)?(<!-- /auto:{name} -->)", re.S)
            s = pat.sub(lambda m: m.group(1) + text + "\n" + m.group(2), s)

        if z or an:
            block("kpi", kpi_table(z, an))
            block("kpi-summary", kpi_summary(z))
        if ui:
            block("ui", ui_table(ui["kinds"]))
        if c is not None:
            def area(m):
                r = rate(c, m.group(2).split(","), m.group(1) == "cf")
                return m.group(0) if r is None else f"<!-- {m.group(1)}:{m.group(2)} -->{r:.1f}%"
            s = re.sub(r"<!-- (ui|cf):([\w\-,]+) -->[0-9.]+%", area, s)
        # (only the stamp changing is not a change)
        if s != old:
            when = datetime.date.today().isoformat() + (f", {a.commit[:7]}" if a.commit else "")
            s = re.sub(r"<!-- measured -->.*?<!-- /measured -->", f"<!-- measured -->{when}<!-- /measured -->", s)
        name = os.path.basename(path)
        if s == old:
            print(f"{name} unchanged")
        elif a.check:
            stale.append(name)
        else:
            open(path, "w").write(s)
            print(f"{name} updated")
    if stale:
        print(f"out of date: {', '.join(stale)}: run python3 scripts/update_readme.py")
        sys.exit(1)


if __name__ == "__main__":
    main()
