# What is supported

The percentages are rustc's own UI tests that pass in that area: run-pass + check-pass + build-pass of the tests/ui
directories named in the comment next to the number (`ui:`), or compile-fail (`cf:`). They count only the tests
`bench/rust_ui.py` can ask zustc to handle (no nightly features, auxiliary crates, special flags or targets other than
Linux x86-64), so 100% does not mean every test of a directory passes. They were measured
<!-- measured -->2026-11-28, f025189<!-- /measured --> by CI on each push to main (bench.yml runs `bench/rust_ui.py`, then
`scripts/update_readme.py` writes them here). The totals for each kind of test are in the
[README](README.md#coverage-of-rustcs-test-suite), the subset of the language in prose is in
[docs/language.md](docs/language.md), and the [reports](docs/reports/) explain each step.

"Does not work yet" names what is missing in the area, with links to rust tests/ui tests that fail because of it.
[Failing tests by directory](#failing-tests-by-directory) lists, for every directory, why its tests fail, from the
same run.

## Overall

| | result | notes |
|---|---|---|
| tests/ui | [table in the README](README.md#coverage-of-rustcs-test-suite) | |
| real crates | fnv, adler2, hex, itoa, rustc-hash and cfg-if build and give the same output (6 / 6) | |
| crates.io's top crates, by category | [CRATES_SUPPORT.md](CRATES_SUPPORT.md) and the [dashboard](https://mizchi.github.io/zust/crates/) | written by CI from `cratebench/survey/survey.py` |
| zust-analyzer against rust-analyzer | 99.0% agreement on zustc's own sources (rust-analyzer: 98.9%) | a snapshot measured by hand (2026-11-18), not by CI: [PERFORMANCE.md](PERFORMANCE.md#against-rustc-relax-and-rust-analyzer-snapshots) |

## Language

| area | pass tests | compile-fail | does not work yet |
|---|---:|---:|---|
| numbers, arithmetic, overflow checks | <!-- ui:numbers-arithmetic -->100.0% (29 / 29) | <!-- cf:numbers-arithmetic -->100.0% (3 / 3) | |
| `for` / `while` / `loop`, labels | <!-- ui:for-loop-while,loops,label -->96.7% (58 / 60) | <!-- cf:for-loop-while,loops,label -->88.6% (39 / 44) | |
| enums, `#[repr(u8)]`, auto-sized tags | <!-- ui:enum,enum-discriminant -->88.1% (37 / 42) | <!-- cf:enum,enum-discriminant -->72.7% (24 / 33) | niche layouts: `Option<&T>` and nested `Option`s take a tag of their own ([issue-32498](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generics/issue-32498.rs), [enum-layout-optimization](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs-enums/enum-layout-optimization.rs)) |
| `cfg` | <!-- ui:cfg -->94.1% (16 / 17) | <!-- cf:cfg -->71.4% (10 / 14) | |
| bindings, patterns, slice patterns, or-patterns (also with guards) | <!-- ui:binding,pattern,or-patterns -->94.1% (174 / 185) | <!-- cf:binding,pattern,or-patterns -->76.3% (151 / 198) | |
| `match` | <!-- ui:match -->87.8% (36 / 41) | <!-- cf:match -->80.0% (36 / 45) | |
| name resolution | <!-- ui:resolve -->91.4% (32 / 35) | <!-- cf:resolve -->79.8% (150 / 188) | a `::name` path in edition 2018 is a crate's even beside a module of that name (`mod std` and `use ::std::io`) ([uniform-paths/basic](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/uniform-paths/basic.rs)) |
| imports, `use` | <!-- ui:imports -->92.0% (81 / 88) | <!-- cf:imports -->67.5% (85 / 126) | |
| statics: ints, floats, raw pointers, fn pointers, `const fn` calls | <!-- ui:statics -->95.0% (19 / 20) | <!-- cf:statics -->66.7% (12 / 18) | |
| structs, `..base` (field by field) | <!-- ui:structs,structs-enums -->91.0% (81 / 89) | <!-- cf:structs,structs-enums -->95.4% (62 / 65) | |
| parser | <!-- ui:parser -->91.9% (91 / 99) | <!-- cf:parser -->87.4% (659 / 754) | |
| closures (capture, `Fn` traits, `move`; with no captures also as fn pointers) | <!-- ui:closures,unboxed-closures,functions-closures -->84.4% (162 / 192) | <!-- cf:closures,unboxed-closures,functions-closures -->82.1% (115 / 140) | moving a captured value out of a closure; a closure's return type that only its uses tell |
| derives: `Debug`, `Clone`, `Copy`, `PartialEq`/`Eq`, `PartialOrd`/`Ord`, `Hash`, `Default` | <!-- ui:derives,deriving -->90.9% (40 / 44) | <!-- cf:derives,deriving -->71.1% (32 / 45) | |
| associated types | <!-- ui:associated-types -->85.3% (122 / 143) | <!-- cf:associated-types -->51.8% (59 / 114) | |
| generics | <!-- ui:generics -->95.2% (40 / 42) | <!-- cf:generics -->83.3% (45 / 54) | a type parameter known only from the expected type is not inferred. An integer literal for `T: Foo` takes the one integer type that implements Foo |
| consts | <!-- ui:consts -->79.5% (217 / 273) | <!-- cf:consts -->41.2% (101 / 245) | heap values, generic `const fn`; a const of a generic impl (`const C: Option<T> = None`) takes its type at each use |
| drop | <!-- ui:drop -->76.8% (53 / 69) | <!-- cf:drop -->61.5% (8 / 13) | drops during unwinding ([drop-trait-enum](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/drop-trait-enum.rs)); a `Vec` drops its elements last to first; a plain block that is another block's tail drops its tail's temporaries at its own end (before edition 2024) |
| traits | <!-- ui:traits -->75.1% (181 / 241) | <!-- cf:traits -->64.9% (155 / 239) | an integer literal's type taken from the bounds of an impl (`impl<T: Speak> Speak for Option<T>` with `Some(3).hi()`, [bug-7183-generics](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/bug-7183-generics.rs)); a type argument taken from the one impl a `dyn` coercion selects ([infer-struct-with-trait-object](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/infer-struct-with-trait-object.rs)); `impl<T> Tr for &T` only without functions; no coherence |
| `macro_rules!` | <!-- ui:macros -->80.7% (113 / 140) | <!-- cf:macros -->57.5% (96 / 167) | doc comments matched as `#[doc = ..]` attributes ([macro-doc-raw-str-hashes](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/macro-doc-raw-str-hashes.rs)); hygiene beyond `let`, labels and arm bindings; not every fragment kind |
| unions | <!-- ui:union -->84.6% (22 / 26) | <!-- cf:union -->76.7% (23 / 30) | `repr(packed)` |
| const generics | <!-- ui:const-generics -->66.3% (67 / 101) | <!-- cf:const-generics -->65.9% (85 / 129) | `generic_const_exprs`; `type_name` of a const generic instance ([const-generic-type_name](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/const-generic-type_name.rs)) |
| generic associated types | <!-- ui:generic-associated-types -->86.7% (39 / 45) | <!-- cf:generic-associated-types -->59.0% (36 / 61) | |
| casts | <!-- ui:cast -->82.8% (24 / 29) | <!-- cf:cast -->72.9% (35 / 48) | `as _` / `as *const _` take the source or expected type; a pointer of unknown kind is E0641 |
| `impl Trait` | <!-- ui:impl-trait -->77.1% (91 / 118) | <!-- cf:impl-trait -->46.6% (76 / 163) | type alias `impl Trait` (TAIT). Return position works, also nested (`-> Option<impl Debug>`), in trait methods and generic fns (typed per instance) |
| layout | <!-- ui:packed,layout -->67.7% (21 / 31) | <!-- cf:packed,layout -->55.0% (11 / 20) | `repr(packed)` / `repr(align)` are ignored ([packed-struct-size](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/packed-struct-size.rs)); no niche optimization (see enums) |

Also supported, with no directory of their own: type aliases and paths (`impl foo::Point`) as impl self types,
discriminants from constants and earlier variants, where clauses on concrete types; GAT bindings, `dyn for<'a> Fn`,
2015 anonymous parameters, raw lifetimes `'r#a`, half-open range patterns, shebangs, Unicode white space;
`macro_rules!` fragments matched by kind (`ident`, `literal`, `lifetime`, `vis`); methods of `impl Tr for Box<S>`,
`..b` / `..=b` / `..` values; qualified paths (`<T as Tr>::C`, `<() as Tr>::f()`, associated functions through a type
alias); let chains, also in match guards; inline `const { .. }`, `&raw const` / `&raw mut`.

## Checks (compile-fail)

| check | compile-fail | does not work yet |
|---|---:|---|
| borrow checking: moves, conflicting loans, escapes, mutability ([report 08](docs/reports/2026-11-18-08-borrow-checking.md)) | borrowck <!-- cf:borrowck -->91.3% (324 / 355) | |
| type errors, missing items, generics declarations (E0403, E0207, E0392, E0072 ..) | typeck <!-- cf:typeck -->81.5% (154 / 189), structs <!-- cf:structs -->95.8% (46 / 48) | |
| region errors against signatures ("lifetime may not live long enough", E0621, E0597, E0716, E0515, E0309, E0310, ..; [issue #1](https://github.com/mizchi/zust/issues/1)) | regions <!-- cf:regions -->89.7% (104 / 116), lifetimes <!-- cf:lifetimes -->85.2% (109 / 128) | what a closure's body requires of its fn; higher-ranked trait selection; the requirements of the impl a coercion selects; the prelude's iterators (their types hold no lifetimes) |
| unsafe operations outside `unsafe` (E0133): calls to `unsafe fn` and extern fns, raw pointer derefs, `static mut`, union field reads | — | inside closures or consts; the `unsafe_op_in_unsafe_fn` lint |
| non-exhaustive `match` (E0004): bool, unit, enums, structs and tuples, nested and with or-patterns | — | integer and slice patterns, and types left generic |
| impl methods against the trait's declaration: parameter count (E0050), kind of receiver (E0053) | — | |
| unreachable match arms under `#![deny(unreachable_patterns)]`; array patterns of the wrong length (E0527, E0528) | — | |
| coherence, bounds of blanket impls, dyn compatibility (E0038), the bindings of `dyn Tr<A = T>` against the associated type's bounds | — | done only when T is a primitive, `str` or a struct name |
| lints and warnings | lint <!-- cf:lint -->48.1% (100 / 208) | most lints are not implemented |
| editions | — | 2015-only errors |

## Runtime and std

| area | pass tests | does not work yet |
|---|---:|---|
| the prelude (zustc's own small std; the list is below the table) | — | `std::panic::Location::caller` ([tracked-trait-obj](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-2091-track-caller/tracked-trait-obj.rs)); `type_name` gives the type's short name, not its path ([type-name-intrinsic-usage-61894](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/type-name-intrinsic-usage-61894.rs)); `thread::park_timeout` ([park-timeout-wakeup-59020](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/std/park-timeout-wakeup-59020.rs)) |
| async / await | <!-- ui:async-await -->82.5% (85 / 103) | an `async fn` runs at once and returns its value: it is not lowered to a state machine that `poll`s, and there is no executor or real `Waker`. Futures implemented and polled by hand work |
| threads | <!-- ui:threads-sendsync -->72.4% (42 / 58) | `thread::scope`; poisoning (a panic does not unwind). OS threads (pthreads) run in parallel: atomic `Arc` and atomics, `Mutex` / `Condvar` / `RwLock` / `Once` / `OnceLock` / `park` on futexes, `mpsc`, `thread_local!` per thread, `thread::Builder` |
| iterators | <!-- ui:iterators -->47.1% (8 / 17) | `iter::empty()` cannot infer its item from the target |
| FFI | <!-- ui:abi -->81.0% (34 / 42) | structs by value, other ABIs; scalars only |
| panics | <!-- ui:panics -->11.1% (1 / 9) | a panic aborts: no unwinding, `catch_unwind` ([catch-unwind-bang](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/catch-unwind-bang.rs)), `set_hook` ([panic-handler-chain](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/panic-handler-chain.rs)) |

The prelude has `Option`/`Result`, `Vec`, `String`, `HashMap`/`HashSet`, `BTreeMap`/`BTreeSet`, `VecDeque`,
`BinaryHeap`, `Rc`/`Arc`, `Cell`/`RefCell`, `Mutex`, `OnceLock`, `ManuallyDrop`, `Duration`, `AtomicPtr`, `RangeTo` /
`RangeFull` as types, `Fuse`, `iter::Empty`, `NonNull`, `Pin` (a plain pointer: `Pin::new`, `get_mut`, `into_inner`,
deref), `std::task` (`Poll`, `Context`, `Waker::noop`) and the `Future` trait for futures implemented and polled by
hand, `UnsafeCell`, `PhantomData`, atomics, `mpsc`, `mem::*`, `ptr::{read, write, drop_in_place, null}`,
`iter::{repeat, once, empty, repeat_with, successors}`, `reverse` / `fill` / `rotate_left` / `rotate_right` on arrays
and slices, `Vec::from` / `.into()` of an array, a slice, a `&str` or a String (not yet of a `Box<[T]>`, a `VecDeque`
or a `BinaryHeap`), formatting, stdin/stdout/fs/env/process basics.

## What is next

| work | state | cost |
|---|---|---|
| type parameters | `let v: Vec<Box<_>> = ..` (holes in an annotation) and generic fns passed as values (`.map(drop)`, `.map(Result::unwrap)`) infer; `iter::empty().collect()`, `let f = id;` and const parameters inferred from patterns do not (~35 tests) | known approach, small |
| items inside blocks | `mod`, `trait`, `impl`, `type`, `union` and `pub` items in a fn body move out, and so do items a macro expands to in statement position; from a macro in expression position not yet (~12 tests) | known approach, small |
| builtin macros | `module_path!`, `file!`, other forms of `include!`, `asm!` (~8 tests). `offset_of!`, `addr_of!` / `addr_of_mut!`, `try!`, `stringify!`, `concat!`, `line!`, `column!`, `env!`, `option_env!`, `assert_matches!`, `format_args!` and `include!` of a literal path work | known approach, small |
| `repr(packed)` / `repr(align)` | ~16 tests | known approach, small |
| `Pin` | `pin!`; `Pin<&T>` / `Pin<&mut T>`, `Box::pin(fut)`, `Pin<Box<dyn Future<Output = T>>>`, `self: Pin<&mut Self>` and `self: Box<Self>` receivers work | known approach, small |
| dyn compatibility (E0038), generic argument counts of traits | | known approach, small |
| closures that capture `&mut` are not `Copy` | | known approach, small |
| coherence and bounds of blanket impls | | `check` time, LSP |
| field-precise closure captures in codegen | | binary size |
| a larger prelude | the prelude, ~11k lines, is scanned on every build | no-op build and cache size |
| executors: `async` lowered to state machines that `poll`, wakers | so that `Pin<Box<dyn Future>>` holds a pending future, not its value (~25 tests) | build time, binary size |
| unwinding: `catch_unwind`, panic hooks | ~22 tests | landing pads: build time and binary size |
| FFI: structs by value, other ABIs | ~27 tests | |
| a full trait solver: specialization, auto traits through `dyn` / `async`, higher-ranked bounds | | |
| inline asm | | |
| proc macros | none run yet; zurgo stops on a use of one ([issue #29](https://github.com/mizchi/zust/issues/29)) | |

Region checking against signatures is measured in each of its PRs, with the KPIs before and after in
[issue #1](https://github.com/mizchi/zust/issues/1). SIMD is done in part ([tests/simd/README.md](tests/simd/README.md),
[issue #30](https://github.com/mizchi/zust/issues/30)).

## Failing tests by directory

Generated from `bench/rust_ui.py --reasons`: for each tests/ui directory with three failing tests or more (most first),
the most common reasons its tests fail, with the names and numbers in a message replaced by `…` and `N`. **kind** is
the test's kind (run, check, build, fail for compile-fail), with "wrong line" for a compile-fail test whose error is
at no annotated line. Each reason links to some of the tests.

<!-- auto:failures -->
<details><summary><code>consts</code>: 200 failing (pass tests 217 / 273 · compile-fail 101 / 245)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 115 | [const-assert-unchecked-ub.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-assert-unchecked-ub.rs), [const-cast-different-types.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-cast-different-types.rs), [const-cast-wrong-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-cast-wrong-type.rs) |
| check | array length must be a literal or a const | 4 | [const_let_assign3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const_let_assign3.rs), [control-flow/feature-gate-const-if-match.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/control-flow/feature-gate-const-if-match.rs), [issue-66342.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/issue-66342.rs) |
| fail, wrong line | array length must be a literal or a const | 4 | [const-eval/issue-52475.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-eval/issue-52475.rs), [const-mut-refs/issue-76510.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-mut-refs/issue-76510.rs), [long-constant-evaluation-cause-dead-code.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/long-constant-evaluation-cause-dead-code.rs) |
| fail, wrong line | unsupported expression in constant | 4 | [const-eval/const-eval-span.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-eval/const-eval-span.rs), [const-tup-index-span.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-tup-index-span.rs), [issue-54954.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/issue-54954.rs) |
| fail, wrong line | overflow in constant | 3 | [const-eval/const-eval-overflow-4.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-eval/const-eval-overflow-4.rs), [const-eval/shift_overflow.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-eval/shift_overflow.rs), [eval-enum.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/eval-enum.rs) |
| run | in static `…`: constants of type `…` are not supported | 3 | [const-block.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-block.rs), [const-cast.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/const-cast.rs), [issue-46553.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/consts/issue-46553.rs) |
| | 57 other reasons | 67 | |

</details>

<details><summary><code>traits</code>: 144 failing (pass tests 181 / 241 · compile-fail 155 / 239)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 65 | [ambiguous-trait-matching-distinct-lifetimes.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/ambiguous-trait-matching-distinct-lifetimes.rs), [ambiguous-where-bounds-differ-by-lifetime.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/ambiguous-where-bounds-differ-by-lifetime.rs), [assoc-type-in-superbad.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/assoc-type-in-superbad.rs) |
| run | mismatched types: expected `…`, found `…` | 15 | [dyn-drop-principal.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/dyn-drop-principal.rs), [multidispatch1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/multidispatch1.rs), [object/trait-object-lifetime-conversion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/object/trait-object-lifetime-conversion.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 5 | [object/incomplete-multiple-super-projection.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/object/incomplete-multiple-super-projection.rs), [subtype-recursion-limit.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/subtype-recursion-limit.rs), [trait-upcasting/subtrait-method.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/trait-upcasting/subtrait-method.rs) |
| run | no method `…` on type `…` | 5 | [bound/generic_trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/bound/generic_trait.rs), [bug-7183-generics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/bug-7183-generics.rs), [fn-type-trait-impl-15444.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/fn-type-trait-impl-15444.rs) |
| check | cannot infer type parameter `…` | 4 | [issue-58344.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/issue-58344.rs), [next-solver/generalize/constrain-inference-during-normalize.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/next-solver/generalize/constrain-inference-during-normalize.rs), [solver-cycles/inductive-canonical-cycle.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/solver-cycles/inductive-canonical-cycle.rs) |
| fail, wrong line | no method named `…` found for `…` | 3 | [negative-impls/pin-unsound-issue-66544-clone.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/negative-impls/pin-unsound-issue-66544-clone.rs), [negative-impls/pin-unsound-issue-66544-derefmut.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/negative-impls/pin-unsound-issue-66544-derefmut.rs), [object/issue-44454-3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/traits/object/issue-44454-3.rs) |
| | 37 other reasons | 47 | |

</details>

<details><summary><code>lint</code>: 135 failing (pass tests 217 / 244 · compile-fail 100 / 208)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 87 | [anonymous-reexport.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/anonymous-reexport.rs), [c-void-returns.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/c-void-returns.rs), [crate_level_only_lint.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/crate_level_only_lint.rs) |
| check | mismatched types: expected `…`, found `…` | 3 | [dead-code/const-and-self.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/dead-code/const-and-self.rs), [trivial-casts-dyn-any-issue-148219.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/trivial-casts-dyn-any-issue-148219.rs), [unused/unused-allocation-box-ref-issue-151846.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/unused/unused-allocation-box-ref-issue-151846.rs) |
| check | no method `…` on type `…` | 3 | [dead-code/trait-impl.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/dead-code/trait-impl.rs), [non-local-defs/generics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/non-local-defs/generics.rs), [unused_import_warning_issue_45268.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/unused_import_warning_issue_45268.rs) |
| check | no method named `…` found for `…` | 3 | [fn_must_use.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/fn_must_use.rs), [for_loop_over_fallibles.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/for_loop_over_fallibles.rs), [issue-112489.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/issue-112489.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 3 | [lint-deref-nullptr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/lint-deref-nullptr.rs), [semicolon-in-expressions-from-macros/warn-semicolon-in-expressions-from-macros.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/semicolon-in-expressions-from-macros/warn-semicolon-in-expressions-from-macros.rs), [unused/must_use-pin.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/unused/must_use-pin.rs) |
| fail, wrong line | cannot find type `…` | 2 | [non-snake-case/lint-non-snake-case-modules.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/non-snake-case/lint-non-snake-case-modules.rs), [suspicious-double-ref-op.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lint/suspicious-double-ref-op.rs) |
| | 31 other reasons | 34 | |

</details>

<details><summary><code>impl-trait</code>: 114 failing (pass tests 91 / 118 · compile-fail 76 / 163)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 65 | [apit/arg-position-impl-trait-too-long.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/apit/arg-position-impl-trait-too-long.rs), [apit/impl-generic-mismatch-ab.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/apit/impl-generic-mismatch-ab.rs), [auto-trait-leakage/auto-trait-contains-err.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/auto-trait-leakage/auto-trait-contains-err.rs) |
| fail, wrong line | cannot infer the `…` return type of `…` | 10 | [cross-return-site-inference.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/cross-return-site-inference.rs), [fallback_inference.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/fallback_inference.rs), [in-trait/cycle-effective-visibilities-during-dyn-compatibility-check.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/in-trait/cycle-effective-visibilities-during-dyn-compatibility-check.rs) |
| check | cannot infer the `…` return type of `…` | 4 | [defining-use-uncaptured-non-universal-region.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/defining-use-uncaptured-non-universal-region.rs), [lifetime-ambiguity-regression.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/lifetime-ambiguity-regression.rs), [rpit-assoc-pair-with-lifetime.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/rpit-assoc-pair-with-lifetime.rs) |
| check | unsupported expression in constant | 3 | [in-ctfe/array-len-size-of.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/in-ctfe/array-len-size-of.rs), [in-ctfe/array-len.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/in-ctfe/array-len.rs), [in-ctfe/enum-discr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/in-ctfe/enum-discr.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 3 | [dont-suggest-box-on-empty-else-arm.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/dont-suggest-box-on-empty-else-arm.rs), [issues/infinite-impl-trait-issue-38064.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/issues/infinite-impl-trait-issue-38064.rs), [multiple-defining-usages-in-body.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/multiple-defining-usages-in-body.rs) |
| check | mismatched types: expected `…`, found `…` | 2 | [question_mark.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/question_mark.rs), [trait_upcasting.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/impl-trait/trait_upcasting.rs) |
| | 22 other reasons | 27 | |

</details>

<details><summary><code>parser</code>: 103 failing (pass tests 91 / 99 · compile-fail 659 / 754)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 47 | [assoc/assoc-const-underscore-semantic-fail.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/assoc/assoc-const-underscore-semantic-fail.rs), [assoc/assoc-static-syntactic-fail.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/assoc/assoc-static-syntactic-fail.rs), [attribute/attr-pat-struct-rest.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/attribute/attr-pat-struct-rest.rs) |
| fail, wrong line | expected `…` (found `…`) | 10 | [bare-struct-body.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/bare-struct-body.rs), [bounds-type-where.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/bounds-type-where.rs), [issues/diagnostics-parenthesized-type-arguments-issue-120892-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/issues/diagnostics-parenthesized-type-arguments-issue-120892-1.rs) |
| fail, wrong line | unterminated impl block | 4 | [deli-ident-issue-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/deli-ident-issue-1.rs), [mismatched-braces/missing-close-brace-in-impl-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/mismatched-braces/missing-close-brace-in-impl-trait.rs), [mismatched-braces/missing-close-brace-in-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/mismatched-braces/missing-close-brace-in-trait.rs) |
| fail, wrong line | unterminated item in impl block | 4 | [default-unmatched-assoc.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/default-unmatched-assoc.rs), [macro/trait-non-item-macros.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/macro/trait-non-item-macros.rs), [removed-syntax/removed-syntax-static-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/removed-syntax/removed-syntax-static-fn.rs) |
| fail, wrong line | expected a literal (`…`, `…`, `…`, etc.) here, found expression | 3 | [attribute/attr-missing-comma.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/attribute/attr-missing-comma.rs), [attribute/properly-recover-from-trailing-outer-attribute-in-body-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/attribute/properly-recover-from-trailing-outer-attribute-in-body-1.rs), [issues/issue-118531-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/issues/issue-118531-ice.rs) |
| fail, wrong line | expected an item (found `…`) | 2 | [issues/issue-21146.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/issues/issue-21146.rs), [issues/issue-62895.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parser/issues/issue-62895.rs) |
| | 32 other reasons | 33 | |

</details>

<details><summary><code>macros</code>: 98 failing (pass tests 113 / 140 · compile-fail 96 / 167)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 37 | [assert-ne-no-invalid-help-issue-146204.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/assert-ne-no-invalid-help-issue-146204.rs), [attr-expr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/attr-expr.rs), [attr-path-metavar-generic-args.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/attr-path-metavar-generic-args.rs) |
| fail, wrong line | no rules of `…` matched this call | 7 | [issue-112342-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/issue-112342-1.rs), [issue-57597.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/issue-57597.rs), [macro-error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/macro-error.rs) |
| check | mismatched types: expected `…`, found `…` | 2 | [assert-desugaring-145770.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/assert-desugaring-145770.rs), [issue-2804-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/issue-2804-2.rs) |
| fail, wrong line | expected `…` (found `…`) | 2 | [include-single-expr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/include-single-expr.rs), [macro-adjacent-ident-on-token.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/macro-adjacent-ident-on-token.rs) |
| fail, wrong line | expected an item (found `…`) | 2 | [issue-54441.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/issue-54441.rs), [reparse-expr-issue-139495.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/reparse-expr-issue-139495.rs) |
| fail, wrong line | no method `…` on type `…` | 2 | [issue-25385.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/issue-25385.rs), [macro-backtrace-invalid-internals.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/macros/macro-backtrace-invalid-internals.rs) |
| | 41 other reasons | 46 | |

</details>

<details><summary><code>async-await</code>: 80 failing (pass tests 85 / 103 · compile-fail 98 / 160)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 49 | [async-await-let-else.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/async-await-let-else.rs), [async-borrowck-escaping-block-error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/async-borrowck-escaping-block-error.rs), [async-closures/cast-as-infer-self-borrows.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/async-closures/cast-as-infer-self-borrows.rs) |
| check | cannot infer the `…` return type of `…` | 2 | [issue-64130-4-async-move.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/issue-64130-4-async-move.rs), [issues/issue-64477-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/issues/issue-64477-2.rs) |
| check | the trait bound `…` is not satisfied (for `…`) | 2 | [async-closures/moro-example.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/async-closures/moro-example.rs), [async-closures/validate-synthetic-body.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/async-closures/validate-synthetic-body.rs) |
| fail, wrong line | cannot infer the `…` return type of `…` | 2 | [issue-70935-complex-spans.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/issue-70935-complex-spans.rs), [unnecessary-await.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/unnecessary-await.rs) |
| fail, wrong line | cannot infer the return type of this closure; annotate it (`…`) | 2 | [issue-72442.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/issue-72442.rs), [missing-return-in-async-block.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/missing-return-in-async-block.rs) |
| build | cannot infer the type of this closure parameter; add a type annotation | 1 | [async-closures/signature-deduction.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/async-await/async-closures/signature-deduction.rs) |
| | 22 other reasons | 22 | |

</details>

<details><summary><code>const-generics</code>: 78 failing (pass tests 67 / 101 · compile-fail 85 / 129)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 36 | [assoc_const_as_type_argument.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/assoc_const_as_type_argument.rs), [associated-type-bound-fail.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/associated-type-bound-fail.rs), [bad-generic-in-copy-impl.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/bad-generic-in-copy-impl.rs) |
| run | cannot infer type parameter `…` | 5 | [defaults/const-param-in-ty-defaults.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/defaults/const-param-in-ty-defaults.rs), [infer_arg_from_pat.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/infer_arg_from_pat.rs), [issues/issue-69654-run-pass.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/issues/issue-69654-run-pass.rs) |
| run | mismatched types: expected `…`, found `…` | 3 | [array-wrapper-struct-ctor.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/array-wrapper-struct-ctor.rs), [coerce_unsized_array.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/coerce_unsized_array.rs), [defaults/complex-unord-param.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/defaults/complex-unord-param.rs) |
| build | cannot infer type parameter `…` | 2 | [issues/issue-70180-1-stalled_on.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/issues/issue-70180-1-stalled_on.rs), [issues/issue-70180-2-stalled_on.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/issues/issue-70180-2-stalled_on.rs) |
| check | cannot find constant `…` | 2 | [backcompat/trait-resolution-breakage.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/backcompat/trait-resolution-breakage.rs), [generic_arg_infer/paren_infer.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/generic_arg_infer/paren_infer.rs) |
| check | cannot infer type parameter `…` | 2 | [infer_arr_len_from_pat.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/infer_arr_len_from_pat.rs), [where-clauses.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const-generics/where-clauses.rs) |
| | 25 other reasons | 28 | |

</details>

<details><summary><code>associated-types</code>: 76 failing (pass tests 122 / 143 · compile-fail 59 / 114)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 45 | [assoc-type-element-mismatch-in-hrtb.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/assoc-type-element-mismatch-in-hrtb.rs), [associated-type-as-value.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/associated-type-as-value.rs), [associated-type-macro.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/associated-type-macro.rs) |
| run | mismatched types: expected `…`, found `…` | 4 | [associated-types-enum-field-named.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/associated-types-enum-field-named.rs), [associated-types-enum-field-numbered.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/associated-types-enum-field-numbered.rs), [associated-types-ref-in-struct-literal.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/associated-types-ref-in-struct-literal.rs) |
| fail, wrong line | cannot find value `…` | 3 | [hr-associated-type-bound-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/hr-associated-type-bound-1.rs), [hr-associated-type-bound-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/hr-associated-type-bound-2.rs), [hr-associated-type-bound-param-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/hr-associated-type-bound-param-1.rs) |
| build | cannot find value (unsupported path) | 2 | [assoc-type-in-strategy-pattern-iterator.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/assoc-type-in-strategy-pattern-iterator.rs), [issue-64848.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/issue-64848.rs) |
| check | cannot find associated type `…` for `…` | 2 | [issue-50301.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/issue-50301.rs), [issue-91234.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/issue-91234.rs) |
| check | no method `…` on type `…` | 2 | [issue-21726.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/issue-21726.rs), [issue-65934.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-types/issue-65934.rs) |
| | 16 other reasons | 18 | |

</details>

<details><summary><code>suggestions</code>: 69 failing (pass tests 4 / 4 · compile-fail 316 / 385)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 57 | [assoc-const-as-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/assoc-const-as-fn.rs), [bound-suggestions.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/bound-suggestions.rs), [clone-bounds-121524.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/clone-bounds-121524.rs) |
| fail, wrong line | cannot find type `…` | 2 | [borrow-from-overloaded-deref-issue-154826.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/borrow-from-overloaded-deref-issue-154826.rs), [issue-68049-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/issue-68049-1.rs) |
| fail, wrong line | cannot infer the `…` return type of `…` | 2 | [impl-on-dyn-trait-with-implicit-static-bound-needing-more-suggestions.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/impl-on-dyn-trait-with-implicit-static-bound-needing-more-suggestions.rs), [impl-on-dyn-trait-with-implicit-static-bound.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/impl-on-dyn-trait-with-implicit-static-bound.rs) |
| fail, wrong line | `…` is only supported directly in a `…` loop | 1 | [suggest-remove-refs-4.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/suggest-remove-refs-4.rs) |
| fail, wrong line | cannot find value `…` | 1 | [mut-borrow-needed-by-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/mut-borrow-needed-by-trait.rs) |
| fail, wrong line | cannot infer the return type of this closure; annotate it (`…`) | 1 | [cow-into-owned-suggestion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/suggestions/cow-into-owned-suggestion.rs) |
| | 5 other reasons | 5 | |

</details>

<details><summary><code>rfcs</code>: 50 failing (pass tests 64 / 85 · compile-fail 58 / 87)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 23 | [impl-trait/higher-ranked-regions-diag.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/impl-trait/higher-ranked-regions-diag.rs), [rfc-1623-static/rfc1623-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-1623-static/rfc1623-2.rs), [rfc-1937-termination-trait/issue-103052-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-1937-termination-trait/issue-103052-2.rs) |
| run | cannot find type `…` | 3 | [rfc-1937-termination-trait/termination-trait-for-result-box-error_ok.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-1937-termination-trait/termination-trait-for-result-box-error_ok.rs), [rfc-1937-termination-trait/termination-trait-for-result.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-1937-termination-trait/termination-trait-for-result.rs), [rfc-2091-track-caller/track-caller-ffi.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-2091-track-caller/track-caller-ffi.rs) |
| run | cannot find value (unsupported path) | 3 | [rfc-1789-as-cell/from-mut.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-1789-as-cell/from-mut.rs), [rfc-2091-track-caller/tracked-trait-impls.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-2091-track-caller/tracked-trait-impls.rs), [rfc-2091-track-caller/tracked-trait-obj.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-2091-track-caller/tracked-trait-obj.rs) |
| run | mismatched types: expected `…`, found `…` | 3 | [rfc-2361-dbg-macro/dbg-macro-expected-behavior.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-2361-dbg-macro/dbg-macro-expected-behavior.rs), [rfc-3348-c-string-literals/basic.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-3348-c-string-literals/basic.rs), [rfc-3348-c-string-literals/non-ascii.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-3348-c-string-literals/non-ascii.rs) |
| run | `…` must be `…` | 2 | [rfc-1937-termination-trait/termination-trait-for-exitcode.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-1937-termination-trait/termination-trait-for-exitcode.rs), [rfc-1937-termination-trait/termination-trait-for-impl-termination.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-1937-termination-trait/termination-trait-for-impl-termination.rs) |
| check | `…` doesn't implement `…` | 1 | [rfc-2361-dbg-macro/dbg-macro-ref-impl.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rfcs/rfc-2361-dbg-macro/dbg-macro-ref-impl.rs) |
| | 15 other reasons | 15 | |

</details>

<details><summary><code>imports</code>: 48 failing (pass tests 81 / 88 · compile-fail 85 / 126)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 28 | [ambiguous-import-visibility-globglob-priv.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/ambiguous-import-visibility-globglob-priv.rs), [ambiguous-panic-glob-vs-multiouter.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/ambiguous-panic-glob-vs-multiouter.rs), [ambiguous-panic-globvsglob.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/ambiguous-panic-globvsglob.rs) |
| fail, wrong line | unsupported item | 4 | [ambiguous-panic-re-emit.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/ambiguous-panic-re-emit.rs), [issue-109148.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/issue-109148.rs), [multiple-extern-by-macro-for-buitlin.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/multiple-extern-by-macro-for-buitlin.rs) |
| fail, wrong line | the name `…` is defined multiple times | 3 | [absolute-paths-in-nested-use-groups.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/absolute-paths-in-nested-use-groups.rs), [shadow-glob-module-resolution-3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/shadow-glob-module-resolution-3.rs), [shadow-glob-module-resolution-4.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/shadow-glob-module-resolution-4.rs) |
| fail, wrong line | unresolved import `…` (E0432) | 2 | [extern-crate-used.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/extern-crate-used.rs), [issue-32833.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/issue-32833.rs) |
| fail, wrong line | unterminated item starting here (byte N) | 2 | [import-prefix-macro-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/import-prefix-macro-1.rs), [import-prefix-macro-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/import-prefix-macro-2.rs) |
| run | cannot find type `…` | 2 | [import-rename.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/import-rename.rs), [macro-use-as.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/imports/macro-use-as.rs) |
| | 6 other reasons | 7 | |

</details>

<details><summary><code>pattern</code>: 47 failing (pass tests 60 / 66 · compile-fail 129 / 170)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 25 | [bindings-after-at/wild-before-at-syntactically-rejected.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/bindings-after-at/wild-before-at-syntactically-rejected.rs), [generic-in-path.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/generic-in-path.rs), [manually-drop-dangling-ref.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/manually-drop-dangling-ref.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 3 | [issue-92074-macro-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/issue-92074-macro-ice.rs), [usefulness/slice-pattern-const-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/usefulness/slice-pattern-const-2.rs), [usefulness/slice-pattern-const.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/usefulness/slice-pattern-const.rs) |
| run | mismatched types: expected `…`, found `…` | 3 | [issue-22546.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/issue-22546.rs), [move-ref-patterns/move-ref-patterns-dynamic-semantics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/move-ref-patterns/move-ref-patterns-dynamic-semantics.rs), [usefulness/issue-30240-rpass.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/usefulness/issue-30240-rpass.rs) |
| fail, wrong line | N:N: in constant `…`: N:N: type annotations needed for `…` | 2 | [non-structural-match-types-cycle-err.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/non-structural-match-types-cycle-err.rs), [non-structural-match-types.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/non-structural-match-types.rs) |
| fail, wrong line | unsupported type | 2 | [usefulness/issue-119493-type-error-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/usefulness/issue-119493-type-error-ice.rs), [usefulness/issue-119778-type-error-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/usefulness/issue-119778-type-error-ice.rs) |
| check | cannot find value (unsupported path) | 1 | [issue-88074-pat-range-type-inference.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/pattern/issue-88074-pat-range-type-inference.rs) |
| | 11 other reasons | 11 | |

</details>

<details><summary><code>resolve</code>: 41 failing (pass tests 32 / 35 · compile-fail 150 / 188)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 30 | [attr-macros-positional-rejection.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/attr-macros-positional-rejection.rs), [crate-in-paths.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/crate-in-paths.rs), [export-fully-qualified-2018.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/export-fully-qualified-2018.rs) |
| fail, wrong line | unsupported item | 2 | [ice-inconsistent-resolution-149821.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/ice-inconsistent-resolution-149821.rs), [ice-on-shadowing-std-with-macro.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/ice-on-shadowing-std-with-macro.rs) |
| check | type `…` has no fields | 1 | [conflicting-primitive-names.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/conflicting-primitive-names.rs) |
| check | unsupported raw pointer method `…` | 1 | [pointer-type-impls-14254.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/pointer-type-impls-14254.rs) |
| fail, wrong line | `…` takes N type arguments, but N were given | 1 | [disambiguate-identical-names.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/disambiguate-identical-names.rs) |
| fail, wrong line | conflicting implementations of trait `…` for `…` | 1 | [issue-102946.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/resolve/issue-102946.rs) |
| | 5 other reasons | 5 | |

</details>

<details><summary><code>higher-ranked</code>: 40 failing (pass tests 41 / 51 · compile-fail 15 / 45)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 25 | [anonymous-higher-ranked-lifetime.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/anonymous-higher-ranked-lifetime.rs), [hr-fn-ptr-trait-impl-mismatch-29061.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/hr-fn-ptr-trait-impl-mismatch-29061.rs), [hrtb-fn-ptr-impl-not-general-enough-57936.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/hrtb-fn-ptr-impl-not-general-enough-57936.rs) |
| check | cannot infer type parameter `…` | 2 | [trait-bounds/normalize-under-binder/issue-56556.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/trait-bounds/normalize-under-binder/issue-56556.rs), [trait-bounds/normalize-under-binder/issue-62529-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/trait-bounds/normalize-under-binder/issue-62529-1.rs) |
| build | cannot infer type parameter `…` | 1 | [trait-bounds/normalize-under-binder/issue-80706.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/trait-bounds/normalize-under-binder/issue-80706.rs) |
| check | cannot infer type parameter `…`; annotate the type | 1 | [subtype/return-static.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/subtype/return-static.rs) |
| check | no method `…` on type `…` | 1 | [trait-bounds/normalize-under-binder/issue-44005.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/trait-bounds/normalize-under-binder/issue-44005.rs) |
| check | type annotations needed for `…` | 1 | [trait-bounds/normalize-under-binder/issue-62529-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/higher-ranked/trait-bounds/normalize-under-binder/issue-62529-2.rs) |
| | 9 other reasons | 9 | |

</details>

<details><summary><code>typeck</code>: 40 failing (pass tests 22 / 27 · compile-fail 154 / 189)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 17 | [call-non-fn-type-param.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/call-non-fn-type-param.rs), [const-in-fn-call-generics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/const-in-fn-call-generics.rs), [index-out-of-bounds-on-partial-cmp-ice-114056.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/index-out-of-bounds-on-partial-cmp-ice-114056.rs) |
| fail, wrong line | cannot find value `…` | 2 | [issue-87181/tuple-field.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/issue-87181/tuple-field.rs), [issue-87181/tuple-method.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/issue-87181/tuple-method.rs) |
| fail, wrong line | cannot infer type parameter `…` | 2 | [bad-type-in-vec-contains.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/bad-type-in-vec-contains.rs), [bad-type-in-vec-push.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/bad-type-in-vec-push.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 2 | [issue-65611.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/issue-65611.rs), [suggestions/macro-shorthand-issue-140659.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/suggestions/macro-shorthand-issue-140659.rs) |
| fail, wrong line | return type cannot be a trait object without pointer indirection (E0746) | 2 | [return-dyn-type-mismatch-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/return-dyn-type-mismatch-2.rs), [return-dyn-type-mismatch.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/return-dyn-type-mismatch.rs) |
| check | conflicting implementations of trait `…` for `…` | 1 | [tuple-ref-order-distinct-impls.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/typeck/tuple-ref-order-distinct-impls.rs) |
| | 14 other reasons | 14 | |

</details>

<details><summary><code>closures</code>: 39 failing (pass tests 102 / 120 · compile-fail 94 / 115)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 14 | [2229_closure_analysis/issue-90465.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/issue-90465.rs), [2229_closure_analysis/match/issue-88331.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/match/issue-88331.rs), [2229_closure_analysis/migrations/auto_traits.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/migrations/auto_traits.rs) |
| fail, wrong line | moving a captured value out of a closure is not supported | 4 | [2229_closure_analysis/diagnostics/closure-origin-tuple-diagnostics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/diagnostics/closure-origin-tuple-diagnostics.rs), [2229_closure_analysis/diagnostics/repr_packed.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/diagnostics/repr_packed.rs), [2229_closure_analysis/migrations/multi_diagnostics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/migrations/multi_diagnostics.rs) |
| check | cannot infer the type of this closure parameter; add a type annotation | 3 | [closure-capture-hrtb-gat-no-ice-120811.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/closure-capture-hrtb-gat-no-ice-120811.rs), [deduce-from-object-supertrait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/deduce-from-object-supertrait.rs), [ref-binding-lifetime-in-closure-pattern.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/ref-binding-lifetime-in-closure-pattern.rs) |
| run | `…` takes N type arguments, but N were given | 2 | [2229_closure_analysis/migrations/insignificant_drop.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/migrations/insignificant_drop.rs), [region-obligations-closure-and-projection.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/region-obligations-closure-and-projection.rs) |
| run | moving a captured value out of a closure is not supported | 2 | [2229_closure_analysis/migrations/precise_no_migrations.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/migrations/precise_no_migrations.rs), [2229_closure_analysis/run_pass/fru_syntax.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/run_pass/fru_syntax.rs) |
| check | a returned closure that captures must be `…` | 1 | [2229_closure_analysis/deref-mut-in-pattern.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/closures/2229_closure_analysis/deref-mut-in-pattern.rs) |
| | 13 other reasons | 13 | |

</details>

<details><summary><code>borrowck</code>: 34 failing (pass tests 85 / 88 · compile-fail 324 / 355)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 18 | [alias-liveness/escaping-bounds-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/borrowck/alias-liveness/escaping-bounds-2.rs), [array-slice-coercion-mismatch-15783.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/borrowck/array-slice-coercion-mismatch-15783.rs), [borrowck-in-static.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/borrowck/borrowck-in-static.rs) |
| check | parameter `…` is never used | 1 | [async-reference-generality.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/borrowck/async-reference-generality.rs) |
| fail, wrong line | `…` does not live long enough (dropped while still borrowed) | 1 | [span-semicolon-issue-139049.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/borrowck/span-semicolon-issue-139049.rs) |
| fail, wrong line | `…` doesn't implement `…` | 1 | [assign-op-invariance-lifetime.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/borrowck/assign-op-invariance-lifetime.rs) |
| fail, wrong line | `…` return types must be `…` | 1 | [issue-95079-missing-move-in-nested-closure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/borrowck/issue-95079-missing-move-in-nested-closure.rs) |
| fail, wrong line | `…` without initializer is not supported | 1 | [borrowck-uninit.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/borrowck/borrowck-uninit.rs) |
| | 11 other reasons | 11 | |

</details>

<details><summary><code>feature-gates</code>: 34 failing (pass tests 15 / 15 · compile-fail 152 / 186)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 30 | [bench.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/feature-gates/bench.rs), [feature-gate-allow-internal-unsafe-nested-macro.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/feature-gates/feature-gate-allow-internal-unsafe-nested-macro.rs), [feature-gate-allow-internal-unstable-nested-macro.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/feature-gates/feature-gate-allow-internal-unstable-nested-macro.rs) |
| fail, wrong line | `…` is only available in impls | 1 | [feature-gate-import-trait-associated-functions.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/feature-gates/feature-gate-import-trait-associated-functions.rs) |
| fail, wrong line | only fns and statics are supported in extern blocks | 1 | [feature-gate-extern_types.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/feature-gates/feature-gate-extern_types.rs) |
| fail, wrong line | the `…` attribute side-steps feature gating and stability checks (E0658) | 1 | [feature-gate-check-nested-macro-invocation.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/feature-gates/feature-gate-check-nested-macro-invocation.rs) |
| fail, wrong line | unsupported item | 1 | [feature-gated-feature-in-macro-arg.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/feature-gates/feature-gated-feature-in-macro-arg.rs) |

</details>

<details><summary><code>nll</code>: 34 failing (pass tests 67 / 76 · compile-fail 162 / 187)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 15 | [closure-requirements/thread_scope_incorrect_implied_bound.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/closure-requirements/thread_scope_incorrect_implied_bound.rs), [ice-106874.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/ice-106874.rs), [issue-50716.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/issue-50716.rs) |
| check | mismatched types: expected `…`, found `…` | 2 | [issue-27583.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/issue-27583.rs), [issue-57280.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/issue-57280.rs) |
| check | `…` return types must be `…` | 1 | [issue-63154-normalize.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/issue-63154-normalize.rs) |
| check | cannot find associated type `…` for `…` | 1 | [vimwiki-core-regression.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/vimwiki-core-regression.rs) |
| check | cannot find struct or variant | 1 | [user-annotations/issue-54570-bootstrapping.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/user-annotations/issue-54570-bootstrapping.rs) |
| check | cannot find value (unsupported path) | 1 | [issue-57280-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/nll/issue-57280-1.rs) |
| | 13 other reasons | 13 | |

</details>

<details><summary><code>generic-associated-types</code>: 31 failing (pass tests 39 / 45 · compile-fail 36 / 61)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 16 | [bugs/hrtb-implied-3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/bugs/hrtb-implied-3.rs), [collectivity-regression.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/collectivity-regression.rs), [const_params_have_right_type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/const_params_have_right_type.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 3 | [issue-68642-broken-llvm-ir.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/issue-68642-broken-llvm-ir.rs), [issue-68643-broken-mir.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/issue-68643-broken-mir.rs), [issue-92033.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/issue-92033.rs) |
| run | cannot find constant `…` | 3 | [const-generics-gat-in-trait-return-type-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/const-generics-gat-in-trait-return-type-1.rs), [const-generics-gat-in-trait-return-type-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/const-generics-gat-in-trait-return-type-2.rs), [const-generics-gat-in-trait-return-type-3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/const-generics-gat-in-trait-return-type-3.rs) |
| fail, wrong line | cannot find value `…` | 2 | [issue-68645-codegen-fulfillment.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/issue-68645-codegen-fulfillment.rs), [issue-68656-unsized-values.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/issue-68656-unsized-values.rs) |
| check | cannot find associated type `…` for `…` | 1 | [issue-87750.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/issue-87750.rs) |
| fail, wrong line | cannot move out of `…` because it is borrowed (at N:N) | 1 | [gat-bounds-not-checked-with-right-substitutions.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generic-associated-types/gat-bounds-not-checked-with-right-substitutions.rs) |
| | 5 other reasons | 5 | |

</details>

<details><summary><code>lifetimes</code>: 28 failing (pass tests 44 / 53 · compile-fail 109 / 128)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 9 | [conflicting-bounds.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lifetimes/conflicting-bounds.rs), [elided-lifetime-mismatch-in-self-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lifetimes/elided-lifetime-mismatch-in-self-type.rs), [elided-lint-in-mod.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lifetimes/elided-lint-in-mod.rs) |
| build | no method `…` on type `…` | 1 | [anonymize-unnamed-bound-vars-in-binders.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lifetimes/anonymize-unnamed-bound-vars-in-binders.rs) |
| check | cannot infer the type of this closure parameter; add a type annotation | 1 | [bare-trait-object-borrowck.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lifetimes/bare-trait-object-borrowck.rs) |
| check | only type arguments are supported | 1 | [issue-70917-lifetimes-in-fn-def.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lifetimes/issue-70917-lifetimes-in-fn-def.rs) |
| check | this variant is not a struct-like variant | 1 | [no-lt-elision-in-variant-path-seg-as-enum-path-seg-has-gen-args.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lifetimes/no-lt-elision-in-variant-path-seg-as-enum-path-seg-has-gen-args.rs) |
| check | unsupported item | 1 | [raw/three-tokens.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lifetimes/raw/three-tokens.rs) |
| | 14 other reasons | 14 | |

</details>

<details><summary><code>error-codes</code>: 26 failing (pass tests 1 / 1 · compile-fail 178 / 204)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 24 | [E0015.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/error-codes/E0015.rs), [E0017.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/error-codes/E0017.rs), [E0030.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/error-codes/E0030.rs) |
| fail, wrong line | cannot find value `…` | 1 | [E0719.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/error-codes/E0719.rs) |
| fail, wrong line | overflow in constant | 1 | [E0080.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/error-codes/E0080.rs) |

</details>

<details><summary><code>inference</code>: 26 failing (pass tests 23 / 27 · compile-fail 44 / 66)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 10 | [ambiguous-numeric-in-closure-ref.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/ambiguous-numeric-in-closure-ref.rs), [cannot-infer-iterator-sum-return-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/cannot-infer-iterator-sum-return-type.rs), [issue-12028.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/issue-12028.rs) |
| fail, wrong line | cannot find type `…` | 4 | [ambiguous-cast-suggestion-issue-41261.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/ambiguous-cast-suggestion-issue-41261.rs), [cannot-infer-async.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/cannot-infer-async.rs), [issue-104649.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/issue-104649.rs) |
| check | no method `…` on type `…` | 1 | [issue-70703.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/issue-70703.rs) |
| fail, wrong line | cannot find struct | 1 | [need_type_info/issue-103053.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/need_type_info/issue-103053.rs) |
| fail, wrong line | cannot infer the return type of this closure; annotate it (`…`) | 1 | [cannot-infer-partial-try-return.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/cannot-infer-partial-try-return.rs) |
| fail, wrong line | conflicting implementations of trait `…` for `…` | 1 | [issue-80816.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inference/issue-80816.rs) |
| | 8 other reasons | 8 | |

</details>

<details><summary><code>array-slice-vec</code>: 23 failing (pass tests 41 / 58 · compile-fail 14 / 20)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [array_const_index-0.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/array_const_index-0.rs), [array_const_index-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/array_const_index-1.rs), [issue-69103-extra-binding-subslice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/issue-69103-extra-binding-subslice.rs) |
| run | mismatched types: expected `…`, found `…` | 4 | [subslice-patterns-const-eval-match.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/subslice-patterns-const-eval-match.rs), [subslice-patterns-const-eval.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/subslice-patterns-const-eval.rs), [variance-vec-covariant.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/variance-vec-covariant.rs) |
| run | run: panicked: Oh no | 3 | [box-of-array-of-drop-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/box-of-array-of-drop-1.rs), [box-of-array-of-drop-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/box-of-array-of-drop-2.rs), [nested-vec-3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/nested-vec-3.rs) |
| run | array length must be a literal or a const | 2 | [huge-largest-array.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/huge-largest-array.rs), [vec-fixed-length.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/vec-fixed-length.rs) |
| build | cannot find constant `…` | 1 | [large-zst-array-compilation-time-68010.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/large-zst-array-compilation-time-68010.rs) |
| check | no method named `…` found for `…` | 1 | [infer_array_len.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/array-slice-vec/infer_array_len.rs) |
| | 8 other reasons | 8 | |

</details>

<details><summary><code>span</code>: 22 failing (pass tests 5 / 5 · compile-fail 63 / 85)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 11 | [E0493.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/E0493.rs), [import-ty-params.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/import-ty-params.rs), [issue-27522.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/issue-27522.rs) |
| fail, wrong line | mismatched types: expected a tuple of N elements, found `…` | 2 | [send-is-not-static-std-sync-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/send-is-not-static-std-sync-2.rs), [send-is-not-static-std-sync.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/send-is-not-static-std-sync.rs) |
| fail, wrong line | `…` has no implementation for `…` | 1 | [issue-26656.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/issue-26656.rs) |
| fail, wrong line | cannot find type `…` | 1 | [issue-33884.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/issue-33884.rs) |
| fail, wrong line | cannot infer the type of this `…`; annotate it | 1 | [mut-ptr-cant-outlive-ref.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/mut-ptr-cant-outlive-ref.rs) |
| fail, wrong line | impls for references to a type parameter (`…`) are supported only when they define each default method of the trait, forwarding to T's: `…` is left out | 1 | [issue-24805-dropck-trait-has-items.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/span/issue-24805-dropck-trait-has-items.rs) |
| | 5 other reasons | 5 | |

</details>

<details><summary><code>attributes</code>: 21 failing (pass tests 22 / 24 · compile-fail 48 / 67)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 15 | [attr-order-deprecated.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/attributes/attr-order-deprecated.rs), [check-builtin-attr-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/attributes/check-builtin-attr-ice.rs), [crate-name-empty.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/attributes/crate-name-empty.rs) |
| check | expected `…` (found `…`) | 1 | [dont-dup-expr-attrs.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/attributes/dont-dup-expr-attrs.rs) |
| fail, wrong line | cannot find type `…` | 1 | [ambiguous_derive_helpers.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/attributes/ambiguous_derive_helpers.rs) |
| fail, wrong line | cannot infer type parameter `…` | 1 | [rustc_confusables_std_cases.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/attributes/rustc_confusables_std_cases.rs) |
| fail, wrong line | couldn't read `…`: No such file or directory (os error N) | 1 | [extented-attribute-macro-error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/attributes/extented-attribute-macro-error.rs) |
| fail, wrong line | in `…`: nested items are not supported (Struct) | 1 | [dont-allow-inline-and-repr-at-invalid-positions.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/attributes/dont-allow-inline-and-repr-at-invalid-positions.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>drop</code>: 21 failing (pass tests 53 / 69 · compile-fail 8 / 13)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [drop-foreign-fundamental.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/drop-foreign-fundamental.rs), [lint-tail-expr-drop-order-borrowck.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/lint-tail-expr-drop-order-borrowck.rs), [tail_expr_drop_order-on-coroutine-unwind.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/tail_expr_drop_order-on-coroutine-unwind.rs) |
| run | cannot find value (unsupported path) | 3 | [move-closure-drop-on-unwind.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/move-closure-drop-on-unwind.rs), [panic-during-drop-14875.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/panic-during-drop-14875.rs), [repeat-drop.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/repeat-drop.rs) |
| run | cannot find type `…` | 2 | [box-conditional-drop-allocator.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/box-conditional-drop-allocator.rs), [dynamic-drop-async.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/dynamic-drop-async.rs) |
| run | run: panicked: assertion `…` failed | 2 | [drop-order-comparisons-let-chains.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/drop-order-comparisons-let-chains.rs), [if-let-guards-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/if-let-guards-2.rs) |
| run | run: panicked: explicit panic | 2 | [panic-during-slice-init.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/panic-during-slice-init.rs), [terminate-in-initializer.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/terminate-in-initializer.rs) |
| fail, wrong line | `…` return types must be `…` | 1 | [drop_elaboration_with_errors3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/drop/drop_elaboration_with_errors3.rs) |
| | 8 other reasons | 8 | |

</details>

<details><summary><code>methods</code>: 21 failing (pass tests 21 / 25 · compile-fail 51 / 68)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 16 | [bad-wf-when-selecting-method.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/methods/bad-wf-when-selecting-method.rs), [call-ambig-trait-object-method.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/methods/call-ambig-trait-object-method.rs), [disambiguate-multiple-impl.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/methods/disambiguate-multiple-impl.rs) |
| run | mismatched types: expected `…`, found `…` | 2 | [method-self-arg-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/methods/method-self-arg-trait.rs), [method-two-trait-defer-resolution-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/methods/method-two-trait-defer-resolution-1.rs) |
| fail, wrong line | no method `…` on type `…` | 1 | [dont-ice-on-object-lookup-w-error-region.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/methods/dont-ice-on-object-lookup-w-error-region.rs) |
| run | conflicting implementations of trait `…` for `…` | 1 | [method-two-trait-defer-resolution-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/methods/method-two-trait-defer-resolution-2.rs) |
| run | no method `…` on type `…` | 1 | [method-mut-self-modifies-mut-slice-lvalue.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/methods/method-mut-self-modifies-mut-slice-lvalue.rs) |

</details>

<details><summary><code>associated-consts</code>: 20 failing (pass tests 24 / 31 · compile-fail 13 / 26)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 12 | [associated-const-ambiguity-report.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/associated-const-ambiguity-report.rs), [associated-const-generic-obligations.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/associated-const-generic-obligations.rs), [associated-const-impl-wrong-lifetime.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/associated-const-impl-wrong-lifetime.rs) |
| run | in constant `…`: N:N: internal: unknown closure | 2 | [mismatched_impl_ty_2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/mismatched_impl_ty_2.rs), [mismatched_impl_ty_3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/mismatched_impl_ty_3.rs) |
| build | cannot find value (unsupported path) | 1 | [traits-associated-consts-ice-56870.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/traits-associated-consts-ice-56870.rs) |
| check | cannot find type `…` | 1 | [associated-const-trait-bound.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/associated-const-trait-bound.rs) |
| fail, wrong line | N:N: in constant `…`: N:N: cannot find value (unsupported path) | 1 | [associated-const-no-item.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/associated-const-no-item.rs) |
| run | cannot parse the arguments of `…` | 1 | [associated-const-range-match-patterns.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-consts/associated-const-range-match-patterns.rs) |
| | 2 other reasons | 2 | |

</details>

<details><summary><code>regions</code>: 20 failing (pass tests 89 / 97 · compile-fail 104 / 116)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 9 | [forall-wf-ref-reflexive.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/forall-wf-ref-reflexive.rs), [higher-ranked-implied.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/higher-ranked-implied.rs), [issue-101280.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/issue-101280.rs) |
| fail, wrong line | cannot find type `…` | 3 | [impl-trait-lifetime-conflict-hashmap-keys.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/impl-trait-lifetime-conflict-hashmap-keys.rs), [resolve-re-error-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/resolve-re-error-ice.rs), [wf-bound-region-in-local-soundness-issue-148854.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/wf-bound-region-in-local-soundness-issue-148854.rs) |
| run | the trait bound `…` is not satisfied (for `…`) (E0277) | 2 | [regions-early-bound-used-in-bound.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/regions-early-bound-used-in-bound.rs), [regions-early-bound-used-in-type-param.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/regions-early-bound-used-in-type-param.rs) |
| check | cannot find type `…` | 1 | [relate-bound-region-in-method-return.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/relate-bound-region-in-method-return.rs) |
| check | only type arguments are supported | 1 | [closure-prop-issue-148289.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/closure-prop-issue-148289.rs) |
| run | cannot find value (unsupported path) | 1 | [regions-mock-codegen.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/regions/regions-mock-codegen.rs) |
| | 3 other reasons | 3 | |

</details>

<details><summary><code>cast</code>: 18 failing (pass tests 24 / 29 · compile-fail 35 / 48)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 12 | [array-field-ptr-cast-14845.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cast/array-field-ptr-cast-14845.rs), [cast-char.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cast/cast-char.rs), [func-pointer-issue-140491.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cast/func-pointer-issue-140491.rs) |
| fail, wrong line | cannot find trait `…` | 1 | [dyn-any-to-fn-with-missing-generics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cast/dyn-any-to-fn-with-missing-generics.rs) |
| run | cannot find value (unsupported path) | 1 | [codegen-object-shim.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cast/codegen-object-shim.rs) |
| run | cannot infer type parameter `…`; use turbofish (`…`) or annotate the fn pointer type | 1 | [as-cast-unifies-raw-pointer-and-fn-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cast/as-cast-unifies-raw-pointer-and-fn-type.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [cast-rfc0401.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cast/cast-rfc0401.rs) |
| run | the trait `…` is not implemented for `…` | 1 | [trait-object-cast-segfault-4333.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cast/trait-object-cast-segfault-4333.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>trait-bounds</code>: 18 failing (pass tests 4 / 4 · compile-fail 15 / 33)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 17 | [apit-unsized.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/trait-bounds/apit-unsized.rs), [associated-error-bound-issue-145586.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/trait-bounds/associated-error-bound-issue-145586.rs), [duplicate-relaxed-bounds.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/trait-bounds/duplicate-relaxed-bounds.rs) |
| fail, wrong line | cannot find value (unsupported path) | 1 | [shadowed-path-in-trait-bound-suggestion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/trait-bounds/shadowed-path-in-trait-bound-suggestion.rs) |

</details>

<details><summary><code>derives</code>: 17 failing (pass tests 40 / 44 · compile-fail 32 / 45)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 11 | [clone-copy/duplicate-derive-copy-clone-diagnostics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/derives/clone-copy/duplicate-derive-copy-clone-diagnostics.rs), [clone-debug-dead-code-in-the-same-struct.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/derives/clone-debug-dead-code-in-the-same-struct.rs), [default/derives-span-Default.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/derives/default/derives-span-Default.rs) |
| check | unresolved import `…` (E0432) | 1 | [derive-renamed.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/derives/derive-renamed.rs) |
| fail, wrong line | missing generics for `…` | 1 | [deriving-with-repr-packed-move-errors.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/derives/deriving-with-repr-packed-move-errors.rs) |
| fail, wrong line | unsupported type | 1 | [debug/nonsense-input-to-debug.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/derives/debug/nonsense-input-to-debug.rs) |
| run | cannot find value (unsupported path) | 1 | [derive-hash-on-unit-struct.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/derives/derive-hash-on-unit-struct.rs) |
| run | in `…`: `…` cannot be compared with `…` | 1 | [eq-ord/deriving-eq-ord-boxed-slice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/derives/eq-ord/deriving-eq-ord-boxed-slice.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>mismatched_types</code>: 17 failing (compile-fail 63 / 80)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 10 | [closure-arg-type-mismatch.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/closure-arg-type-mismatch.rs), [collect-method-type-mismatch-66923.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/collect-method-type-mismatch-66923.rs), [const-fn-in-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/const-fn-in-trait.rs) |
| fail, wrong line | cannot find type `…` | 2 | [type-error-diagnostic-in-complex-return.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/type-error-diagnostic-in-complex-return.rs), [wrap-suggestion-privacy.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/wrap-suggestion-privacy.rs) |
| fail, wrong line | cannot infer type parameter `…` | 2 | [issue-35030.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/issue-35030.rs), [vec-hashset-type-mismatch.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/vec-hashset-type-mismatch.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 2 | [fn-variance-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/fn-variance-1.rs), [issue-26480.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/issue-26480.rs) |
| fail, wrong line | the name `…` is defined multiple times (E0428) | 1 | [similar_paths.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mismatched_types/similar_paths.rs) |

</details>

<details><summary><code>threads-sendsync</code>: 16 failing (pass tests 42 / 58 · compile-fail 1 / 1)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | `…` doesn't implement `…` | 2 | [task-comm-10.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/threads-sendsync/task-comm-10.rs), [task-comm-16.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/threads-sendsync/task-comm-16.rs) |
| run | cannot find type `…` | 2 | [std-sync-right-kind-impls.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/threads-sendsync/std-sync-right-kind-impls.rs), [tls-in-global-alloc.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/threads-sendsync/tls-in-global-alloc.rs) |
| check | cannot find value (unsupported path) | 1 | [recursive-thread-spawn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/threads-sendsync/recursive-thread-spawn.rs) |
| run | cannot infer the return type of this closure; annotate it (`…`) | 1 | [tls-try-with.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/threads-sendsync/tls-try-with.rs) |
| run | expected `…` (found `…`) | 1 | [sync-send-iterators-in-libcore.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/threads-sendsync/sync-send-iterators-in-libcore.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [send_str_hashmap.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/threads-sendsync/send_str_hashmap.rs) |
| | 8 other reasons | 8 | |

</details>

<details><summary><code>coherence</code>: 15 failing (pass tests 15 / 18 · compile-fail 27 / 39)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 11 | [associated-type2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coherence/associated-type2.rs), [coherence-fn-implied-bounds.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coherence/coherence-fn-implied-bounds.rs), [coherence-free-vs-bound-region.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coherence/coherence-free-vs-bound-region.rs) |
| check | cannot find type `…` | 1 | [pr-review-132289-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coherence/pr-review-132289-1.rs) |
| check | conflicting implementations of trait `…` for `…` | 1 | [error-type-coherence-no-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coherence/error-type-coherence-no-ice.rs) |
| fail, wrong line | cannot find associated type `…` for `…` | 1 | [illegal-copy-bad-projection.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coherence/illegal-copy-bad-projection.rs) |
| run | `…` is only allowed in impls | 1 | [coherence-impl-in-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coherence/coherence-impl-in-fn.rs) |

</details>

<details><summary><code>mir</code>: 15 failing (pass tests 36 / 47 · compile-fail 13 / 17)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [field-projection-mutating-context.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mir/field-projection-mutating-context.rs), [issue-112269.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mir/issue-112269.rs) |
| run | cannot find value (unsupported path) | 2 | [mir_drop_order.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mir/mir_drop_order.rs), [mir_let_chains_drop_order.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mir/mir_let_chains_drop_order.rs) |
| build | cannot find associated type `…` for `…` | 1 | [issue-107678-projection-with-lifetime.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mir/issue-107678-projection-with-lifetime.rs) |
| build | the trait bound `…` is not satisfied (for `…`) (E0277) | 1 | [issue-80949.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mir/issue-80949.rs) |
| build | unsupported cast `…` as `…` | 1 | [issue-75419-validation-impl-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mir/issue-75419-validation-impl-trait.rs) |
| fail, wrong line | a returned closure that captures must be `…` | 1 | [mir-build-2021-closure-capture-ice-110453-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/mir/mir-build-2021-closure-capture-ice-110453-1.rs) |
| | 7 other reasons | 7 | |

</details>

<details><summary><code>dropck</code>: 14 failing (pass tests 16 / 18 · compile-fail 9 / 21)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 12 | [constrained_by_assoc_type_equality_and_self_ty.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dropck/constrained_by_assoc_type_equality_and_self_ty.rs), [drop-impl-for-type-param-with-trait-bound.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dropck/drop-impl-for-type-param-with-trait-bound.rs), [drop-impl-sized-bound-mismatch.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dropck/drop-impl-sized-bound-mismatch.rs) |
| check | cannot infer the return type of this closure; annotate it (`…`) | 2 | [coroutine-liveness-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dropck/coroutine-liveness-1.rs), [coroutine-liveness-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dropck/coroutine-liveness-2.rs) |

</details>

<details><summary><code>iterators</code>: 14 failing (pass tests 8 / 17 · compile-fail 14 / 19)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| check | no method `…` on type `…` | 3 | [into-iter-on-boxed-slices-2021.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/into-iter-on-boxed-slices-2021.rs), [into-iter-on-boxed-slices-2024.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/into-iter-on-boxed-slices-2024.rs), [rsplit-clone.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/rsplit-clone.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 2 | [invalid-iterator-chain-fixable.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/invalid-iterator-chain-fixable.rs), [invalid-iterator-chain.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/invalid-iterator-chain.rs) |
| run | no method `…` on type `…` | 2 | [into-iter-on-arrays-lint.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/into-iter-on-arrays-lint.rs), [into-iter-on-boxed-slices-lint.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/into-iter-on-boxed-slices-lint.rs) |
| check | cannot find type `…` | 1 | [into-iter-on-arrays-2021.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/into-iter-on-arrays-2021.rs) |
| check | mismatched types: expected `…`, found `…` | 1 | [into-iter-on-arrays-2018.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/into-iter-on-arrays-2018.rs) |
| check | ranges can only be used directly in `…` loops here | 1 | [array-of-ranges.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/iterators/array-of-ranges.rs) |
| | 4 other reasons | 4 | |

</details>

<details><summary><code>match</code>: 14 failing (pass tests 36 / 41 · compile-fail 36 / 45)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [intended-binding-pattern-is-const.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/intended-binding-pattern-is-const.rs), [issue-72896-non-partial-eq-const.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/issue-72896-non-partial-eq-const.rs), [match-range-fail-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/match-range-fail-2.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 2 | [match-incompat-type-semi.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/match-incompat-type-semi.rs), [match-tail-expr-never-type-error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/match-tail-expr-never-type-error.rs) |
| build | /root/.cargo/bin/wild failed: exit status: N | 1 | [match-stack-overflow-72933-.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/match-stack-overflow-72933-.rs) |
| check | cannot find value (unsupported path) | 1 | [large-match-mir-gen.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/large-match-mir-gen.rs) |
| check | this variant has fields: use `…` or `…` | 1 | [enum-and-break-in-match-issue-41213.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/enum-and-break-in-match-issue-41213.rs) |
| check | type annotations needed for `…` | 1 | [innocent-looking-match-crash-46964.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/match/innocent-looking-match-crash-46964.rs) |
| | 5 other reasons | 5 | |

</details>

<details><summary><code>privacy</code>: 14 failing (pass tests 15 / 15 · compile-fail 47 / 61)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 9 | [ctor-not-accessible-due-to-inaccessible-field-in-reexport.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/privacy/ctor-not-accessible-due-to-inaccessible-field-in-reexport.rs), [issue-30079.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/privacy/issue-30079.rs), [legacy-ctor-visibility.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/privacy/legacy-ctor-visibility.rs) |
| fail, wrong line | unsupported impl self type | 2 | [issue-111220-2-tuple-struct-fields-projection.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/privacy/issue-111220-2-tuple-struct-fields-projection.rs), [issue-111220-tuple-struct-fields.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/privacy/issue-111220-tuple-struct-fields.rs) |
| fail, wrong line | access to union field is unsafe and requires unsafe block (E0133) | 1 | [union-field-privacy-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/privacy/union-field-privacy-2.rs) |
| fail, wrong line | array length must be a literal or a const | 1 | [no-ice-on-inference-failure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/privacy/no-ice-on-inference-failure.rs) |
| fail, wrong line | cannot find value `…` | 1 | [destructure-box-private-fields.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/privacy/destructure-box-private-fields.rs) |

</details>

<details><summary><code>proc-macro</code>: 14 failing (pass tests 2 / 5 · compile-fail 12 / 23)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 5 | [export-macro.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/export-macro.rs), [exports.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/exports.rs), [issue-83469-global-alloc-invalid-stmt.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/issue-83469-global-alloc-invalid-stmt.rs) |
| fail, wrong line | cannot find type `…` | 4 | [attribute.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/attribute.rs), [define-two.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/define-two.rs), [invalid-attributes.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/invalid-attributes.rs) |
| check | cannot find type `…` | 2 | [allowed-signatures.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/allowed-signatures.rs), [proc-macro-deprecated-attr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/proc-macro-deprecated-attr.rs) |
| fail, wrong line | cannot find associated type `…` for `…` | 1 | [bad-projection.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/bad-projection.rs) |
| fail, wrong line | function `…` is private (E0603) | 1 | [test-same-crate.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/test-same-crate.rs) |
| run | cannot find type `…` | 1 | [derive-test.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/proc-macro/derive-test.rs) |

</details>

<details><summary><code>rust-2018</code>: 14 failing (pass tests 10 / 10 · compile-fail 4 / 18)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 12 | [async-ident-allowed.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2018/async-ident-allowed.rs), [async-ident.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2018/async-ident.rs), [dyn-keyword.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2018/dyn-keyword.rs) |
| fail, wrong line | at least one trait is required for an object type (found `…`) | 1 | [dyn-trait-compatibility.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2018/dyn-trait-compatibility.rs) |
| fail, wrong line | unresolved import `…` (E0432) | 1 | [uniform-paths/prelude-fail-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2018/uniform-paths/prelude-fail-2.rs) |

</details>

<details><summary><code>self</code>: 14 failing (pass tests 47 / 55 · compile-fail 33 / 39)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [arbitrary_self_types_needing_box_or_arc_wrapping.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/arbitrary_self_types_needing_box_or_arc_wrapping.rs), [dispatch-dyn-incompatible-that-does-not-deref.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/dispatch-dyn-incompatible-that-does-not-deref.rs), [issue-61882.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/issue-61882.rs) |
| run | mismatched types: expected `…`, found `…` | 4 | [arbitrary_self_types_nested.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/arbitrary_self_types_nested.rs), [arbitrary_self_types_struct.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/arbitrary_self_types_struct.rs), [arbitrary_self_types_trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/arbitrary_self_types_trait.rs) |
| check | mismatched types: expected `…`, found `…` | 1 | [arbitrary_self_types_pin_lifetime.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/arbitrary_self_types_pin_lifetime.rs) |
| fail, wrong line | array length must be a literal or a const | 1 | [arbitrary-self-from-method-substs-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/arbitrary-self-from-method-substs-ice.rs) |
| fail, wrong line | expected a type (found `…`) | 1 | [self-vs-path-ambiguity.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/self-vs-path-ambiguity.rs) |
| run | `…` has no implementation for `…` | 1 | [self-impl-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/self/self-impl-2.rs) |
| | 2 other reasons | 2 | |

</details>

<details><summary><code>wf</code>: 14 failing (pass tests 3 / 3 · compile-fail 25 / 39)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 11 | [closure-wf.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/wf/closure-wf.rs), [hir-wf-canonicalized.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/wf/hir-wf-canonicalized.rs), [issue-103573.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/wf/issue-103573.rs) |
| fail, wrong line | cannot find associated type `…` for `…` | 2 | [wf-foreign-fn-decl-ret.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/wf/wf-foreign-fn-decl-ret.rs), [wf-packed-on-proj-of-type-as-unimpl-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/wf/wf-packed-on-proj-of-type-as-unimpl-trait.rs) |
| fail, wrong line | only type arguments are supported | 1 | [wf-fn-def-check-sig-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/wf/wf-fn-def-check-sig-1.rs) |

</details>

<details><summary><code>associated-type-bounds</code>: 13 failing (pass tests 27 / 31 · compile-fail 13 / 22)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 8 | [ambiguous-associated-type2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-type-bounds/ambiguous-associated-type2.rs), [cant-see-copy-bound-from-child-rigid-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-type-bounds/cant-see-copy-bound-from-child-rigid-2.rs), [consts.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-type-bounds/consts.rs) |
| check | cannot find type `…` | 2 | [bad-bounds-on-assoc-in-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-type-bounds/bad-bounds-on-assoc-in-trait.rs), [trait-params.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-type-bounds/trait-params.rs) |
| check | no method `…` on type `…` | 1 | [nested-bounds-dont-eliminate-alias-bounds.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-type-bounds/nested-bounds-dont-eliminate-alias-bounds.rs) |
| fail, wrong line | parameter `…` is never used | 1 | [overlaping-bound-suggestion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-type-bounds/overlaping-bound-suggestion.rs) |
| run | parameter `…` is never used | 1 | [struct-bounds.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/associated-type-bounds/struct-bounds.rs) |

</details>

<details><summary><code>coercion</code>: 13 failing (pass tests 36 / 45 · compile-fail 22 / 26)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [coerce-block-tail-83783.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coercion/coerce-block-tail-83783.rs), [retslot-cast.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coercion/retslot-cast.rs), [structural_identity_dependent_reborrows.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coercion/structural_identity_dependent_reborrows.rs) |
| check | cannot infer type parameter `…` | 1 | [struct-coerce-vec-to-slice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coercion/struct-coerce-vec-to-slice.rs) |
| check | mismatched types: expected `…`, found `…` | 1 | [issue-37655.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coercion/issue-37655.rs) |
| run | `…` doesn't implement `…` | 1 | [any-trait-object-debug-12744.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coercion/any-trait-object-debug-12744.rs) |
| run | `…` has no implementation for `…` | 1 | [trait-object-coercion-distribution-9951.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coercion/trait-object-coercion-distribution-9951.rs) |
| run | cannot find trait `…` | 1 | [fn-item-to-dyn-any.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coercion/fn-item-to-dyn-any.rs) |
| | 4 other reasons | 4 | |

</details>

<details><summary><code>conditional-compilation</code>: 12 failing (pass tests 5 / 5 · compile-fail 9 / 21)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 11 | [cfg-arg-invalid-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/conditional-compilation/cfg-arg-invalid-1.rs), [cfg-arg-invalid-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/conditional-compilation/cfg-arg-invalid-2.rs), [cfg-arg-invalid-3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/conditional-compilation/cfg-arg-invalid-3.rs) |
| fail, wrong line | no `…` function | 1 | [cfg-arg-invalid-7.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/conditional-compilation/cfg-arg-invalid-7.rs) |

</details>

<details><summary><code>type</code>: 12 failing (pass tests 5 / 6 · compile-fail 54 / 65)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 7 | [closure-with-wrong-borrows.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type/closure-with-wrong-borrows.rs), [issue-58355.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type/issue-58355.rs), [type-check-defaults.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type/type-check-defaults.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 2 | [type-check/point-at-inference-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type/type-check/point-at-inference-2.rs), [type-check/point-at-inference.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type/type-check/point-at-inference.rs) |
| fail, wrong line | expected `…` (found `…`) | 1 | [ascription/issue-34255-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type/ascription/issue-34255-1.rs) |
| fail, wrong line | type annotations needed for `…` | 1 | [type-check/point-at-inference-4.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type/type-check/point-at-inference-4.rs) |
| run | cannot infer the `…` return type of `…` | 1 | [type-name-basic.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type/type-name-basic.rs) |

</details>

<details><summary><code>unboxed-closures</code>: 12 failing (pass tests 32 / 40 · compile-fail 19 / 23)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [unboxed-closure-sugar-not-used-on-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/unboxed-closure-sugar-not-used-on-fn.rs), [unboxed-closures-unsafe-extern-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/unboxed-closures-unsafe-extern-fn.rs), [unboxed-closures-wrong-abi.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/unboxed-closures-wrong-abi.rs) |
| run | moving a captured value out of a closure is not supported | 4 | [unboxed-closures-drop.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/unboxed-closures-drop.rs), [unboxed-closures-infer-fnonce-move.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/unboxed-closures-infer-fnonce-move.rs), [unboxed-closures-infer-fnonce.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/unboxed-closures-infer-fnonce.rs) |
| check | `…` is not a function type | 1 | [callback-trait-prediction-70746.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/callback-trait-prediction-70746.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [unboxed-closures-monomorphization.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/unboxed-closures-monomorphization.rs) |
| run | no method `…` on type `…` | 1 | [self-param-space-conflict-in-unboxed-closure-18685.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/self-param-space-conflict-in-unboxed-closure-18685.rs) |
| run | run: panicked: assertion failed: a != b | 1 | [type-id-higher-rank.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unboxed-closures/type-id-higher-rank.rs) |

</details>

<details><summary><code>c-variadic</code>: 11 failing (pass tests 1 / 8 · compile-fail 6 / 10)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | no method `…` on type `…` | 3 | [inherent-method.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/inherent-method.rs), [trait-method.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/trait-method.rs), [valid.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/valid.rs) |
| fail | accepted: no error reported | 2 | [not-async.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/not-async.rs), [reject-varargs-without-pattern-post-expansion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/reject-varargs-without-pattern-post-expansion.rs) |
| run | expected N arguments, found N | 2 | [copy.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/copy.rs), [same-program-multiple-abis-x86_64.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/same-program-multiple-abis-x86_64.rs) |
| check | expected N arguments, found N | 1 | [variadic-unreachable-arg-error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/variadic-unreachable-arg-error.rs) |
| fail, wrong line | cannot find type `…` | 1 | [variadic-ffi-4.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/variadic-ffi-4.rs) |
| fail, wrong line | no method `…` on type `…` | 1 | [not-dyn-compatible.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/c-variadic/not-dyn-compatible.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>generics</code>: 11 failing (pass tests 40 / 42 · compile-fail 45 / 54)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 7 | [export-name-on-generics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generics/export-name-on-generics.rs), [generic-no-mangle.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generics/generic-no-mangle.rs), [generics-on-self-mod-segment.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generics/generics-on-self-mod-segment.rs) |
| fail, wrong line | unsupported impl self type | 1 | [generic-struct-self-unconstrained-inference-vars-69306.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generics/generic-struct-self-unconstrained-inference-vars-69306.rs) |
| fail, wrong line | unsupported item | 1 | [wrong-number-of-args.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generics/wrong-number-of-args.rs) |
| run | cannot find type `…` | 1 | [mid-path-type-params.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generics/mid-path-type-params.rs) |
| run | run: panicked: assertion `…` failed | 1 | [issue-32498.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/generics/issue-32498.rs) |

</details>

<details><summary><code>packed</code>: 11 failing (pass tests 10 / 16 · compile-fail 2 / 7)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 5 | [issue-27060-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/issue-27060-2.rs), [issue-27060.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/issue-27060.rs), [packed-struct-borrow-element-64bit.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/packed-struct-borrow-element-64bit.rs) |
| run | run: panicked: assertion `…` failed | 3 | [packed-struct-optimized-enum.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/packed-struct-optimized-enum.rs), [packed-struct-size.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/packed-struct-size.rs), [packed-tuple-struct-size.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/packed-tuple-struct-size.rs) |
| run | cannot transmute between types of different sizes, or dependently-sized types (E0512): `…` (N bits) to `…` (N bits) | 2 | [issue-118537-field-offset-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/issue-118537-field-offset-ice.rs), [issue-118537-field-offset.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/issue-118537-field-offset.rs) |
| run | no method `…` on type `…` | 1 | [dyn-trait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/packed/dyn-trait.rs) |

</details>

<details><summary><code>union</code>: 11 failing (pass tests 22 / 26 · compile-fail 23 / 30)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [issue-41073.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/issue-41073.rs), [projection-as-union-type-error-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/projection-as-union-type-error-2.rs), [union-repr-c.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/union-repr-c.rs) |
| fail, wrong line | the trait bound `…` is not satisfied (E0277) | 2 | [issue-81199.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/issue-81199.rs), [union-deref.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/union-deref.rs) |
| check | unsupported expression in constant | 1 | [union-const-eval.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/union-const-eval.rs) |
| fail, wrong line | cannot find associated type `…` for `…` | 1 | [projection-as-union-type-error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/projection-as-union-type-error.rs) |
| run | expected identifier, found keyword `…` (found `…`) | 1 | [union-backcomp.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/union-backcomp.rs) |
| run | literal patterns need an integer, char, bool or string | 1 | [union-pat-refutability.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/union/union-pat-refutability.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>binop</code>: 10 failing (pass tests 13 / 16 · compile-fail 26 / 33)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [binop-consume-args.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/binop-consume-args.rs), [borrow-suggestion-109352.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/borrow-suggestion-109352.rs), [issue-93927.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/issue-93927.rs) |
| fail, wrong line | expression is not assignable | 2 | [false-binop-caused-by-missing-semi.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/false-binop-caused-by-missing-semi.rs), [nested-assignment-may-be-deref.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/nested-assignment-may-be-deref.rs) |
| run | mismatched types: expected `…`, found `…` | 2 | [binops-issue-22743.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/binops-issue-22743.rs), [operator-multidispatch.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/operator-multidispatch.rs) |
| fail, wrong line | cannot multiply `…` by `…` (E0369) | 1 | [multiply-is-deref-on-rhs.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/multiply-is-deref-on-rhs.rs) |
| run | cannot transmute between types of different sizes, or dependently-sized types (E0512): `…` (N bits) to `…` (N bits) | 1 | [augmented-assignment.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binop/augmented-assignment.rs) |

</details>

<details><summary><code>destructuring-assignment</code>: 10 failing (pass tests 2 / 9 · compile-fail 8 / 11)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | expression is not assignable | 4 | [nested_destructure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/nested_destructure.rs), [slice_destructure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/slice_destructure.rs), [tuple_destructure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/tuple_destructure.rs) |
| fail, wrong line | cannot infer the type of this `…`; annotate it | 2 | [tuple_destructure_fail.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/tuple_destructure_fail.rs), [tuple_struct_destructure_fail.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/tuple_struct_destructure_fail.rs) |
| check | unsupported path | 1 | [struct-or-enum-variant-path.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/struct-or-enum-variant-path.rs) |
| fail, wrong line | expression is not assignable | 1 | [dont-suggest-wrong-expect-suggestion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/dont-suggest-wrong-expect-suggestion.rs) |
| run | `…` without initializer is not supported for this type | 1 | [drop-order.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/drop-order.rs) |
| run | expected an expression (found `…`) | 1 | [struct_destructure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/destructuring-assignment/struct_destructure.rs) |

</details>

<details><summary><code>unsized</code>: 10 failing (pass tests 7 / 13 · compile-fail 24 / 28)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [issue-30355.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/issue-30355.rs), [return-unsized-from-trait-method.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/return-unsized-from-trait-method.rs), [trait-method-unsized-self-param.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/trait-method-unsized-self-param.rs) |
| check | mismatched types: expected `…`, found `…` | 2 | [issue-40231-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/issue-40231-1.rs), [issue-75899-but-gats.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/issue-75899-but-gats.rs) |
| check | cannot infer type parameter `…` | 1 | [issue-40231-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/issue-40231-2.rs) |
| fail, wrong line | mismatched types: expected `…`, found `Box<or::{closure#N}<&dyn Fn(& 'a Something) -> Result <() ,() >, &dyn Fn(& 'a Something) -> Result <() ,() >, Result<(), | 1 | [issue-91801.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/issue-91801.rs) |
| run | cannot find value (unsupported path) | 1 | [nonnull-unsized-slice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/nonnull-unsized-slice.rs) |
| run | cannot transmute between types of different sizes, or dependently-sized types (E0512): `…` (N bits) to `…` (N bits) | 1 | [issue-23649-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsized/issue-23649-1.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>abi</code>: 9 failing (pass tests 34 / 42 · compile-fail 0 / 1)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | a pointer to an extern fn that takes or returns a struct by value is not supported | 2 | [abi-sysv64-arg-passing.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/abi/abi-sysv64-arg-passing.rs), [foreign/foreign-fn-with-byval.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/abi/foreign/foreign-fn-with-byval.rs) |
| run | passing this struct by value to an extern fn is not supported (only C structs of up to N bytes are) | 2 | [issue-28676.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/abi/issue-28676.rs), [struct-enums/struct-return.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/abi/struct-enums/struct-return.rs) |
| fail | accepted: no error reported | 1 | [invalid-self-parameter-type-56806.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/abi/invalid-self-parameter-type-56806.rs) |
| run | cannot find type `…` | 1 | [variadic-ffi.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/abi/variadic-ffi.rs) |
| run | expected N arguments, found N | 1 | [mir/mir_codegen_calls_variadic.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/abi/mir/mir_codegen_calls_variadic.rs) |
| run | run: panicked: assertion `…` failed | 1 | [nullable-pointer-ffi-compat.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/abi/nullable-pointer-ffi-compat.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>did_you_mean</code>: 9 failing (compile-fail 46 / 55)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 7 | [issue-105225-named-args.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/did_you_mean/issue-105225-named-args.rs), [issue-105225.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/did_you_mean/issue-105225.rs), [issue-21659-show-relevant-trait-impls-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/did_you_mean/issue-21659-show-relevant-trait-impls-1.rs) |
| fail, wrong line | expected `…` (found `…`) | 1 | [issue-87830-try-brackets-for-arrays.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/did_you_mean/issue-87830-try-brackets-for-arrays.rs) |
| fail, wrong line | macro expansion is too deep (recursion limit N) | 1 | [recursion_limit_macro.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/did_you_mean/recursion_limit_macro.rs) |

</details>

<details><summary><code>dst</code>: 9 failing (pass tests 10 / 16 · compile-fail 17 / 20)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | type `…` cannot be dereferenced | 2 | [dst-deref-mut.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dst/dst-deref-mut.rs), [dst-deref.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dst/dst-deref.rs) |
| fail | accepted: no error reported | 1 | [dst-index-fail.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dst/dst-index-fail.rs) |
| fail, wrong line | the name `…` is defined multiple times (E0428) | 1 | [dyn-fn-type-in-generic-enum.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dst/dyn-fn-type-in-generic-enum.rs) |
| fail, wrong line | the size for values of type `…` cannot be known at compilation time | 1 | [issue-113447.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dst/issue-113447.rs) |
| run | cannot index into a value of type `…` | 1 | [dst-index-success.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dst/dst-index-success.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [dst-coerce-rc.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dst/dst-coerce-rc.rs) |
| | 2 other reasons | 2 | |

</details>

<details><summary><code>dyn-compatibility</code>: 9 failing (pass tests 8 / 9 · compile-fail 26 / 34)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [assoc_type_bounds2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dyn-compatibility/assoc_type_bounds2.rs), [assoc_type_bounds_implicit_sized.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dyn-compatibility/assoc_type_bounds_implicit_sized.rs), [assoc_type_bounds_sized_used.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dyn-compatibility/assoc_type_bounds_sized_used.rs) |
| check | mismatched types: expected `…`, found `…` | 1 | [multiple-supers-should-work.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dyn-compatibility/multiple-supers-should-work.rs) |
| fail, wrong line | cannot find trait `…` | 1 | [sized-3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dyn-compatibility/sized-3.rs) |
| fail, wrong line | no `…` function | 1 | [ice-generics-of-crate-root-152335.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dyn-compatibility/ice-generics-of-crate-root-152335.rs) |
| fail, wrong line | no method `…` on type `…` | 1 | [spurious-dyn-compat-errors-58734.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dyn-compatibility/spurious-dyn-compat-errors-58734.rs) |
| fail, wrong line | the trait bound `…` is not satisfied (E0277) | 1 | [supertrait-with-self-in-type-arg.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/dyn-compatibility/supertrait-with-self-in-type-arg.rs) |

</details>

<details><summary><code>extern</code>: 9 failing (pass tests 21 / 21 · compile-fail 14 / 23)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 5 | [extern-main-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/extern/extern-main-fn.rs), [extern-main-issue-86110.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/extern/extern-main-issue-86110.rs), [extern-rust-fn-type-error-10764.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/extern/extern-rust-fn-type-error-10764.rs) |
| fail, wrong line | only fns and statics are supported in extern blocks | 2 | [issue-112363-extern-item-where-clauses-debug-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/extern/issue-112363-extern-item-where-clauses-debug-ice.rs), [issue-116203.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/extern/issue-116203.rs) |
| fail, wrong line | cannot find constant `…` | 1 | [extern-static-size-overflow.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/extern/extern-static-size-overflow.rs) |
| fail, wrong line | cannot find value (unsupported path) | 1 | [extern-crate-multiple-missing.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/extern/extern-crate-multiple-missing.rs) |

</details>

<details><summary><code>hygiene</code>: 9 failing (pass tests 9 / 14 · compile-fail 6 / 10)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [hygienic-label-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/hygiene/hygienic-label-1.rs), [hygienic-label-3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/hygiene/hygienic-label-3.rs), [missing-self-diag.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/hygiene/missing-self-diag.rs) |
| check | expected a literal (only literals, like `…`, `…` and `…`, can be passed to `…`) | 1 | [eager-from-opaque-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/hygiene/eager-from-opaque-2.rs) |
| check | format argument must be a string literal | 1 | [eager-from-opaque.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/hygiene/eager-from-opaque.rs) |
| run | cannot find value `…` | 1 | [hygiene.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/hygiene/hygiene.rs) |
| run | cannot infer the type of this closure parameter; add a type annotation | 1 | [lambda-var-hygiene.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/hygiene/lambda-var-hygiene.rs) |
| run | identifier `…` is bound more than once in the same pattern (E0416) (found `…`) | 1 | [issue-29746.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/hygiene/issue-29746.rs) |

</details>

<details><summary><code>implied-bounds</code>: 9 failing (pass tests 14 / 15 · compile-fail 6 / 14)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 5 | [assoc-ty-wf-used-to-get-assoc-ty.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/implied-bounds/assoc-ty-wf-used-to-get-assoc-ty.rs), [impl-implied-bounds-compatibility.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/implied-bounds/impl-implied-bounds-compatibility.rs), [issue-100690.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/implied-bounds/issue-100690.rs) |
| fail, wrong line | lifetime may not live long enough (`…` must outlive `…`) | 2 | [from-trait-impl.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/implied-bounds/from-trait-impl.rs), [impl-implied-bounds-compatibility-unnormalized.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/implied-bounds/impl-implied-bounds-compatibility-unnormalized.rs) |
| check | parameter `…` is never used | 1 | [dyn-erasure-no-tait.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/implied-bounds/dyn-erasure-no-tait.rs) |
| fail, wrong line | only type arguments are supported | 1 | [impl-header-unnormalized-types.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/implied-bounds/impl-header-unnormalized-types.rs) |

</details>

<details><summary><code>object-lifetime</code>: 9 failing (pass tests 10 / 13 · compile-fail 6 / 12)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [object-lifetime-default-ambiguous.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/object-lifetime/object-lifetime-default-ambiguous.rs), [object-lifetime-default-gat-binding-item-bounds.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/object-lifetime/object-lifetime-default-gat-binding-item-bounds.rs), [object-lifetime-default-gat-binding.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/object-lifetime/object-lifetime-default-gat-binding.rs) |
| fail, wrong line | `…` cannot be called through `…` (it uses `…` or generics) | 2 | [object-lifetime-default-assoc-ty-binding-item-bounds-non-static.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/object-lifetime/object-lifetime-default-assoc-ty-binding-item-bounds-non-static.rs), [object-lifetime-default-assoc-ty-binding-item-bounds.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/object-lifetime/object-lifetime-default-assoc-ty-binding-item-bounds.rs) |
| check | `…` cannot be called through `…` (it uses `…` or generics) | 1 | [object-lifetime-default-assoc-ty-binding-item-bounds-static.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/object-lifetime/object-lifetime-default-assoc-ty-binding-item-bounds-static.rs) |
| check | mismatched types: expected `…`, found `…` | 1 | [object-lifetime-default-trait-ref.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/object-lifetime/object-lifetime-default-trait-ref.rs) |
| check | this variant is not a struct-like variant | 1 | [object-lifetime-default-enum-variant.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/object-lifetime/object-lifetime-default-enum-variant.rs) |

</details>

<details><summary><code>panics</code>: 9 failing (pass tests 1 / 9 · compile-fail 0 / 1)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | cannot find value (unsupported path) | 5 | [catch-unwind-bang.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/catch-unwind-bang.rs), [panic-handler-chain.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/panic-handler-chain.rs), [panic-handler-flail-wildly.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/panic-handler-flail-wildly.rs) |
| run | run: panicked: explicit panic | 2 | [panic-in-dtor-drops-fields.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/panic-in-dtor-drops-fields.rs), [rvalue-cleanup-during-box-panic.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/rvalue-cleanup-during-box-panic.rs) |
| fail | accepted: no error reported | 1 | [panic-2021.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/panic-2021.rs) |
| run | cannot find value `…` | 1 | [nested_panic_caught.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/panics/nested_panic_caught.rs) |

</details>

<details><summary><code>recursion</code>: 9 failing (pass tests 6 / 7 · compile-fail 9 / 17)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [issue-23122-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/recursion/issue-23122-1.rs), [issue-26548-recursion-via-normalize.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/recursion/issue-26548-recursion-via-normalize.rs), [recursive-requirements.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/recursion/recursive-requirements.rs) |
| fail, wrong line | unsupported path in constant | 2 | [issue-23302-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/recursion/issue-23302-1.rs), [issue-23302-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/recursion/issue-23302-2.rs) |
| fail, wrong line | cannot find attribute `…` in this scope | 1 | [recursion_limit/invalid-attribute-105700.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/recursion/recursion_limit/invalid-attribute-105700.rs) |
| fail, wrong line | macro expansion is too deep (recursion limit N) | 1 | [recursion_limit/zero.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/recursion/recursion_limit/zero.rs) |
| run | cannot infer the return type of this closure; annotate it (`…`) | 1 | [future-stream-buffer-unordered-40003.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/recursion/future-stream-buffer-unordered-40003.rs) |

</details>

<details><summary><code>test-attrs</code>: 9 failing (pass tests 9 / 15 · compile-fail 5 / 8)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | run: stdout differs from the .run.stdout file | 3 | [test-passed.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/test-passed.rs), [tests-listing-format-default.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/tests-listing-format-default.rs), [tests-listing-format-terse.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/tests-listing-format-terse.rs) |
| fail | accepted: no error reported | 2 | [test-function-elided-no-main.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/test-function-elided-no-main.rs), [test-should-panic-attr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/test-should-panic-attr.rs) |
| check | function `…` is private (E0603) | 1 | [issue-53675-a-test-called-panic.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/issue-53675-a-test-called-panic.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 1 | [test-function-signature.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/test-function-signature.rs) |
| run | cannot find value (unsupported path) | 1 | [test-panic-while-printing.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/test-panic-while-printing.rs) |
| run | run: exit code N | 1 | [test-filter-multiple.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/test-attrs/test-filter-multiple.rs) |

</details>

<details><summary><code>type-inference</code>: 9 failing (pass tests 5 / 6 · compile-fail 8 / 16)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [panic-with-unspecified-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/panic-with-unspecified-type.rs), [sort_by_key.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/sort_by_key.rs), [unbounded-associated-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/unbounded-associated-type.rs) |
| fail, wrong line | cannot infer the type of this `…`; annotate it | 2 | [direct-self-reference-occurs-check.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/direct-self-reference-occurs-check.rs), [infinite-type-occurs-check.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/infinite-type-occurs-check.rs) |
| fail, wrong line | cannot infer type parameter `…` | 1 | [send-with-unspecified-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/send-with-unspecified-type.rs) |
| fail, wrong line | no method named `…` found for `…` | 1 | [or_else-multiple-type-params.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/or_else-multiple-type-params.rs) |
| fail, wrong line | type annotations needed for `…` | 1 | [issue-30225.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/issue-30225.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [partial-type-hint-12909.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-inference/partial-type-hint-12909.rs) |

</details>

<details><summary><code>layout</code>: 8 failing (pass tests 11 / 15 · compile-fail 9 / 13)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | run: panicked: assertion `…` failed | 2 | [issue-60431-unsized-tail-behind-projection.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/layout/issue-60431-unsized-tail-behind-projection.rs), [null-pointer-optimization-sizes.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/layout/null-pointer-optimization-sizes.rs) |
| fail | accepted: no error reported | 1 | [normalization-failure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/layout/normalization-failure.rs) |
| fail, wrong line | array length must be a literal or a const | 1 | [base-layout-is-sized-ice-123078.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/layout/base-layout-is-sized-ice-123078.rs) |
| fail, wrong line | cannot find associated type `…` for `…` | 1 | [ice-type-error-in-tail-124031.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/layout/ice-type-error-in-tail-124031.rs) |
| fail, wrong line | the size for values of type `…` cannot be known at compilation time (E0277) | 1 | [invalid-unsized-const-eval.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/layout/invalid-unsized-const-eval.rs) |
| run | cannot find constant `…` | 1 | [big-type-no-err.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/layout/big-type-no-err.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>let-else</code>: 8 failing (pass tests 10 / 14 · compile-fail 19 / 23)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [let-else-bool-binop-init.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/let-else/let-else-bool-binop-init.rs), [let-else-brace-before-else.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/let-else/let-else-brace-before-else.rs) |
| check | mismatched types: expected `…`, found `…` | 1 | [let-else-deref-coercion-annotated.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/let-else/let-else-deref-coercion-annotated.rs) |
| check | the trait bound `…` is not satisfied (for `…`) | 1 | [let-else-ref-bindings-pass.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/let-else/let-else-ref-bindings-pass.rs) |
| fail, wrong line | cannot find value `…` | 1 | [let-else-scope.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/let-else/let-else-scope.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 1 | [let-else-ref-bindings.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/let-else/let-else-ref-bindings.rs) |
| run | cannot find value (unsupported path) | 1 | [let-else-drop-order.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/let-else/let-else-drop-order.rs) |
| | 1 other reasons | 1 | |

</details>

<details><summary><code>lexer</code>: 8 failing (pass tests 3 / 7 · compile-fail 20 / 24)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [lex-bad-char-literals-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lexer/lex-bad-char-literals-1.rs), [lex-bare-cr-string-literal-doc-comment.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lexer/lex-bare-cr-string-literal-doc-comment.rs) |
| run | mismatched types: expected `…`, found `…` | 2 | [crlf-in-byte-string-literal.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lexer/crlf-in-byte-string-literal.rs), [lexer-crlf-line-endings-string-literal-doc-comment.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lexer/lexer-crlf-line-endings-string-literal-doc-comment.rs) |
| check | use of undeclared lifetime name `…` (E0261) | 1 | [ident_normalization.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lexer/ident_normalization.rs) |
| fail, wrong line | cannot parse string into token stream | 1 | [emoji-literal-prefix.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lexer/emoji-literal-prefix.rs) |
| fail, wrong line | no rules of `…` matched this call | 1 | [error-stage.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lexer/error-stage.rs) |
| run | in `…`: internal: `…` is missing from the prelude | 1 | [floating-point-0e10-issue-40408.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lexer/floating-point-0e10-issue-40408.rs) |

</details>

<details><summary><code>unsafe</code>: 8 failing (pass tests 9 / 10 · compile-fail 16 / 23)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 6 | [unsafe-around-compiler-generated-unsafe.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsafe/unsafe-around-compiler-generated-unsafe.rs), [unsafe-const-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsafe/unsafe-const-fn.rs), [unsafe-fn-used-as-value.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsafe/unsafe-fn-used-as-value.rs) |
| check | dereference of raw pointer is unsafe and requires unsafe block (E0133) | 1 | [place-expr-safe.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsafe/place-expr-safe.rs) |
| fail, wrong line | no method named `…` found for `…` | 1 | [issue-45107-unnecessary-unsafe-in-closure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/unsafe/issue-45107-unnecessary-unsafe-in-closure.rs) |

</details>

<details><summary><code>codegen</code>: 7 failing (pass tests 28 / 34 · compile-fail 0 / 1)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| build | mismatched types: expected `…`, found `…` | 2 | [issue-99551.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codegen/issue-99551.rs), [sub-principals-in-codegen.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codegen/sub-principals-in-codegen.rs) |
| build | use of unstable library feature `…` (E0658) | 1 | [subtyping-enforces-type-equality.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codegen/subtyping-enforces-type-equality.rs) |
| fail | accepted: no error reported | 1 | [no-mangle-on-panic-handler.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codegen/no-mangle-on-panic-handler.rs) |
| run | cannot find value (unsupported path) | 1 | [llvm-miscompile-metadata-invalidation-36023.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codegen/llvm-miscompile-metadata-invalidation-36023.rs) |
| run | cannot infer type parameter `…` | 1 | [issue-55976.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codegen/issue-55976.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [dynamic-size-of-prefix-correctly-36278.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codegen/dynamic-size-of-prefix-correctly-36278.rs) |

</details>

<details><summary><code>enum</code>: 7 failing (pass tests 28 / 29 · compile-fail 18 / 24)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [enum-discrim-too-small.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum/enum-discrim-too-small.rs), [enum-drop-cast-error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum/enum-drop-cast-error.rs), [enum-in-scope.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum/enum-in-scope.rs) |
| fail, wrong line | literal out of range for `…` | 2 | [enum-discrim-autosizing.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum/enum-discrim-autosizing.rs), [enum-to-float-cast.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum/enum-to-float-cast.rs) |
| fail, wrong line | unsupported path in constant | 1 | [enum-discriminant-missing-variant.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum/enum-discriminant-missing-variant.rs) |
| run | run: exit code -N | 1 | [enum-clike-ffi-as-int.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum/enum-clike-ffi-as-int.rs) |

</details>

<details><summary><code>enum-discriminant</code>: 7 failing (pass tests 9 / 13 · compile-fail 6 / 9)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [arbitrary_enum_discriminant-no-repr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum-discriminant/arbitrary_enum_discriminant-no-repr.rs), [ice-from-type-error-issue-148515.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum-discriminant/ice-from-type-error-issue-148515.rs) |
| check | cannot evaluate `…` in a constant | 1 | [size-of-discriminant-no-cycle.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum-discriminant/size-of-discriminant-no-cycle.rs) |
| fail, wrong line | not an integer constant | 1 | [issue-41394.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum-discriminant/issue-41394.rs) |
| run | cannot transmute between types of different sizes, or dependently-sized types (E0512): `…` (N bits) to `…` (N bits) | 1 | [niche-prefer-zero.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum-discriminant/niche-prefer-zero.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [issue-70509-partial_eq.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum-discriminant/issue-70509-partial_eq.rs) |
| run | not a literal | 1 | [issue-50689.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/enum-discriminant/issue-50689.rs) |

</details>

<details><summary><code>expr</code>: 7 failing (pass tests 24 / 25 · compile-fail 19 / 25)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail, wrong line | cannot infer the type of this closure parameter; add a type annotation | 3 | [malformed_closure/missing_block_in_let_binding.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/expr/malformed_closure/missing_block_in_let_binding.rs), [malformed_closure/ruby_style_closure.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/expr/malformed_closure/ruby_style_closure.rs), [malformed_closure/ruby_style_closure_parse_error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/expr/malformed_closure/ruby_style_closure_parse_error.rs) |
| build | /root/.cargo/bin/wild failed: exit status: N | 1 | [if/expr-stack-overflow.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/expr/if/expr-stack-overflow.rs) |
| fail | accepted: no error reported | 1 | [if/attrs/else-attrs.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/expr/if/attrs/else-attrs.rs) |
| fail, wrong line | expected `…` (found `…`) | 1 | [malformed_closure/missing_braces_around_block.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/expr/malformed_closure/missing_braces_around_block.rs) |
| fail, wrong line | unsupported path in constant | 1 | [issue-22933-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/expr/issue-22933-2.rs) |

</details>

<details><summary><code>modules</code>: 7 failing (pass tests 26 / 27 · compile-fail 4 / 10)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail, wrong line | file not found for module `…` (E0583) | 3 | [missing_non_modrs_mod.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/modules/missing_non_modrs_mod.rs), [path-invalid-form.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/modules/path-invalid-form.rs), [path-macro.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/modules/path-macro.rs) |
| fail | accepted: no error reported | 1 | [mod_file_disambig.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/modules/mod_file_disambig.rs) |
| fail, wrong line | file not found for module `…` | 1 | [missing_non_modrs_mod_inline.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/modules/missing_non_modrs_mod_inline.rs) |
| fail, wrong line | unsupported item | 1 | [issue-56411.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/modules/issue-56411.rs) |
| run | cannot find value (unsupported path) | 1 | [use-keyword-reexport-type-alias.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/modules/use-keyword-reexport-type-alias.rs) |

</details>

<details><summary><code>moves</code>: 7 failing (pass tests 10 / 10 · compile-fail 42 / 49)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [issue-34721.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/moves/issue-34721.rs), [move-out-of-slice-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/moves/move-out-of-slice-2.rs), [region-var-in-moved-ty-issue-133118.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/moves/region-var-in-moved-ty-issue-133118.rs) |
| fail, wrong line | `…` without initializer is not supported | 1 | [assign-value-after-at-pattern-issue-145564.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/moves/assign-value-after-at-pattern-issue-145564.rs) |
| fail, wrong line | cannot find value (unsupported path) | 1 | [suggest-clone-when-some-obligation-is-unmet.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/moves/suggest-clone-when-some-obligation-is-unmet.rs) |
| fail, wrong line | cannot infer type parameter `…`; use turbofish (`…`) or annotate the fn pointer type | 1 | [use-correct-generic-args-in-borrow-suggest.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/moves/use-correct-generic-args-in-borrow-suggest.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 1 | [move-fn-self-receiver.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/moves/move-fn-self-receiver.rs) |

</details>

<details><summary><code>or-patterns</code>: 7 failing (pass tests 22 / 24 · compile-fail 13 / 18)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [fn-param-wrap-parens.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/or-patterns/fn-param-wrap-parens.rs), [or-patterns-syntactic-fail-2018.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/or-patterns/or-patterns-syntactic-fail-2018.rs), [remove-leading-vert.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/or-patterns/remove-leading-vert.rs) |
| check | array length must be a literal or a const | 1 | [const-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/or-patterns/const-fn.rs) |
| fail, wrong line | cannot find value `…` | 1 | [nested-undelimited-precedence.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/or-patterns/nested-undelimited-precedence.rs) |
| fail, wrong line | expected identifier, found keyword `…` (found `…`) | 1 | [issue-64879-trailing-before-guard.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/or-patterns/issue-64879-trailing-before-guard.rs) |
| run | cannot find value `…` | 1 | [macro-pat.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/or-patterns/macro-pat.rs) |

</details>

<details><summary><code>range</code>: 7 failing (pass tests 4 / 6 · compile-fail 11 / 16)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [exclusive-range-patterns-2021.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/range/exclusive-range-patterns-2021.rs), [range_traits-1.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/range/range_traits-1.rs) |
| fail, wrong line | bad range pattern (found `…`) | 1 | [range-inclusive-pattern-precedence.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/range/range-inclusive-pattern-precedence.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 1 | [new_range_stability.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/range/new_range_stability.rs) |
| fail, wrong line | missing generics for `…` | 1 | [issue-73553-misinterp-range-literal.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/range/issue-73553-misinterp-range-literal.rs) |
| run | no method named `…` found for `…` | 1 | [range_inclusive.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/range/range_inclusive.rs) |
| run | the trait bound `…` is not satisfied (for `…`) | 1 | [range-implement-clone.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/range/range-implement-clone.rs) |

</details>

<details><summary><code>reachable</code>: 7 failing (pass tests 8 / 10 · compile-fail 14 / 19)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [expr_return_in_macro.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/reachable/expr_return_in_macro.rs), [expr_while.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/reachable/expr_while.rs), [unreachable-in-call.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/reachable/unreachable-in-call.rs) |
| check | cannot infer the `…` return type of `…` | 1 | [expr_cast.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/reachable/expr_cast.rs) |
| check | type annotations needed for `…` | 1 | [unreachable-question-mark-forbid.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/reachable/unreachable-question-mark-forbid.rs) |
| fail, wrong line | cannot find struct or variant | 1 | [unreachable-arm.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/reachable/unreachable-arm.rs) |
| fail, wrong line | cannot infer the type of this `…`; annotate it | 1 | [expr_assign.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/reachable/expr_assign.rs) |

</details>

<details><summary><code>rustdoc</code>: 7 failing (pass tests 3 / 3 · compile-fail 4 / 11)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 7 | [check-doc-alias-attr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rustdoc/check-doc-alias-attr.rs), [deny-invalid-doc-attrs.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rustdoc/deny-invalid-doc-attrs.rs), [doc-alias-same-name.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rustdoc/doc-alias-same-name.rs) |

</details>

<details><summary><code>statics</code>: 7 failing (pass tests 19 / 20 · compile-fail 12 / 18)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 6 | [check-immutable-mut-slices.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/statics/check-immutable-mut-slices.rs), [extern-static-promotion-rejected.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/statics/extern-static-promotion-rejected.rs), [mutable_memory_validation.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/statics/mutable_memory_validation.rs) |
| run | cannot find associated type `…` for `…` | 1 | [static-lazy-init-with-arena-set.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/statics/static-lazy-init-with-arena-set.rs) |

</details>

<details><summary><code>structs-enums</code>: 7 failing (pass tests 52 / 58 · compile-fail 16 / 17)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | run: panicked: assertion `…` failed | 2 | [align-enum.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs-enums/align-enum.rs), [nonzero-enum.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs-enums/nonzero-enum.rs) |
| fail | accepted: no error reported | 1 | [issue-2718-a.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs-enums/issue-2718-a.rs) |
| run | cannot find struct | 1 | [struct-aliases.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs-enums/struct-aliases.rs) |
| run | expected an identifier (found `…`) | 1 | [numeric-fields.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs-enums/numeric-fields.rs) |
| run | run: panicked: assertion failed: is_aligned_to(&*large, 0x10000) | 1 | [align-struct.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs-enums/align-struct.rs) |
| run | run: panicked: assertion failed: mem::size_of::<Box<i32>>() == mem::size_of::<Nullable<Box<i32>>>() | 1 | [enum-layout-optimization.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs-enums/enum-layout-optimization.rs) |

</details>

<details><summary><code>transmute</code>: 7 failing (pass tests 4 / 4 · compile-fail 4 / 11)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 7 | [ambiguity-in-closure-arg.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/transmute/ambiguity-in-closure-arg.rs), [diverging-fn-transmute.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/transmute/diverging-fn-transmute.rs), [recursive-size-skeleton.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/transmute/recursive-size-skeleton.rs) |

</details>

<details><summary><code>allocator</code>: 6 failing (pass tests 1 / 3 · compile-fail 0 / 4)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [function-allocator.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/allocator/function-allocator.rs), [not-an-allocator.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/allocator/not-an-allocator.rs) |
| fail, wrong line | cannot find type `…` | 2 | [allocator-args.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/allocator/allocator-args.rs), [two-allocators.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/allocator/two-allocators.rs) |
| run | cannot find value (unsupported path) | 1 | [weak-uninhabited-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/allocator/weak-uninhabited-type.rs) |
| run | cannot find value `…` | 1 | [dyn-compatible.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/allocator/dyn-compatible.rs) |

</details>

<details><summary><code>never_type</code>: 6 failing (pass tests 17 / 19 · compile-fail 17 / 21)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [fallback_change/defaulted-never-note.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/never_type/fallback_change/defaulted-never-note.rs), [fallback_change/fallback-closure-wrap.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/never_type/fallback_change/fallback-closure-wrap.rs), [regress/divergent-block-with-tail.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/never_type/regress/divergent-block-with-tail.rs) |
| fail, wrong line | cannot calculate the remainder of `…` divided by `…` (E0277) | 1 | [regress/span-bug-issue-121445.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/never_type/regress/span-bug-issue-121445.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [basic/never_coercions.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/never_type/basic/never_coercions.rs) |
| run | no method `…` on type `…` | 1 | [regress/never-type-fallback-option.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/never_type/regress/never-type-fallback-option.rs) |

</details>

<details><summary><code>rust-2021</code>: 6 failing (pass tests 9 / 13 · compile-fail 4 / 6)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| check | the trait bound `…` is not satisfied (for `…`) | 2 | [future-prelude-collision-turbofish.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2021/future-prelude-collision-turbofish.rs), [inherent-method-collision.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2021/inherent-method-collision.rs) |
| fail | accepted: no error reported | 2 | [future-prelude-collision-shadow.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2021/future-prelude-collision-shadow.rs), [panic.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2021/panic.rs) |
| check | `…` is only supported directly in a `…` loop | 1 | [array-into-iter-ambiguous.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2021/array-into-iter-ambiguous.rs) |
| check | cannot find value `…` | 1 | [future-prelude-collision.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2021/future-prelude-collision.rs) |

</details>

<details><summary><code>type-alias-enum-variants</code>: 6 failing (pass tests 2 / 5 · compile-fail 6 / 9)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail, wrong line | cannot find value (unsupported path) | 2 | [module-type-args-error.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-enum-variants/module-type-args-error.rs), [no-type-application-on-aliased-enum-variant.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-enum-variants/no-type-application-on-aliased-enum-variant.rs) |
| check | cannot find value (unsupported path) | 1 | [issue-61801-path-pattern-can-infer.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-enum-variants/issue-61801-path-pattern-can-infer.rs) |
| fail | accepted: no error reported | 1 | [enum-variant-priority-lint-ambiguous_associated_items.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-enum-variants/enum-variant-priority-lint-ambiguous_associated_items.rs) |
| run | literal patterns need an integer, char, bool or string | 1 | [type-alias-enum-variants-pass.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-enum-variants/type-alias-enum-variants-pass.rs) |
| run | mismatched types: expected `…`, found `…` | 1 | [enum-variant-generic-args-pass.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-enum-variants/enum-variant-generic-args-pass.rs) |

</details>

<details><summary><code>use</code>: 6 failing (pass tests 3 / 3 · compile-fail 15 / 21)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [attr-in-use-tree.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/use/attr-in-use-tree.rs), [pub-use-self-super-crate.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/use/pub-use-self-super-crate.rs), [use-nested-groups-unused-imports.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/use/use-nested-groups-unused-imports.rs) |
| fail, wrong line | cannot find trait `…` | 1 | [use-after-move-implicity-coerced-object.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/use/use-after-move-implicity-coerced-object.rs) |
| fail, wrong line | the name `…` is defined multiple times | 1 | [use-mod/use-mod-4.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/use/use-mod/use-mod-4.rs) |

</details>

<details><summary><code>autoref-autoderef</code>: 5 failing (pass tests 13 / 18 · compile-fail 2 / 2)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | mismatched types: expected `…`, found `…` | 2 | [autoderef-method-twice-but-not-thrice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/autoref-autoderef/autoderef-method-twice-but-not-thrice.rs), [autoderef-privacy.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/autoref-autoderef/autoderef-privacy.rs) |
| check | cannot find trait `…` | 1 | [auto-deref-on-cow-regression-91489.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/autoref-autoderef/auto-deref-on-cow-regression-91489.rs) |
| run | expected a function, found `…` | 1 | [autoderef-arc-boxed-closure-call.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/autoref-autoderef/autoderef-arc-boxed-closure-call.rs) |
| run | no method named `…` found for `…` | 1 | [autoderef-vec-to-slice-by-value.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/autoref-autoderef/autoderef-vec-to-slice-by-value.rs) |

</details>

<details><summary><code>cfg</code>: 5 failing (pass tests 16 / 17 · compile-fail 10 / 14)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [cfg-eval-derive-invalid-reparse-no-ice.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cfg/cfg-eval-derive-invalid-reparse-no-ice.rs), [invalid-cli-cfg-pred.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cfg/invalid-cli-cfg-pred.rs), [path-kw-as-cfg-pred.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cfg/path-kw-as-cfg-pred.rs) |
| run | function `…` should have a snake case name | 1 | [cfg_stmt_expr.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/cfg/cfg_stmt_expr.rs) |

</details>

<details><summary><code>diagnostic_namespace</code>: 5 failing (pass tests 4 / 4 · compile-fail 9 / 14)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [deny_malformed_attribute.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/diagnostic_namespace/deny_malformed_attribute.rs), [multiline_spans.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/diagnostic_namespace/multiline_spans.rs), [suggest_typos.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/diagnostic_namespace/suggest_typos.rs) |
| fail, wrong line | cannot find value (unsupported path) | 1 | [on_move/std_impls.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/diagnostic_namespace/on_move/std_impls.rs) |

</details>

<details><summary><code>fmt</code>: 5 failing (pass tests 8 / 10 · compile-fail 29 / 32)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [closing-brace-as-fill.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fmt/closing-brace-as-fill.rs), [ifmt-unimpl.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fmt/ifmt-unimpl.rs), [issue-89173.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fmt/issue-89173.rs) |
| check | internal: unknown closure | 1 | [format-with-yield-point.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fmt/format-with-yield-point.rs) |
| run | cannot find value `…` | 1 | [format-args-capture.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fmt/format-args-capture.rs) |

</details>

<details><summary><code>fn</code>: 5 failing (pass tests 9 / 11 · compile-fail 27 / 30)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [fn-item-type-note-late-bound-145558.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fn/fn-item-type-note-late-bound-145558.rs), [param-mismatch-no-names.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fn/param-mismatch-no-names.rs), [param-mismatch-trait-fn.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fn/param-mismatch-trait-fn.rs) |
| check | unsupported impl self type | 1 | [implied-bounds-impl-header-projections.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fn/implied-bounds-impl-header-projections.rs) |
| run | run: panicked: assertion `…` failed: addr: 0x115c52a0 | 1 | [dyn-fn-alignment.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/fn/dyn-fn-alignment.rs) |

</details>

<details><summary><code>repeat-expr</code>: 5 failing (pass tests 4 / 5 · compile-fail 8 / 12)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [copy-inference-side-effects-are-lazy.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/repeat-expr/copy-inference-side-effects-are-lazy.rs), [zero_diverging_consts.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/repeat-expr/zero_diverging_consts.rs), [zero_diverging_type_relative_consts.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/repeat-expr/zero_diverging_type_relative_consts.rs) |
| check | cannot infer the type of `…`; annotate it | 1 | [copy-inference-side-effects.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/repeat-expr/copy-inference-side-effects.rs) |
| fail, wrong line | cannot find constant `…` | 1 | [zero_infer_diverging_consts.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/repeat-expr/zero_infer_diverging_consts.rs) |

</details>

<details><summary><code>repr</code>: 5 failing (pass tests 4 / 4 · compile-fail 9 / 14)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 5 | [conflicting-repr-enum-attrs.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/repr/conflicting-repr-enum-attrs.rs), [repr-packed-contains-align.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/repr/repr-packed-contains-align.rs), [repr-transparent-other-reprs.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/repr/repr-transparent-other-reprs.rs) |

</details>

<details><summary><code>static</code>: 5 failing (pass tests 12 / 12 · compile-fail 20 / 25)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 5 | [raw-ref-deref-without-unsafe.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/static/raw-ref-deref-without-unsafe.rs), [static-drop-scope.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/static/static-drop-scope.rs), [static-mut-not-constant.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/static/static-mut-not-constant.rs) |

</details>

<details><summary><code>type-alias</code>: 5 failing (pass tests 5 / 6 · compile-fail 3 / 7)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [dummy-binder-102964.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias/dummy-binder-102964.rs), [issue-62263-self-in-atb.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias/issue-62263-self-in-atb.rs), [issue-62305-self-assoc-ty.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias/issue-62305-self-assoc-ty.rs) |
| check | array length must be a literal or a const | 1 | [lack-of-wfcheck.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias/lack-of-wfcheck.rs) |
| fail, wrong line | no method `…` on type `…` | 1 | [dyn-trait-type-alias-return-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias/dyn-trait-type-alias-return-type.rs) |

</details>

<details><summary><code>binding</code>: 4 failing (pass tests 92 / 95 · compile-fail 9 / 10)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| check | unsupported expression (Let) | 1 | [irrefutable-in-let-chains.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binding/irrefutable-in-let-chains.rs) |
| fail | accepted: no error reported | 1 | [const-param.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binding/const-param.rs) |
| run | cannot find value (unsupported path) | 1 | [fn-arg-incomplete-pattern-drop-order.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binding/fn-arg-incomplete-pattern-drop-order.rs) |
| run | range patterns need an integer or char | 1 | [match-range.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/binding/match-range.rs) |

</details>

<details><summary><code>box</code>: 4 failing (pass tests 21 / 22 · compile-fail 4 / 7)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [box-lifetime-argument-not-allowed.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/box/box-lifetime-argument-not-allowed.rs), [unit/unique-object-noncopyable.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/box/unit/unique-object-noncopyable.rs), [unit/unique-pinned-nocopy.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/box/unit/unique-pinned-nocopy.rs) |
| run | cannot infer type parameter `…`; annotate the type | 1 | [unit/unique-send.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/box/unit/unique-send.rs) |

</details>

<details><summary><code>for-loop-while</code>: 4 failing (pass tests 51 / 53 · compile-fail 14 / 16)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail, wrong line | the `…` block of `…` must diverge (return / break / panic) | 2 | [for-else-let-else-err.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/for-loop-while/for-else-let-else-err.rs), [while-else-let-else-err.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/for-loop-while/while-else-let-else-err.rs) |
| run | cannot infer type parameter `…` | 1 | [issue-51345.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/for-loop-while/issue-51345.rs) |
| run | type annotations needed for an empty array | 1 | [loop-break-value.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/for-loop-while/loop-break-value.rs) |

</details>

<details><summary><code>functions-closures</code>: 4 failing (pass tests 28 / 32 · compile-fail 2 / 2)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | in constant `…`: N:N: internal: unknown closure | 1 | [closure-to-fn-coercion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/functions-closures/closure-to-fn-coercion.rs) |
| run | run: panicked: assertion `…` failed | 1 | [nullable-pointer-opt-closures.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/functions-closures/nullable-pointer-opt-closures.rs) |
| run | type `…` cannot be dereferenced | 1 | [closure-expected-type/supply-just-return-type.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/functions-closures/closure-expected-type/supply-just-return-type.rs) |
| run | unsupported path | 1 | [fn-item-type-zero-sized.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/functions-closures/fn-item-type-zero-sized.rs) |

</details>

<details><summary><code>half-open-range-patterns</code>: 4 failing (pass tests 5 / 5 · compile-fail 7 / 11)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 1 | [half-open-range-pats-ref-ambiguous-interp.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/half-open-range-patterns/half-open-range-pats-ref-ambiguous-interp.rs) |
| fail, wrong line | expected `…` (found `…`) | 1 | [half-open-range-pats-inclusive-dotdotdot-bad-syntax.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/half-open-range-patterns/half-open-range-pats-inclusive-dotdotdot-bad-syntax.rs) |
| fail, wrong line | expected an identifier (found `…`) | 1 | [half-open-range-pats-inclusive-no-end.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/half-open-range-patterns/half-open-range-pats-inclusive-no-end.rs) |
| fail, wrong line | ranges can only be used directly in `…` loops here | 1 | [exclusive_range_pattern_syntax_collision3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/half-open-range-patterns/exclusive_range_pattern_syntax_collision3.rs) |

</details>

<details><summary><code>keyword</code>: 4 failing (pass tests 2 / 2 · compile-fail 9 / 13)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [keyword-self-as-identifier.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/keyword/keyword-self-as-identifier.rs), [keyword-super-as-identifier.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/keyword/keyword-super-as-identifier.rs), [keyword-super.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/keyword/keyword-super.rs) |

</details>

<details><summary><code>lub-glb</code>: 4 failing (pass tests 0 / 2 · compile-fail 1 / 3)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| check | only type arguments are supported | 2 | [empty-binder-future-compat.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lub-glb/empty-binder-future-compat.rs), [empty-binders.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lub-glb/empty-binders.rs) |
| fail | accepted: no error reported | 1 | [old-lub-glb-hr-eq.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lub-glb/old-lub-glb-hr-eq.rs) |
| fail, wrong line | only type arguments are supported | 1 | [empty-binders-err.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lub-glb/empty-binders-err.rs) |

</details>

<details><summary><code>malformed</code>: 4 failing (compile-fail 4 / 8)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 4 | [ignore-with-lint-name.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/malformed/ignore-with-lint-name.rs), [issue-69341-malformed-derive-inert.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/malformed/issue-69341-malformed-derive-inert.rs), [malformed-derive-entry.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/malformed/malformed-derive-entry.rs) |

</details>

<details><summary><code>overloaded</code>: 4 failing (pass tests 14 / 18)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | cannot index into a value of type `…` | 2 | [overloaded-autoderef-indexing.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/overloaded/overloaded-autoderef-indexing.rs), [overloaded-index-autoderef.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/overloaded/overloaded-index-autoderef.rs) |
| run | cannot infer type parameter `…` | 1 | [overloaded-deref-count.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/overloaded/overloaded-deref-count.rs) |
| run | run: panicked: assertion `…` failed | 1 | [overloaded-autoderef-order.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/overloaded/overloaded-autoderef-order.rs) |

</details>

<details><summary><code>parallel-rustc</code>: 4 failing (pass tests 3 / 3 · compile-fail 9 / 13)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [nested-type-alias-cycle-issue-129911.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parallel-rustc/nested-type-alias-cycle-issue-129911.rs), [trait-solver-global-cache.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parallel-rustc/trait-solver-global-cache.rs) |
| fail, wrong line | cannot find value `…` | 1 | [default-trait-shadow-cycle-issue-151358.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parallel-rustc/default-trait-shadow-cycle-issue-151358.rs) |
| fail, wrong line | the placeholder `…` is not allowed within types on item signatures for functions (E0121) | 1 | [variances-cycle-ice-154560.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/parallel-rustc/variances-cycle-ice-154560.rs) |

</details>

<details><summary><code>rust-2024</code>: 4 failing (pass tests 5 / 7 · compile-fail 10 / 12)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| check | no method `…` on type `…` | 2 | [prelude-migration/nested-use-statement.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2024/prelude-migration/nested-use-statement.rs), [prelude2024.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2024/prelude2024.rs) |
| fail | accepted: no error reported | 2 | [unsafe-extern-blocks/unsafe-extern-suggestion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2024/unsafe-extern-blocks/unsafe-extern-suggestion.rs), [unsafe-extern-blocks/unsafe-on-extern-block-issue-126756.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/rust-2024/unsafe-extern-blocks/unsafe-on-extern-block-issue-126756.rs) |

</details>

<details><summary><code>structs</code>: 4 failing (pass tests 29 / 31 · compile-fail 46 / 48)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail, wrong line | cannot find struct | 1 | [structure-constructor-type-mismatch.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs/structure-constructor-type-mismatch.rs) |
| fail, wrong line | expected `…`, or `…`, found `…` | 1 | [parse-error-with-type-param.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs/parse-error-with-type-param.rs) |
| run | cannot find struct | 1 | [struct-path-associated-type-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs/struct-path-associated-type-2.rs) |
| run | no method `…` on type `…` | 1 | [unit-like-struct-drop-run.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/structs/unit-like-struct-drop-run.rs) |

</details>

<details><summary><code>type-alias-impl-trait</code>: 4 failing (pass tests 6 / 8 · compile-fail 3 / 5)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| check | cannot find value (unsupported path) | 1 | [split-paths-may-dangle.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-impl-trait/split-paths-may-dangle.rs) |
| check | cannot infer the `…` return type of `…` | 1 | [match-unification.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-impl-trait/match-unification.rs) |
| fail | accepted: no error reported | 1 | [issue-98604.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-impl-trait/issue-98604.rs) |
| fail, wrong line | cannot infer the `…` return type of `…` | 1 | [issue-98608.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/type-alias-impl-trait/issue-98608.rs) |

</details>

<details><summary><code>uninhabited</code>: 4 failing (pass tests 4 / 6 · compile-fail 3 / 5)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [uninhabited-pin-field.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/uninhabited/uninhabited-pin-field.rs), [void-branch.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/uninhabited/void-branch.rs) |
| check | cannot find type `…` | 1 | [unreachable.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/uninhabited/unreachable.rs) |
| run | cannot find value (unsupported path) | 1 | [uninhabited-transparent-return-abi.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/uninhabited/uninhabited-transparent-return-abi.rs) |

</details>

<details><summary><code>argument-suggestions</code>: 3 failing (compile-fail 24 / 27)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 1 | [formal-and-expected-differ.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/argument-suggestions/formal-and-expected-differ.rs) |
| fail, wrong line | cannot infer type parameter `…` | 1 | [issue-96638.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/argument-suggestions/issue-96638.rs) |
| fail, wrong line | expected an expression (found `…`) | 1 | [extra_arguments.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/argument-suggestions/extra_arguments.rs) |

</details>

<details><summary><code>codemap_tests</code>: 3 failing (pass tests 2 / 2 · compile-fail 8 / 11)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail, wrong line | cannot find trait `…` | 1 | [two_files.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codemap_tests/two_files.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 1 | [issue-11715.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codemap_tests/issue-11715.rs) |
| fail, wrong line | no method named `…` found for `…` | 1 | [tab_3.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/codemap_tests/tab_3.rs) |

</details>

<details><summary><code>collections</code>: 3 failing (pass tests 5 / 6 · compile-fail 5 / 7)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail, wrong line | cannot find value (unsupported path) | 2 | [btreemap/btreemap_dropck.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/collections/btreemap/btreemap_dropck.rs), [hashmap/hashset_generics.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/collections/hashmap/hashset_generics.rs) |
| run | `…` does not live long enough (borrowed value must outlive the function) | 1 | [hashmap/hashmap-memory.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/collections/hashmap/hashmap-memory.rs) |

</details>

<details><summary><code>compare-method</code>: 3 failing (compile-fail 7 / 10)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [issue-90444.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/compare-method/issue-90444.rs), [reordered-type-param.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/compare-method/reordered-type-param.rs), [traits-misc-mismatch-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/compare-method/traits-misc-mismatch-2.rs) |

</details>

<details><summary><code>const_prop</code>: 3 failing (pass tests 3 / 6 · compile-fail 1 / 1)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | cannot find value (unsupported path) | 2 | [dont-propagate-generic-instance-2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const_prop/dont-propagate-generic-instance-2.rs), [dont-propagate-generic-instance.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const_prop/dont-propagate-generic-instance.rs) |
| build | /root/.cargo/bin/wild failed: exit status: N | 1 | [ice-issue-96944.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/const_prop/ice-issue-96944.rs) |

</details>

<details><summary><code>coroutine</code>: 3 failing (pass tests 4 / 5 · compile-fail 2 / 4)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [stalled-coroutine-obligations.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coroutine/stalled-coroutine-obligations.rs), [static-closure-unexpanded.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coroutine/static-closure-unexpanded.rs) |
| run | cannot find trait `…` | 1 | [issue-93161.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/coroutine/issue-93161.rs) |

</details>

<details><summary><code>editions</code>: 3 failing (pass tests 1 / 1 · compile-fail 2 / 5)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [async-block-2015.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/editions/async-block-2015.rs), [edition-raw-pointer-method-2015.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/editions/edition-raw-pointer-method-2015.rs), [edition-raw-pointer-method-2018.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/editions/edition-raw-pointer-method-2018.rs) |

</details>

<details><summary><code>entry-point</code>: 3 failing (pass tests 2 / 3 · compile-fail 6 / 8)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 1 | [main-in-submodule.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/entry-point/main-in-submodule.rs) |
| fail, wrong line | no `…` function | 1 | [imported_main_const_forbidden.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/entry-point/imported_main_const_forbidden.rs) |
| run | no `…` function | 1 | [imported_main_from_inner_mod.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/entry-point/imported_main_from_inner_mod.rs) |

</details>

<details><summary><code>inline-const</code>: 3 failing (pass tests 12 / 13 · compile-fail 9 / 11)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [const-expr-lifetime-err.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inline-const/const-expr-lifetime-err.rs), [reject-const-block-pat-pre-expansion.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inline-const/reject-const-block-pat-pre-expansion.rs) |
| check | expected `…` (found `…`) | 1 | [macro-with-const.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/inline-const/macro-with-const.rs) |

</details>

<details><summary><code>link-native-libs</code>: 3 failing (compile-fail 3 / 6)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [issue-43925.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/link-native-libs/issue-43925.rs), [kind-framework.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/link-native-libs/kind-framework.rs), [modifiers-override-4.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/link-native-libs/modifiers-override-4.rs) |

</details>

<details><summary><code>linkage-attr</code>: 3 failing (pass tests 1 / 1 · compile-fail 5 / 8)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [raw-dylib/windows/link-ordinal-and-name.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/linkage-attr/raw-dylib/windows/link-ordinal-and-name.rs), [raw-dylib/windows/link-ordinal-too-large.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/linkage-attr/raw-dylib/windows/link-ordinal-too-large.rs), [raw-dylib/windows/link-ordinal-unsupported-link-kind.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/linkage-attr/raw-dylib/windows/link-ordinal-unsupported-link-kind.rs) |

</details>

<details><summary><code>liveness</code>: 3 failing (pass tests 5 / 7 · compile-fail 15 / 16)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| check | macro `…` is not supported | 1 | [liveness-asm.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/liveness/liveness-asm.rs) |
| fail | accepted: no error reported | 1 | [liveness-dead.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/liveness/liveness-dead.rs) |
| run | cannot find type `…` | 1 | [var-defined-after-early-return.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/liveness/var-defined-after-early-return.rs) |

</details>

<details><summary><code>lowering</code>: 3 failing (compile-fail 1 / 4)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [expr-in-pat-issue-99380.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lowering/expr-in-pat-issue-99380.rs), [span-bug-issue-121431.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lowering/span-bug-issue-121431.rs) |
| fail, wrong line | cannot infer the type of this `…`; annotate it | 1 | [expr-metavar-in-let-pattern.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/lowering/expr-metavar-in-let-pattern.rs) |

</details>

<details><summary><code>shadowed</code>: 3 failing (pass tests 2 / 2 · compile-fail 5 / 8)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [match-binding-shadows-const.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/shadowed/match-binding-shadows-const.rs), [shadowed-type-parameter.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/shadowed/shadowed-type-parameter.rs), [shadowing-generic-item.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/shadowed/shadowing-generic-item.rs) |

</details>

<details><summary><code>std</code>: 3 failing (pass tests 2 / 5)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | cannot find value (unsupported path) | 3 | [fs-nul-byte-paths.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/std/fs-nul-byte-paths.rs), [park-timeout-wakeup-59020.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/std/park-timeout-wakeup-59020.rs), [stdio-from.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/std/stdio-from.rs) |

</details>

<details><summary><code>tool-attributes</code>: 3 failing (pass tests 2 / 2 · compile-fail 5 / 8)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [tool_lints-fail.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/tool-attributes/tool_lints-fail.rs), [tool_lints.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/tool-attributes/tool_lints.rs), [unknown-lint-tool-name.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/tool-attributes/unknown-lint-tool-name.rs) |

</details>

<details><summary><code>ufcs</code>: 3 failing (pass tests 1 / 2 · compile-fail 2 / 4)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 2 | [bad-builder.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/ufcs/bad-builder.rs), [ufcs-qpath-missing-params.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/ufcs/ufcs-qpath-missing-params.rs) |
| run | cannot find type `…` | 1 | [ufcs-polymorphic-paths.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/ufcs/ufcs-polymorphic-paths.rs) |

</details>

<details><summary><code>uniform-paths</code>: 3 failing (pass tests 1 / 4)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| run | cannot find value (unsupported path) | 3 | [basic.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/uniform-paths/basic.rs), [macros-nested.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/uniform-paths/macros-nested.rs), [macros.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/uniform-paths/macros.rs) |

</details>

<details><summary><code>variance</code>: 3 failing (pass tests 5 / 5 · compile-fail 20 / 23)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 1 | [variance-associated-types2.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/variance/variance-associated-types2.rs) |
| fail, wrong line | `…` takes N type arguments, but N were given | 1 | [variance-btree-invariant-types.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/variance/variance-btree-invariant-types.rs) |
| fail, wrong line | mismatched types: expected `…`, found `…` | 1 | [variance-trait-matching.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/variance/variance-trait-matching.rs) |

</details>

<details><summary><code>where-clauses</code>: 3 failing (pass tests 14 / 14 · compile-fail 7 / 10)</summary>

| kind | reason | tests | examples |
|---|---|---:|---|
| fail | accepted: no error reported | 3 | [where-clause-on-unit-struct.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/where-clauses/where-clause-on-unit-struct.rs), [where-for-self.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/where-clauses/where-for-self.rs), [where-lifetime-resolution.rs](https://github.com/rust-lang/rust/blob/0ef1bc386059596dada8fb65b50425aab2ea637a/tests/ui/where-clauses/where-lifetime-resolution.rs) |

</details>
<!-- /auto:failures -->
