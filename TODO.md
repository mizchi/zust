# TODO

What is known to be left. Each security item is a test case marked `//@ todo: <why>` (or `//@ todo(valgrind):` when
only valgrind sees the failure): `cargo test --release --test security` runs it and reports it as TODO without
failing. Once a case passes, the test fails until its `todo` line and its entry here are removed, so this list can
only shrink. `todo_md_lists_every_todo` checks that every marked case is named here. How a case reads:
[tests/security/README.md](tests/security/README.md). Background: [docs/reports/2026-11-21-01-security-cves.md](docs/reports/2026-11-21-01-security-cves.md).

## Security: holes zustc does not catch yet (`tests/security/`)

These are in the type and borrow checks that the compat and region-check work changes, so they were left there.

### Borrow and region checking

| case | what is accepted that rustc rejects |
|---|---|
| `region_block_tail_escapes` | a loan held by a block's local, or by what a by-value method returns (`Some(&v).unwrap()`), is not followed out of the block's tail (E0597) |

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
| `send_dyn_thread` | `dyn Trait` types do not keep `+ Send`, so a `Box<dyn Fn()>` (which may hold an `Rc`) moves to a thread |
| `cve_2019_12083_error_type_id` | an impl may define a method its trait does not declare (E0407). zustc has no downcast, so no type confusion follows |
| `const_overflow` | overflow in a const (E0080) is found only when it is evaluated at run time (a panic) |
| `unsafe_union_pattern` | rejected, but as an unsupported pattern (a union pattern of one field), not as E0133 |

### Runtime

| case | what goes wrong |
|---|---|
| `rt_box_move_out` | `let x = *b;` moves the value but never frees the Box's block (a leak; valgrind) |
| `rt_box_move_patterns` | the same leak through patterns (`let Pair { a, b } = *b`, `match *b`); no double free (valgrind) |
| `rt_pattern_partial_move` | what a pattern leaves to `_` / `..` (`let (a, _) = pair();`, `let W { a, .. } = w;`) is never dropped (a leak) |

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
  - threads (green threads need `swapcontext`);
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
- memchr (42 crates of dioxus, 29 of leptos depend on it) stops at items a `macro_rules!` expansion puts in a function
  body (`unsafe_ifunc!`: `type Fn = *mut ();`, `static FN: AtomicPtr<()>`, `fn detect()` inside a block): "nested
  items are not supported". Nested items written in a body are hoisted; those of an expression macro's expansion are
  not. Also `is_x86_feature_detected!` is `true` only for sse / sse2 (no detection at run time), and the vector types
  `__m128` / `__m128i` / `__m128d` / `v128` are one type, so impls of a trait for two of them conflict.
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
  Builds for wasm32 too (one module per bin, run with cargo's runner variable). Left: two versions of one crate and
  renamed dependencies (zustc's workspace has one name per crate), a crate's own edition, link instructions of build
  scripts, `zurgo test`.
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
  fn() -> T>` used as `LazyCell<T>`), std items imported under another name (`use core::cell::LazyCell as Lazy;`).
  It stops on:
  - inherent impls for two instances with one method name, `impl<T: Copy> Nullable<&T> { fn copied }` beside
    `impl<T: Copy> Nullable<&mut T> { fn copied }` (`nullable.rs:256`, as std's `Option<&T>::copied`): a duplicate
    definition.
  - `LocalKey<T>` is a type with `with`, but a `thread_local!` is not one of them yet (each has a key type of its
    own), so a `&'static LocalKey<T>` cannot point at one.
- memchr 2.8.3, which 42 crates of dioxus and 29 of leptos depend on: an impl of a trait for a SIMD vector type is
  "cannot find type" (`impl Vector for v128` in its wasm32 module, `impl Vector for __m128i` in its x86-64 one).
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
- A vector type's overload group with an `unsafe fn m` and a safe `fn m` (two traits): when the unsafe one stays
  selected, no E0133 is reported. Check the id `pick_overload` returns, after selection.
- Serialized const arguments drop a local's type annotation (`{ let x: u8 = !0; ... }` in `Buf<{ .. }>`), and a const
  fn's return type through an alias (`type Id<T> = T; const fn f() -> Id<u8>`) is not applied.

## Tooling

- `tests/selfhost_compiler.sh` fails on main: rustc does not build the combined source
  (`tests/selfhost/build_compiler.py`), with 6 errors E0277 "can't compare `Ident` with `&str`" (e.g.
  `*i == *l.as_str()`).
