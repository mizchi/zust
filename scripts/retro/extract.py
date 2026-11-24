#!/usr/bin/env python3
"""Extract the data behind the development retrospective dashboard (commits, tests/ui pass rates, KPIs, code size,
agent sessions) from git history, so the dashboard can be rebuilt at any later commit.

usage: python3 scripts/retro/extract.py [--out out/retro] [--rev HEAD] [--loc-every-hours 3] [--pulls pulls.jsonl]

Writes one JSON array per dataset into --out (see scripts/retro/README.md for how they become the dashboard):
- hourly.json      commits per UTC hour (no merges, no github-actions[bot]) and their running total
- ui.json          tests/ui pass rate and passing count per kind, per CI run (gh-pages: dev/bench/data.js)
- kpi.json         bench/kpi_log.tsv, one row per change, with a commit time (interpolated where a commit was rebased away)
- loc.json         lines of .rs code: implementation vs tests, crates/zustscript kept apart, sampled along first-parent
- daily.json       commits, merged PRs, end-of-day pass rates and code size per UTC day
- sessions.json    agent sessions (the Claude-Session trailer) with at least 5 commits
- milestones.json  copied from scripts/retro/milestones.json (hand-picked decision points)
- pending.json     what each open pull request adds to bench/rust_ui_ratchet.txt, by kind (--pulls: the open PRs)
- forecast.json    by kind and in all: the last CI run's passing tests, what the open PRs add, and the days to 90% at
                   the pace of the last two days

Classification of .rs lines (loc.json):
- crates/zustscript/** is counted only as zustscript_lines
- tests: tests/**, crates/*/tests/**, and in any other source file the lines from its first `#[cfg(test)]` to the end
- implementation: src/**, crates/*/** (without tests/, benches/, bench/, examples/, evals/) and build.rs
- everything else (cratebench/, examples/, formal/, bench/) is not counted
"""
import argparse
import collections
import csv
import datetime as dt
import json
import math
import os
import re
import subprocess

ROOT = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True).stdout.strip()
HERE = os.path.dirname(os.path.abspath(__file__))
BOT = "github-actions[bot]"
# The commit that made rustc's tests/ui the spec suite: splits the history into "build-up" and "compat".
COMPAT_START = "c155863e"
UI_KINDS = ("run-pass", "check-pass", "build-pass", "compile-fail")


def git(*args):
    return subprocess.run(["git", "-C", ROOT, *args], capture_output=True, text=True).stdout


def utc(s):
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00")).astimezone(dt.timezone.utc)


def iso(t):
    return t.astimezone(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def commits(rev):
    raw = git("log", "--reverse", "--no-merges", rev,
              "--format=@@%H|%aI|%an|%(trailers:key=Claude-Session,valueonly,separator=;)|%s", "--numstat")
    out = []
    for block in raw.split("@@")[1:]:
        lines = block.strip("\n").split("\n")
        h, d, a, sess, s = lines[0].split("|", 4)
        if a == BOT:
            continue
        ins = dele = 0
        for l in lines[1:]:
            p = l.split("\t")
            if len(p) == 3 and p[0] != "-":
                ins += int(p[0])
                dele += int(p[1])
        out.append(dict(h=h, t=utc(d), a=a, s=s, sess=sess.strip().split(";")[0].rsplit("/", 1)[-1], ins=ins, dele=dele))
    return out


def hourly(cs, compat_t):
    by = collections.Counter(c["t"].replace(minute=0, second=0) for c in cs)
    # (the bounds from all commits' times: git's traversal order is not author-time order)
    ts = [c["t"] for c in cs]
    h, end, cum, rows = min(ts).replace(minute=0, second=0), max(ts), 0, []
    while h <= end:
        cum += by[h]
        rows.append(dict(hour=iso(h), commits=by[h], cumulative=cum, phase="compat" if h >= compat_t.replace(minute=0, second=0) else "build-up"))
        h += dt.timedelta(hours=1)
    return rows


def ui_rates():
    """Pass rates as CI measured them on every push (github-action-benchmark data on the gh-pages branch)."""
    git("fetch", "-q", "origin", "gh-pages")
    text = git("show", "origin/gh-pages:dev/bench/data.js")
    data = json.loads(text[text.index("{"):])
    rows = []
    for e in data["entries"].get("rustc tests/ui", []):
        b = {x["name"]: x["value"] for x in e["benches"]}
        r = dict(time=iso(utc(e["commit"]["timestamp"])), commit=e["commit"]["id"][:8])
        for k in UI_KINDS:
            key = k.replace("-", "_")
            r[f"{key}_rate"] = b.get(f"tests/ui {k} pass rate")
            r[f"{key}_passing"] = b.get(f"tests/ui {k} passing")
        rows.append(r)
    rows.sort(key=lambda r: r["time"])
    return rows


def kpi_rows():
    def num(v):
        try:
            return float(v)
        except ValueError:
            return None

    rows = list(csv.DictReader(open(os.path.join(ROOT, "bench/kpi_log.tsv")), delimiter="\t"))
    ts = []
    for r in rows:
        t = git("log", "-1", "--format=%aI", r["commit"].rstrip("+"), "--").strip()
        ts.append(utc(t) if t else None)
    known = [i for i, t in enumerate(ts) if t]
    est = [t is None for t in ts]
    for i in range(len(ts)):
        if ts[i] is None:  # a rebased commit: place it between its neighbours in log order
            a = max((j for j in known if j < i), default=None)
            b = min((j for j in known if j > i), default=None)
            ts[i] = ts[b] if a is None else ts[a] if b is None else ts[a] + (ts[b] - ts[a]) * ((i - a) / (b - a))
    cols = dict(noop_build_M="noop_M", edit_build_M="edit_M", check_M="check_M", cache_KiB="cache_KiB", lsp_session_G="lsp_G",
                traits_check_M="tr_check_M", traits_edit_M="tr_edit_M", crates_noop_M="cr_noop_M", crates_edit_M="cr_edit_M",
                patterns_check_M="pt_check_M", patterns_edit_M="pt_edit_M", ui_run="run", ui_check="check", ui_build="build", ui_cfail="cfail")
    out = [dict(n=i, time=iso(ts[i]), time_estimated=est[i], commit=r["commit"], change=r["change"],
                **{k: num(r[v]) for k, v in cols.items()}) for i, r in enumerate(rows)]
    # the log is in the order changes were measured, which parallel sessions make differ from commit time
    return sorted(out, key=lambda r: (r["time"], r["n"]))


def ratchet_at(rev):
    """{kind: tests} of bench/rust_ui_ratchet.txt at rev (empty when it has none)"""
    out = {}
    for line in git("show", f"{rev}:bench/rust_ui_ratchet.txt").splitlines():
        f = line.split("\t")
        if len(f) >= 2 and not line.startswith("#") and f[1].strip().isdigit():
            out[f[0]] = int(f[1])
    return out


def read_pulls(path):
    """The open pull requests, one JSON object a line (pages.yml writes them with `gh api`): number, title, head and
    base branch names. None when no list was given"""
    if not path:
        return None
    out = []
    for line in open(path):
        if line.strip():
            p = json.loads(line)
            out.append(dict(number=int(p["number"]), title=p["title"], head=p["head"], base=p["base"], draft=bool(p.get("draft"))))
    return out


def pending(prs):
    """What each open PR adds to the ratchet, by kind: its head's ratchet against the ratchet where it left its base
    (their merge base). A PR stacked on another one counts only its own part, so the rows add up to what merging them
    all would add; two PRs to main that pass the same test count it twice. A kind a PR has fewer of than its base (not
    yet merged with it) adds 0"""
    if not prs:
        return []
    refs = [f"+refs/pull/{p['number']}/head:refs/retro/pull/{p['number']}" for p in prs]
    refs += [f"+refs/heads/{b}:refs/retro/base/{b}" for b in sorted({p["base"] for p in prs})]
    if subprocess.run(["git", "-C", ROOT, "fetch", "-q", "origin", *refs], capture_output=True).returncode != 0:
        # (one ref that is gone (a base branch deleted) fails the fetch: each on its own then)
        for r in refs:
            git("fetch", "-q", "origin", r)
    rows = []
    for p in sorted(prs, key=lambda p: p["number"]):
        head, base = f"refs/retro/pull/{p['number']}", f"refs/retro/base/{p['base']}"
        mb = git("merge-base", head, base).strip()
        if not mb:
            continue
        new, old = ratchet_at(head), ratchet_at(mb)
        row = dict(pr=p["number"], title=p["title"], head=p["head"], base=p["base"], draft=p["draft"],
                   commit=git("rev-parse", "--short=8", head).strip())
        for k in UI_KINDS:
            row[k.replace("-", "_") + "_gain"] = max(0, new.get(k, 0) - old.get(k, 0)) if new else 0
        rows.append(row)
    return rows


def readme_applicable(rev):
    """{kind: applicable tests} of the README's tests/ui table at rev (`<!-- auto:ui -->`, which CI writes from its
    results): the exact counts, which the benchmark's rates (rounded to two decimals) only approximate"""
    text = git("show", f"{rev}:README.md")
    out = {}
    for k in UI_KINDS:
        m = re.search(r"^\| " + re.escape(k) + r"\b[^|]*\| (\d+) \|", text, re.M)
        if m:
            out[k.replace("-", "_")] = int(m.group(1))
    return out


def forecast(ui, pend, applicable, target=0.9, days=2.0):
    """By kind and in all: the last CI run that measured every kind (its passing tests; the applicable ones from the
    README's table, else from the run's rate), what the open PRs add, and the days until `target` of the applicable
    tests pass at the pace of the runs in the `days` days before that one, from the earliest of them: the same number
    of tests a day (linear), or the same share of the remaining tests a day (geometric, which slows down as the easy
    tests are taken). No pace (nor days) when that window has no earlier run"""
    keys = [k.replace("-", "_") for k in UI_KINDS]
    runs = [r for r in ui if all(r[f"{k}_passing"] is not None and r[f"{k}_rate"] for k in keys)]
    if not runs:
        return []
    last = runs[-1]
    t1 = utc(last["time"])
    first = next(r for r in runs if utc(r["time"]) >= t1 - dt.timedelta(days=days))
    span = (t1 - utc(first["time"])).total_seconds() / 86400
    rows = []
    for k in keys + ["all"]:
        ks = keys if k == "all" else [k]
        n = sum(applicable.get(q) or round(last[f"{q}_passing"] * 100 / last[f"{q}_rate"]) for q in ks)
        p = sum(int(last[f"{q}_passing"]) for q in ks)
        p0 = sum(int(first[f"{q}_passing"]) for q in ks)
        pe = sum(r[f"{q}_gain"] for r in pend for q in ks)
        target_n = math.ceil(target * n)
        need = max(0, target_n - p - pe)
        pace = (p - p0) / span if span > 0 else None
        lin = geo = None
        if need == 0:
            lin = geo = 0.0
        elif pace is not None and pace > 0:
            lin = need / pace
            share = pace / max(n - p, 1)
            gap0, gap1 = n - p - pe, n - target_n
            if 0 < share < 1 and gap0 > 0 and gap1 > 0:
                geo = math.log(gap1 / gap0) / math.log(1 - share)
        rows.append(dict(kind=k.replace("_", "-"), time=last["time"], commit=last["commit"], applicable=n, passing=p,
                         rate=round(p * 100 / n, 2), pending=pe, rate_with_pending=round((p + pe) * 100 / n, 2),
                         target_rate=round(target * 100, 2), target_passing=target_n, needed=need,
                         pace_from=first["time"], pace_days=round(span, 2), pace_per_day=None if pace is None else round(pace, 1),
                         days_linear=None if lin is None else round(lin, 1), days_geometric=None if geo is None else round(geo, 1)))
    return rows


def classify(path):
    if path.startswith("crates/zustscript/"):
        return "zs"
    if path.startswith("tests/") or re.match(r"crates/[^/]+/tests/", path):
        return "test"
    if re.match(r"crates/[^/]+/(benches|bench|examples|evals)/", path):
        return None
    if path.startswith(("src/", "crates/")) or path == "build.rs":
        return "impl"
    return None


def loc_at(full):
    total = {}
    for l in git("grep", "-c", "", full, "--", "*.rs").splitlines():
        p, n = l[len(full) + 1:].rsplit(":", 1)
        total[p] = int(n)
    inline = {}
    for l in git("grep", "-n", "-m1", r"#\[cfg(test)\]", full, "--", "*.rs").splitlines():
        p, ln, _ = l[len(full) + 1:].split(":", 2)
        inline[p] = total[p] - int(ln) + 1
    r = collections.Counter()
    for p, n in total.items():
        c = classify(p)
        if c == "impl":
            r["impl"] += n - inline.get(p, 0)
            r["inline"] += inline.get(p, 0)
        elif c == "test":
            r["test"] += n
        elif c == "zs":
            r["zs"] += n
    codes = set(git("grep", "-ohE", "E0[0-9]{3}", full, "--", "src", "crates", ":!crates/zustscript").split())
    return dict(impl_lines=r["impl"], test_lines=r["test"] + r["inline"], test_files_lines=r["test"],
                inline_test_lines=r["inline"], zustscript_lines=r["zs"], error_codes=len(codes))


def loc_series(rev, every):
    # (one sample a bucket, its last commit: the closing size of the bucket, and the requested revision itself in the
    #  last bucket)
    picked = collections.OrderedDict()
    for l in git("log", "--reverse", "--first-parent", "--format=%H|%aI", rev).splitlines():
        h, d = l.split("|")
        t = utc(d)
        key = (t.date(), t.hour // every)
        picked[key] = (h, t)
        # (moved to the end: a later commit with an earlier author date keeps the requested revision the last row)
        picked.move_to_end(key)
    return [dict(time=iso(t), commit=h[:8], **loc_at(h)) for h, t in picked.values()]


def daily(cs, prs, ui, loc):
    days = collections.OrderedDict()
    for c in cs:
        e = days.setdefault(c["t"].date().isoformat(), dict(commits=0, prs_merged=0))
        e["commits"] += 1
    for t in prs:
        d = t.date().isoformat()
        if d in days:
            days[d]["prs_merged"] += 1
    out = []
    for d, e in days.items():
        u = [x for x in ui if x["time"][:10] <= d]
        l = [x for x in loc if x["time"][:10] <= d][-1]
        row = dict(date=d, **e)
        for k in UI_KINDS:
            key = k.replace("-", "_") + "_rate"
            row[key + "_end"] = u[-1][key] if u else None
        row.update(impl_lines_end=l["impl_lines"], test_lines_end=l["test_lines"])
        out.append(row)
    return out


def sessions(cs, min_commits=5):
    by = collections.defaultdict(list)
    for c in cs:
        if c["sess"]:
            by[c["sess"]].append(c["t"])
    rows = sorted(by.items(), key=lambda kv: min(kv[1]))
    return [dict(session=f"S{i + 1}", session_id=k[-8:], commits=len(v), first=iso(min(v)), last=iso(max(v)))
            for i, (k, v) in enumerate(x for x in rows if len(x[1]) >= min_commits)]


def col(type_, desc="", unit=None, nullable=False, values=None):
    c = {"type": type_, "description": desc}
    if unit:
        c["unit"] = unit
    if nullable:
        c["nullable"] = True
    if values:
        c["values"] = values
    return c


def datasets_spec():
    """The data contract (repo-retrospective/v1) for what main() writes: views may rely on exactly this."""
    T, C = col("time", "UTC"), col("commit", "first 8 hex digits")
    kinds = [k.replace("-", "_") for k in UI_KINDS]
    kpi_cols = dict(noop_build_M="gen5000 no-op build", edit_build_M="gen5000 build after editing one function",
                    check_M="gen5000 zustc check", traits_check_M="bench/gen_traits.py 50, check",
                    traits_edit_M="bench/gen_traits.py 50, edit build", crates_noop_M="cratebench crates, no-op build (sum)",
                    crates_edit_M="cratebench crates, edit build (sum)", patterns_check_M="bench/gen_patterns.py mix 4, check",
                    patterns_edit_M="bench/gen_patterns.py mix 4, edit build")
    return {
        "hourly": dict(title="Commits per hour", grain="one UTC hour", provenance="derived",
                       source="git log --no-merges, without github-actions[bot]",
                       columns=dict(hour=col("time", "start of the UTC hour"), commits=col("int", "", "commits"),
                                    cumulative=col("int", "running total", "commits"),
                                    phase=col("enum", "before or after tests/ui became the spec", values=["build-up", "compat"]))),
        "ui": dict(title="rustc tests/ui pass rates", grain="one CI benchmark run", provenance="recorded",
                   source="gh-pages dev/bench/data.js, entry 'rustc tests/ui' (bench/rust_ui.py --bench-json)",
                   columns=dict(time=T, commit=C,
                                **{f"{k}_rate": col("number", f"{k} pass rate over applicable tests, higher is better", "%", True) for k in kinds},
                                **{f"{k}_passing": col("number", f"{k} tests passing", "tests", True) for k in kinds})),
        "kpi": dict(title="Speed KPIs per change", grain="one row of bench/kpi_log.tsv", provenance="recorded",
                    source="bench/kpi_log.tsv (bench/kpi_track.py)",
                    columns=dict(n=col("int", "row number in the log"), time=T,
                                 time_estimated=col("bool", "the commit was rebased away; time placed between neighbours"),
                                 commit=col("string", "short hash as logged; '+' marks a change measured before committing"),
                                 change=col("string", "what the change was"),
                                 **{k: col("number", f"{v}, instructions, lower is better", "M instr", True) for k, v in kpi_cols.items()},
                                 cache_KiB=col("number", "gen5000 build cache size", "KiB", True),
                                 lsp_session_G=col("number", "LSP session on a fixed snapshot, lower is better", "G instr", True),
                                 ui_run=col("number", "tests/ui run-pass passing", "tests", True),
                                 ui_check=col("number", "tests/ui check-pass passing", "tests", True),
                                 ui_build=col("number", "tests/ui build-pass passing", "tests", True),
                                 ui_cfail=col("number", "tests/ui compile-fail passing", "tests", True))),
        "loc": dict(title="Lines of code", grain="one first-parent commit per 3 UTC hours", provenance="derived",
                    source="git grep -c over *.rs at each sampled commit",
                    columns=dict(time=T, commit=C,
                                 impl_lines=col("int", "src/, crates/* without tests/benches/examples, build.rs; without crates/zustscript", "lines"),
                                 test_lines=col("int", "test_files_lines + inline_test_lines", "lines"),
                                 test_files_lines=col("int", "tests/ and crates/*/tests/", "lines"),
                                 inline_test_lines=col("int", "from the first #[cfg(test)] of a source file to its end", "lines"),
                                 zustscript_lines=col("int", "crates/zustscript, counted apart", "lines"),
                                 error_codes=col("int", "distinct E0xxx codes in src/ and crates/", "codes"))),
        "daily": dict(title="By day", grain="one UTC day with commits", provenance="derived",
                      source="git log, the ui and loc datasets",
                      columns=dict(date=col("date", "UTC day"), commits=col("int", "", "commits"),
                                   prs_merged=col("int", "'Merge pull request #' merges", "PRs"),
                                   **{f"{k}_rate_end": col("number", f"last {k} pass rate of the day", "%", True) for k in kinds},
                                   impl_lines_end=col("int", "", "lines"), test_lines_end=col("int", "", "lines"))),
        "sessions": dict(title="Agent sessions", grain="one Claude-Session trailer with 5+ commits", provenance="derived",
                         source="git log trailers",
                         columns=dict(session=col("string", "S1, S2, ... in order of the first commit"),
                                      session_id=col("string", "last 8 characters of the session id"),
                                      commits=col("int", "", "commits"), first=col("time", "first commit"), last=col("time", "last commit"))),
        "milestones": dict(title="Milestones", grain="one decision", provenance="hand-picked",
                           source="scripts/retro/milestones.json",
                           columns=dict(time=T, commit=C, label=col("string"), detail=col("string"),
                                        phase=col("enum", "", values=["build-up", "compat"]))),
        "pending": dict(title="Open pull requests' tests/ui gains", grain="one open pull request", provenance="derived",
                        source="the open pull requests (GitHub API, pages.yml) and bench/rust_ui_ratchet.txt at each one's "
                               "head and at its merge base with its base branch; empty when no list was given",
                        columns=dict(pr=col("int", "pull request number"), title=col("string"),
                                     head=col("string", "head branch"), base=col("string", "base branch: main, or the PR it is stacked on"),
                                     draft=col("bool", "a draft"), commit=col("commit", "the head, first 8 hex digits"),
                                     **{f"{k}_gain": col("int", f"{k} tests its ratchet adds to its base's", "tests") for k in kinds})),
        "forecast": dict(title="tests/ui now, with the open pull requests, and the way to 90%", grain="one kind, and all kinds",
                         provenance="derived", source="the ui and pending datasets",
                         columns=dict(kind=col("enum", "", values=list(UI_KINDS) + ["all"]),
                                      time=col("time", "the CI run the numbers are from"), commit=C,
                                      applicable=col("int", "tests zustc can be asked to handle (the README's table, else passing / rate)", "tests"),
                                      passing=col("int", "passing in that run", "tests"), rate=col("number", "", "%"),
                                      pending=col("int", "what the open pull requests add (the pending dataset)", "tests"),
                                      rate_with_pending=col("number", "the rate once they are merged", "%"),
                                      target_rate=col("number", "the target", "%"),
                                      target_passing=col("int", "tests passing at the target", "tests"),
                                      needed=col("int", "tests still needed for the target after the open pull requests", "tests"),
                                      pace_from=col("time", "the run the pace is measured from: the earliest in the two days before the last"),
                                      pace_days=col("number", "days between the two runs", "days"),
                                      pace_per_day=col("number", "passing tests gained a day between them; null with no earlier run in the window", "tests/day", True),
                                      days_linear=col("number", "days to the target at pace_per_day; null when the pace is not positive", "days", True),
                                      days_geometric=col("number", "days to the target gaining the same share of the remaining tests a day", "days", True))),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(ROOT, "out/retro"))
    ap.add_argument("--rev", default="HEAD")
    ap.add_argument("--loc-every-hours", type=int, default=3)
    ap.add_argument("--pulls", default=None, help="the open pull requests, one JSON object a line (number, title, head, base, draft)")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)

    cs = commits(a.rev)
    compat_t = utc(git("log", "-1", "--format=%aI", COMPAT_START).strip())
    prs = [utc(l) for l in git("log", "--merges", "--format=%aI", "--grep=^Merge pull request #", a.rev).splitlines()]
    ui = ui_rates()
    loc = loc_series(a.rev, a.loc_every_hours)
    milestones = json.load(open(os.path.join(HERE, "milestones.json")))
    for m in milestones:
        m["phase"] = "compat" if utc(m["time"]) >= compat_t else "build-up"
    pend = pending(read_pulls(a.pulls))
    data = dict(hourly=hourly(cs, compat_t), ui=ui, kpi=kpi_rows(), loc=loc, daily=daily(cs, prs, ui, loc),
                sessions=sessions(cs), milestones=milestones, pending=pend, forecast=forecast(ui, pend, readme_applicable(a.rev)))
    spec = datasets_spec()
    for name, rows in data.items():
        spec[name]["file"] = f"{name}.json"
        with open(os.path.join(a.out, f"{name}.json"), "w") as f:
            json.dump(rows, f, ensure_ascii=False)
        print(f"{name}.json\t{len(rows)} rows")
    manifest = dict(contract="repo-retrospective/v1", repo="mizchi/zust", head=git("rev-parse", "--short=8", a.rev).strip(),
                    generated_at=iso(dt.datetime.now(dt.timezone.utc)), generator="scripts/retro/extract.py",
                    phases=[dict(name="build-up", start_commit=cs[0]["h"][:8], why="the first commit of this repository"),
                            dict(name="compat", start_commit=COMPAT_START, why="rustc's tests/ui became the spec suite")],
                    datasets=spec)
    with open(os.path.join(a.out, "manifest.json"), "w") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=1)
    print("manifest.json")


if __name__ == "__main__":
    main()
