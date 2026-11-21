#!/usr/bin/env python3
"""results/NNN-name/rK-persona.json を集計して、点の表と指摘の一覧を Markdown で出す。

usage: python3 tools/summarize.py results/001-variants [--round K]
"""
import glob, json, os, re, sys

CRITERIA = ["R1", "R2", "R3", "R4", "R5", "R6", "R7", "R8"]
# (rubric.md の persona ごとの重み)
WEIGHTS = {
    "ts-dev": [2, 1, 1, 1, 1, 2, 2, 1],
    "rust-dev": [1, 1, 1, 2, 2, 1, 1, 1],
    "implementer": [1, 2, 2, 1, 1, 1, 1, 2],
}


def load(d):
    """{round: {persona: entry}}"""
    out = {}
    for p in sorted(glob.glob(os.path.join(d, "r*-*.json"))):
        m = re.match(r"r(\d+)-(.+)\.json$", os.path.basename(p))
        if not m:
            continue
        data = json.load(open(p))
        entry = data[0] if isinstance(data, list) else data
        out.setdefault(int(m.group(1)), {})[m.group(2)] = entry
    return out


def score(e, c):
    s = e.get("scores", {}).get(c, {})
    return s.get("score") if isinstance(s, dict) else s


def table(rounds):
    lines = ["| round | persona | " + " | ".join(CRITERIA) + " | mean | weighted | blockers |", "|" + "---|" * (len(CRITERIA) + 5)]
    for k in sorted(rounds):
        for persona, e in sorted(rounds[k].items()):
            ss = [score(e, c) or 0 for c in CRITERIA]
            w = WEIGHTS.get(persona, [1] * len(CRITERIA))
            mean = sum(ss) / len(ss)
            wmean = sum(a * b for a, b in zip(ss, w)) / sum(w)
            lines.append(f"| r{k} | {persona} | " + " | ".join(str(s) for s in ss) + f" | {mean:.2f} | {wmean:.2f} | {len(e.get('blocker') or [])} |")
        # (観点ごとの平均)
        per = [sum(score(e, c) or 0 for e in rounds[k].values()) / len(rounds[k]) for c in CRITERIA]
        lines.append(f"| r{k} | **all** | " + " | ".join(f"{x:.1f}" for x in per) + f" | {sum(per) / len(per):.2f} | | |")
    return "\n".join(lines)


def issues(rounds, k):
    order = {"high": 0, "medium": 1, "low": 2}
    rows = []
    for persona, e in rounds[k].items():
        for b in e.get("blocker") or []:
            rows.append((-1, persona, "blocker", b if isinstance(b, str) else json.dumps(b, ensure_ascii=False)))
        for i in e.get("issues") or []:
            rows.append((order.get(i.get("severity"), 3), persona, i.get("severity", "?"), f"{i.get('where', '')}: {i.get('problem', '')} → {i.get('suggestion', '')}"))
    rows.sort()
    out = [f"### r{k}: 指摘({len(rows)} 件)", ""]
    out += [f"- [{sev}] ({persona}) {text}" for _, persona, sev, text in rows]
    qs = [(p, q) for p, e in rounds[k].items() for q in e.get("questions") or []]
    if qs:
        out += ["", f"### r{k}: 問い", ""] + [f"- ({p}) {q}" for p, q in qs]
    alts = [(p, e["alternative"]) for p, e in rounds[k].items() if e.get("alternative") and e["alternative"].get("summary")]
    if alts:
        out += ["", f"### r{k}: 代替案", ""] + [f"- ({p}) {a['summary']}\n\n  ```zsc\n  " + a.get("example", "").replace("\n", "\n  ") + "\n  ```" for p, a in alts]
    return "\n".join(out)


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    d = sys.argv[1]
    rounds = load(d)
    if not rounds:
        sys.exit(f"no results in {d}")
    k = int(sys.argv[sys.argv.index("--round") + 1]) if "--round" in sys.argv else max(rounds)
    print(f"## {os.path.basename(d.rstrip('/'))}\n")
    print(table(rounds))
    print()
    print(issues(rounds, k))


if __name__ == "__main__":
    main()
