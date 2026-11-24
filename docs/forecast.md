# Coverage forecast

How far zustc is from passing 90% of rustc's tests/ui, and how long the rest may take, as of 2026-11-24. The numbers
come from `bench/forecast.py`, which reads only this repository's history. To estimate again later, run:

```
python3 bench/forecast.py --pending <branch of each open PR, a stack in its order>
```

The report has the following parts:

- the README's tests/ui table now, and at the end of each day;
- what the open PRs add (each one's `bench/rust_ui_ratchet.txt` against where it left main);
- the days to the target, at the recent pace;
- what each merged change gained;
- with `bench/effort_log.tsv` filled in: tokens per test, and the tokens the rest would take.

## Summary (2026-11-24)

| | pass | of 11,737 applicable |
|---|---:|---:|
| main (README, 235dae8) | 8,170 | 69.6% |
| main and the open PRs (#106, #121, #124, #122, #110) and compat-next44 | about 8,343 | about 71% |
| 90% | 10,564 | |

To reach 90%, about 2,220 more tests have to pass. Merged work has added about 280 tests a day over the last two
days. The daily gain has fallen from +982 to about +260, because the tests that pass first are the easy ones.

| assumption | days to 90% (all kinds) |
|---|---:|
| the same number of tests a day (linear) | about 8 |
| the same share of the remaining tests a day (geometric) | about 13 |

**Estimate: 2 to 3 weeks of the current work.** Each kind reaching 90% on its own takes longer:

- **compile-fail** is most of the remaining tests: +1,790 at about 240 a day, 7 to 13 days.
- **run-pass** grew by 43 in the last two days, and needs +305. Part of the rest needs the large features that
  [SUPPORT.md](../SUPPORT.md#large-features-not-started) lists as not started: unwinding (~22 tests), executors (~25),
  FFI structs by value (~27), real threads. At the recent pace it takes 13 to 19 days.
- **check-pass** needs +107, about a week.
- **build-pass** needs +20 and has barely moved.

What these numbers mean:

- "Applicable" is the tests/ui tests `bench/rust_ui.py` can ask zustc to handle: no nightly features, auxiliary
  crates or special flags. 90% of them is not 90% of Rust.
- A compile-fail test passes when zustc reports an error on a line an annotation points at. The message is not
  compared.
- The README counts the accidental passes of `bench/rust_ui_accidental.txt` (56 compile-fail on main). The ratchets,
  and so the "not merged" column, do not.

## Estimating tokens

A session cannot read its own token usage, so the tokens come from outside. For a merged PR, write a row in
`bench/effort_log.tsv`, separated by tabs: the PR's number, the tokens it took (from the usage dashboard), and
optionally the hours. `bench/forecast.py` then prints, for those PRs:

- tokens per test gained (the PR's raised ratchet, all kinds);
- that rate times the tests still needed for the target.

The later tests are harder than the ones already done, so take the result as a lower bound. The table "What each
merged change gained" below gives each PR's yield in tests, for choosing which PRs to measure.

## Report (bench/forecast.py, 2026-11-24)

### Measured (README of origin/main, 2026-11-24 08:43 UTC)

| kind | applicable | pass | rate |
|---|---:|---:|---:|
| run-pass | 2403 | 1857 | 77.3% |
| check-pass | 1813 | 1524 | 84.1% |
| build-pass | 135 | 102 | 75.6% |
| compile-fail | 7386 | 4687 | 63.5% |
| all | 11737 | 8170 | 69.6% |

### History (all kinds, the last measurement of each day, UTC)

| day | pass | rate | gain |
|---|---:|---:|---:|
| 2026-11-18 | 5546 | 47.3% |  |
| 2026-11-19 | 6528 | 55.9% | +982 |
| 2026-11-20 | 6920 | 59.2% | +392 |
| 2026-11-21 | 7446 | 63.7% | +526 |
| 2026-11-22 | 7850 | 67.2% | +404 |
| 2026-11-23 | 8111 | 69.4% | +261 |
| 2026-11-24 | 8170 | 69.6% | +59 |

### Not merged yet (bench/rust_ui_ratchet.txt of each branch)

| branch | run-pass | check-pass | build-pass | compile-fail |
|---|---:|---:|---:|---:|
| origin/claude/compat-next42 | +0 | +0 | +0 | +29 |
| origin/claude/compat-next43 | +0 | +0 | +0 | +78 |
| origin/claude/compat-next44 | +0 | +0 | +0 | +47 |
| origin/claude/sleepy-dijkstra-8sankk | +0 | +0 | +0 | +9 |
| origin/claude/serde-core-std | +1 | +1 | +0 | +8 |
| origin/claude/popular-crates | +0 | +0 | +0 | +0 |

(each branch against the ratchet of the commit it left REF at, and of the pending branches it is stacked on; a branch that did not raise its ratchet shows +0)

### Forecast to 90% (pace of the README's last 2.0 days)

| kind | pass now | + not merged | needed | tests/day | days (linear) | days (geometric) |
|---|---:|---:|---:|---:|---:|---:|
| run-pass | 1857 | +1 | 305 | 23 | 13.3 | 19.2 |
| check-pass | 1524 | +1 | 107 | 15 | 6.9 | 8.5 |
| build-pass | 102 | +0 | 20 | 0 | 40.3 | 61.4 |
| compile-fail | 4687 | +171 | 1790 | 241 | 7.4 | 13.1 |
| all | 8170 | +173 | 2221 | 280 | 7.9 | 13.0 |

### What each merged change gained (first-parent commits of origin/main that raised bench/rust_ui_ratchet.txt)

| time (UTC) | change | run-pass | check-pass | build-pass | compile-fail | tokens | tokens / test |
|---|---|---:|---:|---:|---:|---:|---:|
| 2026-11-21 11:40 | Merge pull request #24 from mizchi/claude/compat-next20 |  |  |  | +35 |  |  |
| 2026-11-21 14:34 | Merge pull request #26 from mizchi/claude/compat-next21 | +1 |  |  | +55 |  |  |
| 2026-11-21 16:13 | Merge pull request #27 from mizchi/claude/compat-next22 | +4 | +4 |  | +7 |  |  |
| 2026-11-21 18:37 | Merge pull request #32 from mizchi/claude/compat-next23 |  |  |  | +18 |  |  |
| 2026-11-21 23:00 | Merge pull request #39 from mizchi/claude/sleepy-dijkstra-8sankk | +2 | +2 |  | +249 |  |  |
| 2026-11-22 00:04 | Merge pull request #40 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +16 |  |  |
| 2026-11-22 00:57 | Merge pull request #36 from mizchi/claude/compat-next24 |  | +5 | +1 | +9 |  |  |
| 2026-11-22 02:14 | Merge pull request #42 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +12 |  |  |
| 2026-11-22 05:19 | Merge pull request #43 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +47 |  |  |
| 2026-11-22 05:46 | Merge pull request #41 from mizchi/claude/compat-next25 | +7 | +11 | +2 | +28 |  |  |
| 2026-11-22 06:33 | Merge pull request #46 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +16 |  |  |
| 2026-11-22 08:20 | Merge pull request #47 from mizchi/claude/compat-next29 |  |  |  | +5 |  |  |
| 2026-11-22 09:03 | Merge pull request #48 from mizchi/claude/sleepy-dijkstra-8sankk | +2 |  |  | +21 |  |  |
| 2026-11-22 11:33 | Merge pull request #51 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +20 |  |  |
| 2026-11-22 11:50 | Merge pull request #50 from mizchi/claude/compat-next30 |  |  |  | +21 |  |  |
| 2026-11-22 14:29 | Merge pull request #53 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +30 |  |  |
| 2026-11-22 15:44 | Merge pull request #52 from mizchi/claude/compat-next31 | +2 | +2 | +1 | +40 |  |  |
| 2026-11-22 16:18 | Merge pull request #55 from mizchi/claude/compat-next32 | +1 | +3 |  | +31 |  |  |
| 2026-11-22 18:06 | Merge pull request #57 from mizchi/claude/compat-next33 | +2 | +4 |  |  |  |  |
| 2026-11-22 19:39 | Merge pull request #54 from mizchi/claude/sleepy-dijkstra-8sankk | +6 | +7 |  | +28 |  |  |
| 2026-11-22 21:30 | Merge pull request #65 from mizchi/claude/compat-next35 | +3 | +1 |  | +4 |  |  |
| 2026-11-22 21:48 | Merge pull request #64 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +12 |  |  |
| 2026-11-23 05:49 | Merge pull request #67 from mizchi/claude/sleepy-dijkstra-8sankk |  |  |  | +21 |  |  |
| 2026-11-23 06:14 | Merge pull request #66 from mizchi/claude/compat-next36 | +5 | +1 |  | +13 |  |  |
| 2026-11-23 08:01 | Merge pull request #68 from mizchi/claude/compat-next37 | +1 | +1 |  | +25 |  |  |
| 2026-11-23 08:56 | Merge pull request #70 from mizchi/claude/compat-next38 |  |  |  | +9 |  |  |
| 2026-11-23 11:04 | Merge pull request #71 from mizchi/claude/sleepy-dijkstra-8sankk | +1 |  |  | +20 |  |  |
| 2026-11-23 12:45 | Merge pull request #72 from mizchi/claude/compat-next39 |  |  |  | +14 |  |  |
| 2026-11-23 14:10 | Region check part 16: deref coercions, block values and guards that... |  |  |  | +11 |  |  |
| 2026-11-23 15:12 | memchr builds for wasm32 (#90) |  | +2 |  |  |  |  |
| 2026-11-23 17:43 | Lints in expressions, E0308 where rustc points, more E0658 gates, E... |  |  |  | +79 |  |  |
| 2026-11-23 19:22 | Region check part 17: raw pointers, DerefMut receivers, scrutinee a... | +7 | +1 |  | +12 |  |  |
| 2026-11-23 19:36 | E0107 at GAT uses and dyn types, E0449, E0434, E0191 at dyn types (... |  |  |  | +42 |  |  |
| 2026-11-24 01:44 | Region check part 18: let extension through constructors, closure c... | +1 |  |  | +5 |  |  |
| 2026-11-24 02:30 | W1 of #38: proc macros run through the toolchain's proc-macro serve... |  | +3 |  |  |  |  |
| 2026-11-24 03:36 | Region check part 19: closures' calls and requirements, fn items as... | +6 | +1 |  | +10 |  |  |
| 2026-11-24 06:02 | Merge pull request #108 from mizchi/claude/tender-sagan-iatvun | +4 | +1 |  |  |  |  |
| 2026-11-24 06:29 | Merge pull request #107 from mizchi/claude/serde-core | +7 | +4 |  | +9 |  |  |
| 2026-11-24 07:48 | Region check part 20: closures against the regions their bounds bin... |  |  |  | +5 |  |  |
| 2026-11-24 08:38 | Region check part 21: fn items coerced to fn pointers, deref coerci... |  |  |  | +3 |  |  |

Tokens: bench/effort_log.tsv has no tokens yet. Write a row `pr<TAB>tokens<TAB>hours` for a merged PR (from the usage dashboard) and this report gives tokens per test and the tokens the rest would take.
