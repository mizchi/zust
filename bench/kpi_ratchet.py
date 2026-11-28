#!/usr/bin/env python3
"""The KPI ratchet: the most each KPI of the `KPI` workflow may be (bench/kpi_ratchet.txt).

usage:
  python3 bench/kpi_ratchet.py check   bench/kpi_ratchet.txt --base base.txt --head head.txt --env ENV
  python3 bench/kpi_ratchet.py tighten bench/kpi_ratchet.txt --head head.txt --env ENV --sha SHA
  python3 bench/kpi_ratchet.py check   bench/kpi_ratchet.txt --head main.txt --env ENV --warn

base.txt / head.txt are what `bench/kpi_track.py --dry-run` printed (kpi_pr.parse reads them); ENV is the environment
the workflow's plan wrote (the toolchain, valgrind, the runner image and its C library).

A KPI that grows by less than the noise passes a comparison with the base however often it does, so a cost taken in
steps below kpi_pr.py's threshold (+1%) is never seen. A ceiling does not move with the base: a pull request whose head
is above a ceiling by more than the ceiling's tolerance fails the workflow (`check`), and nothing raises a ceiling but
an edit of the file in a pull request, which its diff shows. Every push to main lowers each ceiling that main
measures below by more than its tolerance to what it measured (`tighten`, in a commit of its own), so a cost removed
stays removed. (By more than the tolerance: a run that happens to count low does not move a ceiling, which every later
run would then be measured against.)

When main is above a ceiling already (two pull requests that each passed, merged one after the other), a head above it
fails only if it is above its base by more than half the tolerance too: the pull request has not made it worse, and
its comment shows main's excess.

The ceilings hold in the environment they were measured in (the file's `env` line). In another one the counts move
(a new toolchain, a new image): nothing is checked, and the next push to main measures every ceiling anew.
"""
import argparse, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import kpi_pr  # noqa: E402

# (the tolerance of a new ceiling, in %: two runs of one commit on the workflow's runners differ by up to about 0.2% in
# a build (edit one fn +0.22% on #341, whose second push changed no code), the LSP session's more (it moves with how
# often the client polls); the no-op builds are small counts, which a few files more to look at move by tenths of a
# percent)
TOLERANCE = {"no-op build": 2.0, "crates no-op": 2.0, "LSP session": 1.0}
DEFAULT_TOLERANCE = 0.5


def read(path):
    """{"env": str or None, "ceil": {kpi: [value, tolerance %, commit]}, "header": [comment lines]}"""
    r = {"env": None, "ceil": {}, "header": []}
    if not os.path.exists(path):
        return r
    for line in open(path):
        line = line.rstrip("\n")
        if line.startswith("#") or not line.strip():
            if not r["ceil"] and r["env"] is None:
                r["header"].append(line)
            continue
        # (tab-separated; or by runs of two spaces or more, as a line copied from a rendered comment may be)
        f = line.split("\t") if "\t" in line else re.split(r"\s{2,}", line.strip())
        if f[0] == "env":
            r["env"] = f[1] if len(f) > 1 and f[1] != "-" else None
        elif f[0] in kpi_pr.KPIS and len(f) >= 3:
            r["ceil"][f[0]] = [float(f[1]), float(f[2]), f[3] if len(f) > 3 else "-"]
        else:
            sys.exit(f"{path}: cannot read {line!r} (a KPI's name, its ceiling, its tolerance in %, a commit)")
    return r


def line_of(k, v, tol, sha):
    return f"{k}\t{v!r}\t{tol:g}\t{sha}"


def write(path, r):
    lines = list(r["header"])
    lines.append(f"env\t{r['env'] or '-'}")
    for k in kpi_pr.KPIS:
        if k in r["ceil"]:
            lines.append(line_of(k, *r["ceil"][k]))
    open(path, "w").write("\n".join(lines) + "\n")


def tolerance(r, k):
    return r["ceil"][k][1] if k in r["ceil"] else TOLERANCE.get(k, DEFAULT_TOLERANCE)


def check(r, env, base, head):
    """One row per KPI: {kpi, ceil, tol, over (% of the head above the ceiling), status}, status one of
    "ok", "fail", "main over" (the base is above the limit too, and the head not above the base by more than half the
    tolerance), "none" (no ceiling) and "env" (measured in another environment: not checked)."""
    rows = []
    for k in kpi_pr.KPIS:
        h, b = head.get(k), base.get(k)
        row = {"kpi": k, "ceil": None, "tol": tolerance(r, k), "over": None, "status": "none"}
        rows.append(row)
        if k not in r["ceil"] or h is None:
            continue
        c = r["ceil"][k][0]
        row["ceil"], row["over"] = c, kpi_pr.pct(h, c)
        if r["env"] != env:
            row["status"] = "env"
        elif h <= c * (1 + row["tol"] / 100):
            row["status"] = "ok"
        elif b is not None and b > c * (1 + row["tol"] / 100) and h <= b * (1 + row["tol"] / 200):
            row["status"] = "main over"
        else:
            row["status"] = "fail"
    return rows


def tighten(r, env, head, sha):
    """Lower the ceilings to what main measured (head), where it is below by more than the tolerance; in another
    environment, every ceiling becomes what main measured. Returns the lines that changed, as `old -> new`."""
    changed = []
    reset = r["env"] != env
    r["env"] = env
    for k in kpi_pr.KPIS:
        h = head.get(k)
        if h is None:
            # (a ceiling of the other environment that this run did not measure anew holds nowhere now)
            if reset and k in r["ceil"]:
                changed.append(f"{k}: {r['ceil'].pop(k)[0]!r} -> - (not measured in this environment)")
            continue
        tol = tolerance(r, k)
        old = r["ceil"].get(k)
        if reset or old is None or h < old[0] * (1 - tol / 100):
            r["ceil"][k] = [h, tol, sha]
            changed.append(f"{k}: {'-' if old is None else repr(old[0])} -> {h!r}")
    return changed


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("what", choices=["check", "tighten"])
    ap.add_argument("file")
    ap.add_argument("--base")
    ap.add_argument("--head", required=True)
    ap.add_argument("--env", required=True)
    ap.add_argument("--sha", default="-")
    # (check on main, after a merge: a KPI above its ceiling is a ::warning::, which the push's run shows, not a failure)
    ap.add_argument("--warn", action="store_true")
    a = ap.parse_args()
    r = read(a.file)
    head = kpi_pr.parse(a.head)
    if not head:
        sys.exit(f"{a.head}: no KPI values")
    if a.what == "tighten":
        changed = tighten(r, a.env, head, a.sha)
        write(a.file, r)
        print("\n".join(changed) if changed else "no ceiling lowered")
        return
    rows = check(r, a.env, kpi_pr.parse(a.base) if a.base else {}, head)
    for row in rows:
        print(f"{row['kpi']}\t{row['status']}\t{kpi_pr.fmt_pct(row['over'])} of the ceiling (tolerance {row['tol']:g}%)")
        if a.warn and row["status"] == "fail":
            print(f"::warning::KPI {row['kpi']}: main is {kpi_pr.fmt_pct(row['over'])} above its ceiling in bench/kpi_ratchet.txt "
                  f"(tolerance {row['tol']:g}%): two changes that each passed, or one merged without the check. Take the cost out, "
                  "or raise the ceiling on purpose")
    sys.exit(1 if not a.warn and any(row["status"] == "fail" for row in rows) else 0)


if __name__ == "__main__":
    main()
