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
> - zustc's checks (types, borrow checking without region errors) are partial, so it is not a replacement for rustc for checking correctness.
> - zust-analyzer implements only part of what rust-analyzer does.

## How fast

### Against rustc

Measured with `python3 bench/vs_rustc.py` and `python3 cratebench/run.py` (2026-11-18, 4 cores, rustc 1.100.0; times
are medians of 3 runs). gen1000 is a generated program with 1000 functions:

| gen1000 | cold | no-op | edit one function | on disk | binary (stripped) | run |
|---|---:|---:|---:|---:|---:|---:|
| zustc | 131 ms | 15 ms | 22 ms | 766 KiB | 324 KiB | 10 ms |
| zustc `-O` | 270 ms | 57 ms | 56 ms | 1199 KiB | 341 KiB | 3 ms |
| rustc `-C incremental` | 643 ms | 172 ms | 348 ms | 10.8 MiB | 1038 KiB | 9 ms |
| rustc `-O` | 15456 ms | - | - | - | 377 KiB | 2 ms |
| **zustc vs rustc `-C incremental`** | **4.9x faster** | **11x faster** | **16x faster** | **14x smaller** | **3.2x smaller** | |

Small programs (`bench/rt`: fib, hashmap, matmul, nbody, sieve, sort, strings) and real crates (cratebench: fnv,
adler2, hex, itoa, rustc-hash, cfg-if; all six give the same output as rustc):

| | zustc vs rustc `-C incremental` (bench/rt) | zustc vs rustc (cratebench) |
|---|---:|---:|
| cold build | 2.9–4.1x faster (24–49 ms vs 82–178 ms) | 2.3–4.4x faster (21–75 ms vs 93–306 ms) |
| edit and rebuild | 12–18x faster (5–7 ms vs 68–98 ms) | 4.7–8.8x faster (12–28 ms vs 63–140 ms) |
| binary | 11–24x smaller | 90–550x smaller (8–48 KiB vs 4.4 MiB) |
| run time vs rustc `-O` | zustc 1.1–4.1x slower, zustc `-O` 1.1–3.2x slower | zustc 3.1–17x slower, zustc `-O` 0.9–7.3x |

zustc's code is close to rustc's debug build at run time, and faster than it in all seven small programs (rustc's
debug build is 3–26x slower than rustc `-O` there).

zust-analyzer against rust-analyzer on zustc's own sources (40 query points, `python3 bench/lsp_parity.py`,
2026-11-18):

| | startup until symbols resolve | memory | references p50 | agreement (40 points) |
|---|---:|---:|---:|---:|
| zust-analyzer | 144 ms | 70 MB | 4.6 ms | 95.3% |
| rust-analyzer | 6302 ms | 2150 MB | 154.1 ms | 96.2% |

Details are in [bench/lsp_parity_result.md](bench/lsp_parity_result.md).

### KPIs over time

The KPIs are build speed, build cache size and LSP latency, measured with `python3 bench/ci_bench.py` on generated
programs of 1000 / 5000 functions. Builds are compared by instruction counts (valgrind), because wall-clock times on
the shared machine vary by about ±20%.

| point | gen5000 no-op build | gen5000 edit 1 fn | gen5000 `check` | gen5000 cache | gen1000 cache | gen5000 cold peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| before borrow checking (ac2361e) | | | 3883.9 M | | | |
| borrow checking added ([report 08](docs/reports/2026-11-18-08-borrow-checking.md)) | 746.0 M | 709.3 M | 3971.0 M | 1902.8 KiB | 387.2 KiB | 76.0 MiB |
| compatibility round ([report 09](docs/reports/2026-11-18-09-compat-with-kpis.md), 03367bc) | 685.7 M | 648.1 M | 3912.7 M | 1902.8 KiB | 387.2 KiB | 54.6 MiB |
| faster prelude selection (3a1fd22) | 661.1 M | 622.4 M | 3887.3 M | 1902.8 KiB | 387.2 KiB | 51.8 MiB |
| now: async, statics, const fn (e598fdf) | 661.9 M | 622.6 M | 3892.9 M | 1902.8 KiB | 387.2 KiB | 51.9 MiB |
| **change since borrow checking** | **-11.3%** | **-12.2%** | **-2.0%** | **0%** | **0%** | **-32%** |

LSP (p50 of `bench/lsp_parity.py` on zustc's own sources; these are wall-clock times):

| point | definition | hover | completion | references | startup until indexed |
|---|---:|---:|---:|---:|---:|
| borrow checking added | 0.65 ms | 0.47 ms | 1.28 ms | 3.36 ms | 135 ms |
| compatibility round (03367bc) | 0.53 ms | 0.43 ms | 1.09 ms | 3.06 ms | 116 ms |
| faster prelude selection (3a1fd22) | 0.58 ms | 0.47 ms | 1.37 ms | 3.45 ms | 109 ms |
| now (e598fdf) | 0.42 ms | 0.34 ms | 0.90 ms | 1.78 ms | 277 ms (two targets) |

Each change is kept only if these do not get worse. When an LSP time rises in a full run, the two builds are
compared again on the same machine, alternating runs, before it is called noise. The last row comes from such a
comparison, and its startup is the sum over two target files.

The [benchmark dashboard](https://mizchi.github.io/zust/dev/bench/) tracks these numbers for every commit to main.
They are measured on shared GitHub Actions runners, so times fluctuate. The instruction-count series show changes
more clearly.

### Coverage of rustc's test suite

zustc run against rustc's own UI tests (`tests/ui` of rust-lang/rust 1.100.0, `python3 bench/rust_ui.py`), counting
only the tests it can be asked to handle (no nightly features, auxiliary crates or special flags):

| kind | applicable | pass |
|---|---:|---:|
| run-pass (builds, runs, same output) | 2413 | 1364 (56.5%) |
| check-pass (accepted by `zustc check`) | 1807 | 1098 (60.8%) |
| build-pass | 135 | 81 (60.0%) |
| compile-fail (rejected at an annotated line) | 7358 | 2808 (38.2%) |

[SUPPORTED.md](SUPPORTED.md) breaks this down by area as a checklist.

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

Japanese versions of the docs are under [docs/ja/](docs/ja/).

- [docs/language.md](docs/language.md): the subset of Rust that zustc supports, and what it does not
- [docs/architecture.md](docs/architecture.md): crate layout, the pipeline, how incremental builds work, value representation
- [docs/reports/](docs/reports/README.md): work log, as dated reports (benchmark results and what was done at each step)
- [CONTRIBUTING.md](CONTRIBUTING.md): how to develop: build, test, benchmark, and write reports
- [SUPPORTED.md](SUPPORTED.md): what is supported, what is in sight, what would cost performance, and what is not started
- [TODO.md](TODO.md): what is next

## License

Like Rust itself, zust is dual-licensed; you may use it under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this repository, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

The crates in `cratebench/vendor/` (git submodules used by the benchmarks) are under their own upstream licenses.
