# What zustc supports

Status as of 2026-11-18, updated with each round of work. The percentages are rustc's own UI tests that pass in that area
(`python3 bench/rust_ui.py`, rust-lang/rust tests/ui at 1.100.0; run-pass + check-pass + build-pass of the directory).
[TODO.md](TODO.md) has the work list; the [reports](docs/reports/) explain each step.

- `[x]` works for ordinary code (most tests of the area pass)
- `[ ]` partial or missing; the note says what is left

## Overall

- [x] tests/ui run-pass: 58.9% (1421 / 2413)
- [x] tests/ui check-pass: 62.7% (1133 / 1807)
- [x] tests/ui build-pass: 60.7% (82 / 135)
- [ ] tests/ui compile-fail (an error on an annotated line): 38.3% (2816 / 7358)
- [x] real crates: fnv, adler2, hex, itoa, rustc-hash and cfg-if build and give the same output (6 / 6)
- [x] zust-analyzer answers like rust-analyzer: 95.4% (rust-analyzer: 96.2%)

## Language

- [x] numbers, arithmetic, overflow checks: 89.7%
- [x] `for` / `while` / `loop`, labels: 88.7%
- [x] enums, `#[repr(u8)]`, auto-sized tags: 82.8%
- [x] `cfg`: 81.2%
- [x] bindings, patterns, slice patterns, or-patterns (also with guards): 72–81%
- [x] `match`: 78.0%
- [x] name resolution: 77.1%
- [x] statics: ints, floats, raw pointers, fn pointers, `const fn` calls: 75.0%
- [x] structs, `..base` (field by field): 74.2%
- [x] parser: 73.7%
- [x] closures (capture, `Fn` traits, `move`): 71.7%
- [x] derives: `Debug`, `Clone`, `Copy`, `PartialEq`/`Eq`, `PartialOrd`/`Ord`, `Hash`, `Default`: 68.2%
- [x] imports, `use`: 67.0%
- [x] associated types: 63.6%
- [x] generics: 61.9% (a type parameter known only from the expected type is not inferred)
- [ ] consts: 54.9% (no heap values, no generic `const fn`)
- [ ] drop: 53.6% (the drop order of some temporaries differs)
- [ ] traits: 51.0% (generic traits, blanket impls, `dyn`, `impl .. for dyn Tr` work; no coherence)
- [ ] `macro_rules!`: 47.1% (hygiene for `let`, labels, arm bindings; not every fragment kind)
- [ ] unions: 46.2% (no `repr(packed)`)
- [ ] const generics: 43.6% (no `generic_const_exprs`)
- [ ] generic associated types: 40.0%
- [ ] casts: 34.5%
- [ ] `impl Trait`: 29.7% (return position, also in trait methods; not from generic fns, not nested, no TAIT)
- [ ] layout: `repr(packed)` / `repr(align)` are ignored: 12–33%

## Checks (compile-fail)

- [x] borrow checking: moves, conflicting loans, escapes, mutability: borrowck 49.4%
  ([report 08](docs/reports/2026-11-18-08-borrow-checking.md))
- [x] type errors, missing items, generics declarations (E0403, E0207, E0392, E0072 ..): typeck 53.4%, structs 66.7%
- [ ] region errors against signatures (E0621, E0521): regions 12.9%, lifetimes 19.5%
- [ ] coherence, bounds of blanket impls, dyn compatibility (E0038)
- [ ] lints and warnings: lint 8.3%
- [ ] editions (2015-only errors)

## Runtime and std

- [x] the prelude (zustc's own small std): `Option`/`Result`, `Vec`, `String`, `HashMap`/`HashSet`,
  `BTreeMap`/`BTreeSet`, `VecDeque`, `BinaryHeap`, `Rc`/`Arc`, `Cell`/`RefCell`, `Mutex`, `OnceLock`, `ManuallyDrop`,
  `NonNull`, `UnsafeCell`, `PhantomData`, atomics, `mpsc`, `mem::*`, `ptr::{read, write, drop_in_place, null}`,
  `iter::{repeat, once, empty, repeat_with, successors}`, formatting, stdin/stdout/fs/env/process basics
- [ ] async / await: 64.1%. It runs at once, with no executor (no `poll`, `Waker`, `Pin<Box<dyn Future>>`)
- [ ] threads: 61.4%. One thread: `thread::spawn` runs the closure at once
- [ ] iterators: 35.3% (`iter::empty()` cannot infer its item from the target)
- [ ] FFI: 14.3%. Scalars only, no structs by value
- [ ] panics: 11.1%. A panic aborts: no unwinding, `catch_unwind`, `set_hook`

## In sight (known approach, small cost)

- [ ] type parameters: `let v: Vec<Box<_>> = ..` (holes in an annotation) and generic fns passed as values
  (`.map(drop)`, `.map(Result::unwrap)`) now infer; `iter::empty().collect()`, `let f = id;` and const parameters
  inferred from patterns do not (~35 tests)
- [ ] items inside blocks: `mod`, `trait`, `impl`, `type`, `union` and `pub` items in a fn body now move out; items that
  macro expansion produces inside a body do not yet (~30 tests)
- [ ] builtin macros: `offset_of!`, `addr_of!` / `addr_of_mut!`, `try!`, `stringify!`, `concat!`, `line!`, `column!`,
  `option_env!`, `assert_matches!`, `format_args!` now work; `module_path!`, `file!`, `include!`, `asm!` do not (~8 tests)
- [ ] `impl Trait` returned by generic functions: ~21 tests
- [ ] `repr(packed)` / `repr(align)`: ~16 tests
- [ ] `Pin` without an executor (`Pin<&mut T>`, `Box::pin`)
- [ ] dyn compatibility (E0038), generic argument counts of traits
- [ ] closures that capture `&mut` are not `Copy`

## Expected to cost performance (measured before kept)

- [ ] region checking against signatures: `check` time (borrowck costs +2.2% today)
- [ ] coherence and bounds of blanket impls: `check` time, LSP
- [ ] real async (state machines, `poll`, an executor): build time, binary size
- [ ] unwinding (landing pads): build time, binary size
- [ ] field-precise closure captures in codegen: binary size
- [ ] a larger prelude: no-op build and cache size (the prelude, ~11k lines, is scanned on every build)
- [ ] real threads (a pthread runtime): runtime size

## Large features not started

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

## Speed against rustc (2026-11-18)

- [x] cold build: 7.0x faster than rustc `-C incremental` on gen1000, 2.8–4.3x on real crates
- [x] edit and rebuild: 15x faster on gen1000, 12–18x on small programs, 4.7–7.9x on real crates
- [x] build cache on disk: 14x smaller (766 KiB vs 10.8 MiB on gen1000)
- [x] binary: 3.2x smaller on gen1000, 90–550x on real crates
- [ ] run time: zustc is 1.1–4.1x slower than rustc `-O` on small programs (faster than rustc's debug build);
  zustc `-O` 1.1–3.2x

The tables are in the [README](README.md#against-rustc).

## KPIs now

| KPI | now | before this work |
|---|---:|---:|
| gen5000 no-op build | 663.8 M instr | 746.0 M |
| gen5000 build after editing one function | 620.8 M instr | 709.3 M |
| gen5000 `check` | 3892.9 M instr | 3971.0 M |
| gen5000 build cache | 1902.8 KiB | 1902.8 KiB |
| LSP definition / hover / completion / references p50 | 0.42 / 0.34 / 0.90 / 1.78 ms | — |
| LSP ready (startup to indexed, zustc + ide targets) | 277 ms | — |
