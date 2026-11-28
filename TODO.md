# TODO

What is known to be left. Each security item is a test case marked `//@ todo: <why>` (or `//@ todo(valgrind):` when
only valgrind sees the failure): `cargo test --release --test security` runs it and reports it as TODO without
failing. Once a case passes, the test fails until its `todo` line and its entry here are removed, so this list can
only shrink. `todo_md_lists_every_todo` checks that every marked case is named here. How a case reads:
[tests/security/README.md](tests/security/README.md). Background: [docs/reports/2026-11-21-01-security-cves.md](docs/reports/2026-11-21-01-security-cves.md).

## Crate coverage: what to fix next

The order of work for crates.io's top crates (epic [#248](https://github.com/mizchi/zust/issues/248)), from the last
crate survey: each fix is ranked by the share of the surveyed crates' downloads it holds back. The table between the
markers is written by `cratebench/survey/survey.py` (CI rewrites it after each survey on main); the full ranking is in
[CRATES_SUPPORT.md](CRATES_SUPPORT.md#what-to-fix-by-importance) and on the
[dashboard](https://mizchi.github.io/zust/crates/). Pick the top row whose issue is open; a row without an issue
needs one filed first (then add it to [cratebench/survey/issues.tsv](cratebench/survey/issues.tsv)).

<!-- crate-priorities -->

Survey of 2026-11-28 09:07 UTC on `ef3469e`: 49 of 273 crates build (17.3% of their downloads); 1130 errors in 178 packages.

| # | fix | issue | holds back | crates it alone unblocks |
|---:|---|---|---:|---:|
| 1 | a crate's own item zustc cannot find (resolution, cfg, a macro's output) | [#254](https://github.com/mizchi/zust/issues/254) | 11.2% | 13 |
| 2 | mismatched types (to sort by cause) | [#259](https://github.com/mizchi/zust/issues/259) | 7.2% | 3 |
| 3 | methods zustc does not find (std methods the prelude lacks; lookup) | [#256](https://github.com/mizchi/zust/issues/256) | 3.6% | 2 |
| 4 | cannot infer an `impl Trait` return type | [#262](https://github.com/mizchi/zust/issues/262) | 2.5% | 6 |
| 5 | conflicting implementations of trait `…` for type `…` (E0119) | none yet | 2.5% | 4 |
| 6 | false E0276 in bitflags' impls | [#264](https://github.com/mizchi/zust/issues/264) | 2.3% | 4 |
| 7 | value paths zustc cannot resolve | [#265](https://github.com/mizchi/zust/issues/265) | 2.2% | 0 |
| 8 | false E0277: a bound zustc does not see | [#261](https://github.com/mizchi/zust/issues/261) | 2.0% | 2 |
| 9 | build: the trait bound `…` is not satisfied (for `…`) (E0277) | none yet | 1.9% | 5 |
| 10 | std iterator adapters' arity (`Copied`, `Cloned`, `Map`) | [#266](https://github.com/mizchi/zust/issues/266) | 1.8% | 4 |
| 11 | recursive type `…` has infinite size | none yet | 1.8% | 0 |
| 12 | inherent impls cannot be unsafe (E0197) | none yet | 1.7% | 4 |

By issue, with its tier (the issue's label: **T0** holds back 2% or more of the surveyed downloads with all its fixes, **T1** 1% or more, **T2** less; relabel an issue when its tier here changes):

| tier | issue | holds back | fixes | the largest |
|---|---|---:|---:|---|
| T0 | [#254](https://github.com/mizchi/zust/issues/254) | 11.2% | 1 | a crate's own item zustc cannot find (resolution, cfg, a macro's outpu… |
| T0 | [#250](https://github.com/mizchi/zust/issues/250) | 9.2% | 38 | prelude: type `Weak` |
| T0 | [#259](https://github.com/mizchi/zust/issues/259) | 7.2% | 1 | mismatched types (to sort by cause) |
| T0 | [#256](https://github.com/mizchi/zust/issues/256) | 3.6% | 1 | methods zustc does not find (std methods the prelude lacks; lookup) |
| T0 | [#262](https://github.com/mizchi/zust/issues/262) | 2.5% | 1 | cannot infer an `impl Trait` return type |
| T0 | [#264](https://github.com/mizchi/zust/issues/264) | 2.3% | 1 | false E0276 in bitflags' impls |
| T0 | [#265](https://github.com/mizchi/zust/issues/265) | 2.2% | 1 | value paths zustc cannot resolve |
| T0 | [#261](https://github.com/mizchi/zust/issues/261) | 2.0% | 1 | false E0277: a bound zustc does not see |
| T1 | [#266](https://github.com/mizchi/zust/issues/266) | 1.8% | 1 | std iterator adapters' arity (`Copied`, `Cloned`, `Map`) |
| T2 | [#257](https://github.com/mizchi/zust/issues/257) | 0.7% | 1 | parser: syntax zustc does not parse |
| T2 | [#28](https://github.com/mizchi/zust/issues/28) | 0.7% | 1 | zurgo: two versions of one crate in a graph |
| T2 | [#253](https://github.com/mizchi/zust/issues/253) | 0.7% | 1 | a fn pointer cast to `*mut ()` in a static |
| T2 | [#258](https://github.com/mizchi/zust/issues/258) | 0.4% | 1 | macro_rules: calls zustc's matcher rejects |
| T2 | [#298](https://github.com/mizchi/zust/issues/298) | 0.3% | 1 | a std trait's default method zustc requires |

No issue yet, the next to file: conflicting implementations of trait `…` for type `…` (E0119… (2.5%); build: the trait bound `…` is not satisfied (for `…`) (E0277… (1.9%); recursive type `…` has infinite size (1.8%); inherent impls cannot be unsafe (E0197) (1.7%); cannot evaluate `…` in a constant (1.5%).

<!-- /crate-priorities -->

## Security: holes zustc does not catch yet (`tests/security/`)

These are in the type and borrow checks that the compat and region-check work changes, so they were left there.

### Privacy

| case | what is accepted that rustc rejects |
|---|---|
| `privacy_field` | reading a private field from outside its module (E0616) |
| `privacy_field_write` | writing a private field from outside its module (E0616) |
| `privacy_ctor` | building a struct with private fields from outside (E0451) |
| `privacy_fn` | calling a private function from outside (E0603) |

Visibility is not checked at all. Unsafe code inside a module relies on safe code outside not touching its
invariants.

### Traits, types and the unsafe boundary

| case | what is accepted that rustc rejects |
|---|---|
| `unsafe_fn_pointer` | fn pointer types do not record `unsafe`, so calling an `unsafe fn` pointer needs no unsafe block |
| `cve_2019_12083_error_type_id` | an impl may define a method its trait does not declare (E0407). zustc has no downcast, so no type confusion follows |
| `const_overflow` | overflow in a const (E0080) is found only when it is evaluated at run time (a panic) |

### Runtime

| case | what goes wrong |
|---|---|
| `rt_box_move_out` | `let x = *b;` moves the value but never frees the Box's block (a leak; valgrind) |
| `rt_box_move_patterns` | the same leak through patterns (`let Pair { a, b } = *b`, `match *b`); no double free (valgrind) |
| `rt_pattern_partial_move` | what a pattern leaves to `_` / `..` (`let (a, _) = pair();`, `let W { a, .. } = w;`) is never dropped (a leak) |

## Threads (`crates/syntax/src/prelude_lib.rs.in`, `tests/threads/`)

OS threads since [report 2026-11-27-01](docs/reports/2026-11-27-01-native-threads.md). Left:

- No poisoning of a Mutex or RwLock: a panic does not unwind, so one a panicking thread holds stays locked, and the
  threads that wait for it wait forever (std's is unlocked and poisoned; `lock` then gives `Err`). The other values of
  that thread are not dropped either. Both need unwinding (SUPPORT.md, "Large features"). (A Once or OnceLock the
  thread was running, and a StdoutLock it held, are let go: the thread's context keeps them.)
- Not there: `thread::scope`, `Condvar::wait_timeout`, `Mutex::try_lock`, `sync::Weak`,
  `std::time::Instant`. `JoinHandle::join`'s Err is not a `Box<dyn Any + Send>` (no downcast of the payload).
  `Builder::spawn_scoped` needs `thread::scope`.
- A closure's capture held by reference is known to be mutable only when the body assigns to it or takes a `&mut` of
  it (ast_closure::mutates_capture). One it changes only through a `&mut self` method (`|| v.push(Cell::new(1))`)
  may be either kind, so the auto traits ask both rules of it (Send needs the value to be Sync and Send): such a
  closure of a `Vec<Cell<_>>` is not Send, where rustc's is.
- The main thread's thread_local! values are not dropped at exit (glibc's std runs their drops).
- The location of an `unwrap` that panics is kept in one static (`__EXR_CALLER`), which another thread may write
  meanwhile: the message can point at another thread's call. It needs a thread-local of its own (an ELF TLS slot).

## Trait solving (`crates/sema/src/traits.rs`, `formal/lean/`)

The program's own traits are solved by the algorithms proved in formal/lean, including E0119, supertrait
obligations, dyn coercions and auto traits (reports: [part 1](docs/reports/2026-11-23-04-trait-solver.md),
[part 2](docs/reports/2026-11-23-05-trait-solver-2.md), [part 3](docs/reports/2026-11-23-06-trait-solver-3.md), [part 4](docs/reports/2026-11-23-07-trait-solver-4.md), [part 5](docs/reports/2026-11-24-01-trait-solver-5.md)). Left:

- Associated types ([part 3](docs/reports/2026-11-23-06-trait-solver-3.md)): a generic fn's or a nested opaque return type
  (`-> Option<impl Tr>`) is not checked against its bounds; projections by a name std also defines (`T::Item`) and
  generic associated types are not normalized by the solver; type inference still uses `Program::assoc_of`.
- E0119 against std's impls (`impl<S> Into<S> for GenX<S>`): std's impls are not clauses.
- `Sized` ([report](docs/reports/2026-11-25-09-sized.md)): the prelude declares no `?Sized`, so only `Vec`, `Option`,
  `Result` and the collections are checked of std's types (`ManuallyDrop<T>` of a `T: ?Sized` is not unsized); an
  impl's own `?Sized` parameters are taken as sized at its methods' calls; an associated type's value is not checked
  against the trait's implicit `Sized`.
- Lifetimes in impl heads (fn pointers, higher-ranked bounds) need lifetimes in `Ty`.
- Type inference through a projection whose definition depends on the trait's arguments, in generic code
  (unsized/issue-40231-*.rs).
- Most of std's traits are still judged by `Program::implements` at calls (with known exceptions); the prelude's
  impls as solver clauses would make both directions exact (measured in part 5: few tests depend on it).
- Structs with an unsized tail ([slices](docs/reports/2026-11-24-04-dst-struct-tails.md),
  [`dyn`, boxes, `size_of_val`](docs/reports/2026-11-24-05-dst-dyn-tails.md)): a boxed literal unsized inside
  `vec![..]`, an array literal or `if` / `match` arms (`vec![Box::new(W { t: [1, 2] })]` as `Vec<Box<W<[u8]>>>`)
  is a false "mismatched types"; each element should be coerced to the expected type.
- dangerous_implicit_autorefs: the range-index form (`(*p)[..2]`) is not caught, and an `#[allow]` anywhere turns
  the lint off for the whole crate (as `overflowing_literals` already does): never a false error, a missed one.
- A qualified projection whose const argument zustc cannot evaluate (`<S as Pick<{ match .. }>>::Out`) with several
  impls that may fit, or one whose trait arguments are not resolved: an "unsupported" error (never the wrong impl).
  Evaluating such const expressions would select the impl.
- Missed errors (zustc accepts; main did too), from the review of #128: a supertrait's arguments (`trait Sub:
  Base<u8>` with only `impl Base<u16>`); `<S as Tr<u8>>::Item` falling back to another impl's definition; a
  binding checked against the trait's arguments (`T: Tr<u8, Item = u8>`); an explicit `unsafe impl Send for W<u8>`
  taken for every `W<T>`; `<S as Other>::Item` in a method of `impl Own for S` rewritten as `Own`'s.
- A qualified call names the trait's arguments, which method selection does not match against each impl's
  (`<S as Tag<u16>>::tag(&s)` with `impl Tag<u8> for S` and `impl Tag<u16> for S` takes `Tag<u8>`'s; main too): a
  per-impl record of the trait's arguments is needed.
- `<S as Tr<2>>::get()` with impls of `Tr<1>` and `Tr<2>`: the call takes `Tr<1>`'s method (now a compile error,
  as the annotation `<S as Tr<2>>::Item` resolves to the right type; main printed the wrong value).
- A local named like an intrinsic (`let size_of = || 7u8; size_of()`) is a false error, and a program's own
  `mem::size_of_val` is typed as the intrinsic (main too).
- From the third review of #128 (main too unless noted):
  - a non-generic struct with an unsized tail (`struct Tail(u8, [u32])`) is not tagged: `*const Tail` is thin;
  - `size_of::<*const Tail<[u8]>>()` as an array length is 8: the syntax-level rewrite of `size_of` takes every
    pointer to a path as thin, before types are known;
  - `where` found inside an attribute's string (`#[doc = "where"]`) hides the impl's where clause;
  - an impl header's trait argument that resolves only later (`impl Pick<<Seed as Key>::Arg> for S` before `Seed`'s
    impl) is kept unread.
- `impl Tr for [u8; 3]` then a dyn call: "no implementation" (main too).

## Security: CVEs whose API zustc does not have (`expect: unsupported`)

These cannot happen today. Each case fails as soon as zustc gets the API, and must then become a test of the CVE's
behavior: `cve_2020_36323_join_borrow` (join over a `Borrow<str>` of the program's), `cve_2021_28875_read_to_end`
(a `Read` impl of the program's), `cve_2021_28876_28878_zip_both_ends` (`Zip::next_back`), `cve_2021_29922_ip_octal`
(`std::net`), `cve_2022_21658_remove_dir_all` and `rt_char_boundary_get` (`str::get`). The full table is
[tests/security/cves.txt](tests/security/cves.txt).

## wasm32-unknown-unknown (`crates/wasm`, `tests/wasm/`)

- `Op::Fma` is a multiply and an add, not a fused multiply-add: `f64::mul_add` can differ from the native build in
  the last bit. Use a correctly rounded helper, or reject `mul_add` on wasm until there is one.
- Speed: every block is a case of one `loop` + `br_table`, and every value a local (3–15x slower than native under
  Node). A relooper or stackifier, and reuse of locals.
- Not supported yet:
  - i128 division and i128 to / from float;
  - threads (`spawn` needs pthreads; on one thread the atomics, Mutex and the rest work as plain accesses);
  - zustc built as wasm by itself.
- Traps carry no panic message (the native build prints the location from its trap table).

## SIMD (issue #30)

Done: vector types of 128 / 256 / 512 bits and the SIMD instructions in the IR (`Type::Vec`, `Op::Simd(Shape, VOp,
..)`), the wasm backend (128-bit only), `core::arch::wasm32` (all of `simd128` and `relaxed-simd`), tests against
rustc's wasm32-wasip1 builds (tests/wasm/README.md). Left:

- Cranelift: 128- and 256-bit vectors are done (tests/simd/README.md); a 256-bit one is two 128-bit halves (Cranelift
  has no ymm registers yet): real AVX2 code when Cranelift has it. 512-bit vectors (four halves) are not tested, and
  nothing produces them while AVX-512 is skipped.
- `core::arch::x86_64`: rounding-mode forms (`round_ps`, `floor_ps`), `rcp_ps` /
  `rsqrt_ps`, `sign_epi*`, string / CRC / AES / F16C (tests/simd/README.md has the subset done). Integer `hadd` is
  two `pshufb`s and an add, not `phaddw`; gathers and `sllv` are lane by lane, not `vpgatherdd` / `vpsllvd`.
- AVX-512 (intrinsics, 512-bit registers): skipped for now. Few machines have it and implementations of it differ in
  problems; the Cranelift ISA leaves it out.
- `{:x}` of a negative signed integer prints `-1` where rustc prints `ffffffff` (the prelude's formatting; found by
  the SIMD tests, which print `as u32`).
- The IR's scalar `Load` / `Store` carry no alignment: Cranelift lowers them with `MemFlagsData::trusted()` (which
  claims the address is aligned) although `ptr::read_unaligned` and a gather off an unaligned base (scale >= the
  element size) use them too. x86 scalar moves do not need alignment, so the code is right today; an alignment
  operand (or an unaligned load op) on `Op::Load` / `Op::Store` would make the flag true.
- memchr 2.8.3 builds for wasm32 (what dioxus and leptos use) and gives rustc's results under node
  (`memchr`, `memrchr`, `memchr2/3`, `memmem::find/rfind`, `Finder::find_iter`). Its x86-64 build (a host program)
  stops at items a `macro_rules!` expansion puts in a function
  body (`unsafe_ifunc!`: `type Fn = *mut ();`, `static FN: AtomicPtr<()>`, `fn detect()` inside a block): "nested
  items are not supported". Nested items written in a body are hoisted; those of an expression macro's expansion are
  not. Also `is_x86_feature_detected!` is `true` only for sse / sse2 (no detection at run time), and the vector types
  `__m128` / `__m128i` / `__m128d` / `v128` are one type, so impls of a trait for two of them conflict.
- Hoisting renames a nested item that another block or the top level also names (`crates/syntax/src/lexer.rs`,
  `hoist_parts_at`). Where a fn and a type of one name are both renamed, a use takes the innermost rename of the
  namespace its neighbouring tokens tell (`m(`, `m::<u8>(`, `fn m` a value; `m::`, `m<`, `-> m`, `impl m`,
  `let a: m`, `(x: m`, `|x: m|`, a struct literal a type). A struct literal's field value (`S { f: m, .. }`) and a
  middle parameter (`fn h(a: u8, x: m, b: u8)`) look alike to it and take the innermost rename of either namespace,
  which may be the wrong one. Resolving the names on the parsed AST would settle these.
- Incremental preprocessing (`prep` with a `PrepMemo`) can differ from `prep_full` on unbalanced input: an item in a
  brace that an edit leaves open (`println!("{}", std::mem::size_o{ fn inner() {} }` inside `fn main`) is hoisted to
  the top level by the full pass but left in place by the chunked one. Found by `tests/prep_inc.rs` when
  `examples/const_eval_typed.rs` grew; the example's new items were moved to its end, which changes the random edits.
  A second case found the same way: a `m!()` call that the full pass expands and the chunked one leaves (an edit
  near `let y17 = y16 & y16;` in that example).
- Nested generic aliases (`type T1 = D<T0, T0>; type T2 = D<T1, T1>; ..`) take time exponential in the depth to
  compile (about 2x per level: 2.2 s at depth 24, 33 s at depth 28), with or without `TypeId` (whose key is the
  fixed-size `hash_ty`). Pre-existing (not SIMD): probably the aliases' expansion.
- Wider byte shuffles and constants: `Shuffle` / `Swizzle` / `Vconst` are 128-bit only.
- Wasm flexible vectors (proposal, phase 3), when engines have them: a width the engine sets at run time, an
  instruction for the lane count, lane shifts and half concatenations (the IR's SIMD section has the plan). Until
  then wasm stays at 128 bits.
- minclif: scalarize each `VOp` (one lane at a time), so that every backend accepts every instruction (it rejects
  vectors today).
- A `core::simd` subset (`f32x4`, `Simd<T, N>`, lanewise operators) as generic code over the intrinsics.
- Intrinsics as function values (`map(f32x4_add)`): they are lowered at the call, not functions.
- The kernels of experiments/zustscript are 1.5–1.8x the hand-written v128 module: the dispatcher between blocks and
  the address checks (the relooper / stackifier above).
- Auto-vectorization of simple slice loops (map / sum).

## UI frameworks for wasm32 (dioxus, leptos)

Neither builds yet. `python3 cratebench/ui/run.py` builds their wasm32 crate graphs leaves first and lists the first
error of each failing crate. Details and the order of the work: [docs/reports/2026-11-21-03-ui-frameworks-wasm.md](docs/reports/2026-11-21-03-ui-frameworks-wasm.md).

- Cargo graphs: `crates/zurgo` ([#28](https://github.com/mizchi/zust/issues/28), [docs/zurgo.md](docs/zurgo.md))
  reads Cargo.toml / Cargo.lock, turns on features as cargo does and builds path and crates.io packages (M0); runs
  build scripts and gives their cfgs, `rustc-env` and `OUT_DIR` to the crates (M1). thiserror 2's library builds.
  Builds for wasm32 too (one module per bin, run with cargo's runner variable). Left: two versions of one crate
  (zustc's workspace has one name per crate; renamed dependencies are `extern=ALIAS=NAME`, #249), a crate's own
  edition, link instructions of build scripts. `zurgo test` runs each test in a process of its own ([#96](https://github.com/mizchi/zust/issues/96));
  left there: doc tests, `test = false`, building only the chosen targets.
  - What the caches do not see yet:
    - the process environment that an `env!($k)` made by a macro reads (the crate's own environment is in its
      fingerprint, the values zustc falls back to are not);
    - the variables a rustc-built script or host library reads with `env!` (cargo reads them from rustc's dep-info,
      `# env-dep:`);
    - a file removed from `OUT_DIR` by hand (cargo does not rerun the script either).
  - The proc-macro check:
    - reads a proc-macro crate's root with its features but not with its own build script's cfgs;
    - evaluates `cfg!` in the arguments of a user's macro named like std's format macros (`print!`) before that
      macro's rules match.
- Proc macros ([#29](https://github.com/mizchi/zust/issues/29)) run through the toolchain's proc-macro server: zurgo
  builds the dylib with rustc, zustc finds the uses by name and expands them (crates/syntax/src/pm.rs). Not yet:
  - hygiene and spans of their output (every span is the call site's);
  - a proc macro among a build script's build-dependencies (rustc builds that script);
  - zustc alone (`zustc build` with no `pm=` dylib, or without the server) still drops an unknown attribute silently
    (`#[wasm_bindgen]`, `#[component]`).
- Crates that zurgo now reaches and that stop on zustc: anyhow 1.0.104 (`error.rs:808`, E0106), libc 0.2.191 and
  serde_core 1.0.229 (`mod x;` inside a `macro_rules!` body: rustc finds the file where the macro is expanded, zustc
  beside the macro's own file). Their build scripts compile with rustc only (`env::vars()`, `Command` with an
  `OsString`, `Output` and other std gaps).
- wasm-bindgen-shared 0.2.130 (W0 of [#38](https://github.com/mizchi/zust/issues/38)) builds with zurgo for x86-64 and
  for wasm32. An app that calls its whole API prints, under node, what its cargo build prints
  (`crates/zurgo/tests/zurgo.rs`). Next, the wasm-bindgen crate itself (W1). Its `#[wasm_bindgen]` uses expand
  (proc macros, above), and these no longer stop it: `closures!` (a tt-muncher's `@closure` marker), impls for `&&str`,
  macro calls in an item's expression, doc comments on a cfg'd-out item, `pub extern crate`, `::dep::path`, an
  `extern` block in a function body, a type bound in a `where` clause, `AssertUnwindSafe`, `LocalKey` and
  `LazyCell` (as types of the prelude), the unwind-safety auto traits, default type parameters (`LazyCell<T, F =
  fn() -> T>` used as `LazyCell<T>`), std items imported under another name (`use core::cell::LazyCell as Lazy;`),
  inherent impls for two disjoint instances with one method name (`Nullable<&T>::copied` / `Nullable<&mut T>::copied`),
  a module named `__rt`, const generic parameters inside a module, a nested `unsafe extern "C" fn`, names a glob
  re-export brings in through chains of globs (`use crate::{convert::*}` with `pub use self::traits::*;`, also in
  paths `crate::convert::WasmRet`), the program's `impl TryFrom<X> for T` / `impl From<J> for f64`, two impls of
  one trait for one type with their own associated types (`TryFrom<JsValue>` / `TryFrom<&JsValue>` for f64), impls
  told apart by a parameter's bound (`Result<T: Into<JsValue>, E>` beside `Result<(), E>`), `<J as
  PartialEq<str>>::eq(..)`, operators' impls for references (`-&v`, `&a << &b`, `BitAnd::bitand(&a, b)`) and
  `<&'static J as BitAnd<&'static J>>::Output` beside another trait's `Output` (tests/probe/w1_items.rs);
  qualified paths whose self type is a reference or an associated type, an impl for a type itself before a blanket
  one, associated types naming another impl's, `&[T]`'s impl beside `&mut [u8]`'s, a renamed fn at a range's end
  (tests/probe/w1_items2.rs); items in a `const _: () = { .. };` a macro writes on one line (what `#[wasm_bindgen]`,
  `closures!` and derives wrap impls in; one written by hand is still skipped),
  `<T as Tr>::null()` that is no `ptr::null()`, impls for references to generic structs as bounds,
  `<Box<[u8]>>::from_abi` among Box's impls, `<T as Tr>::Abi` of a parameter once it is known, a macro's `let`
  names beside a field or method of the name and in a struct literal's shorthand, `Vec::from_raw_parts`,
  `char::from_u32_unchecked` (tests/probe/w1_items3.rs); a generic method called right in a `match` (its type
  parameter from the arms' type), `impl<T: Tr + ?Sized> Tr for &T`'s functions without `self` (`<&u8 as
  WasmDescribe>::describe()`), a blanket impl's `self`-by-value method on a number, on a reference and through a
  qualified path (`<&Box<str> as MaybeIntoWasmAbi>::none()`), a reference's or a number's own impl before a blanket
  one, `T::none()` of T's bound's trait beside another trait's `none`, `String::from_utf8_unchecked`
  (tests/probe/w1_items4.rs); the cases of #139's review: an inherent method before a blanket or trait one at the same
  receiver, overloads of one self type told apart by the argument, an impl's own `ne`, a shift's `Output`, a
  zero-capacity `Vec::from_raw_parts`, a renamed name through two globs under `use .. as`, a generic impl kept in an
  anonymous const, impls for two instances of a trait object, and E0119 / E0277 / E0592 / E0201 through aliases,
  `&Box<S>`, `From`'s bounds, const generic lengths (tests/probe/w1_items5.rs, tests/compile_errors/w1_*). It stops on:
  - `externref.rs:130`: "expected a function".
  - `LocalKey<T>` is a type with `with`, but a `thread_local!` is not one of them yet (each has a key type of its
    own, over a `__TlsKey`), so a `&'static LocalKey<T>` cannot point at one.
  - a method call on a `Box<[T]>` with the program's `impl Tr for Box<[T]>` derefs to `[T]` and misses it; a
    `Box<str>` cannot be made (`into_boxed_str`, `"x".into()`).
  - the items a macro's `const _: () = { .. };` moves out are visible outside it (rustc scopes its fns to the
    block); the hand-written one in wasm-bindgen's lib.rs (`__wbindgen_skip_interpret_calls`) is not compiled. One
    whose fn, `use`, trait or impl that would go out names a const, static or type that stays in it (`const N` or
    `struct Local<T>` beside an `impl Tr for u8` that names it) is skipped whole, as before this branch.
  - an impl for a trait object with auto traits (`impl M for dyn Obj + Send`) is found by the object's trait alone, so
    a call of its method on a `dyn Obj` is rejected as a mismatched type (E0308), where rustc reports E0599 (no
    method). The object's types do keep `+ Send` / `+ Sync` (report 2026-11-27-03).
  - a blanket impl's `self`-by-value method on a `&Box<u8>` (`impl<T: Marker> Tr for T` with `impl Marker for
    &Box<u8>`) asks the bound of the Box's content ("`u8: Marker` is not satisfied").
  - impls for trait objects with parameters (`impl<R> Tr for dyn Fn() -> R`) are matched by the object's trait's name;
    two impls for instances of one object (`dyn Obj<u8>` / `dyn Obj<u16>`) that override one method call the first
    one's. Equal associated consts of `dyn Fn(..)` impls of each arity are kept once.
- Found in the review of #139, also so before it (`<S as Tr<u8>>::Out` beside `<S as Tr<u16>>::Out` is #128's now: the
  trait's arguments pick the impl):
  - a struct field of `<W as B>::Out` whose value is `<u8 as B>::Out`, with `impl B for u8` written after W's: the
    field takes the value before that impl is read.
  - `m::f()` finds an `f` that only a fn body in `m` glob-imports (`fn helper() { use crate::a::*; .. }`): accepted
    where rustc reports E0425.
  - `a != b` calls `!a.eq(b)`, not the impl's own `ne` (a qualified `<S as PartialEq<R>>::ne` calls it).
  - `Vec::<u32>::from_raw_parts(NonNull::dangling().as_ptr(), 0, 0)`: "cannot infer type parameter `T`" (with
    `let p: *mut u32 = ..` it compiles).
  - `Vec::from_raw_parts` of zero-sized elements takes a pointer that is not dangling (not below 4096, not the
    alignment) for the prelude's one-byte buffer, which drop frees: the prelude's `reserve` allocates for zero-sized
    elements, where std's Vec never does.
  - a cycle of associated types through blanket impls (`impl<T> A for T { type Out = <T as B>::Out; }` and B's
    back) is no error while nothing names it, and not E0275 where something does.
- Found while building memchr, not fixed:
  - a generic struct literal whose field is a `<v128 as Tr>::f(..)` call (`Pair { v1: <v128 as Vector>::splat(b) }`)
    does not infer `V = v128` ("expected `T0`, found `v128`"); an annotation or an intrinsic's value does;
  - `fn f<'a>(x: &&'a T) -> &'a T { x }`: the deref coercion is applied, but the region check takes the outer
    reference's lifetime ("`'_` must outlive `'a`"). With elided lifetimes (`fn as_slice(&self) -> &[u8]`) it passes;
  - `None => &[]` in a match whose type is `&'a [u8]`: "type annotations needed for an empty array";
  - zustc's own parser counts the destructuring assignment `(a.x, a.y) = t;` as a `let` (tests/parse.rs).
- `cratebench/ui/run.py` does not run build scripts: a crate gets cargo's variables (`CARGO_PKG_*`) but not what its
  script would give (zurgo does both).
- `x.as_ref()` on text takes the `AsRef` targets of all the bounds in scope, not those of `x`'s own parameter: in
  `fn f<A: AsRef<str>, B: AsRef<[u8]>>(a: A, b: B)` with a text `B`, `b.as_ref()` is inferred as `&str`.
- `include!("x.rs")` as an item shifts the line numbers after it (`line!()`, panic locations) by the lines of the
  included file: its code goes in as it is, not on one line.
- async is erased (an `async fn` runs when called): [#8](https://github.com/mizchi/zust/issues/8).
- wasm-bindgen's ABI (32-bit pointers, imports and exports for JS, its custom section): W2 / W3 of
  [#38](https://github.com/mizchi/zust/issues/38).
- Macros: larger tt-munchers (`pin_project!`, `bitflags!`), `#[macro_use] extern crate`, `#[macro_use] mod` in a file
  ("cannot find macro"), `#![recursion_limit]` (the limit stays 64).
- Language and std: the tables of sections 2.5 and 2.7 of the report.

## Typed constant evaluator: open cases from the review of #49

- An untyped `let` whose only use is in an `if` arm that is not taken is not validated at its inferred type:
  `const N: u8 = { let x = 255 + 1; if false { x } else { 0u8 } };` is accepted (rustc: overflow). Finalize every
  executed initializer at the type inference gives it, not only those whose untyped trial failed.
- That type should come from both arms (`let x = 1 / ((!0 >> 7) + 1);` used as `i64` in a skipped arm divides by
  zero at `i64`); the fallback in `Program::eval_block_env` accepts success at any integer type.
- `!FLAG` with `const FLAG: bool` (and `!{ true }`) is folded as an integer complement: `(!FLAG) as u8` is 254, not 0.
  `is_bool_expr` sees only literals, comparisons and `&&`/`||`; the bool type should be carried through constants and
  blocks.
- Serialized const arguments drop a local's type annotation (`{ let x: u8 = !0; ... }` in `Buf<{ .. }>`), and a const
  fn's return type through an alias (`type Id<T> = T; const fn f() -> Id<u8>`) is not applied.

## zurgo test against cargo test (cratebench)

`cratebench/test.py` runs each cratebench crate's own tests under `cargo test` and `zurgo test`, and compares them
test by test (`cratebench/test_result.md`). fnv, rustc-hash, cfg-if and adler2 match, and CI keeps them so
(`--check`). Tracking issue for the rest: #200.

- either does not build under zurgo yet. The first error is the empty array of its `seek` test,
  `Left(io::Cursor::new([]))` (`type annotations needed for an empty array`): rustc takes the element type from the
  other arm's `Right(io::Cursor::new(&mockdata[..]))` and the `Read` the reader is used as. Its `deref` test (the deref
  coercion of `is_str(&value)` through `impl Deref for Either<L, R>`, tests/probe/deref_either.rs) and its `fmt_traits`
  test (tests/probe/fmt_radix_traits.rs) are right in a single file. Then serde_core, through the dev-dependency serde_json: #197, #198, and std types
  the prelude lacks (`CString` / `CStr`, net addresses, `SystemTime`, `LinkedList`, `RwLock`, `Weak`, `Bound`).
- A method of a trait with several impls for one type that differ only in the trait's arguments is not chosen by a
  bound: `fn f<T: AsMut<[u8]>>(t: &mut T) { t.as_mut() }` with `T = Either<..>` (either's `AsMut<str>` /
  `AsMut<[Target]>` / `AsMut<Target>` ..) takes the first impl's `as_mut`, or cannot infer `Target`. The expected
  type chooses (`let x: u16 = s.get()`), and a bound does when the impls' arguments are concrete; an impl's trait
  arguments are not kept per method (FnSig has the trait's name only), so a generic one (`AsMut<[Target]>`) cannot
  be matched against the bound.
- hex and itoa: their dev-dependency criterion needs two versions of one crate (#28). The error message names
  `itoa v1.0.19` twice (#178).
- Smaller gaps found on the way: inherent impls on instances of one type (associated fns, implicit `Sized`: #201),
  an integer literal's type from a later use (#202), a Cursor seek past `i64::MAX` (#203), `Read` / `BufRead`
  through `&mut &[u8]` (#173), and items the hoister misses (#198).

## Tooling

- `tests/selfhost.sh`: rustc builds every step's combined source, but zustc compiles only the `link` and `minclif`
  steps (the script stops at `lexer`). zustc's own sources use what zustc does not support yet: `[u8]::starts_with`
  and `Option::get_or_insert` (lexer.rs), `as_deref` on an `Option<Box<Vec<_>>>` (parse.rs `Spos::entries`),
  `str::get(..2)`, `String::ends_with` with a closure, `std::process::Child` (pm.rs), and maybe more behind these.
  The `enc` step's own `main` stops at `"x".into()` in a struct literal with `..Default::default()`.
- `tests/selfhost_compiler.sh` fails on main: rustc does not build the combined source
  (`tests/selfhost/build_compiler.py`), which lacks the modules `pm`, `test_harness` and session's `dead` /
  `dyncompat`.
- `bench/rust_ui.py --check` checks nothing without `--baseline`: it exits 0 whatever passes. CI runs
  `python3 bench/rust_ui.py --baseline bench/rust_ui_passing.txt --check`, which compares test by test and against
  `bench/rust_ui_ratchet.txt`; run that locally before pushing a change to the compiler.
- `tests/parse.rs` (`parses_like_syn`) compares zust's parse of the repository's own sources with syn's. zust counts a
  destructuring assignment (`(a, b) = (c, d);`) as a `let`, so writing one in zust's sources fails the test; assign
  the parts one by one, through temporaries when a part reads another (`let t = b; b = a; a = t;` for
  `(a, b) = (b, a)`).
- `scripts/docs_check.sh --update` may leave the pair `docs/language.md` / `docs/ja/language.md` unjudged
  (`excerpt-budget`), and then fails with 3: the pair is large. A change there may need to be made smaller.
