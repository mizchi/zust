# zust

[![CI](https://github.com/mizchi/zust/actions/workflows/ci.yml/badge.svg)](https://github.com/mizchi/zust/actions/workflows/ci.yml)
[![Benchmark](https://github.com/mizchi/zust/actions/workflows/bench.yml/badge.svg)](https://mizchi.github.io/zust/dev/bench/)

**zustc** is a compiler for a subset of Rust built for fast incremental builds. **zust-analyzer** is a lightweight LSP server.

- **zustc** has its own front end: lexer, parser and type inference. It generates code with Cranelift (the default) or
  minclif, its own small x86-64 backend. It is tuned for the edit-one-thing-and-rebuild loop of development, and tries
  to minimize:
  - rebuild time
  - peak memory
  - disk used by the cache
  - output binary size

  zustc can compile itself.
- **zust-analyzer** is an LSP server for editors. It answers definition, references, hover, completion, rename and more
  with a light analysis over the token stream and a table of definitions. It works on ordinary Rust workspaces, and
  also indexes std and dependency crates.

> **Status**: experimental.
>
> - zustc targets Linux x86-64 and does not support all of Rust ([supported language](docs/language.md)).
>   `--target wasm32-unknown-unknown` writes a wasm module (experimental: [tests/wasm/README.md](tests/wasm/README.md)).
> - Security regression tests from past Rust CVEs and soundness bugs: [tests/security/README.md](tests/security/README.md).
>   What zustc does not catch yet, and what else is left: [TODO.md](TODO.md).
> - zustc's checks (types, borrow checking, region errors against signatures) are partial, so it is not a replacement for rustc for checking correctness.
> - zust-analyzer implements only part of what rust-analyzer does.

## Requirements

- Linux x86-64
- A Rust toolchain that supports edition 2024 (rustup stable is fine)
- The C compiler driver `cc` (used to build the link line)
- A linker: wild, mold, ld.lld or ld. zustc picks the first one on PATH in the order wild > mold > ld.lld > ld.
- For zust-analyzer to resolve std definitions, rust-src: `rustup component add rust-src`

## Install

```bash
git clone https://github.com/mizchi/zust.git
```

```bash
cd zust
```

```bash
cargo install --path .
```

```bash
cargo install --path crates/analyzer
```

```bash
cargo install --path crates/zurgo
```

This puts `zustc`, `zust-analyzer` and `zurgo` in `~/.cargo/bin`. You can also run `cargo build --release --workspace` and use
the binaries in `target/release/` without installing.

## Using zustc

### A single-file program

```bash
zustc hello.rs -o hello
```

```bash
./hello
```

- **Output path:** without `-o`, the output goes next to the input with the extension removed (`hello`).
- **Files next to the output:**
  - `hello.cache`: the cache
  - `hello.o`: the object file
  - `hello.link`: the saved link line
- **Rebuilds:** only the functions that changed are recompiled. If nothing changed, zustc does not even link.

`mod x;` reads `x.rs` or `x/mod.rs`.

### Check only

```bash
zustc check hello.rs
```

This runs up to type checking (no code generation) and prints all diagnostics. It also checks unreachable functions
and every requested instance of generic functions. When a round of the check has at least 32 functions per worker to
check, they are split across at most `-j N` forked processes (N is the number of CPUs by default); the diagnostics are
the same at any `-j`.

### Multiple crates

Pass a file that lists one crate per line. Each line is `lib|bin <name> <root file> [dependencies..]`, with paths
relative to that file:

```text
lib util util/src/lib.rs
lib geom geom/src/lib.rs util
bin app app/src/main.rs geom util
```

```bash
zustc build workspace.txt -o target
```

- Words after the dependencies set more of the crate:
  - `features=a,b`: its features;
  - `cfg=name` / `cfg=name=value`: other cfgs (what a build script's `rustc-cfg` gives);
  - `env=KEY=VALUE`: the values `env!` / `option_env!` see (`CARGO_PKG_*`, `OUT_DIR`);
  - `exe=NAME`: a bin's executable name;
  - `extern=ALIAS=NAME`: the dependency NAME under the name ALIAS in this crate (a renamed dependency, rustc's
    `--extern ALIAS=..`). Every module of the crate sees it; NAME keeps its own name everywhere else.

  In a value, `%`, `#` and white space are written `%XX`. A name `env!` does not find there is looked up in zustc's
  own environment, as rustc does.
- Each crate gets its own object file and cache, and they are linked at the end. With
  `--target wasm32-unknown-unknown`, the crates' functions and data are put into one wasm module per bin.
- Changing the body of a function in a crate that others depend on does not recompile the dependents; zustc only
  relinks.
- See `tests/workspace/` for an example.
- zustc does not read Cargo.toml: zurgo (next section) does.

### Cargo packages (zurgo)

`zurgo` builds a Cargo package with zustc: it reads Cargo.toml and Cargo.lock, takes crates.io sources from
`$CARGO_HOME/registry/src` (where `cargo fetch` puts them), turns on features as cargo does, and builds the graph as a
zustc workspace ([docs/zurgo.md](docs/zurgo.md)).

```bash
zurgo run
```

- Commands: `build`, `run`, `test`, `metadata` (the JSON of `cargo metadata`), `clean`, with cargo's flags for them.
  `zurgo test` runs each test in a process of its own, all at once, as cargo-nextest does.
- Output goes to `target/zust/<profile>/`. With `--target wasm32-unknown-unknown`, each bin is a wasm module in
  `target/zust/wasm32-unknown-unknown/<profile>/`, and `zurgo run` runs it with
  `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER`, as cargo does.
- Build scripts run as under cargo (compiled by zustc, or by rustc when zustc cannot), and their cfgs, `rustc-env` and
  `OUT_DIR` reach the crates. A proc-macro dependency is built by rustc and run by the toolchain's proc-macro server
  (`rust-analyzer-proc-macro-srv`): its attributes, derives and function-like macros expand (mizchi/zust#29).
- A renamed dependency (`common = { package = "crypto-common" }`) is the library under that name in its dependent
  only, as rustc's `--extern common=..`.
- Not yet: two versions of one crate, git dependencies, its own resolver (mizchi/zust#28).

### Options

| option | meaning |
|---|---|
| `-o PATH` | output path (the output directory for `build`) |
| `-O` | optimize: inline small functions and use Cranelift's `speed` level. Disables overflow checks |
| `--backend=cranelift\|minclif` | code generator (default cranelift; also `ZUSTC_BACKEND`) |
| `--linker=wild\|mold\|lld\|bfd\|PATH` | choose the linker |
| `--features=a,b` | enable `#[cfg(feature = "..")]` |
| `--cfg NAME` / `--cfg NAME="V"` | set a cfg, as rustc's `--cfg` |
| `--test` | build the `#[test]` functions with a harness: `--list`, and one test per process (see [docs/zurgo.md](docs/zurgo.md#tests)) |
| `-j N` | at most N worker processes: for code generation, and for `check` (default: the number of CPUs) |
| `--time` | print the time of each phase and the peak RSS |
| `--no-cache` | neither read nor write the cache |

Panics are reported in the same form as rustc: `thread 'main' panicked at src/main.rs:LINE:COL:`, then the message,
then exit code 101. Integer overflow is checked as in a rustc debug build; `-O` turns the checks off.

## Using zust-analyzer

zust-analyzer is an LSP server over stdio; start `zust-analyzer` from your editor's LSP client. It treats the
directory with Cargo.toml as the workspace root and indexes:

- the crates: lib, bin, tests, examples and build.rs, plus the files reachable through `mod`
- std, from rust-src
- the dependencies listed in Cargo.toml, using the versions from Cargo.lock, that are present under
  `~/.cargo/registry/src`

### Neovim (0.11 or later)

```lua
vim.lsp.config('zust_analyzer', {
  cmd = { 'zust-analyzer' },
  filetypes = { 'rust' },
  root_markers = { 'Cargo.toml', '.git' },
})
vim.lsp.enable('zust_analyzer')
```

### Helix (`languages.toml`)

```toml
[language-server.zust-analyzer]
command = "zust-analyzer"

[[language]]
name = "rust"
language-servers = ["zust-analyzer"]
```

Any other editor works too, as long as its client can start an arbitrary command as an LSP server.

### Features

| feature | supported |
|---|---|
| documentSymbol / workspace/symbol | yes |
| definition / references / documentHighlight | yes |
| hover / completion / signatureHelp | yes |
| rename / prepareRename | yes (edits only files inside the workspace) |
| publishDiagnostics | yes (diagnostics from zustc's check; code zustc cannot handle may get false diagnostics) |
| semanticTokens / inlayHint / codeAction / formatting | no |

Documents are synced incrementally: didChange receives the ranges an edit changed (or the whole text, if the client sends it).

### Configuration (environment variables)

| variable | meaning |
|---|---|
| `ZUST_ANALYZER_MEM_MB` | how much workspace file text to keep in memory, in MB (default 4). Files beyond it are reread when needed |
| `ZUST_ANALYZER_NO_STD` | do not index std |
| `ZUST_ANALYZER_NO_DEPS` | do not index dependency crates |
| `ZUST_ANALYZER_SYSROOT` | sysroot to look for the std sources in (default: `rustc --print sysroot`) |

Known differences from rust-analyzer are listed in [CONTRIBUTING.md](CONTRIBUTING.md#zust-analyzer-remaining-differences-from-rust-analyzer).

## Status

Numbers measured <!-- measured -->2026-11-28, 8e4e02d<!-- /measured --> by CI on each push to main. Details:
[SUPPORT.md](SUPPORT.md) (what is supported) and [PERFORMANCE.md](PERFORMANCE.md) (speed, size, memory, and
comparisons with rustc and others).

### Coverage of rustc's test suite

zustc run against rustc's own UI tests (`tests/ui` of rust-lang/rust 1.100.0, `python3 bench/rust_ui.py`), counting
only the tests it can be asked to handle (no nightly features, auxiliary crates or special flags):

<!-- auto:ui -->
| kind | applicable | pass |
|---|---:|---:|
| run-pass (builds, runs, same output) | 2403 | 1938 (80.6%) |
| check-pass (accepted by `zustc check`) | 1813 | 1562 (86.2%) |
| build-pass | 135 | 104 (77.0%) |
| compile-fail (rejected at an annotated line) | 7386 | 5462 (74.0%) |
<!-- /auto:ui -->

[SUPPORT.md](SUPPORT.md) breaks this down by area: the language, the checks, the runtime and std, and what is left.

### Speed

Generated programs of 1000 / 5000 functions, on shared GitHub Actions runners (times vary by about ±20%). The full
table, with instruction counts, cache and binary sizes and memory, is in [PERFORMANCE.md](PERFORMANCE.md#kpis):

<!-- auto:kpi-summary -->
| | gen1000 | gen5000 |
|---|---:|---:|
| cold build | 134 ms | 587 ms |
| no-op build | 0.7 ms | 0.7 ms |
| build after editing one function | 19.3 ms | 69.9 ms |
| `check` | 43 ms | 196 ms |
<!-- /auto:kpi-summary -->

Against `rustc -C incremental` on gen1000 (a snapshot measured by hand): about 6x faster cold, 15x faster after an
edit, and a 14x smaller cache ([PERFORMANCE.md](PERFORMANCE.md#against-rustc-relax-and-rust-analyzer-snapshots)).

## Roadmap

What is planned and what blocks it: [CONTRIBUTING.md](CONTRIBUTING.md#roadmap) and the
[issues](https://github.com/mizchi/zust/issues).

## Documentation

- [docs/language.md](docs/language.md): the subset of Rust that zustc supports, and what it does not
- [docs/architecture.md](docs/architecture.md): crate layout, the pipeline, how incremental builds work, value representation
- [docs/reports/](docs/reports/README.md): work log, as dated reports (benchmark results and what was done at each step)
- [SUPPORT.md](SUPPORT.md): what is supported, by area, with rustc's tests/ui pass rates
- [PERFORMANCE.md](PERFORMANCE.md): the KPIs, and comparisons with rustc, relax, blaze and rust-analyzer
- [CONTRIBUTING.md](CONTRIBUTING.md): how to develop (setting up a clone, tests, KPIs, benchmarks, reports) and the roadmap

## License

Like Rust itself, zust is dual-licensed; you may use it under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this repository, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

The crates in `cratebench/vendor/` (git submodules used by the benchmarks) are under their own upstream licenses.
