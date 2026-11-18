# What zustc supports

Status as of 2026-11-18 (commit e598fdf). The numbers come from rustc's own UI tests
(`python3 bench/rust_ui.py`, rust-lang/rust tests/ui at 1.100.0) and from the KPI benchmarks (`bench/ci_bench.py`).
[TODO.md](TODO.md) has the work list; the [reports](docs/reports/) explain each step.

## Summary

| measure | value |
|---|---|
| tests/ui run-pass (compiles, runs, exits 0) | **56.5%** (1364 / 2413) |
| tests/ui check-pass (type checks) | **60.8%** (1098 / 1807) |
| tests/ui build-pass | **60.0%** (81 / 135) |
| tests/ui compile-fail (an error on an annotated line) | **38.2%** (2808 / 7358) |
| real crates (cratebench: fnv, adler2, hex, itoa, rustc-hash, cfg-if) | 6 / 6 build and give the same output |
| zust-analyzer agreement with rust-analyzer (bench/lsp_parity.py) | 95.4% (rust-analyzer: 96.2%) |

Tests that need things zustc does not try to do are not counted ("not applicable": other targets, `aux-build`,
proc-macro crates, `-Z` flags and so on).

## Features by area

The percentages are tests/ui pass rates of the directory (run-pass + check-pass + build-pass together; compile-fail
separately), so they measure how much real Rust of that area works, not a checklist.

| area | passing (run/check/build) | compile-fail | state |
|---|---:|---:|---|
| numbers, arithmetic | 89.7% | 66.7% | done |
| `for` / `while` / `loop` | 88.7% | 37.5% | done |
| enums | 82.8% | 45.8% | done; `#[repr(u8)]` and auto-sized tags |
| `cfg` | 81.2% | 41.7% | done |
| bindings, patterns, or-patterns | 72–81% | 20–40% | mostly; slice patterns, guards with or-patterns |
| `match` | 78.0% | 15.6% | mostly |
| regions (lifetimes in signatures) | 78.4% | 12.9% | accepted, not checked (see "Not started") |
| name resolution (`resolve`) | 77.1% | 46.3% | mostly |
| statics / consts | 75.0% / 54.9% | 11.1% / 9.8% | ints, floats, raw pointers, fn pointers, `const fn` calls; not heap values, not generic const fns |
| structs | 74.2% | 66.7% | done; FRU (`..base`) moves field by field |
| borrow checking | 73.9% | 49.4% | NLL-like loans + Polonius-style liveness ([report 08](docs/reports/2026-11-18-08-borrow-checking.md)); no region errors yet |
| parser | 73.7% | 64.1% | mostly |
| closures | 71.7% | 33.9% | mostly; captures by place (RFC 2229) for borrowck, by variable for codegen |
| derives | 68.2% | 22.2% | `Debug`, `Clone`, `Copy`, `PartialEq`/`Eq`, `PartialOrd`/`Ord` (NaN-aware), `Hash`, `Default` |
| imports | 67.0% | 9.7% | mostly |
| async / await | 64.1% | 31.9% | **run at once** (no executor): `async` blocks/fns/closures, `.await`, `impl Future<Output = T>` |
| associated types | 63.6% | 28.1% | mostly |
| lints | 63.4% | 8.3% | warnings are not reported (compile-fail lints need them) |
| generics | 61.9% | 46.3% | mostly; inference of a type parameter from the expected type is partial |
| threads, Send/Sync | 61.4% | — | **one thread**: `thread::spawn` runs the closure at once; `mpsc`, `Arc`, atomics |
| array / slice / Vec | 58.6% | 20.0% | mostly |
| drop | 53.6% | 53.8% | drop flags, temporaries; drop order of some temporaries differs |
| traits | 51.0% | 31.8% | generic traits, blanket impls, `dyn` (vtables), `impl .. for dyn Tr`; no coherence checks |
| macros (`macro_rules!`) | 47.1% | 22.2% | hygiene for `let`, labels and arm bindings; not every fragment kind |
| unions | 46.2% | 26.7% | layout and literals; no `repr(packed)` |
| const generics | 43.6% | 52.7% | values and array lengths; no `generic_const_exprs` |
| generic associated types | 40.0% | 29.5% | partial |
| iterators | 35.3% | 36.8% | std adapters in the prelude; `iter::empty()` cannot infer its item from the target |
| cast | 34.5% | 56.2% | partial |
| `impl Trait` | 29.7% | 31.3% | in return position; not in generic fns / nested / TAIT |
| layout, `repr(packed)` | 12–33% | 0–23% | `repr(packed)` and `repr(align)` are ignored |
| FFI (`extern "C"`) | 14.3% (abi) | — | scalars only: structs by value are not passed |
| panics, unwinding | 11.1% | — | panic aborts the process: no `catch_unwind`, no `set_hook` |

The prelude (zustc's own small std, `crates/syntax/src/prelude_lib.rs.in`) has `Option`/`Result`, `Vec`, `String`,
`HashMap`/`HashSet`, `BTreeMap`/`BTreeSet`, `VecDeque`, `BinaryHeap`, `Rc`/`Arc`, `Cell`/`RefCell`, `Mutex`,
`OnceLock`, `ManuallyDrop`, `NonNull`, `UnsafeCell`, `PhantomData`, the atomics, `mpsc`, `mem::{swap, replace, take,
transmute, forget, size_of_val}`, `ptr::{read, write, drop_in_place, null}`, `iter::{repeat, once, empty,
repeat_with, successors}`, formatting and stdin/stdout/fs/env/process basics. Each program gets only the items it
names.

## In sight (known approach, small cost)

| item | tests it blocks (failing run/check/build-pass) | approach |
|---|---:|---|
| type parameters inferred only from the expected type (`iter::empty().collect()`) | ~44 | solve the call's parameters from the expected return type too |
| items inside blocks the preprocessor does not move | ~39 | move them out like the other block items |
| macros zustc does not know (`concat_idents!`, `stringify!` corners, `matches!` corners ..) | ~36 | per macro |
| `impl Trait` returned by generic functions | ~21 | instantiate the opaque type per instance |
| `Pin` without an executor (`Pin<&mut T>`, `Box::pin`) | ~3 (+ the async ones) | a transparent wrapper in the prelude |
| E0038 (dyn compatibility), E0592/E0201 positions, trait generic argument counts | compile-fail | checks on the summary |
| closures that capture `&mut` are not `Copy` | compile-fail | capture mode in the closure type |
| `repr(packed)` / `repr(align)` | ~16 | layout options per struct |

## Expected to cost performance

These are worth doing for compatibility but will make a KPI worse; each would be measured before it is kept.

| item | KPI it hits | why |
|---|---|---|
| region checking against signatures (E0621, E0521, "may not live long enough") | `check` time (borrowck is +2.2% today) | universal regions and outlives constraints on every function |
| coherence and bound checking of blanket impls | `check` time, LSP | every impl against every other impl of the trait |
| a real async (state machines, `Future::poll`, wakers) | build time, binary size | a coroutine transform per async fn; an executor in the prelude |
| field-precise closure captures in codegen | binary size | one environment field per captured place |
| unwinding (`catch_unwind`, drop on panic) | binary size, build time | landing pads in every function with drops |
| a larger prelude (more of std) | no-op build, cache size | the prelude is scanned and selected on every build (now ~11k lines) |
| real threads | runtime library size | a pthread-based runtime instead of the one-thread shims |

## Large features not started

| feature | tests it blocks | note |
|---|---:|---|
| real concurrency (threads that run in parallel, `Mutex` that blocks) | threads-sendsync 39% failing | today one thread, closures run at once |
| executors, `Future::poll`, `Waker`, `Pin<Box<dyn Future>>` | ~25 | async runs eagerly |
| unwinding, `catch_unwind`, panic hooks | ~22 | panics abort |
| proc macros | not applicable here (separate crates) | |
| lints and warnings | lint compile-fail 8.3% | no warning machinery |
| FFI: structs by value, other ABIs | ~27 | C ABI for scalars only |
| full region / lifetime checking | regions compile-fail 12.9%, lifetimes 19.5% | lifetimes are erased after parsing |
| full trait solver (specialization, auto traits through `dyn`/`async`, higher-ranked bounds) | traits compile-fail 31.8% | |
| editions | — | no edition setting: 2015-only errors are not given |
| SIMD, inline asm | few | |

## KPIs now

| KPI | value |
|---|---|
| gen5000 no-op build | 661.9 M instructions (746.0 M before this work) |
| gen5000 build after editing one function | 622.6 M instructions (709.3 M) |
| gen5000 `check` | 3892.9 M instructions (3971.0 M) |
| gen5000 build cache | 1902.8 KiB (unchanged) |
| LSP definition / hover / completion / references p50 | 0.42 / 0.34 / 0.90 / 1.78 ms (same-machine median) |
| LSP ready (startup to indexed, zustc + ide targets) | 277 ms |
