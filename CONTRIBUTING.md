# Contributing

English | [日本語](CONTRIBUTING.ja.md)

This document covers how to change and extend zust (zustc and zust-analyzer). See [README.md](README.md) for usage
and [docs/architecture.md](docs/architecture.md) for how it works inside.

## Getting started

Steps for a fresh clone, in order. After them, every command in this document works.

1. Install the tools (Linux x86-64):
   - a Rust toolchain with edition 2024 (rustup stable), with rust-src (`rustup component add rust-src`) for
     zust-analyzer's std definitions
   - `cc` (it drives the link line) and a linker. zustc picks wild > mold > ld.lld > ld. The benchmark numbers and the
     CI size budget are measured with wild 0.11.0 (`cargo install --locked wild-linker --version 0.11.0`, or a release
     binary)
   - python3 (the scripts under `bench/` and `scripts/`)
   - valgrind: `tests/compare.sh` checks memory errors and leaks with it, and `bench/kpi_track.py` counts instructions
     with cachegrind
   - optional:
     - rust-analyzer (`bench/lsp_parity.py`, `bench/ide_golden_ra.py`)
     - cargo-mutants (mutation testing)
     - cargo-nextest (runs the tests faster, see "Tests"; CI uses it)
     - blaze (`bench/vs_blaze.py`)
     - Node 24+ and a TypeSafe API key (jev-lint, see "English and Japanese documents")
2. Clone with the submodules. `cratebench/` and `bench/deps_bench.py` build real crates from them:

   ```bash
   git clone --recurse-submodules https://github.com/mizchi/zust.git
   ```

   In an existing clone, run `git submodule update --init cratebench/vendor`.
3. Build the two release binaries, `target/release/zustc` and `target/release/zust-analyzer`. Most tests and scripts
   use these:

   ```bash
   cargo build --release --workspace
   ```

4. Fetch rustc's UI tests. This is a sparse clone of `tests/ui` and `tests/auxiliary` only, about 170 MB, into
   `out/rust-tests`:

   ```bash
   python3 bench/rust_ui.py --fetch
   ```

   A checkout fetched before `tests/auxiliary` was added needs it added once. The C helpers that compiletest links
   into every test (`rust_test_helpers.c`) are in it:

   ```bash
   git -C out/rust-tests sparse-checkout add tests/auxiliary
   ```

5. Enable the pre-commit hook. It keeps the English / Japanese documents in agreement:

   ```bash
   git config core.hooksPath .githooks
   ```

6. Check that everything passes before changing anything:

   ```bash
   cargo test --release --workspace
   ```

   ```bash
   tests/compare.sh
   ```

   ```bash
   python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --check
   ```

7. Take the KPIs once without writing them, to see this machine's numbers. Instruction counts do not depend on the
   machine's load, so they can be compared with the log:

   ```bash
   python3 bench/kpi_track.py "baseline" --dry-run
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

This runs all the tests. With [cargo-nextest](https://nexte.st/) the same tests run in about two thirds of the time
(each test in its own process, the test binaries side by side; 71s -> 50s on 4 cores). It does not run doc tests,
so CI runs those with `cargo test --doc`:

```bash
cargo nextest run --release --workspace
```

`.config/nextest.toml` holds its settings (the `ci` profile: no fail-fast, a hung test stopped after 5 minutes). A
test that builds and runs many programs spreads them over the cores itself (`tests/common/par.rs`), so `cargo test`
is not slow either.

The tests cover:

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
1.100.0). Fetch them once (a sparse clone of only `tests/ui` and `tests/auxiliary`, about 170 MB, into `out/rust-tests`):

```bash
python3 bench/rust_ui.py --fetch
```

```bash
python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --check
```

Each test's kind comes from its header directives, as in compiletest. A run-pass test must build, exit 0 and print
its `.run.stdout`. A check-pass test must pass `zustc check`. A compile-fail test must be rejected with an error on a
line that one of its `//~ ERROR` annotations points at. Tests that need what zustc cannot be asked for (nightly
`#![feature]`, auxiliary crates, `compile-flags` other than `--test` and `--cfg`, `revisions`, other targets) are counted as not applicable.

`bench/rust_ui_passing.txt` lists the tests zustc passes. CI fails if one of them no longer passes. When a change
makes more tests pass, rewrite the list:

```bash
python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --write-baseline
```

`bench/rust_ui_ratchet.txt` is a ratchet on the counts: the least number of tests of each kind zustc must pass. CI
fails below it, and `--write-baseline` raises it to the run's counts, never lowers it. Passes in
`bench/rust_ui_accidental.txt` are not counted.

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

## KPIs and open work

Every change that touches the compiler or the analyzer is measured before it is kept.

- `python3 bench/kpi_track.py "short label"` counts instructions (cachegrind) for:
  - a no-op build, a one-function edit and `check` of a generated program with 5000 functions;
  - a trait- and inference-heavy program (`bench/gen_traits.py`);
  - the cratebench crates;
  - the shapes of application code (`bench/gen_patterns.py mix`): an interpreter, record processing, error handling
    with `?`, state machines, trait-object plugins, zero-copy parsing with lifetimes, `macro_rules!`, nested modules
    and generic containers;
  - with `--lsp`, a zust-analyzer session.

  `python3 bench/patterns.py` measures each of those patterns alone (a cold build, `check`, an edit build; with
  `--instructions`, counted) and checks that zustc's build prints what rustc's does. A new pattern goes in
  `bench/gen_patterns.py`; `tests/bench_patterns.rs` builds every one of them.

  `bench/kpi_track.py` appends a row to `bench/kpi_log.tsv` / `bench/kpi_log.md` (`bench/patterns.py` only prints).
  A value outside the noise band of the last 7 rows is printed as `REGRESSION`: find the cause before keeping the
  change, or state the cost in the commit message.
  In a pull request, use `--dry-run` and do not commit rows to the log: the `KPI` workflow measures the pull request
  against its base and keeps the result in a comment on it (see CI below). Rows go in with commits made on main;
  rows added in a branch would conflict with every row main gets in the meantime.
  `bench/kpi_ratchet.txt` holds a ceiling for each KPI: a pull request whose head is above one by more than its
  tolerance fails the `KPI` workflow. Take the cost out, or raise the ceiling in the pull request (the KPI comment
  gives the line) and say in its description why the cost is worth it. Every push to main lowers the ceilings that
  main measures below; nothing but such an edit raises one.
- `bench/rust_ui.py --check` must report 0 regressions. When tests start passing, rewrite the baseline
  (`--write-baseline`).
  - Look at what a newly passing compile-fail test is rejected for: one rejected at its line for another reason than
    the one it tests goes into `bench/rust_ui_accidental.txt`, which keeps it out of the baseline.
- Real crates: `python3 cratebench/run.py` builds each crate and an app with zustc and rustc and compares the output.
- The large blockers (lifetimes, trait checks, unwinding, inference through methods, unsupported types, the size of
  the prelude, wasm output, threads and async) are tracked as [issues](https://github.com/mizchi/zust/issues).
  Each one says where to start, how to measure it, and what it must not cost.

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

- `.github/workflows/ci.yml` runs a release build, the tests (`cargo nextest run --release --workspace --profile ci`
  and the doc tests), `tests/compare.sh` and the tests/ui baseline check on every push and pull request. Its test job
  builds without the crate-local ThinLTO (`CARGO_PROFILE_RELEASE_LTO=off`: the workspace builds in ~2/3 of the time)
  and installs valgrind from cached packages while it builds. The size budget depends on the linker, so CI installs wild 0.11.0 and measures with
  `ZUSTC_BENCH_LINKER=wild` (`bench/budget.txt` also holds wild sizes).
- `.github/workflows/kpi.yml` runs on every push to a pull request. It measures the head and the merge base with
  `bench/kpi_track.py --dry-run --only <sections>` (a zust-analyzer session included), in three parts on runners side
  by side, the base's beside the head's: gen5000; traits, crates and patterns; the LSP session. The base's KPIs are
  kept by the environment they are measured in (toolchain, valgrind, OS, C library), which every part checks it has.
  Only the job that comments may write to the pull request. An instruction count is the same on any runner of the
  image. The base's numbers are cached, so a base measured before is not measured again.
  `bench/kpi_pr.py` then keeps one comment on the pull request up to date: the change of each KPI against
  the base, and one row per push with the change against the push before. A KPI that grows by more than 1% is marked and gives a warning.
  A head above a ceiling of `bench/kpi_ratchet.txt` by more than its tolerance fails the job (`bench/kpi_ratchet.py`),
  and on every push to main the `ratchet` job lowers the ceilings that main measures below, in a commit of its own that
  skips CI. The ceilings hold in the environment they were measured in: in another one nothing is checked, and the
  next push to main measures them anew.
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

`README.md` (the overview, how to use zustc and zust-analyzer, and a summary of the measured numbers),
`SUPPORT.md` (what is supported, by area) and `PERFORMANCE.md` (the KPIs and the comparisons with other tools) are in
English only; the roadmap is in this document. The measured numbers in those three files (between `<!-- auto:.. -->`
markers, and after `<!-- ui:.. -->` and `<!-- cf:.. -->` markers) are rewritten by CI (`scripts/update_readme.py`, run
by bench.yml after each push to main); do not edit them by hand. `CONTRIBUTING.md` / `CONTRIBUTING.ja.md` and `docs/X.md` / `docs/ja/X.md` (reports
included) are English / Japanese pairs. When you change one, change the other
too. [jev-lint](https://github.com/mizchi/jev-lint) `docs` asks a model whether each pair still says the same thing.

- `scripts/docs_manifest.py` writes the jev-lint manifests (`.jev-lint/documents*.json`) from the pairs;
  `--check` fails if a file has no counterpart or a manifest is out of date.
- `scripts/docs_check.sh` reads the recorded verdicts for the current documents from the ledger
  (`.jev-lint/judgments/`, jev-lint's judgment ledger: append-only files that one branch merges with another as their
  union, and whose answers are keyed by the text compared, the question and the model). It is offline and needs no API
  key; CI runs it. It fails if a pair contradicts. A pair whose documents changed since its verdict was recorded has
  no answer in the ledger and cannot be judged without an API key: with no `TYPESAFE_API_KEY` its manifest is skipped
  with a warning (`DOCS_CHECK_STRICT=1` makes it fail instead, exit 3). The pairs and the manifests are checked
  either way.
- `scripts/docs_check.sh --update` asks the model about the pairs the ledger has no answer for, and adds the answers
  to the ledger. It needs `TYPESAFE_API_KEY` (or `TYPESAFEAI_API_KEY`) from https://typesafe.ai; the document text is
  sent to that API. A full run of all 64 pairs costs about $0.03.

Enable the pre-commit hook once per clone:

```bash
git config core.hooksPath .githooks
```

When a commit touches Markdown, the hook runs `scripts/docs_check.sh --update`, adds the new answers to the
commit, and stops the commit if a pair contradicts. A verdict is a model's judgment: read the reported pair and fix
whichever side is wrong (or, if the finding is wrong, commit with `--no-verify`).

## Debugging environment variables

| variable | meaning |
|---|---|
| `ZUSTC_IR_VERIFY=1` | check that the value types of the rebuilt instructions match the type checker's record |
| `ZUSTC_CLIF_VERIFY=1` / `ZUSTC_CLIF_DUMP=1` | verify / dump the Cranelift IR |
| `ZUSTC_PREP_VERIFY=1` | compare the result of incremental preprocessing with a full rerun |
| `ZUSTC_CHECK_FORK_MIN=N` | `check` forks children for a wave of functions only when it has at least N per child (default 32); `1` forks for nearly every wave, to test that the diagnostics are as at `-j 1` |
| `ZUSTC_BUILD_PAR_MIN=N` | a build compiles a wave's functions in forked children only when there are at least N per child (default 16); `1` splits nearly every wave, to test that the .o is as at `-j 1` |
| `ZUSTC_NO_POLONIUS=1` | the borrow check's NLL rules where Polonius Alpha's differ (#1): a reborrow returned is live to the end of the function, and an assignment to a part of a local kills no loans |
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
- Record differences from rust-analyzer and anything left undone in the [Roadmap](#roadmap) below (or an issue), and
  what is supported in [SUPPORT.md](SUPPORT.md) (a new area with a percentage gets a
  `<!-- ui:DIR -->` marker naming its tests/ui directories).

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

## Roadmap

### Compiler (zustc)

- **Security** ([TODO.md](TODO.md); [report](docs/reports/2026-11-21-01-security-cves.md)): the holes the security tests
  found and left (the `//@ todo` cases of `tests/security/`), and what is left of the wasm32 target
- **rustc's tests/ui** (`python3 bench/rust_ui.py`; [report](docs/reports/2026-11-18-07-rustc-ui-tests.md))
  - Raise the pass rates ([the README's Status](README.md#status), by area in [SUPPORT.md](SUPPORT.md))
  - Largest gaps: unwinding (`catch_unwind`, `panic::set_hook`), structs by value through FFI, type inference (type
    parameters known only from the expected type), `type` aliases and macros in expression position inside bodies,
    closures in consts, `impl Trait` in argument types nested in other types (`&Vec<impl Display>`)
  - Checks that compile-fail tests need: well-formedness of defaults and bounds, coherence, bounds of blanket
    impls, Send through dyn / async, lifetime shadowing, lints
- **Borrow checking** ([report](docs/reports/2026-11-18-08-borrow-checking.md))
  - Region errors against signatures (E0621, E0521, "lifetime may not live long enough"): universal regions and
    outlives checking of the body against the declaration
  - Loans flowing through types: a local holds the loans of all it read, so E0515 and E0597 for values carried out
    of a block only look at what is borrowed directly
  - E0716 (temporaries dropped while borrowed), E0510, uses by `Drop` at the end of a scope, partial moves out of
    arrays and struct patterns
  - More than one error per function
  - Cost: +2.2% instructions on `zustc check`, mostly the recording during lowering
- **Language**
  - Modules, `DoubleEndedIterator` (`rev` as a value), `HashSet` / `BTreeMap`
  - Share monomorphized instances keyed by layout (make `Vec<A>` and `Vec<B>` one; blaze's polymorph)
  - A minimal std of our own (the relax approach)
- **Incremental**
  - Alpha-normalized hashes
  - Reuse the analysis results of dependency crates (item summaries, parts of Program) on the dependent side (today
    the sources are analyzed again)
  - Linking that replaces only the `.o` files that changed
  - Embedding libwild and keeping `.o` files in memory (no `.o` writes and no `exec`)
- **Incremental linking**
  - Leave padding in `.text` and replace only the functions that changed
  - Calls through a jump table (the blaze approach)
- **Cold builds**
  - Parallelize item summaries and parsing
  - Stop parsing the AST twice in cold builds
- **Size**
  - ICF in the compiler (hash of code + relocation targets)
  - Merge sections for strings
- **`-O`**
  - Our own inlining and stack-slot forwarding (to make up for Cranelift having no SROA)

### zust-analyzer: remaining differences from rust-analyzer

What is left after comparing with `bench/lsp_parity.py`, `bench/ide_golden_ra.py` and checking references one by one
([docs/reports/2026-11-18-06-remaining-differences.md](docs/reports/2026-11-18-06-remaining-differences.md)).

#### Type inference

- [ ] The `Item` of iterators whose `Iterator` impl is generated by a macro inside a dependency (serde_json's
  `map::Iter`, from `delegate_iterator!`). zust-analyzer now answers nothing there instead of a wrong type
- [ ] An `Iter` type with no type arguments returned from a dependency is told apart from same-named types only by its
  module; generic types are still matched by name and number of arguments

#### Definitions and references

- [ ] Go to definition on `x.to_string()`: rust-analyzer jumps to the type's `Display::fmt` impl; zust-analyzer goes
  to `ToString::to_string`
- [ ] In the references of `ToString::to_string`, rust-analyzer lists the blanket impl's method (string.rs:2969) instead
  of the trait declaration (string.rs:2956)
- [ ] References of std modules (`std::collections`, `std::sync`): rust-analyzer also counts the paths inside std's
  own sources (`self::hash_map` in `collections/mod.rs`). zust-analyzer does not keep library text, so it only counts
  the workspace (fixture: 627/629 references)
- [ ] rust-analyzer counts `-> Key` in `crates/syntax/build.rs` (the build script's own `type Key = u128`) as a
  reference of `zust_base::key::Key`. This looks like a rust-analyzer quirk from `lexer.rs` being shared into the
  build script with `#[path]`; zust-analyzer does not count it
- [ ] Definitions of items generated by a macro (the str pattern iterators' `next`, `u32::MAX`): rust-analyzer also
  lists the macro call site (`generate_pattern_iterators! {`, `uint_impl! {`); zust-analyzer lists only the item inside
  the macro body

#### Rename

- [ ] Renaming `Default::default` (rust-analyzer also rewrites inside dependency crates; we only rewrite the workspace.
  An intended difference)
- [ ] Renaming the alias in `use a::Point as Pt` (an intended difference)

#### Speed and memory

- [ ] References p90 on the zustc target is 32.8 ms (it was 65 ms before scanning files on several threads). The slow
  cases are methods with common names (`len`, `iter`), resolved at every occurrence
- [ ] The dependency-crate index raised memory from 55 MB to 66 MB. Drop unused definitions / load them lazily

## License

Unless stated otherwise, contributions are provided under MIT OR Apache-2.0, as described in the README's "License"
section. When bringing in code from other projects, only use code with a compatible license, and either add the
upstream repository as a git submodule (as `cratebench/vendor/` does) or include the original license files.
