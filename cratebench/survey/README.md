# cratebench/survey: the top crates of crates.io

Which of crates.io's most used crates zustc builds, the errors left in each, and which packages to fix first. The
results are in [CRATES_SUPPORT.md](../../CRATES_SUPPORT.md) and on the [dashboard](https://mizchi.github.io/zust/crates/).

Nothing is vendored: the sources come from crates.io, and each probe is laid out in a temporary directory.

```bash
cargo build --release -p zustc -p zurgo
python3 cratebench/survey/survey.py run                 # every crate of crates.tsv (about an hour on 4 cores)
python3 cratebench/survey/survey.py run tokio serde     # some of them (the others' results are kept)
python3 cratebench/survey/survey.py run --tests         # also the own tests of the crates that build
python3 cratebench/survey/survey.py report              # CRATES_SUPPORT.md again from results.json
```

## Files

| file | what | written by |
|---|---|---|
| `crates.tsv` | the crates: the top N by recent downloads, then `extra.tsv`'s; rank, name, version, downloads, kind, category, features | `survey.py list [--top N]` |
| `extra.tsv` | crates surveyed besides the top N, or again with other features (tokio `full`, embassy, embedded crates) | by hand |
| `issues.tsv` | the fixes with a tracking issue: a regex on an error's text, the issue, the fix's name | by hand, when an issue is filed |
| `Cargo.lock` | one lock for every probe (the probes resolved as one workspace); each probe takes the versions it uses | `survey.py lock [--fresh]` |
| `results.json` | each crate's result, each package's errors, and the priorities and error kinds derived from them | `survey.py run` / `report` |
| `history.tsv` | one row per survey on main | `run --history` (CI) |

`list` and `lock` change what is measured, so run them on purpose and commit what they write: a new list or new
versions move the numbers as much as zustc does.

## What a probe is

For a crate `x` of version `v` with features `f`, `run` writes `<work>/probes/x/` (by default
`$TMPDIR/zust-crate-survey`):

```toml
[package]
name = "probe-x"
version = "0.0.0"
edition = "2021"

[lib]
path = "lib.rs"          # empty

[dependencies]
x = { version = "=v", features = [f] }

[workspace]
```

with a copy of `Cargo.lock` (cargo drops the packages the probe does not use and keeps the versions). Then, for
x86_64 Linux:

1. `cargo metadata`: the graph. zustc compiles the packages reached by normal dependencies, not through a proc macro.
2. `zurgo check`: each package's errors. A package's errors do not stop the check of the others, so the count is per
   package. It is a lower bound: zustc stops reading a crate at a syntax or macro error, and checks each function
   only up to its first error.
3. `zurgo build`, when the check found nothing: the crate and its dependencies compiled and linked.
4. With `--tests`, for the crates that build: the package's own tests (lib, bins, `tests/`; not doc tests), copied
   from the crates.io package to `<work>/tests/x/`, under `cargo test` and `zurgo test`, compared test by test.

The probe's target directory is deleted after it; `check.log` and `build.log` stay in the probe's directory.

## Statuses

The first that applies:

| status | meaning |
|---|---|
| graph-fail | zurgo cannot plan the graph (two versions of one crate, mizchi/zust#28, and others) |
| errors | zustc reports errors in the crate's own code |
| blocked | the crate's own code checks, or is not read, and a dependency's does not |
| build-fail | the check finds nothing; the build fails (code generation, linking) |
| builds | zurgo builds the crate and every dependency |
| timeout / fetch-fail | over 900 s / cargo could not resolve or fetch it |

Proc-macro crates are not probed: they run on the host, where rustc builds them, and the crates that use them test
them. Crates for other platforms (windows-*, wasm-bindgen, …) are listed and not probed.

## Categories

Crates of one kind tend to stop zustc in the same places, so each crate has a category: from its crates.io categories
(`SLUGS` in survey.py), or by name where those say little (`BY_NAME`). `list` keeps the category a crate already has
in crates.tsv, so a category corrected by hand stays.

## What to fix, by importance

Every error (and every reason zurgo cannot plan or build a crate) is given the fix it needs:

1. The first row of `issues.tsv` whose regex matches the error's text, which gives its issue.
2. Otherwise, a name zustc cannot find:
   - if the crate does not define it, a prelude addition (#250);
   - if it does, one of the crate's own items zustc lost (#254).
3. Otherwise, the error's text without places and names (its *kind*), with no issue yet.

Each surveyed crate that does not build needs the fixes of every error in its graph, its own and its dependencies'. Its downloads are shared equally among them. A fix's score, *holds back*, is the sum of its shares, as a share of the surveyed crates' downloads.

- **A sole blocker** of many popular crates ranks first.
- **A fix that is one of many** in each graph ranks lower.
- **A frequent error in crates nobody depends on** ranks low too.

*Alone* lists the crates that need only that fix. Fixing it makes them check, or plan, or build, until the next error shows.

The top rows without an issue are the next ones to file. When one is filed, add its row to `issues.tsv`.

## Packages with errors

For each package with errors:

- *needed by*: the surveyed crates (with errors, or blocked) whose graph has it;
- *alone*: those of them in which no other package has errors, so that fixing this package alone makes them check.

The list is ordered by the downloads of the *alone* crates, then by *needed by*. *Errors by kind* groups the errors by
their text without places, names and numbers: a kind in many packages is one gap of zustc's.

## How it relates to the rest of cratebench

- [`../popular/`](../popular/README.md): fewer crates, deeper. A program that uses each crate's API, run, with its
  output compared to rustc's.
- [`../test.py`](../README.md): the vendored crates' own tests. `run --tests` does the same for every crate that builds,
  from crates.io's packages, with no submodule.
