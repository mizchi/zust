# zust

[![CI](https://github.com/mizchi/zust/actions/workflows/ci.yml/badge.svg)](https://github.com/mizchi/zust/actions/workflows/ci.yml)
[![Benchmark](https://github.com/mizchi/zust/actions/workflows/bench.yml/badge.svg)](https://mizchi.github.io/zust/dev/bench/)

English | [日本語](README.ja.md)

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
> - zustc only targets Linux x86-64 and does not support all of Rust ([supported language](docs/language.md)).
> - zustc does no borrow checking and similar checks, so it is not a replacement for rustc for checking correctness.
> - zust-analyzer implements only part of what rust-analyzer does.

## How fast

These numbers are for a generated program with 1000 functions (gen1000), measured with `python3 bench/vs_rustc.py`
(2026-11-15, 4 cores):

| | cold | edit one place | cache on disk |
|---|---:|---:|---:|
| zustc | 118 ms | 28 ms | 105 KiB |
| rustc `-C incremental` | 827 ms | 417 ms | 22 MiB |

Results for other programs are in [docs/reports/2026-11-15-15-status.md](docs/reports/2026-11-15-15-status.md).

zust-analyzer and rust-analyzer were compared on zustc's own sources, with 40 query points (2026-11-18,
`python3 bench/lsp_parity.py`):

| | startup until symbols resolve | memory | references p50 | same reference count as rust-analyzer |
|---|---:|---:|---:|---:|
| zust-analyzer | 129 ms | 66 MB | 4.2 ms | 97% |
| rust-analyzer | 5594 ms | 2.4 GB | 24.1 ms | (baseline) |

Details are in [bench/lsp_parity_result.md](bench/lsp_parity_result.md).

The [benchmark dashboard](https://mizchi.github.io/zust/dev/bench/) tracks these numbers for every commit to main.
They are measured on shared GitHub Actions runners, so times fluctuate. The instruction-count series show changes
more clearly.

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

This puts `zustc` and `zust-analyzer` in `~/.cargo/bin`. You can also run `cargo build --release --workspace` and use
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
  - `hello.crcache2`: the cache
  - `hello.o`: the object file
  - `hello.link`: the saved link line
- **Rebuilds:** only the functions that changed are recompiled. If nothing changed, zustc does not even link.

`mod x;` reads `x.rs` or `x/mod.rs`.

### Check only

```bash
zustc check hello.rs
```

This runs up to type checking (no code generation) and prints all diagnostics. It also checks unreachable functions
and every requested instance of generic functions.

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

- Each crate gets its own object file and cache, and they are linked at the end.
- Changing the body of a function in a crate that others depend on does not recompile the dependents; zustc only
  relinks.
- See `tests/workspace/` for an example.
- Cargo.toml is not read.

### Options

| option | meaning |
|---|---|
| `-o PATH` | output path (the output directory for `build`) |
| `-O` | optimize: inline small functions and use Cranelift's `speed` level. Disables overflow checks |
| `--backend=cranelift\|minclif` | code generator (default cranelift; also `ZUSTC_BACKEND`) |
| `--linker=wild\|mold\|lld\|bfd\|PATH` | choose the linker |
| `--features=a,b` | enable `#[cfg(feature = "..")]` |
| `-j N` | number of code generation threads |
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

Documents are synced in full: didChange receives the whole text.

### Configuration (environment variables)

| variable | meaning |
|---|---|
| `ZUST_ANALYZER_MEM_MB` | how much workspace file text to keep in memory, in MB (default 4). Files beyond it are reread when needed |
| `ZUST_ANALYZER_NO_STD` | do not index std |
| `ZUST_ANALYZER_NO_DEPS` | do not index dependency crates |
| `ZUST_ANALYZER_SYSROOT` | sysroot to look for the std sources in (default: `rustc --print sysroot`) |

Known differences from rust-analyzer are listed in [TODO.md](TODO.md).

## Documentation

The documents below are written in Japanese.

- [docs/language.md](docs/language.md): the subset of Rust that zustc supports, and what it does not
- [docs/architecture.md](docs/architecture.md): crate layout, the pipeline, how incremental builds work, value representation
- [docs/reports/](docs/reports/README.md): work log, as dated reports (benchmark results and what was done at each step)
- [CONTRIBUTING.md](CONTRIBUTING.md): how to develop: build, test, benchmark, and write reports
- [TODO.md](TODO.md): what is next

## History

This project started as `2026-11-14-crustc/` in mizchi/playground, where the compiler was called crustc and the LSP
server crustc-lsp. On 2026-11-18 it moved to this repository with its history, and the names changed:

| before | now |
|---|---|
| `crustc` (compiler) | `zustc` |
| `crustc-lsp` (LSP server) | `zust-analyzer` |
| library crates | `zust-*` |
| environment variables `CRUSTC_*` | `ZUSTC_*` |
| environment variables `CRUSTC_LSP_*` | `ZUST_ANALYZER_*` |

Numbers and dates in docs/reports/ are from before the move.

## License

Like Rust itself, zust is dual-licensed; you may use it under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this repository, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

The crates in `cratebench/vendor/` (git submodules used by the benchmarks) are under their own upstream licenses.
