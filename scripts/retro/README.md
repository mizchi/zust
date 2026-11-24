# Development retrospective

The data behind <https://mizchi.github.io/zust/dashboard/>: how zust was built, read from its git history.

The data is the master and the page is a view of it. `extract.py` writes `manifest.json` plus one JSON array per
dataset, following the `repo-retrospective/v1` data contract (the
[repo-retrospective](https://github.com/mizchi/skills/tree/main/repo-retrospective) skill): every column is declared
with its type, unit and meaning, and every dataset says whether its numbers were recorded at the time, derived from
git, or hand-picked. The page reads the manifest and the files it names and nothing else, so it can be rewritten (or
replaced by another view) without touching the data. Check a data directory with that skill's
`scripts/validate_retro.py out/retro`.

| file | what |
|---|---|
| `extract.py` | writes `manifest.json` and the datasets from git history, `bench/kpi_log.tsv` and the benchmark history on the gh-pages branch; `datasets_spec()` is the declaration of every column |
| `milestones.json` | hand-picked decision points shown on the commit charts (time, commit, label, detail) |
| `../../site/dashboard/index.html` | the page; it fetches `data/*.json` next to it and draws them with d3 |
| `../../.github/workflows/pages.yml` | reruns `extract.py` after each benchmark run and publishes `site/` to gh-pages |

## Rebuilding it locally

```bash
python3 scripts/retro/extract.py --out out/site/dashboard/data
cp -R site/. out/site/
mkdir -p out/site/dev/bench
git show origin/gh-pages:dev/bench/data.js > out/site/dev/bench/data.js   # for the numbers on /
python3 -m http.server -d out/site 8000                                     # then open /dashboard/
```

`extract.py` fetches `origin gh-pages` for the tests/ui pass rates, so it needs that remote.

## Adding a milestone

Milestones are judgement calls, so they are a checked-in list, not computed. To find candidates, look for commits that
change direction rather than add one more thing: renames, crate splits and removals, the first commit of a new test
suite, benchmark or CI gate, and the first commit of each dated report under `docs/reports/`. Add an entry to
`milestones.json` with the commit's author time in UTC.

## What the numbers mean

- Commits: `git log --no-merges` without `github-actions[bot]`, cut by UTC hour and day. The "compat" phase starts at
  `c155863e`, the commit that made rustc's tests/ui the spec suite.
- tests/ui: pass rates as `bench/rust_ui.py --bench-json` reported them on each CI run.
- KPIs: `bench/kpi_log.tsv` as written; rows whose commit was rebased away get a time between their neighbours
  (`time_estimated`).
- Code size: `.rs` lines on one first-parent commit per 3 hours. Tests are `tests/`, `crates/*/tests/` and each
  source file from its first `#[cfg(test)]` to its end; `crates/zustscript` is counted on its own and left out of both.
- Sessions: commits grouped by their `Claude-Session` trailer.
