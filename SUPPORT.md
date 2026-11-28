# What is supported

The percentages are rustc's own UI tests that pass in that area: run-pass + check-pass + build-pass of the tests/ui
directories named in the comment next to the number (`ui:`), or compile-fail (`cf:`). They count only the tests
`bench/rust_ui.py` can ask zustc to handle (no nightly features, auxiliary crates, special flags or targets other than
Linux x86-64), so 100% does not mean every test of a directory passes. They were measured
<!-- measured -->2026-11-28, ef3469e<!-- /measured --> by CI on each push to main (bench.yml runs `bench/rust_ui.py`, then
`scripts/update_readme.py` writes them here). The totals for each kind of test are in the
[README](README.md#coverage-of-rustcs-test-suite), the subset of the language in prose is in
[docs/language.md](docs/language.md), and the [reports](docs/reports/) explain each step.

- `[x]` works for ordinary code (most tests of the area pass)
- `[ ]` partial or missing; the note says what is left

## Overall

- [x] tests/ui: the [table in the README](README.md#coverage-of-rustcs-test-suite)
- [x] real crates: fnv, adler2, hex, itoa, rustc-hash and cfg-if build and give the same output (6 / 6)
- [ ] crates.io's top crates, by category: [CRATES_SUPPORT.md](CRATES_SUPPORT.md) (written by CI from
  `cratebench/survey/survey.py`) and the [dashboard](https://mizchi.github.io/zust/crates/)
- [x] zust-analyzer answers like rust-analyzer: 99.0% agreement on zustc's own sources (rust-analyzer: 98.9%). This is
  a snapshot measured by hand (2026-11-18), not by CI:
  [PERFORMANCE.md](PERFORMANCE.md#against-rustc-relax-and-rust-analyzer-snapshots)

## Language

- [x] numbers, arithmetic, overflow checks: <!-- ui:numbers-arithmetic -->100.0%
- [x] `for` / `while` / `loop`, labels: <!-- ui:for-loop-while,loops,label -->96.7%
- [x] enums, `#[repr(u8)]`, auto-sized tags: <!-- ui:enum,enum-discriminant -->88.1%
- [x] `cfg`: <!-- ui:cfg -->94.1%
- [x] bindings, patterns, slice patterns, or-patterns (also with guards): <!-- ui:binding,pattern,or-patterns -->91.9%
- [x] `match`: <!-- ui:match -->87.8%
- [x] name resolution: <!-- ui:resolve -->88.6%
- [x] statics: ints, floats, raw pointers, fn pointers, `const fn` calls: <!-- ui:statics -->95.0%
- [x] structs, `..base` (field by field): <!-- ui:structs,structs-enums -->88.8%
- [x] type aliases and paths (`impl foo::Point`) as impl self types, discriminants from constants and earlier variants, where clauses on concrete types
- [x] syntax: GAT bindings, `dyn for<'a> Fn`, 2015 anonymous parameters, raw lifetimes `'r#a`, half-open range patterns, shebangs, Unicode white space; `macro_rules!` fragments matched by kind (`ident`, `literal`, `lifetime`, `vis`)
- [x] methods of `impl Tr for Box<S>` (a Box impl next to a blanket impl of the same trait is E0119), `..b` / `..=b` / `..` values, `wrapping_shl` / `checked_shl` / `overflowing_shl` / `reverse_bits`, `wrapping_abs` / `saturating_abs` / `saturating_neg` / `overflowing_neg` / `overflowing_abs`
- [x] qualified paths: `<T as Tr>::C` for a trait's default constant (its `Self::` is the qualified type), `<() as Tr>::f()`, associated functions through a type alias (`type Ham = m::Qux; Ham::new()`)
- [x] let chains (`if let A = x && cond`), also in match guards (`P if let Q = e =>`); inline `const { .. }`, `&raw const` / `&raw mut`
- [x] parser: <!-- ui:parser -->91.9%
- [x] closures (capture, `Fn` traits, `move`; with no captures also as fn pointers): <!-- ui:closures,unboxed-closures,functions-closures -->84.4%
- [x] derives: `Debug`, `Clone`, `Copy`, `PartialEq`/`Eq`, `PartialOrd`/`Ord`, `Hash`, `Default`: <!-- ui:derives,deriving -->90.9%
- [x] imports, `use`: <!-- ui:imports -->92.0%
- [x] associated types: <!-- ui:associated-types -->84.6%
- [x] generics: <!-- ui:generics -->95.2% (an integer literal for `T: Foo` takes the one integer type that implements Foo; a type
  parameter known only from the expected type is not inferred)
- [ ] consts: <!-- ui:consts -->79.5% (no heap values, no generic `const fn`; a const of a generic impl (`const C: Option<T> = None`) takes its type at each use)
- [ ] drop: <!-- ui:drop -->76.8% (the drop order of some temporaries differs)
- [ ] traits: <!-- ui:traits -->74.7% (generic traits, blanket impls, `dyn` (also `dyn Tr<A>`), `impl .. for dyn Tr` work; `impl<T> Tr for &T` only without functions; no coherence)
- [ ] `macro_rules!`: <!-- ui:macros -->80.7% (hygiene for `let`, labels, arm bindings; not every fragment kind)
- [ ] unions: <!-- ui:union -->84.6% (no `repr(packed)`)
- [ ] const generics: <!-- ui:const-generics -->68.3% (no `generic_const_exprs`)
- [ ] generic associated types: <!-- ui:generic-associated-types -->86.7%
- [ ] casts (`as _` / `as *const _` take the source or expected type; a pointer of unknown kind is E0641): <!-- ui:cast -->75.9%
- [ ] `impl Trait`: <!-- ui:impl-trait -->77.1% (return position, also nested (`-> Option<impl Debug>`), in trait methods and generic fns (typed per instance); no TAIT)
- [ ] layout: `repr(packed)` / `repr(align)` are ignored: <!-- ui:packed,layout -->64.5%

## Checks (compile-fail)

- [x] borrow checking: moves, conflicting loans, escapes, mutability: borrowck <!-- cf:borrowck -->91.8%
  ([report 08](docs/reports/2026-11-18-08-borrow-checking.md))
- [x] type errors, missing items, generics declarations (E0403, E0207, E0392, E0072 ..): typeck <!-- cf:typeck -->79.9%, structs <!-- cf:structs -->95.8%
- [x] unsafe operations outside `unsafe` (E0133): calls to `unsafe fn` and extern fns, raw pointer derefs, `static mut`,
  union field reads (not yet inside closures or consts, and not the `unsafe_op_in_unsafe_fn` lint)
- [x] non-exhaustive `match` (E0004): bool, unit, enums (variants that cannot have a value need no arm), structs and
  tuples, nested and with or-patterns (integer and slice patterns, and types left generic, are not checked yet)
- [x] impl methods against the trait's declaration: parameter count (E0050), kind of receiver (E0053)
- [x] unreachable match arms under `#![deny(unreachable_patterns)]` (the same usefulness check, arm by arm);
  array patterns of the wrong length (E0527, E0528)
- [ ] region errors against signatures ("lifetime may not live long enough", E0621): the universal regions of a fn's
  signature against the constraints its body needs (calls, `let`, assignments, struct / variant values, patterns,
  coercions to trait objects), borrows of locals and temporaries that must outlive the fn (E0597, E0716, E0515),
  closure bodies against the bound they are passed to, trait object lifetimes, well-formed written types (E0309,
  E0310, E0491), undeclared / shadowed / unconstrained lifetimes (E0261, E0496, E0207), missing ones in fn pointer
  types (E0106), `'_` where it cannot be used (E0637), `unused_lifetimes` / `single_use_lifetimes`; borrows that
  must outlive the fn live to its end, closures that are not general enough or let borrowed data escape, generic fns
  that are never called (checked with placeholder types). Not yet: what a closure's body requires of its fn,
  higher-ranked trait selection, the requirements of the impl a coercion selects, the prelude's iterators (their types
  hold no lifetimes). regions <!-- cf:regions -->91.4%, lifetimes <!-- cf:lifetimes -->86.7%
  ([issue #1](https://github.com/mizchi/zust/issues/1))
- [ ] coherence, bounds of blanket impls, dyn compatibility (E0038), the bindings of `dyn Tr<A = T>` against the associated type's bounds (done when T is a primitive, `str` or a struct name)
- [ ] lints and warnings: lint <!-- cf:lint -->51.0%
- [ ] editions (2015-only errors)

## Runtime and std

- [x] the prelude (zustc's own small std): `Option`/`Result`, `Vec`, `String`, `HashMap`/`HashSet`,
  `BTreeMap`/`BTreeSet`, `VecDeque`, `BinaryHeap`, `Rc`/`Arc`, `Cell`/`RefCell`, `Mutex`, `Condvar`, `Once`, `OnceLock`, `ManuallyDrop`,
  `Duration`, `AtomicPtr`, `RangeTo` / `RangeFull` as types, `Fuse`, `iter::Empty` / `iter::Repeat`, `ops::ControlFlow`, `alloc::Layout` with `alloc` / `dealloc` / `realloc` / `alloc_zeroed`, `AsMut<[T]>` of arrays, slices and `Vec`, `Extend` for a program's own types, `NonNull`, `Pin` (a plain pointer: `Pin::new`, `get_mut`, `into_inner`, deref), `std::task` (`Poll`, `Context`, `Waker::noop`) and the `Future` trait for futures implemented and polled by hand, `UnsafeCell`, `PhantomData`, atomics, `mpsc`, `mem::*`, `ptr::{read, write, drop_in_place, null}`,
  `iter::{repeat, once, empty, repeat_with, successors}`, `reverse` / `fill` / `rotate_left` / `rotate_right` on arrays and slices, formatting, stdin/stdout/fs/env/process basics
- [ ] async / await: <!-- ui:async-await -->82.5%. An `async fn` runs at once and returns its value: it is not lowered
  to a state machine that `poll`s, and there is no executor or real `Waker`. Futures implemented and polled by hand
  work (above)
- [ ] threads: <!-- ui:threads-sendsync -->87.9%. OS threads (pthreads), in parallel: `spawn` / `join` / `is_finished`, atomic `Arc` and atomics (one instruction each), `Mutex` / `Condvar` / `RwLock` / `Once` / `OnceLock` / `thread::park` on futexes as std's, `mpsc` (also `sync_channel`), `thread_local!` per thread; a panic in a spawned thread ends that thread and its `join` gives `Err`. `thread::Builder` (names, stack sizes), `park_timeout`, `recv_timeout`, `Barrier`. No `thread::scope`, and no poisoning (a panic does not unwind)
- [ ] iterators: <!-- ui:iterators -->47.1% (`iter::empty()` cannot infer its item from the target)
- [ ] FFI: <!-- ui:abi -->81.0%. Scalars only, no structs by value
- [ ] panics: <!-- ui:panics -->22.2%. A panic aborts: no unwinding, `catch_unwind`, `set_hook`

## In sight (known approach, small cost)

- [ ] type parameters: `let v: Vec<Box<_>> = ..` (holes in an annotation) and generic fns passed as values
  (`.map(drop)`, `.map(Result::unwrap)`) now infer; `iter::empty().collect()`, `let f = id;` and const parameters
  inferred from patterns do not (~35 tests)
- [ ] items inside blocks: `mod`, `trait`, `impl`, `type`, `union` and `pub` items in a fn body now move out; items a
  macro expands to in statement position move out too; from a macro in expression position not yet (~12 tests)
- [ ] builtin macros: `offset_of!`, `addr_of!` / `addr_of_mut!`, `try!`, `stringify!`, `concat!`, `line!`, `column!`,
  `env!`, `option_env!`, `assert_matches!`, `format_args!` now work, and so does `include!` of a literal path or of
  `concat!` of literals and `env!`; `module_path!`, `file!`, other forms of `include!`, `asm!` do not (~8 tests)
- [ ] `repr(packed)` / `repr(align)`: ~16 tests
- [ ] `Pin`: `Pin<&T>` / `Pin<&mut T>` work as the pointer; `Box::pin(fut)` and `Pin<Box<dyn Future<Output = T>>>` work (the future's value, as async runs at once); `self: Pin<&mut Self>` and
  `self: Box<Self>` receivers work; `pin!` does not
- [ ] dyn compatibility (E0038), generic argument counts of traits
- [ ] closures that capture `&mut` are not `Copy`

## Expected to cost performance (measured before kept)

- [x] region checking against signatures: each part is measured in its PR, with the KPIs before and after
  in [issue #1](https://github.com/mizchi/zust/issues/1) and its reports
- [ ] coherence and bounds of blanket impls: `check` time, LSP
- [ ] field-precise closure captures in codegen: binary size
- [ ] a larger prelude: no-op build and cache size (the prelude, ~11k lines, is scanned on every build)

## Large features not started

- [ ] executors: `async` lowered to state machines that `poll`, an executor and wakers that wake it, so that
  `Pin<Box<dyn Future>>` holds a pending future, not its value (~25 tests; cost: build time, binary size)
- [ ] unwinding: `catch_unwind`, panic hooks (~22 tests; landing pads cost build time and binary size)
- [ ] FFI: structs by value, other ABIs (~27 tests)
- [ ] a full trait solver: specialization, auto traits through `dyn` / `async`, higher-ranked bounds
- [ ] inline asm
- [ ] proc macros: none run yet; zurgo stops on a use of one ([issue #29](https://github.com/mizchi/zust/issues/29))

Lints, editions and region checking are under [Checks](#checks-compile-fail). SIMD is done in part
([tests/simd/README.md](tests/simd/README.md), [issue #30](https://github.com/mizchi/zust/issues/30)).
