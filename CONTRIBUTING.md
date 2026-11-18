# Contributing

English | [日本語](CONTRIBUTING.ja.md)

This document covers how to change and extend zust (zustc and zust-analyzer). See [README.md](README.md) for usage
and [docs/architecture.md](docs/architecture.md) for how it works inside.

## Setup

In addition to the requirements in the README, the following tools are used:

- rustc: `tests/compare.sh` and `tests/probe.sh` build the same programs with rustc and compare the results.
- python3: the benchmark scripts (`bench/*.py`).
- valgrind (optional): `tests/compare.sh` also checks for memory errors and leaks.
- rust-analyzer (optional): `bench/lsp_parity.py` and `bench/ide_golden_ra.py` compare answers against it.
- cargo-mutants (optional): mutation testing.
- wild (optional): faster linking. The benchmark numbers are measured with wild.
- Node 24+ and a TypeSafe API key (optional): checking that English and Japanese documents agree (jev-lint).

```bash
cargo build --release --workspace
```

This builds `target/release/zustc` and `target/release/zust-analyzer`. Most tests and scripts use these two release
builds.

`cratebench/run.py` and `bench/deps_bench.py` use the sources of real crates from git submodules. Fetch them once
(or clone with `git clone --recurse-submodules`):

```bash
git submodule update --init cratebench/vendor
```

## Repository layout

| path | contents |
|---|---|
| `src/` | the `zustc` CLI (`src/main.rs`) and re-exports of the crates below |
| `crates/base` → `syntax` → `sema` → `typeck` → `codegen` → `core` | the compiler layers (each depends on the ones before it) |
| `crates/minclif` / `crates/clif` / `crates/ir` | our own x86-64 backend / the Cranelift backend / the instruction-stream types |
| `crates/ide` | editor analysis (symbols, definition, references, hover, completion). Not used by the compiler |
| `crates/analyzer` | the LSP server `zust-analyzer` |
| `examples/` | inputs compiled by zustc (not Cargo examples) |
| `tests/` | zustc tests (comparisons with rustc, caching, self-hosting, and more) |
| `benches/` / `bench/` | criterion benchmarks / benchmark and regression-monitoring scripts |
| `cratebench/` | a benchmark that builds small real-world crates |
| `docs/` | the language subset, the design, and work reports (English; Japanese versions in `docs/ja/`) |

What each crate contains is described in detail in [docs/architecture.md](docs/architecture.md).

## Tests

```bash
cargo test --release --workspace
```

This runs all the tests:

- unit tests (the lexer and others)
- diagnostics from check: it keeps going after an error, and it reports errors in generic function bodies even
  through cached callers
- caching and incremental builds
- panic locations and multiple crates
- the size budget (`tests/budget.rs`)
- zust-ide golden files
- zust-analyzer integration tests (they drive the real binary over stdio)

If you only changed the LSP side, this is enough:

```bash
cargo test --release -p zust-ide -p zust-analyzer
```

### Comparing results with rustc

```bash
tests/compare.sh
```

Builds `examples/*.rs` with both zustc and rustc and checks that the output and exit code match. It compares both a
build without a cache and a rebuild from the cache. If valgrind is available, memory errors and definite leaks also
fail the run.

```bash
tests/probe.sh
```

Compares small programs that use common features (`tests/probe/*.rs`) with rustc and prints how many pass. When you
add a language feature, add a probe here.

### Self-hosting

```bash
tests/selfhost.sh
```

Compiles parts of zustc's own source (lexer, prelude, link, macros, parse, type inference, and more) with zustc and
checks that the output is the same as the rustc-compiled version.

```bash
tests/selfhost_compiler.sh
```

Compiles the whole compiler with both zustc and rustc and checks that the two resulting compilers produce the same
`.o` files. It also compiles the compiler again with the zustc-built version and checks that the result is the same
(a fixed point).

### rustc's own test suite

`bench/rust_ui.py` runs zustc against rustc's UI tests (rust-lang/rust `tests/ui`, pinned to the commit of rustc
1.100.0). Fetch them once (a sparse clone of only `tests/ui`, about 170 MB, into `out/rust-tests`):

```bash
python3 bench/rust_ui.py --fetch
```

```bash
python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --check
```

Each test's kind comes from its header directives, as in compiletest. A run-pass test must build, exit 0 and print
its `.run.stdout`. A check-pass test must pass `zustc check`. A compile-fail test must be rejected with an error on a
line that one of its `//~ ERROR` annotations points at. Tests that need what zustc cannot be asked for (nightly
`#![feature]`, auxiliary crates, `compile-flags`, `revisions`, other targets) are counted as not applicable.

`bench/rust_ui_passing.txt` lists the tests zustc passes. CI fails if one of them no longer passes. When a change
makes more tests pass, rewrite the list:

```bash
python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --write-baseline
```

`--filter SUBSTR` runs only matching paths, `--kinds run-pass` one kind, and `out/rust_ui_results.txt` has the result
of every test.

### Golden files

`crates/ide/tests/golden/*.txt` record zust-ide's answers. When you change answers on purpose, rewrite them with:

```bash
UPDATE_GOLDEN=1 cargo test -p zust-ide --test golden
```

Before rewriting, make sure the diff is an improvement. To see whether the answers now agree with rust-analyzer, run:

```bash
python3 bench/ide_golden_ra.py --detail
```

## Benchmarks and regression monitoring

```bash
bench/regress.sh save
```

Before adding a feature, save the current state as the baseline `main`.

```bash
bench/regress.sh
```

Compares against the baseline. It fails if any of these happen:

- criterion (`benches/compile.rs`) reports a regression
- the size budget (`bench/budget.txt`) is exceeded
- output differs from rustc (`tests/compare.sh`)

### Rules for judging

- Changes below 5% are ignored. The significance level is 1%. The cold benchmarks vary by ±10%, so their threshold
  is 15%.
- Values depend on the state of the machine. To compare, measure at the same time as the baseline commit (benchmarking
  HEAD in a git worktree at the same time is reliable). To track down a cause, callgrind instruction counts are stable.
- The size budget holds an upper limit in bytes for each example's binary, `.o` and cache.
  - To increase it on purpose, use `UPDATE_BUDGET=1`.
  - If you only added new examples, use `UPDATE_BUDGET=new` (existing limits stay unchanged).

  Before loosening the budget, find out why the size grew (for example, more prelude items attached). Past cases are
  in [docs/reports/2026-11-15-17-regression-catches.md](docs/reports/2026-11-15-17-regression-catches.md).

### Other benchmarks

| script | what it measures |
|---|---|
| `python3 bench/vs_rustc.py [NAME..]` | comparison with rustc (cold, no-op and one-edit build times, disk, binary size, run time) |
| `python3 bench/runtime.py` | run time and binary size only (a lighter version) |
| `python3 bench/run.py 1000 5000` | cold / no-op / one-function edit on generated programs with N functions |
| `python3 bench/ws_bench.py --rustc` | a multi-crate workspace |
| `python3 bench/deps_bench.py` | multiple crates with many dependencies |
| `python3 bench/lsp_compare.py --cold` | LSP server speed (against rust-analyzer / rust-glancer) |
| `python3 bench/lsp_parity.py` | compares LSP answers with rust-analyzer |

`lsp_parity.py` can compare against the previous numbers and show the difference:

```bash
python3 bench/lsp_parity.py --compare bench/lsp_parity.json --json bench/lsp_parity.json --out bench/lsp_parity_result.md
```

Add `--detail` to print the places where the answers differ.

### Mutation testing

```bash
bench/mutants.sh diff
```

Mutates only the lines changed since origin/main and checks whether the tests catch the mutants. You can also narrow
it with `bench/mutants.sh pkg zust-ide` (a crate) or `bench/mutants.sh file crates/ide/src/symbols.rs` (a file).
Mutants the tests miss are written to `mutants.out/missed.txt`; add tests to kill them.

## CI and the benchmark dashboard

- `.github/workflows/ci.yml` runs a release build, `cargo test --release --workspace`, `tests/compare.sh` and the
  tests/ui baseline check on every push and pull request. The size budget depends on the linker, so CI installs wild 0.11.0 and measures with
  `ZUSTC_BENCH_LINKER=wild` (`bench/budget.txt` also holds wild sizes).
- `.github/workflows/bench.yml` runs `bench/ci_bench.py` on every push to main and stores the results in `dev/bench/`
  on the gh-pages branch with
  [github-action-benchmark](https://github.com/benchmark-action/github-action-benchmark). The dashboard is at
  https://mizchi.github.io/zust/dev/bench/ (publish the gh-pages branch in the repository's Settings → Pages).
  It tracks:
  - zustc: build (cold / no-op / one-function edit) and check times and instruction counts for programs with 1000 and
    5000 functions, peak RSS, output and cache size, and run time of `bench/rt`
  - zust-analyzer: p50 of each query in `bench/lsp_parity.py` (on zustc's own sources), time from startup until
    indexed, and RSS
  - the share of answers that agree with rust-analyzer
  - the pass rates on rustc's tests/ui
  - Anything that got worse by 30% or more since the previous run (5% for the agreement ratios) produces a warning in
    the workflow result. It does not fail the run.

To measure the same things locally:

```bash
python3 bench/ci_bench.py --out-dir out/ci
```

## English and Japanese documents

Each English document has a Japanese translation: `README.md` / `README.ja.md`, `CONTRIBUTING.md` /
`CONTRIBUTING.ja.md`, and `docs/X.md` / `docs/ja/X.md` (reports included). When you change one, change the other
too. [jev-lint](https://github.com/mizchi/jev-lint) `docs` asks a model whether each pair still says the same thing.

- `scripts/docs_manifest.py` writes the jev-lint manifests (`.jev-lint/documents*.json`) from the pairs;
  `--check` fails if a file has no counterpart or a manifest is out of date.
- `scripts/docs_check.sh` replays the recorded verdicts (`.jev-lint/*-baseline.json`) against the current documents.
  It is offline and needs no API key; CI runs it. It fails if a pair contradicts, or if a document changed after its
  verdict was recorded.
- `scripts/docs_check.sh --update` asks the model again for the manifests whose documents changed and rewrites the
  records. It needs `TYPESAFE_API_KEY` (or `TYPESAFEAI_API_KEY`) from https://typesafe.ai; the document text is
  sent to that API. A full run of all 64 pairs costs about $0.03.

Enable the pre-commit hook once per clone:

```bash
git config core.hooksPath .githooks
```

When a commit touches Markdown, the hook runs `scripts/docs_check.sh --update`, adds the updated records to the
commit, and stops the commit if a pair contradicts. A verdict is a model's judgment: read the reported pair and fix
whichever side is wrong (or, if the finding is wrong, commit with `--no-verify`).

## Debugging environment variables

| variable | meaning |
|---|---|
| `ZUSTC_IR_VERIFY=1` | check that the value types of the rebuilt instructions match the type checker's record |
| `ZUSTC_CLIF_VERIFY=1` / `ZUSTC_CLIF_DUMP=1` | verify / dump the Cranelift IR |
| `ZUSTC_PREP_VERIFY=1` | compare the result of incremental preprocessing with a full rerun |
| `ZUSTC_DUMP_PRE=FILE` | write the source after preprocessing (`macro_rules!` expansion, merging `mod x;` files) to FILE |
| `ZUSTC_LOG_COMPILED=1` | print the names of compiled functions (and why, for those that could not be read from the cache) |
| `ZUSTC_PROF=1` | print detailed time per phase |
| `ZUSTC_NO_STRIP=1` | keep symbols in the output (to see function names in callgrind and similar tools) |
| `ZUSTC_BENCH_LINKER` | pin the linker used by the criterion benchmarks |

The other `ZUSTC_NO_*` variables turn off one optimization or incremental mechanism at a time, for comparison. Search
the code for the name to see what each one does.

## How to make changes

- When adding a language feature, first add a program to `examples/` or `tests/probe/` that compares with rustc.
- After a change, pass `cargo test --release --workspace`. If you changed the compiler, also pass `tests/compare.sh`.
  If the change affects speed or size, also pass `bench/regress.sh`.
- If you changed LSP answers, check with `bench/lsp_parity.py` that agreement and times did not get worse.
- After a substantial piece of work, write a report (next section).
- Record differences from rust-analyzer and anything left undone in [TODO.md](TODO.md).

### Writing reports

Work reports go in `docs/reports/YYYY-MM-DD-NN-<slug>.md`, in English. If you also write a Japanese version, put it
in `docs/ja/reports/` under the same name.

- `YYYY-MM-DD` is the day of the work.
- `NN` is the order within that day (starting at 01).
- `<slug>` is a short kebab-case description.

Sorting by file name gives date order. Use this form:

```markdown
# Title

Date: YYYY-MM-DD

What was done, why, how it was measured, the results (numbers), what was learned, what is left
```

Keep the numbers as they were when measured (do not rewrite them later). After writing a report, add a line to the
list in [docs/reports/README.md](docs/reports/README.md) (and to
[docs/ja/reports/README.md](docs/ja/reports/README.md) if you wrote a Japanese version).

### Commits

- Keep one coherent change per commit.
- In the message, name the area (`ide:` / `analyzer:` / `docs:` and so on) and say what changed.
- Use the names zustc / zust-analyzer / zust-*.

## License

Unless stated otherwise, contributions are provided under MIT OR Apache-2.0, as described in the README's "License"
section. When bringing in code from other projects, only use code with a compatible license, and either add the
upstream repository as a git submodule (as `cratebench/vendor/` does) or include the original license files.
