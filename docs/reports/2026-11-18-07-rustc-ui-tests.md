# rustc's own UI tests as a specification suite

Date: 2026-11-18

`tests/ui` in rust-lang/rust is the closest thing Rust has to an executable specification: each file says, in header
directives, whether it must run, must check, or must be rejected (with the expected errors marked by `//~ ERROR`).
`bench/rust_ui.py` runs zustc against it and reports pass rates.

## Fetching

A git submodule would download all of rust-lang/rust, and git cannot make a submodule sparse without manual setup.
The script instead pins a commit (tag 1.100.0, `0ef1bc386`, the same commit as the rustc used here) and does a
shallow, blob-filtered, sparse clone of only `tests/ui` into `out/rust-tests`: 6 seconds and 170 MB, 20,364 test
files.

## What counts as passing

The kind of each test comes from its directives, as in rustc's compiletest:

| kind | files | passes when |
|---|---:|---|
| run-pass | 3771 | zustc builds it, it exits 0, and stdout equals `<name>.run.stdout` if that exists |
| check-pass | 3654 | `zustc check` accepts it |
| build-pass | 572 | zustc builds it |
| compile-fail (no pass/fail directive) | 11707 | `zustc check` rejects it with an error on a line an annotation points at |

Tests that need what zustc cannot be asked for are counted as not applicable: nightly `#![feature]`, auxiliary crates
(`aux-build`, `aux-crate`), `compile-flags` / `rustc-env`, `revisions`, targets other than Linux x86-64, `ignore-test`,
and compile-fail tests without annotations. Each zustc run is limited to 2 GB of address space (`prlimit`) and a
timeout; one test had taken the whole machine's memory before the limit was added.

For compile-fail, "rejected" alone was misleading. A first version counted any rejection and gave 64%. Many of
those were rejections for unrelated reasons, such as a missing type that the test did not mean to test. Requiring an
error on an annotated line (`//~` this line, `//~^` one line up per caret, `//~|` the same line as the previous
annotation) gives 31%. Rejections elsewhere are reported separately as "wrong line".

## Results

On 4 cores with 3 workers the applicable tests take 112 seconds.

| kind | applicable | pass | fail | wrong line | crash | timeout | pass rate | not applicable |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| run-pass | 2413 | 1120 | 1287 | - | 5 | 1 | 46.42% | 1357 |
| check-pass | 1807 | 831 | 972 | - | 4 | 0 | 45.99% | 1843 |
| build-pass | 135 | 64 | 69 | - | 2 | 0 | 47.41% | 354 |
| compile-fail | 7358 | 2291 | 2613 | 2428 | 23 | 3 | 31.14% | 4438 |

Two runs on the same machine gave the same set of passing tests.

## Keeping it from regressing

`bench/rust_ui_passing.txt` lists the 4306 passing tests. CI (`rust-ui` job) fetches `tests/ui` and fails if a test
on the list no longer passes; a timeout only warns. When a change makes more tests pass, `--write-baseline` rewrites
the list. The benchmark workflow also sends the pass rates and counts to the dashboard.

## What is next

The failures are the backlog for the language: `out/rust_ui_results.txt` lists every test with its result, and
`--filter` narrows a run to one area (for example `--filter tests/ui/closures`). The 34 crashes come first, since a
compiler should not panic on any input.
