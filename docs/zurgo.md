# zurgo: Cargo packages with zustc

`zurgo` builds a Cargo package with zustc, as `cargo build` builds it with rustc. It reads Cargo.toml and Cargo.lock,
finds the sources, turns on features as cargo does, and gives the graph to zustc's workspace build in the same
process. `zurgo metadata` writes the JSON of `cargo metadata`. The plan and what is left: mizchi/zust#28.

```bash
cargo install --path crates/zurgo
```

```bash
zurgo run
```

## Commands

| command | what it does |
|---|---|
| `zurgo build` | builds the package of the manifest (every member, for a virtual workspace) and its dependencies |
| `zurgo run [-- args]` | builds, then runs the binary (`--bin` chooses one when there are several) |
| `zurgo test [filter] [-- args]` | builds the tests, then runs each one in a process of its own, all at once (below) |
| `zurgo metadata` | the package graph as `cargo metadata --format-version 1` writes it |
| `zurgo clean` | removes `target/zust` |

Options:
- `--manifest-path <Cargo.toml>`: the package. Without it, zurgo uses the nearest Cargo.toml from the current
  directory up.
- `-r` / `--release`: zustc's `-O`.
- `-F` / `--features a,b`, `--all-features`, `--no-default-features`. `dep/feat` names a dependency's feature.
- `--target <triple>`: `x86_64-unknown-linux-gnu` (the default) or `wasm32-unknown-unknown`.
  - For wasm32, each bin is one wasm module, `target/zust/wasm32-unknown-unknown/<profile>/<bin>.wasm`.
  - `zurgo run` runs it as cargo does: with the program in `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER`, split on
    white space, then the module and the arguments. In this repository the runner is `node tests/wasm/run.mjs`.
  - Build scripts are still built for the host and run on it.
- `--bin <name>`, `-j <n>`, `--time` (zustc's timings), `--no-cache`.
- `--build-script-compiler auto|zustc|rustc`: what compiles build scripts (below). `auto`, the default, tries zustc
  and falls back to rustc.
- `-v`: prints the workspace zurgo gives zustc, in the format of `zustc build`. It is also written to
  `target/zust/<profile>/zurgo-ws.txt`, so `zustc build` can repeat the build. A bin whose crate name differs from its
  executable's (a package's lib and bin share the package's name, so the bin's crate is `<name>-bin`) carries
  `exe=<name>`. Each crate's cfgs from its build script and its environment for `env!` (`CARGO_PKG_*`, `OUT_DIR`)
  are there too (`cfg=`, `env=`).
- `--filter-platform <triple>` (metadata).
- `--locked`, `--offline`, `--frozen` are accepted. zurgo always uses Cargo.lock as it is and never downloads.

Output goes to `target/zust/[<triple>/]<profile>/` (`CARGO_TARGET_DIR` moves `target`), apart from cargo's own
output. A binary is `target/zust/debug/<name>`.

## What it reads

- **Manifests.**
  - `[package]`, with the fields a workspace member inherits (`version.workspace = true` ...).
  - `[lib]` and `[[bin]]`, and the targets Cargo finds by itself: `src/lib.rs` (unless `autolib = false`),
    `src/main.rs`, `src/bin/*.rs`, `src/bin/*/main.rs` (unless `autobins = false`).
  - A bin's `required-features`: a default build leaves the bin out while they are off, and asking for it by name
    says which are missing. `default-run` chooses the bin of `zurgo run`.
  - Dependencies of the three kinds, also under `[target.'cfg(..)']`, with renames, `optional`,
    `default-features` and `features`.
  - `[features]`, and `[workspace]`:
    - members with `*` globs, each directory once;
    - path dependencies inside the workspace's directory, which are members without being listed, as in cargo;
    - `exclude`, `default-members`, `[workspace.dependencies]`;
    - a member's inherited fields also when its own Cargo.toml is given (`--manifest-path`);
    - which packages a command works on: a member's own manifest selects that member; the workspace's root
      manifest selects `default-members` (globs too), else the root package, else every member. `--features` and
      `default-run` apply to that selection.
  - The TOML parser is zurgo's own (`crates/zurgo/src/toml.rs`).
- **Cargo.lock** (versions 1 to 4). zurgo has no resolver yet, so the versions are the lock's. Without a lock, zurgo
  says to run `cargo generate-lockfile`.
- **Sources.**
  - Workspace members and path dependencies.
  - crates.io packages in `$CARGO_HOME/registry/src`, where `cargo fetch` puts them.
  - A package that is not downloaded (one for another platform) is read from cargo's index cache
    (`registry/index/*/.cache`): its dependencies and features, enough for `metadata`. Building it is an error that
    says to run `cargo fetch`.

## Features, as cargo turns them on

- **`zurgo build`** uses the rules of cargo's feature resolver (resolver 2 and 3). It is used for builds:
  - a package built for the host (a build dependency, a proc macro and what they use) has its own feature set;
  - dev-dependencies are left out;
  - a `[target.'cfg(..)']` dependency is checked against the side it is on;
  - a weak `dep?/feat` waits until something else turns `dep` on.
- **`zurgo metadata`** gives what `cargo metadata` reports, which comes from cargo's dependency resolver:
  - one feature set per package, over every kind and every platform;
  - a weak `dep?/feat` also pulls `dep` into the graph;
  - `--filter-platform` only chooses which edges are shown. An edge is shown if one of its declarations applies,
    and its `dep_kinds` list all its declarations.
- **In both:**
  - `x/f` turns on dependency `x`, and, when `x` is optional, the feature named `x` if there is one;
  - an optional dependency that no feature names with `dep:` has an implicit feature of its name.

`crates/zurgo/tests/zurgo.rs` checks that `zurgo metadata` and `cargo metadata` give the same graph for the test
packages and for cratebench/ui's dioxus and leptos apps (344 and 180 packages; also with `--filter-platform` for
wasm32 and x86-64 Linux). For dioxus, zurgo takes 39 ms and cargo 320 ms.

## Build scripts

A package's `build.rs` runs before its crates are built, as under cargo:

- **Compiling it.** It is a binary for the host, with the package's build-dependencies.
  - `auto` (the default): zustc builds the build-dependencies and the script as one zustc workspace. When zustc
    cannot (a std function it lacks, a proc-macro build-dependency), rustc builds them instead: the build-dependencies
    as rlibs (a proc macro as a dylib), then the script.
  - `-v` says which compiler built a script, and why zustc could not. zurgo remembers the choice until the sources
    change.
- **Running it.** It runs in the package's directory, with the variables cargo sets:
  - `OUT_DIR`, `TARGET`, `HOST`, `NUM_JOBS`, `OPT_LEVEL`, `DEBUG`, `PROFILE`;
  - `CARGO_MANIFEST_DIR`, `CARGO_MANIFEST_PATH`, `CARGO_MANIFEST_LINKS`, `CARGO_PKG_*`;
  - `CARGO_FEATURE_<NAME>` and `CARGO_CFG_<KEY>` (rustc's values for the target);
  - `RUSTC`, `RUSTDOC`, `CARGO`, `CARGO_ENCODED_RUSTFLAGS`;
  - `DEP_<LINKS>_<KEY>` from the `links` packages it depends on.

  `RUSTC` is the real rustc, so a script's probes and version checks see what they see under cargo.
- **Its output**, in the `cargo:` and `cargo::` forms:
  - `rustc-cfg` and `rustc-env` go to the package's crates;
  - `warning` is printed, and `error` stops the build;
  - `metadata` (and, in the old form, any other key) goes to the dependents' scripts as `DEP_<LINKS>_<KEY>`;
  - link instructions (`rustc-link-lib`, `rustc-link-search`, `rustc-link-arg`, `rustc-flags`) are kept, but zustc
    does not apply them yet: a warning says so.
- **When it runs again.** Its output is kept in `target/zust/<profile>/build/<package>-<hash>/run`. It runs again when
  the script changes, when cargo's variables change, or when a file or variable that `rerun-if-changed` /
  `rerun-if-env-changed` names changes. A script that prints neither runs again when any file of its package
  changes, as cargo does.
  - A run is not kept when a file it depends on changed while it ran (also one removed: the directory's time says
    so), when a directory could not be read, or when a file it names does not exist (cargo runs such a script every
    time).
  - The old result is removed before a script runs again, so a run that fails leaves nothing to reuse.
  - Two zurgo processes that share a target directory take turns, as cargo does: a build holds a lock on it from
    its first build script to its last compiled crate, so the `OUT_DIR` a script filled is the one its crates read.
- **rustc's builds** of the host libraries and scripts are kept while the rustc (`rustc -vV`), the sources, the
  dependencies' artifacts and the files their build scripts wrote to `OUT_DIR` are the same.

Every crate also gets cargo's variables for `env!` / `option_env!`: `CARGO_PKG_*`, `CARGO_MANIFEST_DIR`,
`CARGO_CRATE_NAME`, and for a bin `CARGO_BIN_NAME`. `OUT_DIR` and the script's `rustc-env` come on top of those.

## Proc-macro dependencies

Proc macros cannot run yet (mizchi/zust#29). A proc-macro dependency is built as a stub library of its name, so a
crate that only re-exports it builds: `pub use thiserror_impl::*;`, or serde's `derive` feature.

zustc would drop an unknown attribute silently, so using a proc macro is an error before anything is built:
- a crate of the build that uses one of the stub's derives (`#[derive(thiserror::Error)]`), or one of its attribute
  macros (`#[wasm_bindgen]`, also renamed: `use wasm_bindgen::prelude::wasm_bindgen as wb;`), is an error naming #29.
  - Only the proc-macro crates a crate reaches count: its dependencies, or what its libraries re-export.
  - Each crate is read as zustc reads it: the files of its module tree, with its features and cfgs. So a use in a
    file it does not reach, under a cfg that is off, or in a `macro_rules!` body that is neither called nor exported
    does not count.
  - A derive of the language (`#[derive(Debug)]`) is a proc macro's only when the crate imports one of that name
    from outside std (`use derive_more::Debug;`).
  - The proc-macro crate's root is read with its features, so `#[cfg_attr(feature = "x", proc_macro_attribute)]`
    counts when the feature is on;
- a function-like macro (`html!(..)`) expands to a `compile_error!` naming #29.

## Tests

`zurgo test` works on nextest's model, not libtest's (mizchi/zust#96). It builds the selected packages' test crates:
the lib's unit tests, each bin's, and each integration test (`[[test]]`, `tests/*.rs`, `tests/*/main.rs`), with the
dev-dependencies. It lists the tests of every executable, then runs each test in a process of its own, over the cores
at once.

```bash
zurgo test
```

```bash
zurgo test --lib parse -- --include-ignored
```

- Each test crate (but a `harness = false` one, below) is built with `zustc --test`. That keeps `#[cfg(test)]` items, collects the `#[test]` functions (those a `macro_rules!` makes too: they are
  collected from the expanded source)
  and adds a harness `main` (the crate's own `main` is not called). The harness only lists its tests and runs the
  one named:
  - a panic exits with 101, and a `Result` test that returns `Err` exits with 1;
  - a `#[should_panic]` test runs in a child process, whose exit code and stderr tell whether it panicked with the
    `expected` text.

  So the compiled program needs no `catch_unwind`, threads or output capture. The command line is libtest's
  (`--list --format terse`, `--exact`, `--ignored`, `--include-ignored`, `--skip`, filters), so cargo-nextest can
  run these executables too. Run directly with no arguments, an executable runs its tests one after another, each in
  a child, and prints libtest's report.
- A test passes when its process exits with 0. The runner speaks libtest's command line, so it runs the executables
  `cargo test --no-run` builds as well. The tests compare the two runs (`crates/zurgo/tests/zurgo.rs`).
- Tests run in the package's directory, as under cargo. An integration test finds the package's binaries in
  `CARGO_BIN_EXE_<name>`.
- Output: one line per test as it finishes (`PASS [ 0.012s] (3/9) pkg (lib) tests::adds`, or `FAIL`, `TIMEOUT`,
  `SIGNAL`). A failure is followed by its stdout and stderr. A summary comes last. The exit code is 101 when a test
  failed.
- The longest tests start first, ordered by their last times in `target/zust/<profile>/test-times`. A long test that
  starts last would run alone at the end.
- Options:
  - `--lib`, `--bin <name>`, `--bins`, `--tests` and `--test <name>` choose the targets: only those are built (the
    bins too, for `CARGO_BIN_EXE_`);
  - `--no-run` builds them and lists the executables;
  - `--fail-fast` stops starting tests after a failure (by default every test runs, and `--no-fail-fast` is
    accepted).
- After `--`: `<filter>`, `--exact`, `--skip <s>`, `--ignored`, `--include-ignored`, `--list` and
  `--test-threads <n>` (the cores by default). `--nocapture` and `--show-output` are accepted: a failure's output is
  always shown.
- A test still running after 60s is marked `SLOW`, and one running after 300s is stopped and fails
  (`ZURGO_TEST_SLOW`, `ZURGO_TEST_TIMEOUT`, in seconds).
- `test = false` on a target leaves its tests out, unless the target is named (`--lib`, `--bin x`, `--test x`), as under
  cargo. A `harness = false` target is built without the harness (its own `main`, with `cfg(test)`, as cargo builds
  it) and run as one test, named after the target, that passes when it exits with 0. `zurgo metadata` lists
  the integration tests as cargo does.
- `--target wasm32-unknown-unknown`: each test crate is a wasm module, and each test runs as
  `$CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER <module> <name> --exact` (in this repository the runner is
  `node tests/wasm/run.mjs`). A module starts no child process, so a `#[should_panic]` test runs in its module and the
  runner checks its exit code (101) and panic message. An arithmetic check traps there without a message, so
  `expected` cannot match it.
- Not yet: doc tests and benchmarks.

## Not yet (mizchi/zust#28)

- **Two versions of one library**, and **renamed dependencies**, are errors: zustc's workspace has one name per
  crate.
- **Other sources:** git dependencies, other registries, source replacement.
- **No `[patch]`, no resolver of its own, no downloads, no `doc`.**
- **Editions:** zustc has no edition setting: it accepts what any edition accepts, so a crate's edition is not
  passed on. (rustc, building build scripts, gets each crate's own.)
