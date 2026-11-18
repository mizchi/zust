# Mutation testing (cargo-mutants, 2026-11-17)

Date: 2026-11-17

Installed with `cargo install cargo-mutants --locked` (27.1.0), and run with `bench/mutants.sh`.

- Settings and scope:
  - Settings are in `.cargo/mutants.toml`. Builds use `profile.mutants` (the same optimization as release, with incremental builds).
  - Not mutated: CLI argument handling, build.rs, test and bench code, Debug / Display.
  - There are 18,240 mutants in total. Each one takes a few seconds to 1 minute of tests, so not all of them are run.
- Narrowing the scope:
  - `bench/mutants.sh diff [BASE]`: only mutants on lines in the diff from BASE (default origin/main). For checking changes.
  - `bench/mutants.sh pkg zust-ide`: per crate.
  - `bench/mutants.sh file crates/ide/src/symbols.rs -- -F match_score -j 2`: specify the file, the function name, and the parallelism.
  - `bench/mutants.sh list ...`: only show the count.
  - Everything after `--` is passed to cargo mutants as-is (`--shard 1/8`, etc.).
- Packages used for testing:
  - Mutants only in crates/ide / crates/analyzer: the tests of zust-ide and zust-analyzer. 2–3 seconds each.
  - Others: the tests of the whole workspace (many tests of the compiler layers are in the root tests/). About 1 minute each.
- The timeout is the larger of 3 times the baseline test time or 180 seconds.
- Results go in `mutants.out/` (missed.txt lists the mutants the tests did not catch).

Results of trial runs:

| Target | Mutants | Caught | Missed | Time (-j 2) |
|---|---:|---:|---:|---:|
| `match_score` (ide) | 25 | 16 → 24 | 9 → 1 | 87 seconds |
| `KeyState::done` (base) | 7 | 5 | 2 | 5 minutes |

- The 9 missed mutants in `match_score` were in the word-start check (separators other than `_`); the tests only looked at `_`.
  Added cases for camelCase, `::`, whitespace, and names with consecutive uppercase letters.
- The remaining 1 (`i > 0` → `i >= 0`) is an equivalent mutant: i = 0 returns earlier as a prefix match, so it never gets there.
- The 2 in `KeyState::done` are in how the hash is mixed (`^` → `|`, `<<` → `>>`). A cache key works as long as the same input gives
  the same value, so functional tests cannot see this (it degrades collision resistance).
