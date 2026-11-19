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
> - zustc only targets Linux x86-64 and does not support all of Rust ([supported language](docs/language.md)).
> - zustc's checks (types, borrow checking without region errors) are partial, so it is not a replacement for rustc for checking correctness.
> - zust-analyzer implements only part of what rust-analyzer does.

## Status

Numbers measured <!-- measured -->2026-11-19, 54dbc31<!-- /measured --> by CI on each push to main (bench.yml runs
`bench/ci_bench.py` and `bench/rust_ui.py`, then `scripts/update_readme.py` writes them here). Times come from shared
GitHub Actions runners and vary by about ±20%; the instruction counts (valgrind) do not.

### Coverage of rustc's test suite

zustc run against rustc's own UI tests (`tests/ui` of rust-lang/rust 1.100.0, `python3 bench/rust_ui.py`), counting
only the tests it can be asked to handle (no nightly features, auxiliary crates or special flags):

<!-- auto:ui -->
| kind | applicable | pass |
|---|---:|---:|
| run-pass (builds, runs, same output) | 2385 | 1618 (67.8%) |
| check-pass (accepted by `zustc check`) | 1807 | 1366 (75.6%) |
| build-pass | 135 | 95 (70.4%) |
| compile-fail (rejected at an annotated line) | 7358 | 3309 (45.0%) |
<!-- /auto:ui -->

[What is supported](#what-is-supported) breaks this down by area.

### KPIs

Build speed, build cache size and LSP latency on generated programs of 1000 / 5000 functions (`bench/ci_bench.py`):

<!-- auto:kpi -->
| KPI | gen1000 | gen5000 |
|---|---:|---:|
| cold build | 81 ms | 341 ms |
| no-op build | 11.2 ms | 45.5 ms |
| build after editing one function | 16.4 ms | 52.0 ms |
| `check` | 67 ms | 317 ms |
| no-op build (instructions) | 141.1 M | 665.8 M |
| edit one function (instructions) | 134.1 M | 625.1 M |
| `check` (instructions) | 793.7 M | 3907.0 M |
| build cache on disk | 387.2 KiB | 1902.8 KiB |
| binary | 324.0 KiB | 1558.3 KiB |
| cold build peak RSS | 17.9 MiB | 52.0 MiB |

| zust-analyzer (zustc's own sources) | |
|---|---:|
| p50: definition, hover, completion, references, rename | 0.51 ms / 0.42 ms / 0.63 ms / 2.44 ms / 0.43 ms |
| startup until indexed | 90 ms |
| peak RSS | 71.3 MiB |
<!-- /auto:kpi -->

Every change is also measured on its own with `python3 bench/kpi_track.py`, so that small increases that add up stay
visible: [bench/kpi_log.md](bench/kpi_log.md) has one row per change with its effect (Δ) and the total since the
reference (Σ). The [benchmark dashboard](https://mizchi.github.io/zust/dev/bench/) has the CI numbers of every commit.

### Against rustc, relax and rust-analyzer (snapshots)

Measured with `python3 bench/vs_rustc.py` and `python3 cratebench/run.py` (2026-11-18, 4 cores, rustc 1.100.0; times are
medians of 3 runs). These comparisons are run by hand (they need rustc, relax and rust-analyzer side by side) and are dated. gen1000 is a generated program with 1000 functions:

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

Against blaze ([indexable-inc/blaze](https://github.com/indexable-inc/blaze), a replacement for cargo whose daemon
compiles only what an edit changed, with its own front end and rustc's language as its target): not run here yet,
so these are blaze's own published numbers (v1, 2026-10-25, Linux x86-64 on 16 cores of an AMD EPYC 9R45; its
[benchmarks](https://github.com/indexable-inc/blaze/blob/main/docs/benchmarks.md)) next to zustc's on this machine.
They are different programs on different machines: blaze builds real cargo projects that zustc cannot build yet
(ripgrep, nushell, bun), and zustc's numbers are a generated single-crate program:

| edit one function, then build | before | after |
|---|---:|---:|
| blaze: ripgrep (cargo → blaze) | 508 ms | 56 ms |
| blaze: helix (cargo → blaze) | 2.15 s | 116 ms |
| blaze: bun, 1.25M lines (cargo → blaze) | 9.11 s | 451 ms |
| zustc: gen1000 (rustc `-C incremental` → zustc) | 406 ms | 27 ms |

blaze's daemon holds a few GB for a small project (16 GB after a ripgrep run, by its own ledger) where zustc's cache
for gen1000 is 766 KiB on disk and nothing stays in memory between builds. Its way of tracking its numbers (each
merge's instruction counts against the median of the last runs, with a noise band) is what `bench/kpi_track.py`
now does too. A run of both on the same machine and the same program (gen1000 as a cargo project) is the next step.

zust-analyzer against rust-analyzer and rust-glancer on zustc's own sources (40 query points,
`python3 bench/lsp_parity.py --servers zustc,rust-analyzer,rust-glancer --targets zustc`, 2026-11-18, e45ee10;
rust-glancer is github.com/rust-glancer/rust-glancer 50dd51a):

| | startup until symbols resolve | memory | references p50 | agreement (mean over the queries) |
|---|---:|---:|---:|---:|
| zust-analyzer | 113 ms | 71 MB | 10.9 ms | 99.0% |
| rust-analyzer | 6118 ms | 2060 MB | 214.0 ms | 98.9% |
| rust-glancer | 9965 ms | 1573 MB | 548.8 ms | 58.5% |

Details are in [bench/lsp_parity_result.md](bench/lsp_parity_result.md).

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

Known differences from rust-analyzer are listed under [Roadmap](#roadmap).

## What is supported

The percentages are rustc's own UI tests that pass in that area: run-pass + check-pass + build-pass of the tests/ui
directories named in the comment next to the number (`ui:`), or compile-fail (`cf:`). CI rewrites them with the rest of
the measured numbers. The [reports](docs/reports/) explain each step.

- `[x]` works for ordinary code (most tests of the area pass)
- `[ ]` partial or missing; the note says what is left

### Overall

- [x] tests/ui run-pass: <!-- total:run-pass -->67.8% (1618 / 2385)
- [x] tests/ui check-pass: <!-- total:check-pass -->75.6% (1366 / 1807)
- [x] tests/ui build-pass: <!-- total:build-pass -->70.4% (95 / 135)
- [ ] tests/ui compile-fail (an error on an annotated line): <!-- total:compile-fail -->45.0% (3309 / 7358)
- [x] real crates: fnv, adler2, hex, itoa, rustc-hash and cfg-if build and give the same output (6 / 6)
- [x] zust-analyzer answers like rust-analyzer: 95.4% (rust-analyzer: 96.2%)

### Language

- [x] numbers, arithmetic, overflow checks: <!-- ui:numbers-arithmetic -->93.1%
- [x] `for` / `while` / `loop`, labels: <!-- ui:for-loop-while,loops,label -->91.7%
- [x] enums, `#[repr(u8)]`, auto-sized tags: <!-- ui:enum,enum-discriminant -->83.3%
- [x] `cfg`: <!-- ui:cfg -->81.2%
- [x] bindings, patterns, slice patterns, or-patterns (also with guards): <!-- ui:binding,pattern,or-patterns -->85.4%
- [x] `match`: <!-- ui:match -->80.5%
- [x] name resolution: <!-- ui:resolve -->82.9%
- [x] statics: ints, floats, raw pointers, fn pointers, `const fn` calls: <!-- ui:statics -->75.0%
- [x] structs, `..base` (field by field): <!-- ui:structs,structs-enums -->80.9%
- [x] type aliases and paths (`impl foo::Point`) as impl self types, discriminants from constants and earlier variants, where clauses on concrete types
- [x] syntax: GAT bindings, `dyn for<'a> Fn`, 2015 anonymous parameters, raw lifetimes `'r#a`, half-open range patterns, shebangs, Unicode white space; `macro_rules!` fragments matched by kind (`ident`, `literal`, `lifetime`, `vis`)
- [x] methods of `impl Tr for Box<S>` (a Box impl next to a blanket impl of the same trait is E0119), `..b` / `..=b` / `..` values, `wrapping_shl` / `checked_shl` / `overflowing_shl` / `reverse_bits`, `wrapping_abs` / `saturating_abs` / `saturating_neg` / `overflowing_neg` / `overflowing_abs`
- [x] qualified paths: `<T as Tr>::C` for a trait's default constant (its `Self::` is the qualified type), `<() as Tr>::f()`, associated functions through a type alias (`type Ham = m::Qux; Ham::new()`)
- [x] let chains (`if let A = x && cond`), also in match guards (`P if let Q = e =>`); inline `const { .. }`, `&raw const` / `&raw mut`
- [x] parser: <!-- ui:parser -->88.9%
- [x] closures (capture, `Fn` traits, `move`; with no captures also as fn pointers): <!-- ui:closures,unboxed-closures,functions-closures -->76.6%
- [x] derives: `Debug`, `Clone`, `Copy`, `PartialEq`/`Eq`, `PartialOrd`/`Ord`, `Hash`, `Default`: <!-- ui:derives,deriving -->79.5%
- [x] imports, `use`: <!-- ui:imports -->77.3%
- [x] associated types: <!-- ui:associated-types -->70.6%
- [x] generics: <!-- ui:generics -->76.2% (an integer literal for `T: Foo` takes the one integer type that implements Foo; a type
  parameter known only from the expected type is not inferred)
- [ ] consts: <!-- ui:consts -->71.1% (no heap values, no generic `const fn`; a const of a generic impl (`const C: Option<T> = None`) takes its type at each use)
- [ ] drop: <!-- ui:drop -->58.0% (the drop order of some temporaries differs)
- [ ] traits: <!-- ui:traits -->64.7% (generic traits, blanket impls, `dyn` (also `dyn Tr<A>`), `impl .. for dyn Tr` work; `impl<T> Tr for &T` only without functions; no coherence)
- [ ] `macro_rules!`: <!-- ui:macros -->66.4% (hygiene for `let`, labels, arm bindings; not every fragment kind)
- [ ] unions: <!-- ui:union -->57.7% (no `repr(packed)`)
- [ ] const generics: <!-- ui:const-generics -->51.5% (no `generic_const_exprs`)
- [ ] generic associated types: <!-- ui:generic-associated-types -->46.7%
- [ ] casts (`as _` / `as *const _` take the source or expected type; a pointer of unknown kind is E0641): <!-- ui:cast -->44.8%
- [ ] `impl Trait`: <!-- ui:impl-trait -->62.7% (return position, also nested (`-> Option<impl Debug>`), in trait methods and generic fns (typed per instance); no TAIT)
- [ ] layout: `repr(packed)` / `repr(align)` are ignored: <!-- ui:packed,layout -->38.7%

### Checks (compile-fail)

- [x] borrow checking: moves, conflicting loans, escapes, mutability: borrowck <!-- cf:borrowck -->54.5%
  ([report 08](docs/reports/2026-11-18-08-borrow-checking.md))
- [x] type errors, missing items, generics declarations (E0403, E0207, E0392, E0072 ..): typeck <!-- cf:typeck -->67.2%, structs <!-- cf:structs -->70.8%
- [x] unsafe operations outside `unsafe` (E0133): calls to `unsafe fn` and extern fns, raw pointer derefs, `static mut`,
  union field reads (not yet inside closures or consts, and not the `unsafe_op_in_unsafe_fn` lint)
- [x] non-exhaustive `match` (E0004): bool, unit, enums (variants that cannot have a value need no arm), structs and
  tuples, nested and with or-patterns (integer and slice patterns, and types left generic, are not checked yet)
- [x] impl methods against the trait's declaration: parameter count (E0050), kind of receiver (E0053)
- [x] unreachable match arms under `#![deny(unreachable_patterns)]` (the same usefulness check, arm by arm);
  array patterns of the wrong length (E0527, E0528)
- [ ] region errors against signatures (E0621, E0521): regions <!-- cf:regions -->12.1%, lifetimes <!-- cf:lifetimes -->19.5%
- [ ] coherence, bounds of blanket impls, dyn compatibility (E0038), the bindings of `dyn Tr<A = T>` against the associated type's bounds (done when T is a primitive, `str` or a struct name)
- [ ] lints and warnings: lint <!-- cf:lint -->11.7%
- [ ] editions (2015-only errors)

### Runtime and std

- [x] the prelude (zustc's own small std): `Option`/`Result`, `Vec`, `String`, `HashMap`/`HashSet`,
  `BTreeMap`/`BTreeSet`, `VecDeque`, `BinaryHeap`, `Rc`/`Arc`, `Cell`/`RefCell`, `Mutex`, `OnceLock`, `ManuallyDrop`,
  `Duration`, `AtomicPtr`, `RangeTo` / `RangeFull` as types, `Fuse`, `iter::Empty`, `NonNull`, `Pin` (a plain pointer: `Pin::new`, `get_mut`, `into_inner`, deref), `std::task` (`Poll`, `Context`, `Waker::noop`) and the `Future` trait for futures implemented and polled by hand, `UnsafeCell`, `PhantomData`, atomics, `mpsc`, `mem::*`, `ptr::{read, write, drop_in_place, null}`,
  `iter::{repeat, once, empty, repeat_with, successors}`, `reverse` / `fill` / `rotate_left` / `rotate_right` on arrays and slices, formatting, stdin/stdout/fs/env/process basics
- [ ] async / await: <!-- ui:async-await -->74.8%. It runs at once, with no executor (no `poll`, `Waker`, `Pin<Box<dyn Future>>`)
- [ ] threads: <!-- ui:threads-sendsync -->66.7%. One thread: `thread::spawn` runs the closure at once
- [ ] iterators: <!-- ui:iterators -->41.2% (`iter::empty()` cannot infer its item from the target)
- [ ] FFI: <!-- ui:abi -->14.3%. Scalars only, no structs by value
- [ ] panics: <!-- ui:panics -->11.1%. A panic aborts: no unwinding, `catch_unwind`, `set_hook`

### In sight (known approach, small cost)

- [ ] type parameters: `let v: Vec<Box<_>> = ..` (holes in an annotation) and generic fns passed as values
  (`.map(drop)`, `.map(Result::unwrap)`) now infer; `iter::empty().collect()`, `let f = id;` and const parameters
  inferred from patterns do not (~35 tests)
- [ ] items inside blocks: `mod`, `trait`, `impl`, `type`, `union` and `pub` items in a fn body now move out; items a
  macro expands to in statement position move out too; from a macro in expression position not yet (~12 tests)
- [ ] builtin macros: `offset_of!`, `addr_of!` / `addr_of_mut!`, `try!`, `stringify!`, `concat!`, `line!`, `column!`,
  `option_env!`, `assert_matches!`, `format_args!` now work; `module_path!`, `file!`, `include!`, `asm!` do not (~8 tests)
- [ ] `repr(packed)` / `repr(align)`: ~16 tests
- [ ] `Pin`: `Pin<&T>` / `Pin<&mut T>` work as the pointer; `Box::pin(fut)` and `Pin<Box<dyn Future<Output = T>>>` work (the future's value, as async runs at once); `self: Pin<&mut Self>` and
  `self: Box<Self>` receivers work; `pin!` does not
- [ ] dyn compatibility (E0038), generic argument counts of traits
- [ ] closures that capture `&mut` are not `Copy`

### Expected to cost performance (measured before kept)

- [ ] region checking against signatures: `check` time (borrowck costs +2.2% today)
- [ ] coherence and bounds of blanket impls: `check` time, LSP
- [ ] real async (state machines, `poll`, an executor): build time, binary size
- [ ] unwinding (landing pads): build time, binary size
- [ ] field-precise closure captures in codegen: binary size
- [ ] a larger prelude: no-op build and cache size (the prelude, ~11k lines, is scanned on every build)
- [ ] real threads (a pthread runtime): runtime size

### Large features not started

- [ ] real concurrency (parallel threads, a blocking `Mutex`)
- [ ] executors: `Future::poll`, `Waker`, `Pin<Box<dyn Future>>` (~25 tests)
- [ ] unwinding: `catch_unwind`, panic hooks (~22 tests)
- [ ] FFI: structs by value, other ABIs (~27 tests)
- [ ] full region / lifetime checking
- [ ] a full trait solver: specialization, auto traits through `dyn` / `async`, higher-ranked bounds
- [ ] lints and warnings
- [ ] editions
- [ ] SIMD, inline asm
- proc macros: not applicable (they are separate crates)

## Roadmap

### Compiler (zustc)

- **rustc's tests/ui** (`python3 bench/rust_ui.py`; [report](docs/reports/2026-11-18-07-rustc-ui-tests.md))
  - Raise the pass rates ([Status](#status))
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

## Documentation

- [docs/language.md](docs/language.md): the subset of Rust that zustc supports, and what it does not
- [docs/architecture.md](docs/architecture.md): crate layout, the pipeline, how incremental builds work, value representation
- [docs/reports/](docs/reports/README.md): work log, as dated reports (benchmark results and what was done at each step)
- [CONTRIBUTING.md](CONTRIBUTING.md): how to develop: build, test, benchmark, and write reports

## License

Like Rust itself, zust is dual-licensed; you may use it under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this repository, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

The crates in `cratebench/vendor/` (git submodules used by the benchmarks) are under their own upstream licenses.
