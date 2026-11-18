# LSP comparison: rust-analyzer / rust-glancer (2026-11-17, `python3 bench/lsp_compare.py --cold`)

Date: 2026-11-17

The 3 LSP servers are run with the same steps (stdio; requests from the server get empty replies):
initialize → initialized → didOpen → query workspace/symbol every 0.1 s until it returns a known name (ready) →
documentSymbol 10 times (median) → workspace/symbol with 5 queries × 5 times (median) → documentSymbol right after a didChange that
adds one function (edit→docsym), and the time until publishDiagnostics arrives (edit→diag, cut off at 20 s). peak RSS is the VmHWM of the
server process; the value in parentheses is the maximum RSS including descendant processes (sampled every 50 ms).

- Target synth: a generated Cargo crate (40 modules, 25 functions each + struct / enum / impl, about 4400 lines). Opens src/m7.rs
- Target zustc: this directory (a Cargo workspace, with dependencies such as cranelift). Opens crates/core/src/driver2.rs
- rust-analyzer 1.100.0 (rustup component). rust-glancer is github.com/rust-glancer/rust-glancer (50dd51a) built with
  `cargo build --release -p rust-glancer` (set with `RUST_GLANCER=<binary>`; needs rust-src).
  `--cold` deletes the rust-glancer index (target/rust_glancer) before measuring

| Target | Server | init | ready | docsym (count) | wsym (found) | edit→docsym | edit→diag | peak RSS (with children) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| synth | zustc | 2.2 ms | 9.6 ms | 0.6 ms (37) | 0.7 ms (5/5) | 2.0 ms | 1.3 ms | 7.5 MB (5.1 MB) |
| synth | zustc (after adding definition / references) | 2.0 ms | 9.7 ms | 0.6 ms (37) | 0.6 ms (5/5) | 1.8 ms | 1.6 ms | 10.7 MB (6.4 MB) |
| synth | rust-analyzer | 18.7 ms | 3243 ms | 1.0 ms (37) | 2.3 ms (5/5) | 8.3 ms | 309 ms | 607 MB (878 MB) |
| synth | rust-glancer | 36.4 ms | 5799 ms | 1.9 ms (38) | 27.0 ms (5/5) | 43.3 ms | - | 16.9 MB (888 MB) |
| zustc | zustc | 2.3 ms | 55.0 ms | 1.8 ms (123) | 3.5 ms (5/5) | 9.7 ms | 7.8 ms | 10.9 MB (9.9 MB) |
| zustc | zustc (after adding definition / references) | 1.9 ms | 91.4 ms | 2.1 ms (123) | 1.3 ms (5/5) | 13.0 ms | 10.4 ms | 29.9 MB (29.4 MB) |
| zustc | rust-analyzer | 23.0 ms | 5954 ms | 3.6 ms (124) | 2.0 ms (5/5) | 26.2 ms | - | 1360 MB (2723 MB) |
| zustc | rust-glancer | 130 ms | 10593 ms | 7.5 ms (113) | 262 ms (5/5) | 48.6 ms | - | 30.7 MB (1798 MB) |

Notes on reading the results:
- The scope of features differs. rust-analyzer does name resolution and type inference across the crate's dependencies, and also supports
  completion, go to definition, and so on. rust-glancer is a lightweight server that builds an index including dependencies (stored on disk).
  zustc lsp checks the open file as a single crate (it does not read Cargo dependencies or other files from `mod x;`), and only reads
  symbols from the token stream. zustc's ready is fast because it builds neither dependencies nor a type index; it only reads the symbols
  of the workspace .rs files.
- edit→diag: for zustc, this is the result of checking only the open file (driver2.rs alone, so it reports errors). rust-analyzer pushes
  its own diagnostics (for the zustc target, none arrived within 20 s). rust-glancer has diagnostics off by default (cargo check is enabled in settings).
- edit→docsym includes processing after receiving didChange (zustc runs check on every didChange, so that time is included).
- rust-analyzer and rust-glancer have large child processes (cargo metadata, rustc, indexing work). rust-glancer's main process is 17–31 MB.
- Differences in symbol counts: rust-glancer counts associated items inside impls etc. differently (the 38 for synth comes from how impl names are output).
