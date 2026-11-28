#!/usr/bin/env python3
"""Compare the KPIs of a pull request with its base, and keep one comment on the PR up to date with the result.

usage (from .github/workflows/kpi.yml):
  python3 bench/kpi_pr.py --base base.txt --head head.txt --base-sha SHA --head-sha SHA [--pr N --repo OWNER/NAME]
                          [--threshold 1.0] [--out summary.md]

base.txt / head.txt are what `python3 bench/kpi_track.py <label> --lsp --dry-run` printed (or its parts, `--only`, put together) for the base (the merge base of
the PR) and for the PR head, measured on runners of the same image. The instruction counts are deterministic to about 0.1%,
so a difference past --threshold (in %, default 1.0) is reported as a regression (a ::warning:: line).

With --ratchet (bench/kpi_ratchet.txt, and --env, the environment of the measurements), the head is checked against
the ceilings there (bench/kpi_ratchet.py): a KPI above its ceiling by more than its tolerance fails the job (exit 1,
after the comment is written), and the comment gives the line that would raise the ceiling. --ratchet-base is the
file at the base, to show a ceiling the pull request raises.

With --pr, the comment on the PR that carries the marker below is updated (or created): it keeps the result of every
push (the history is embedded in it as JSON), newest last, with the change against the base and against the push
before. GH_TOKEN must allow writing PR comments (`gh api`).
"""
import argparse, json, os, re, subprocess, sys

MARKER = "<!-- zust-kpi-pr -->"
# (the names kpi_track.py prints, in its order)
KPIS = ["no-op build", "edit one fn", "check", "cache", "LSP session", "traits check", "traits edit", "crates no-op", "crates edit",
        "patterns check", "patterns edit"]
UNITS = {"cache": "KiB", "LSP session": "G"}


def parse(path):
    """The KPI values kpi_track.py printed: `name   value   Δ ...   Σ ...`"""
    vals = {}
    for line in open(path):
        for k in KPIS:
            m = re.match(rf"^{re.escape(k)}\s+(-?[0-9.]+)\s", line)
            if m:
                vals[k] = float(m.group(1))
    return vals


def pct(a, b):
    if a is None or b is None or b == 0:
        return None
    return (a - b) / b * 100


def fmt_pct(p):
    return "-" if p is None else f"{p:+.2f}%"


def fmt_val(v):
    return "-" if v is None else (f"{v:.1f}" if abs(v) >= 100 else f"{v:.3f}")


def gh(*args, input=None):
    r = subprocess.run(["gh", "api", *args], capture_output=True, text=True, input=input)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.strip())
    return r.stdout


def find_comment(repo, pr):
    out = gh("--paginate", f"repos/{repo}/issues/{pr}/comments", "--jq", f'.[] | select(.body | contains("{MARKER}")) | {{id, body}}')
    found = [json.loads(l) for l in out.splitlines() if l.strip()]
    return found[-1] if found else None


def history_of(body):
    m = re.search(r"<!-- kpi-history (.*?) -->", body or "", re.S)
    return json.loads(m.group(1)) if m else []


def ceil_cell(row, ratchet):
    if not row or row["ceil"] is None:
        return "-"
    over = fmt_pct(row["over"])
    cell = {"ok": f"{fmt_val(row['ceil'])} ({over})",
            "fail": f"{fmt_val(row['ceil'])} ({over} > {row['tol']:g}%) ❌",
            "main over": f"{fmt_val(row['ceil'])} ({over}; the base is over) ⚠️",
            "env": f"{fmt_val(row['ceil'])} (another environment)"}[row["status"]]
    was = ratchet["base_ceil"].get(row["kpi"])
    if was is not None and was[0] != row["ceil"]:
        cell += f" (raised from {fmt_val(was[0])})" if row["ceil"] > was[0] else f" (lowered from {fmt_val(was[0])})"
    return cell


def ratchet_notes(ratchet, last):
    """What the ratchet asks of the pull request, below the table"""
    rows = ratchet["rows"]
    out = []
    def lines_of(side, ks):
        vals, sha = last[side], last[f"{side}_sha"][:8]
        return ["", "```", *[ratchet["line_of"](r["kpi"], vals[r["kpi"]], r["tol"], sha) for r in rows if r["kpi"] in ks and r["kpi"] in vals], "```"]
    fail = [r["kpi"] for r in rows if r["status"] == "fail"]
    over = [r["kpi"] for r in rows if r["status"] == "main over"]
    if fail:
        out += ["", f"❌ Above its ceiling by more than the tolerance: {', '.join(fail)}. Take the cost out, or raise the "
                "ceiling in this pull request, in `bench/kpi_ratchet.txt`, and say in its description why the cost is worth it:"]
        out += lines_of("head", fail)
    if over:
        out += ["", f"⚠️ The base is above the ceiling already: {', '.join(over)} (this pull request does not add to it). "
                "Take main's cost out, or raise the ceiling on purpose."]
    if any(r["status"] == "env" for r in rows):
        out += ["", f"The ceilings were measured in `{ratchet['env_file']}`, this run in `{ratchet['env']}`: not checked. "
                "The next push to main measures them anew."]
    if all(r["status"] == "none" for r in rows):
        out += ["", "`bench/kpi_ratchet.txt` has no ceilings yet: the next push to main measures them. From the base they would be:"]
        block = lines_of("base", KPIS)
        out += block[:2] + [f"env\t{ratchet['env']}"] + block[2:]
    return out


def render(hist, threshold, ratchet=None):
    last = hist[-1]
    lines = [MARKER, "## KPI: this PR against its base", ""]
    lines.append(
        f"Measured by the `KPI` workflow on runners of one image: the merge base `{last['base_sha'][:7]}` and the head "
        f"`{last['head_sha'][:7]}`, each with `bench/kpi_track.py --dry-run --only <sections>` in parts side by side (instruction counts of zustc, M = millions; "
        f"of one zust-analyzer session, G = billions; deterministic to about 0.1%). A change past +{threshold:g}% is marked ⚠️. "
        f"The last column is the head against its ceiling in `bench/kpi_ratchet.txt` (with the tolerance): above it, the job fails. This comment is updated on every push.")
    rows = {r["kpi"]: r for r in (ratchet or {}).get("rows", [])}
    lines += ["", "| KPI | base | head | change | ceiling |", "|---|---:|---:|---:|---:|"]
    for k in KPIS:
        b, h = last["base"].get(k), last["head"].get(k)
        p = pct(h, b)
        mark = " ⚠️" if p is not None and p > threshold else ""
        unit = UNITS.get(k, "M")
        lines.append(f"| {k} ({unit}) | {fmt_val(b)} | {fmt_val(h)} | {fmt_pct(p)}{mark} | {ceil_cell(rows.get(k), ratchet)} |")
    if ratchet:
        lines += ratchet_notes(ratchet, last)
    if len(hist) > 1:
        lines += ["", "### Every push", "",
                  "Change of the head against its base, and (in parentheses) against the push before.", "",
                  "| # | head | base | " + " | ".join(KPIS) + " |", "|---|---|---|" + "---:|" * len(KPIS)]
        prev = None
        for i, r in enumerate(hist):
            cells = []
            for k in KPIS:
                p = pct(r["head"].get(k), r["base"].get(k))
                s = fmt_pct(p)
                if prev is not None:
                    s += f" ({fmt_pct(pct(r['head'].get(k), prev['head'].get(k)))})"
                cells.append(s)
            lines.append(f"| {i + 1} | `{r['head_sha'][:7]}` | `{r['base_sha'][:7]}` | " + " | ".join(cells) + " |")
            prev = r
    lines += ["", "<!-- kpi-history " + json.dumps(hist) + " -->"]
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--head", required=True)
    ap.add_argument("--base-sha", required=True)
    ap.add_argument("--head-sha", required=True)
    ap.add_argument("--pr")
    ap.add_argument("--repo")
    ap.add_argument("--threshold", type=float, default=1.0)
    ap.add_argument("--out")
    ap.add_argument("--ratchet")
    ap.add_argument("--ratchet-base")
    ap.add_argument("--env")
    a = ap.parse_args()
    row = {"base_sha": a.base_sha, "head_sha": a.head_sha, "base": parse(a.base), "head": parse(a.head)}
    if not row["base"] or not row["head"]:
        sys.exit("no KPI values in the kpi_track.py output")
    comment = None
    if a.pr:
        try:
            comment = find_comment(a.repo, a.pr)
        except RuntimeError as e:
            print(f"::warning::cannot read the PR comments: {e}")
    hist = [r for r in history_of(comment["body"] if comment else "") if r["head_sha"] != a.head_sha]
    hist.append(row)
    ratchet = None
    if a.ratchet:
        import kpi_ratchet
        r = kpi_ratchet.read(a.ratchet)
        was = kpi_ratchet.read(a.ratchet_base) if a.ratchet_base else {"ceil": {}}
        ratchet = {"rows": kpi_ratchet.check(r, a.env, row["base"], row["head"]), "base_ceil": was["ceil"],
                   "env": a.env, "env_file": r["env"], "line_of": kpi_ratchet.line_of}
    body = render(hist, a.threshold, ratchet)
    for k in KPIS:
        p = pct(row["head"].get(k), row["base"].get(k))
        if p is not None and p > a.threshold:
            print(f"::warning::KPI {k}: {fmt_pct(p)} against the base ({fmt_val(row['base'][k])} -> {fmt_val(row['head'][k])})")
    failed = [r for r in (ratchet or {}).get("rows", []) if r["status"] == "fail"]
    for r in failed:
        print(f"::error::KPI {r['kpi']}: {fmt_val(row['head'][r['kpi']])}, {fmt_pct(r['over'])} above its ceiling "
              f"{fmt_val(r['ceil'])} in bench/kpi_ratchet.txt (tolerance {r['tol']:g}%)")
    if a.out:
        open(a.out, "w").write(body + "\n")
    if not a.pr:
        print(body)
        sys.exit(1 if failed else 0)
    try:
        if comment:
            gh("-X", "PATCH", f"repos/{a.repo}/issues/comments/{comment['id']}", "-F", "body=@-", input=body)
        else:
            gh("-X", "POST", f"repos/{a.repo}/issues/{a.pr}/comments", "-F", "body=@-", input=body)
    except RuntimeError as e:
        # (a pull request from a fork gets a read-only token: the result is still in the job summary)
        print(f"::warning::cannot write the PR comment: {e}")
    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
