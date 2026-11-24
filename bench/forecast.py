#!/usr/bin/env python3
"""Forecast of rustc tests/ui coverage, from the repository's own history.

    python3 bench/forecast.py [--ref origin/main] [--target 0.9] [--days 2] [--pending REF ...]

Reads three things, all in git, and writes a Markdown report to stdout (docs/forecast.md is one, with its notes):

- the README's tests/ui table (`<!-- auto:ui -->`) at every commit of REF that changed it: the measured pass
  counts over time (these count the accidental passes of bench/rust_ui_accidental.txt too);
- bench/rust_ui_ratchet.txt at every first-parent commit of REF that changed it: what each merged PR gained, by
  kind (accidental passes not counted);
- bench/effort_log.tsv, if it has rows: the tokens (and hours) a PR took, written by hand, so that the cost of the
  rest can be estimated as tokens per test.

`--pending REF` adds a branch not merged yet (an open PR): its ratchet against REF's, per kind.

The forecast is the time until `target` of the applicable tests pass, two ways, from the README's last `--days`
days: at the same number of tests a day (linear), and at the same share of the remaining tests a day (geometric,
which slows down as the easy tests are taken; usually the closer of the two).
"""
import argparse
import datetime as dt
import math
import os
import re
import subprocess
import sys

KINDS = ("run-pass", "check-pass", "build-pass", "compile-fail")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, env={**os.environ, "TZ": "UTC"}).stdout


def readme_table(text):
    """{kind: (applicable, pass)} of the README's auto:ui table"""
    out = {}
    for k in KINDS:
        m = re.search(r"^\| " + re.escape(k) + r"\b[^|]*\| (\d+) \| (\d+)", text, re.M)
        if m:
            out[k] = (int(m.group(1)), int(m.group(2)))
    return out


def ratchet_of(text):
    out = {}
    for line in text.splitlines():
        if line.strip() and not line.startswith("#"):
            k, n = line.split("\t")[:2]
            out[k] = int(n)
    return out


def history(ref):
    """[(time, {kind: (applicable, pass)})], oldest first, one row per change of the numbers"""
    rows = []
    for line in git("log", ref, "--format=%H %ad", "--date=format-local:%Y-%m-%dT%H:%M", "--", "README.md").splitlines():
        h, t = line.split()
        tab = readme_table(git("show", f"{h}:README.md"))
        if len(tab) == len(KINDS):
            rows.append((dt.datetime.fromisoformat(t), tab))
    rows.reverse()
    out = []
    for t, tab in rows:
        if not out or out[-1][1] != tab:
            out.append((t, tab))
    return out


def ledger(ref):
    """[(time, subject, {kind: gain})] of the first-parent commits of ref that moved the ratchet"""
    out = []
    for line in git("log", ref, "--first-parent", "--format=%H\t%P\t%ad\t%s", "--date=format-local:%Y-%m-%dT%H:%M", "--", "bench/rust_ui_ratchet.txt").splitlines():
        h, parents, t, subj = line.split("\t", 3)
        p = parents.split()[0] if parents else None
        new = ratchet_of(git("show", f"{h}:bench/rust_ui_ratchet.txt"))
        old = ratchet_of(git("show", f"{p}:bench/rust_ui_ratchet.txt")) if p else {}
        gain = {k: new.get(k, 0) - old.get(k, 0) for k in KINDS if new.get(k, 0) != old.get(k, 0)}
        if gain:
            out.append((t, subj, gain))
    out.reverse()
    return out


def effort():
    """{PR number: (tokens, hours)} of bench/effort_log.tsv"""
    path = os.path.join(ROOT, "bench/effort_log.tsv")
    out = {}
    if not os.path.exists(path):
        return out
    for line in open(path):
        if not line.strip() or line.startswith("#") or line.startswith("pr\t"):
            continue
        f = line.rstrip("\n").split("\t")
        pr = f[0].lstrip("#")
        tok = float(f[1]) if len(f) > 1 and f[1].strip() else None
        hrs = float(f[2]) if len(f) > 2 and f[2].strip() else None
        out[pr] = (tok, hrs)
    return out


def days_to(target_n, now_n, total, rate_lin, share):
    """(linear days, geometric days) until target_n of total pass"""
    need = target_n - now_n
    if need <= 0:
        return 0.0, 0.0
    lin = need / rate_lin if rate_lin > 0 else float("inf")
    gap0, gap1 = total - now_n, total - target_n
    geo = math.log(gap1 / gap0) / math.log(1 - share) if 0 < share < 1 and gap0 > 0 else float("inf")
    return lin, geo


def fmt_days(d):
    return "reached" if d == 0 else ("-" if d == float("inf") else f"{d:.1f}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ref", default="origin/main")
    ap.add_argument("--target", type=float, default=0.9)
    ap.add_argument("--days", type=float, default=2.0)
    ap.add_argument("--pending", nargs="*", default=[])
    a = ap.parse_args()

    hist = history(a.ref)
    if not hist:
        sys.exit("no README tests/ui table in the history of " + a.ref)
    t1, last = hist[-1]
    app = sum(last[k][0] for k in KINDS)
    now = sum(last[k][1] for k in KINDS)
    print(f"## Measured (README of {a.ref}, {t1:%Y-%m-%d %H:%M} UTC)\n")
    print("| kind | applicable | pass | rate |\n|---|---:|---:|---:|")
    for k in KINDS:
        n, p = last[k]
        print(f"| {k} | {n} | {p} | {p / n:.1%} |")
    print(f"| all | {app} | {now} | {now / app:.1%} |\n")

    # (the pass count at the end of each day, and the pace over the last --days days)
    print("## History (all kinds, the last measurement of each day, UTC)\n")
    print("| day | pass | rate | gain |\n|---|---:|---:|---:|")
    daily = {}
    for t, tab in hist:
        daily[t.date()] = sum(tab[k][1] for k in KINDS), sum(tab[k][0] for k in KINDS)
    prev = None
    for d, (p, n) in sorted(daily.items()):
        print(f"| {d} | {p} | {p / n:.1%} | {'' if prev is None else f'{p - prev:+d}'} |")
        prev = p
    t0 = t1 - dt.timedelta(days=a.days)
    base = [(t, tab) for t, tab in hist if t <= t0]
    t0, tab0 = base[-1] if base else hist[0]
    span = max((t1 - t0).total_seconds() / 86400, 1e-9)
    print()

    # (what is not merged yet)
    ratch = ratchet_of(git("show", f"{a.ref}:bench/rust_ui_ratchet.txt"))
    pend = {k: 0 for k in KINDS}
    if a.pending:
        print(f"## Not merged yet (bench/rust_ui_ratchet.txt of each branch)\n")
        print("| branch | " + " | ".join(KINDS) + " |\n|---|" + "---:|" * len(KINDS))
        seen = []
        for r in a.pending:
            rr = ratchet_of(git("show", f"{r}:bench/rust_ui_ratchet.txt"))
            # (against the ratchet where it left REF, and the pending branches it is stacked on)
            mb = git("merge-base", r, a.ref).strip()
            base = ratchet_of(git("show", f"{mb}:bench/rust_ui_ratchet.txt")) if mb else dict(ratch)
            for p, pr in seen:
                if subprocess.run(["git", "merge-base", "--is-ancestor", p, r], cwd=ROOT).returncode == 0:
                    base = {k: max(base.get(k, 0), pr.get(k, 0)) for k in KINDS}
            g = {k: max(0, rr.get(k, 0) - base.get(k, 0)) for k in KINDS}
            for k in KINDS:
                pend[k] += g[k]
            seen.append((r, rr))
            print(f"| {r} | " + " | ".join(f"{g[k]:+d}" for k in KINDS) + " |")
        print("\n(each branch against the ratchet of the commit it left REF at, and of the pending branches it is "
              "stacked on; a branch that did not raise its ratchet shows +0)\n")

    print(f"## Forecast to {a.target:.0%} (pace of the README's last {span:.1f} days)\n")
    print("| kind | pass now | + not merged | needed | tests/day | days (linear) | days (geometric) |\n|---|---:|---:|---:|---:|---:|---:|")
    rows = [(k, last[k][0], last[k][1], tab0[k][1], pend[k]) for k in KINDS]
    rows.append(("all", app, now, sum(tab0[k][1] for k in KINDS), sum(pend.values())))
    for k, n, p, p0, pe in rows:
        rate = (p - p0) / span
        share = rate / max(n - p, 1)
        target_n = int(-(-a.target * n // 1))
        lin, geo = days_to(target_n, p + pe, n, rate, share)
        print(f"| {k} | {p} | {pe:+d} | {max(0, target_n - p - pe)} | {rate:.0f} | {fmt_days(lin)} | {fmt_days(geo)} |")
    print()

    led = ledger(a.ref)
    eff = effort()
    print(f"## What each merged change gained (first-parent commits of {a.ref} that raised bench/rust_ui_ratchet.txt)\n")
    print("| time (UTC) | change | " + " | ".join(KINDS) + " | tokens | tokens / test |\n|---|---|" + "---:|" * (len(KINDS) + 2))
    tot_tok, tot_tests = 0.0, 0
    for t, subj, g in led[-40:]:
        m = re.search(r"#(\d+)", subj)
        pr = m.group(1) if m else ""
        tok = eff.get(pr, (None, None))[0]
        n = sum(v for v in g.values() if v > 0)
        if tok and n:
            tot_tok, tot_tests = tot_tok + tok, tot_tests + n
        s = subj if len(subj) <= 70 else subj[:67] + "..."
        print(f"| {t.replace('T', ' ')} | {s} | " + " | ".join(f"{g[k]:+d}" if k in g else "" for k in KINDS)
              + f" | {'' if tok is None else f'{tok:,.0f}'} | {'' if not (tok and n) else f'{tok / n:,.0f}'} |")
    print()
    if tot_tests:
        need = max(0, int(-(-a.target * app // 1)) - now - sum(pend.values()))
        per = tot_tok / tot_tests
        print(f"Tokens: {tot_tok:,.0f} for {tot_tests} tests in the PRs bench/effort_log.tsv gives, {per:,.0f} a test; "
              f"the {need} tests still needed for {a.target:.0%} at that rate: about {need * per:,.0f} tokens "
              "(the later tests are harder, so take this as a lower bound).")
    else:
        print("Tokens: bench/effort_log.tsv has no tokens yet. Write a row `pr<TAB>tokens<TAB>hours` for a merged PR "
              "(from the usage dashboard) and this report gives tokens per test and the tokens the rest would take.")


if __name__ == "__main__":
    main()
