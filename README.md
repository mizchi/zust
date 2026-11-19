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

Measured with `python3 bench/vs_rustc.py` and `python3 cratebench/run.py` (2026-11-18, 4 cores, rustc 1.100.0; times are
medians of 3 runs). They are measured again with each round of work. gen1000 is a generated program with 1000 functions:

| gen1000 | cold | no-op | edit one function | on disk | binary (stripped) | run |
|---|---:|---:|---:|---:|---:|---:|
| zustc | 123 ms | 18 ms | 27 ms | 766 KiB | 324 KiB | 11 ms |
| zustc `-O` | 374 ms | 68 ms | 67 ms | 1199 KiB | 341 KiB | 3 ms |
| rustc `-C incremental` | 756 ms | 197 ms | 406 ms | 10.8 MiB | 1038 KiB | 7 ms |
| rustc `-O` | 18345 ms | - | - | - | 377 KiB | 2 ms |
| **zustc vs rustc `-C incremental`** | **6.1x faster** | **11x faster** | **15x faster** | **14x smaller** | **3.2x smaller** | |

Small programs (`bench/rt`: fib, hashmap, matmul, nbody, sieve, sort, strings) and real crates (cratebench: fnv,
adler2, hex, itoa, rustc-hash, cfg-if; all six give the same output as rustc):

| | zustc vs rustc `-C incremental` (bench/rt) | zustc vs rustc (cratebench) |
|---|---:|---:|
| cold build | 2.5–4.0x faster (21–39 ms vs 71–131 ms) | 2.9–4.2x faster (22–61 ms vs 90–234 ms) |
| edit and rebuild | 12–17x faster (4–6 ms vs 63–75 ms) | 4.8–7.0x faster (11–22 ms vs 68–114 ms) |
| binary | 13–24x smaller | 93–550x smaller (8–48 KiB vs 4.4 MiB) |
| run time vs rustc `-O` | zustc 1.2–3.6x slower, zustc `-O` 1.0–2.9x slower | zustc 3.1–22x slower, zustc `-O` 0.9–9.1x |

The same gen1000 comparison over time (rustc's own times move by about ±10% between runs, and zustc's cold build by about ±20%: run alternately against
the previous commit, 1501b30 and 8e797a9 have the same cold median, 96 and 98 ms):

| measured | zustc cold | rustc `-C incremental` cold | ratio | zustc edit | rustc edit | ratio |
|---|---:|---:|---:|---:|---:|---:|
| 2026-11-15 ([report](docs/reports/2026-11-15-15-status.md)) | 118 ms | 827 ms | 7.0x | 28 ms | 417 ms | 14.9x |
| 2026-11-18, 72201ab | 131 ms | 643 ms | 4.9x | 22 ms | 348 ms | 15.8x |
| 2026-11-18, items inside blocks (cbcd294) | 117 ms | 620 ms | 5.3x | 22 ms | 334 ms | 15.2x |
| 2026-11-18, inference and borrowck of call arguments (8e797a9) | 94 ms | 602 ms | 6.4x | 22 ms | 353 ms | 16.0x |
| 2026-11-18, builtin macros (1501b30) | 127 ms | 600 ms | 4.7x | 24 ms | 316 ms | 13.2x |
| 2026-11-18, `impl Trait` in traits, more macros (8cb2f88) | 131 ms | 605 ms | 4.6x | 20 ms | 357 ms | 17.9x |
| 2026-11-18, `assert!` in type inference (4d210d2) | 126 ms | 619 ms | 4.9x | 23 ms | 328 ms | 14.3x |
| 2026-11-18, `assert_eq!` in type inference (441d0c2) | 119 ms | 605 ms | 5.1x | 25 ms | 338 ms | 13.5x |
| 2026-11-18, borrowing values in registers (fb2f2f2) | 112 ms | 595 ms | 5.3x | 22 ms | 367 ms | 16.7x |
| 2026-11-18, `try_from`, `::std` paths, more std functions (096a6bc) | 102 ms | 624 ms | 6.1x | 22 ms | 327 ms | 14.9x |
| 2026-11-18, prelude selection without per-build tables (f077e7c) | 89 ms | 623 ms | 7.0x | 22 ms | 320 ms | 14.5x |
| 2026-11-18, `dyn Trait<A>` (f606b34) | 89 ms | 641 ms | 7.2x | 22 ms | 309 ms | 14.0x |
| 2026-11-18, integer literals for bounds, `Box` comparisons (60cbe60) | 112 ms | 655 ms | 5.8x | 21 ms | 341 ms | 16.2x |
| 2026-11-18, `impl Trait` returning `()` (e7fdc21) | 121 ms | 592 ms | 4.9x | 23 ms | 331 ms | 14.4x |
| 2026-11-18, std type names that user code may redefine (9bd5297) | 120 ms | 625 ms | 5.2x | 21 ms | 331 ms | 15.8x |
| 2026-11-18, `Pin`, generic arguments on variants (e45ee10) | 109 ms | 669 ms | 6.1x | 22 ms | 372 ms | 16.9x |
| 2026-11-18, e45ee10 measured again with all programs | 130 ms | 587 ms | 4.5x | 22 ms | 332 ms | 15.1x |
| 2026-11-18, unsafe checks (E0133) (04b7d0b) | 128 ms | 611 ms | 4.8x | 22 ms | 343 ms | 15.6x |
| 2026-11-18, fn pointer comparisons, `main` as a value (b2620bd) | 90 ms | 625 ms | 6.9x | 23 ms | 326 ms | 14.2x |
| 2026-11-18, diverging `let`, Option in `for`, `2.eq(&3)` (5ecf037) | 93 ms | 621 ms | 6.7x | 21 ms | 309 ms | 14.7x |
| 2026-11-18, let chains, `&raw const` / `&raw mut` (d01898b) | 88 ms | 631 ms | 7.2x | 20 ms | 314 ms | 15.7x |
| 2026-11-19, three parallel lanes: parser, std, item summaries (9effaaa); a slower machine after a container restart (rustc `-O` 18.5 s, was 14.4 s) | 142 ms | 691 ms | 4.9x | 24 ms | 370 ms | 15.4x |
| 2026-11-19, `impl Tr for Box<S>`, shifts and `reverse_bits`, `..b` values (962f2a6) | 150 ms | 847 ms | 5.6x | 27 ms | 447 ms | 16.6x |
| 2026-11-19, `as _` / `as *const _` (75630e2) | 127 ms | 859 ms | 6.8x | 31 ms | 399 ms | 12.9x |
| 2026-11-19, operator traits of numbers, `dyn Tr: Tr` (a6c8ba5) | 122 ms | 763 ms | 6.3x | 25 ms | 417 ms | 16.7x |
| 2026-11-19, bounds of associated types checked for `dyn Tr<Y = T>` (509b25d) | 166 ms | 746 ms | 4.5x | 30 ms | 403 ms | 13.4x |
| 2026-11-19, consts of generic impls | 123 ms | 756 ms | 6.1x | 27 ms | 406 ms | 15.0x |

zustc's code is close to rustc's debug build at run time, and faster than it in all seven small programs (rustc's
debug build is 3–26x slower than rustc `-O` there).

Against relax ([makepad/relax](https://github.com/makepad/relax) 3489f2d, a Rust JIT that compiles each function
when it is first called; `python3 bench/vs_relax.py`, 2026-11-18). relax runs the program while it compiles it, so the
three are compared on the time from source to the program's last output (zustc and rustc: build, then run). Every
output matches rustc `-O`:

| source to finished output | cold | no-op | edit one function | cache on disk |
|---|---:|---:|---:|---:|
| gen1000: zustc | 103 ms | 23 ms | 31 ms | 766 KiB |
| gen1000: rustc `-C incremental` | 585 ms | 173 ms | 384 ms | 10.8 MiB |
| gen1000: relax run | 1193 ms | 74 ms | 569 ms | 145 MiB |
| bench/rt (7 programs): zustc | 87–278 ms | 56–224 ms | 58–266 ms | 37–90 KiB |
| bench/rt: rustc `-C incremental` | 164–1718 ms | 141–1654 ms | 145–1615 ms | 0.1–0.9 MiB |
| bench/rt: relax run | 831–1460 ms | 106–356 ms | 168–435 ms | 136–137 MiB |
| **zustc vs relax** | **5.3–12x faster** | **1.3–3.4x faster** | **1.6–18x faster** | **1/190 (gen1000)–1/3800** |

relax's numbers include starting its runtime and loading its own std; its strength (patching edits into a running
program, keeping its state) is not measured here. On the small programs, the run itself is a large part of every
number: rustc's debug build runs slowly, so relax beats it on five of the seven after an edit.

zust-analyzer against rust-analyzer and rust-glancer on zustc's own sources (40 query points,
`python3 bench/lsp_parity.py --servers zustc,rust-analyzer,rust-glancer --targets zustc`, 2026-11-18, e45ee10;
rust-glancer is github.com/rust-glancer/rust-glancer 50dd51a):

| | startup until symbols resolve | memory | references p50 | agreement (mean over the queries) |
|---|---:|---:|---:|---:|
| zust-analyzer | 113 ms | 71 MB | 10.9 ms | 99.0% |
| rust-analyzer | 6118 ms | 2060 MB | 214.0 ms | 98.9% |
| rust-glancer | 9965 ms | 1573 MB | 548.8 ms | 58.5% |

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
| async, statics, const fn (e598fdf) | 661.9 M | 622.6 M | 3892.9 M | 1902.8 KiB | 387.2 KiB | 51.9 MiB |
| items inside blocks (cbcd294) | 661.5 M | 622.9 M | 3893.1 M | 1902.8 KiB | 387.2 KiB | 51.6 MiB |
| inference and borrowck of call arguments (8e797a9) | 661.6 M | 623.0 M | 3893.3 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| builtin macros (1501b30) | 661.3 M | 622.8 M | 3893.3 M | 1902.8 KiB | 387.2 KiB | 51.6 MiB |
| `impl Trait` in traits, more macros (8cb2f88) | 661.4 M | 622.9 M | 3893.3 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| `assert!` in type inference (4d210d2) | 662.1 M | 622.9 M | 3893.6 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| `assert_eq!` in type inference (441d0c2) | 661.4 M | 622.8 M | 3893.5 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| borrowing values in registers (fb2f2f2) | 662.0 M | 622.7 M | 3893.5 M | 1902.8 KiB | 387.2 KiB | 51.6 MiB |
| `try_from`, `::std` paths, more std functions (096a6bc) | 662.9 M | 623.7 M | 3892.3 M | 1902.8 KiB | 387.2 KiB | 52.0 MiB |
| prelude selection without per-build tables (f077e7c) | 663.8 M | 620.8 M | 3892.9 M | 1902.8 KiB | 387.2 KiB | 52.0 MiB |
| `dyn Trait<A>` (f606b34) | 663.8 M | 620.8 M | 3893.0 M | 1902.8 KiB | 387.2 KiB | 51.8 MiB |
| integer literals for bounds, `Box` comparisons (60cbe60) | 661.0 M | 621.0 M | 3893.0 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| `impl Trait` returning `()` (e7fdc21) | 663.2 M | 620.8 M | 3893.0 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| std type names that user code may redefine (9bd5297) | 663.8 M | 620.8 M | 3893.0 M | 1902.8 KiB | 387.2 KiB | 51.9 MiB |
| `Pin`, generic arguments on variants (e45ee10) | 662.1 M | 621.7 M | 3891.1 M | 1902.8 KiB | 387.2 KiB | 51.9 MiB |
| unsafe checks (E0133) (04b7d0b) | 662.1 M | 621.8 M | 3891.7 M | 1902.8 KiB | 387.2 KiB | 51.8 MiB |
| fn pointer comparisons, `main` as a value (b2620bd) | 664.9 M | 621.8 M | 3894.6 M | 1902.8 KiB | 387.2 KiB | 51.6 MiB |
| diverging `let`, Option in `for`, `2.eq(&3)` (5ecf037) | 664.9 M | 621.8 M | 3895.2 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| let chains, `&raw const` / `&raw mut` (d01898b) | 662.2 M | 621.8 M | 3895.6 M | 1902.8 KiB | 387.2 KiB | 51.8 MiB |
| three parallel lanes: parser, std, item summaries (9effaaa) | 668.7 M | 626.3 M | 3906.9 M | 1902.8 KiB | 387.2 KiB | 51.4 MiB |
| `impl Tr for Box<S>`, shifts and `reverse_bits`, `..b` values (962f2a6) | 666.0 M | 626.3 M | 3907.0 M | 1902.8 KiB | 387.2 KiB | 51.8 MiB |
| `as _` / `as *const _` (75630e2) | 662.8 M | 626.2 M | 3906.8 M | 1902.8 KiB | 387.2 KiB | 52.0 MiB |
| operator traits of numbers, `dyn Tr: Tr` (a6c8ba5) | 668.7 M | 626.3 M | 3906.8 M | 1902.8 KiB | 387.2 KiB | 51.7 MiB |
| bounds of associated types checked for `dyn Tr<Y = T>` (509b25d) | 666.2 M | 626.2 M | 3906.1 M | 1902.8 KiB | 387.2 KiB | 52.1 MiB |
| now: consts of generic impls | 667.1 M | 626.6 M | 3906.2 M | 1902.8 KiB | 387.2 KiB | 51.8 MiB |
| **change since borrow checking** | **-11.3%** | **-12.2%** | **-2.0%** | **0%** | **0%** | **-32%** |

LSP (p50 of `bench/lsp_parity.py` on zustc's own sources; these are wall-clock times):

| point | definition | hover | completion | references | startup until indexed |
|---|---:|---:|---:|---:|---:|
| borrow checking added | 0.65 ms | 0.47 ms | 1.28 ms | 3.36 ms | 135 ms |
| compatibility round (03367bc) | 0.53 ms | 0.43 ms | 1.09 ms | 3.06 ms | 116 ms |
| faster prelude selection (3a1fd22) | 0.58 ms | 0.47 ms | 1.37 ms | 3.45 ms | 109 ms |
| async (e598fdf), same-machine comparison | 0.42 ms | 0.34 ms | 0.90 ms | 1.78 ms | 277 ms (two targets) |
| items inside blocks (cbcd294) | 0.59 ms | 0.49 ms | 1.26 ms | 3.52 ms | 109 ms |
| inference and borrowck of call arguments (8e797a9) | 0.55 ms | 0.45 ms | 1.15 ms | 3.39 ms | 115 ms |
| builtin macros (1501b30) | 0.53 ms | 0.42 ms | 1.21 ms | 3.04 ms | 108 ms |
| `impl Trait` in traits, more macros (8cb2f88) | 0.59 ms | 0.49 ms | 1.22 ms | 3.76 ms | 145 ms (same machine against 1501b30: 281 vs 285 ms, two targets) |
| `assert!` in type inference (4d210d2) | 0.63 ms | 0.51 ms | 1.32 ms | 3.41 ms | 111 ms |
| `assert_eq!` in type inference (441d0c2) | 0.61 ms | 0.47 ms | 1.27 ms | 3.49 ms | 114 ms |
| borrowing values in registers (fb2f2f2) | 0.57 ms | 0.48 ms | 1.19 ms | 3.74 ms | 110 ms |
| `try_from`, `::std` paths, more std functions (096a6bc) | 0.57 ms | 0.48 ms | 1.22 ms | 3.47 ms | 122 ms |
| prelude selection without per-build tables (f077e7c) | 0.55 ms | 0.45 ms | 1.30 ms | 3.26 ms | 111 ms |
| `dyn Trait<A>` (f606b34) | 0.51 ms | 0.43 ms | 1.17 ms | 3.39 ms | 109 ms |
| integer literals for bounds, `Box` comparisons (60cbe60) | 0.60 ms | 0.60 ms | 1.23 ms | 3.25 ms | 109 ms (same machine against f606b34: hover 0.32 vs 0.35 ms, ready 275 vs 287 ms) |
| `impl Trait` returning `()` (e7fdc21) | 0.94 ms | 0.56 ms | 1.30 ms | 11.57 ms | 115 ms (same machine against 60cbe60, 8 alternating runs: definition 0.55 vs 0.55 ms, references 2.34 vs 2.35 ms, ready 291 vs 299 ms) |
| std type names that user code may redefine (9bd5297) | 0.59 ms | 0.46 ms | 1.03 ms | 12.17 ms | 141 ms (3 more runs: 109, 112, 129 ms) |
| `Pin`, generic arguments on variants (e45ee10) | 0.78 ms | 0.56 ms | 1.33 ms | 11.28 ms | 113 ms |
| unsafe checks (E0133) (04b7d0b) | 0.64 ms | 0.50 ms | 1.30 ms | 11.99 ms | 113 ms |
| fn pointer comparisons, `main` as a value (b2620bd) | 0.65 ms | 0.54 ms | 1.18 ms | 11.34 ms | 114 ms |
| diverging `let`, Option in `for`, `2.eq(&3)` (5ecf037) | 0.61 ms | 0.50 ms | 1.12 ms | 12.27 ms | 108 ms |
| let chains, `&raw const` / `&raw mut` (d01898b) | 0.57 ms | 0.46 ms | 1.08 ms | 10.41 ms | 108 ms |
| three parallel lanes: parser, std, item summaries (9effaaa) | 0.94 ms | 0.77 ms | 1.65 ms | 15.79 ms | 141 ms (slower machine; against d01898b on it, 6 alternating runs: references 16.4 vs 14.4 ms, ready 140 vs 134 ms, but one LSP run under cachegrind is 10.953 G vs 10.955 G instructions: the code run is the same) |
| `impl Tr for Box<S>`, shifts and `reverse_bits`, `..b` values (962f2a6) | 0.93 ms | 0.77 ms | 1.84 ms | 15.16 ms | 128 ms |
| `as _` / `as *const _` (75630e2) | 0.98 ms | 0.89 ms | 1.86 ms | 16.81 ms | 130 ms |
| operator traits of numbers, `dyn Tr: Tr` (a6c8ba5) | 0.99 ms | 0.87 ms | 1.71 ms | 16.72 ms | 161 ms |
| bounds of associated types checked for `dyn Tr<Y = T>` (509b25d) | 0.88 ms | 0.80 ms | 1.63 ms | 17.57 ms | 152 ms |
| now: consts of generic impls | 0.94 ms | 0.76 ms | 1.60 ms | 16.12 ms | 138 ms |

Each change is kept only if these do not get worse. When an LSP time rises in a full run, the two builds are
compared again on the same machine, alternating runs, before it is called noise. The last row comes from such a
comparison, and its startup is the sum over two target files; the other rows are full runs of `bench/ci_bench.py`.

The [benchmark dashboard](https://mizchi.github.io/zust/dev/bench/) tracks these numbers for every commit to main.
They are measured on shared GitHub Actions runners, so times fluctuate. The instruction-count series show changes
more clearly.

### Coverage of rustc's test suite

zustc run against rustc's own UI tests (`tests/ui` of rust-lang/rust 1.100.0, `python3 bench/rust_ui.py`), counting
only the tests it can be asked to handle (no nightly features, auxiliary crates or special flags):

| kind | applicable | pass |
|---|---:|---:|
| run-pass (builds, runs, same output) | 2413 | 1536 (63.7%) |
| check-pass (accepted by `zustc check`) | 1807 | 1271 (70.3%) |
| build-pass | 135 | 89 (65.9%) |
| compile-fail (rejected at an annotated line) | 7358 | 2844 (38.7%) |

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
