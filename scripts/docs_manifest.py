#!/usr/bin/env python3
"""Builds the jev-lint docs manifests (.jev-lint/documents*.json) from the pairs of English documents and their Japanese translations.

usage: python3 scripts/docs_manifest.py          rewrites the manifests
       python3 scripts/docs_manifest.py --check  checks that the pairs are complete and the manifests are up to date (writes nothing)

Pairs: CONTRIBUTING.md / CONTRIBUTING.ja.md, docs/X.md / docs/ja/X.md (README.md is English only)
(docs/reports/ included). A jev-lint manifest holds up to 100 files, so the reports are written separately.
"""
import glob, json, os, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, ".jev-lint")
REPORTS_PER_MANIFEST = 40


def pairs():
    """The (English, Japanese) pairs, and the files without a counterpart"""
    out, missing = [], []
    top = [("CONTRIBUTING.md", "CONTRIBUTING.ja.md")]
    en_docs = sorted(p for p in glob.glob("docs/**/*.md", root_dir=ROOT, recursive=True) if not p.startswith("docs/ja/"))
    ja_docs = sorted(glob.glob("docs/ja/**/*.md", root_dir=ROOT, recursive=True))
    for en in en_docs:
        top.append((en, "docs/ja/" + en[len("docs/"):]))
    for en, ja in top:
        if os.path.exists(os.path.join(ROOT, en)) and os.path.exists(os.path.join(ROOT, ja)):
            out.append((en, ja))
        else:
            missing.append(ja if os.path.exists(os.path.join(ROOT, en)) else en)
    for ja in ja_docs:
        if not os.path.exists(os.path.join(ROOT, "docs/" + ja[len("docs/ja/"):])):
            missing.append("docs/" + ja[len("docs/ja/"):])
    return out, missing


def key(path):
    return os.path.splitext(path)[0].replace("/", "_").replace(".", "_")[:70]


def manifest(group, role_of):
    files, comps = {}, []
    for en, ja in group:
        k = key(en)
        files[f"{k}.en"] = {"path": en, "role": role_of(en)}
        files[f"{k}.ja"] = {"path": ja, "role": role_of(en)}
        comps.append({"id": k, "topic": f"The Japanese file {ja} is a faithful translation of {en}", "mode": "translation",
                      "reference": {"source": f"{k}.en"}, "target": {"source": f"{k}.ja"}})
    return {"schema": "jev-lint-documents-manifest-1", "files": files, "comparisons": comps}


def build():
    ps, missing = pairs()
    is_report = lambda en: en.startswith("docs/reports/2")
    current = [p for p in ps if not is_report(p[0])]
    reports = [p for p in ps if is_report(p[0])]
    out = {"documents.json": manifest(current, lambda en: "document")}
    for i in range(0, len(reports), REPORTS_PER_MANIFEST):
        out[f"documents-reports-{i // REPORTS_PER_MANIFEST + 1}.json"] = manifest(reports[i:i + REPORTS_PER_MANIFEST], lambda en: "historical")
    return out, missing


def main():
    check = "--check" in sys.argv[1:]
    out, missing = build()
    if missing:
        print("翻訳の相手がない文書 (英語は docs/X.md、日本語は docs/ja/X.md):", file=sys.stderr)
        for m in missing:
            print(f"  missing: {m}", file=sys.stderr)
    stale = []
    for name, m in out.items():
        text = json.dumps(m, indent=2, ensure_ascii=False) + "\n"
        p = os.path.join(OUT, name)
        old = open(p).read() if os.path.exists(p) else None
        if old != text:
            stale.append(name)
            if not check:
                os.makedirs(OUT, exist_ok=True)
                open(p, "w").write(text)
    extra = [f for f in os.listdir(OUT) if f.startswith("documents") and f.endswith(".json") and "baseline" not in f and f not in out] if os.path.isdir(OUT) else []
    for f in extra:
        stale.append(f)
        if not check:
            os.remove(os.path.join(OUT, f))
    if check and stale:
        print(f"マニフェストが古い: {', '.join(stale)} (python3 scripts/docs_manifest.py で書き直す)", file=sys.stderr)
    if not check:
        for name in out:
            print(f"{name}: {len(out[name]['comparisons'])} comparisons")
    sys.exit(1 if missing or (check and stale) else 0)


if __name__ == "__main__":
    main()
