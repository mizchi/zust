# Can zustc build dioxus and leptos for wasm32?

Date: 2026-11-21

The goal is to build Rust UI libraries for the browser with zustc. This report checks the two most used ones,
dioxus and leptos, against the current zustc: whether each builds, and, since neither does, what is missing. The
check is repeatable: `python3 cratebench/ui/run.py` (new) builds the crates of each framework's wasm32 graph with
zustc, leaves first, and prints the result.

## 1. The answer

rustc builds both counter apps of `cratebench/ui/apps/` (a signal, a heading and a button) for
`wasm32-unknown-unknown`: dioxus 0.7.10 (`web`) in 38 s, leptos 0.8.23 (`csr`) in about 50 s (cold, debug).
zustc builds neither:

| | dioxus 0.7.10 | leptos 0.8.23 |
|---|---:|---:|
| crates built for wasm32 | 168 | 123 |
| lines of `.rs` in their directories | 1.36 M | 1.01 M |
| of which web-sys (enabled features only) | 28 k (106 features) | 36 k (162 features) |
| with a build script | 23 | 22 |
| host crates (proc macros and what they use) | 66 | 76 |
| of which proc-macro crates | 28 | 22 |
| zustc: pass | 15 | 14 |
| zustc: fail on their own code | 47 | 39 |
| zustc: blocked by a dependency that fails | 106 | 70 |

- The crates that pass are small leaves (cfg-if, itoa, fnv, byteorder, scopeguard, pin-project-lite, rustc-hash 2,
  xxhash-rust and so on) or crates that only re-export a proc macro (pin-project, derive_more, zerofrom).
- A library that passes has its non-generic code compiled. zustc checks a generic function's body only where it is
  instantiated.
- Every path to the browser is blocked:
  - wasm-bindgen, by unicode-ident;
  - js-sys and web-sys, also by the futures crates;
  - dioxus-core, dioxus-web, reactive_graph, tachys and leptos.
- `cargo metadata --filter-platform wasm32-unknown-unknown` gives the crate graph, with the versions of the
  committed Cargo.lock and the resolved features. A crate is "built for wasm32" if the app reaches it without going
  through a proc macro.
- Each crate is built as a zustc workspace of itself and its dependencies. A crate whose dependency fails is not
  built.
- `zustc build` does not take wasm32 yet, so the script builds with the x86-64 cfg. A local zustc with the wasm32 cfg
  for workspaces gave the same set of passing and failing crates, with one first error different (same-file).

## 2. What is missing, from the bottom up

The first four parts are not about single crates. Each of them alone stops both frameworks.

### 2.1 Building a Cargo graph

zustc builds a workspace from a hand-written list (`lib <name> <root> [deps] features=..`). A graph from crates.io
also needs:

- what Cargo.toml and Cargo.lock decide: versions, features, renamed dependencies, target-specific dependencies
  (`run.py` takes them from `cargo metadata`);
- two versions of one crate in one graph. A workspace has one name per crate, so thiserror 1 and 2 (in both trees),
  rustc-hash 1 and 2 and const-serialize 0.7 and 0.8 cannot both be there;
- the edition of each crate: both trees mix 2015, 2018, 2021 and 2024;
- build scripts. Their outputs are:
  - cfgs, such as wasm-bindgen's `wbg_reference_types` and those of serde, anyhow and num-traits;
  - environment variables (`cargo:rustc-env`), such as wasm-bindgen-shared's `SCHEMA_FILE_HASH`;
  - generated code (`include!(concat!(env!("OUT_DIR"), ..))`), in serde, serde_core and thiserror 2;
- `env!` / `option_env!` ("macro `env!` is not supported"), and the variables Cargo sets, such as
  `CARGO_PKG_VERSION`;
- workspace builds for wasm32: `zustc build` refuses `--target wasm32-unknown-unknown`.

Cargo can give most of this:

- `cargo metadata` gives the graph;
- `cargo check --message-format=json` runs the build scripts and reports their cfgs, variables and `OUT_DIR`
  (`build-script-executed`);
- the same command builds the proc-macro crates for the host (`compiler-artifact`).

### 2.2 Proc macros

Both frameworks are used through proc macros, and so are many crates under them:

- the 14 lines of each app need `#[component]` and `rsx!` / `view!`;
- js-sys and web-sys declare every binding with `#[wasm_bindgen]` (18 crates of the trees use it);
- the derives of serde, thiserror's `#[derive(Error)]`, pin-project, async-trait, tracing's `#[instrument]`, and
  typed-builder, which makes leptos's props.

About 83 of the 222 crates of the two trees use a derive or an attribute from a proc macro.

zustc cannot run a proc macro, and it does not treat them all alike:

- an unknown derive is an error ("cannot find derive macro");
- an unknown function-like macro is an error ("macro `rsx!` is not supported");
- an unknown attribute is dropped, so `#[wasm_bindgen] extern "C" { .. }` compiles as a plain C import and
  `#[component] fn` as a plain function.

There are three ways to run them:

1. Compile the proc-macro crates with zustc for the host, run them, and give them a `proc_macro` crate of zustc's
   own. They need syn, quote, proc-macro2 and the rest: 66 and 76 host crates (394 k and 411 k lines), which have the
   same gaps as the wasm side.
2. Let rustc build them (cargo does already) and load them through rust-analyzer's proc-macro server. The toolchain
   ships it (`libexec/rust-analyzer-proc-macro-srv` is installed here). Its interface is internal and may change
   ("an IDE implementation detail"), and it ties zustc to the installed rustc.
3. Expand with rustc before zustc (`-Zunpretty=expanded`, which needs `RUSTC_BOOTSTRAP=1`). The output names
   unstable internals (`::alloc::boxed::box_assume_init_into_vec_unsafe`, `#[prelude_import]`), and the expansion
   would need rustc in each incremental build.

The expansion of the two apps (rustc's) shows what the generated code needs, apart from the libraries:

- `rsx!`:
  - statics with `OnceLock`, and `GlobalSignal` statics built from a closure;
  - const evaluation through const_format: an array whose length is a `const fn` call, and
    `*const [u8] as *const str`;
  - `Box<[T]>` from `Box::new([..])`, and `format_args!`;
  - closures such as `move |_| count += 1` on a `Signal<i32>` (`AddAssign`);
- `view!`:
  - a builder with a default type parameter (`struct AppPropsBuilder<TypedBuilderFields = ()>`);
  - `-> impl IntoView` whose type is nested tuples of `HtmlElement<..>`, built by chained `.child(..)` / `.on(..)`.
    This needs traits with type parameters and associated output types, and impls for tuples.

### 2.3 async is erased

zustc accepts `async fn`, `async {}` and `.await`, but compiles them as plain code:

- an `async fn` runs its body when called and returns its value;
- `let x: i32 = f();` type-checks for `async fn f() -> i32`;
- a future that is never polled still runs;
- `pin!` is not supported;
- a `Pin<Box<dyn Future>>` made by `Box::pin(async { .. })` holds the value ("no method `as_mut` on type `i32`").

Both frameworks run on futures:

- dioxus's `spawn` and `use_resource`;
- leptos's `spawn_local`, `Resource` and `Suspense`;
- wasm-bindgen-futures, between JS promises and futures.

53 crates of the trees use async, and 48 use `Waker` / `Context` / `Poll`. They need a lowering of async bodies to
state machines, `Pin`, and the task types (`Waker`, `RawWaker`, `Context`).

This also changes what programs zustc accepts today do: an `async fn` with side effects runs them at the wrong time.
docs/language.md did not say so, and now it does.

### 2.4 The wasm module and the JavaScript side

A zustc wasm module is made for the sandbox of tests/wasm:

- it imports only functions of `env` that the host gives;
- it exports only `memory` and `main`, and runs `main` to completion;
- it keeps 64-bit pointers and `usize` (`target_pointer_width = "64"`).

wasm-bindgen, which every web API of both trees goes through, needs:

- 32-bit `usize` and pointers: its ABI passes pointers and lengths as u32 (`WasmSlice { ptr: u32, len: u32 }`), and
  the JS glue reads memory that way;
- imports from JS, and exported functions that JS calls after `main` has returned: event handlers, closures, the
  allocator, and a table for closures;
- the custom section `__wasm_bindgen_unstable` that `#[wasm_bindgen]` writes;
- the `__wbindgen_describe_*` functions, which the `wasm-bindgen` CLI runs in its interpreter to generate the JS glue.
  The format is pinned to the version of wasm-bindgen-shared (`SCHEMA_VERSION`);
- externref (reference types), which wasm-bindgen's build script turns on.

A JS bridge of zust's own would not need wasm-bindgen's format, but dioxus and leptos use the types of web-sys and
js-sys directly.

### 2.5 Language

These are first errors of crates in the two trees. A ✓ means that a small program with the same construct fails
in zustc too.

| missing | crates where it stops | ✓ |
|---|---|:-:|
| an impl for a reference to a reference (`impl .. for &'a &'b str`, "unsupported impl self type"; wasm-bindgen's `ref_by_copy!` writes `&$t` with `$t = &str`) | wasm-bindgen (its conversions) | ✓ |
| `impl Tr for &T` next to `impl Tr for &mut T` or `Box<S>` (E0119, conflicting implementations) | stable_deref_trait, tower-service | ✓ |
| one inherent method in impls for different self types (`Either<&L, &R>::cloned`, `Either<&mut L, &mut R>::cloned`) | either | ✓ |
| `dyn` with traits other than Send / Sync (`dyn Error + Send + Sync + UnwindSafe + 'a`) | thiserror 1 and 2 | ✓ |
| an associated type of a trait with a type parameter (`impl<S> Layer<S> for Identity { type Service = S; }`) | tower-layer | ✓ |
| a GAT with a type parameter (`type Output<Next> = (Next,);`) | next_tuple | ✓ |
| elision through a path's hidden lifetime (`fn f(e: Ref<ErrorImpl>) -> Option<&Backtrace>`, E0106) | anyhow | ✓ |
| `Rc<dyn Tr>` from `Rc::new(S)` | (dioxus-core keeps `Rc<dyn Any>`) | ✓ |
| `*mut T` where `*const T` is expected, and the raw pointer `as_ref` | ryu, inventory | ✓ |
| default type parameters (`struct B<F = ()>`) | (the output of typed-builder in `view!`) | ✓ |
| `impl Trait` inside an argument type (`Option<&[impl AsRef<str>]>`) | wasm-bindgen-shared (section 3) | ✓ |
| an enum variant through a crate path (`alloc::borrow::Cow::Borrowed(..)`), and `Cow<'_, str>` | wasm-bindgen-shared (section 3) | ✓ |
| const evaluation: a `const fn` with a loop as an array length, a static array built by a `const fn` | percent-encoding, httparse | ✓ |
| `.bitxor(i)` in a chain of integer methods (expected `bool`) | rustc-hash 1 | ✓ |
| an integer method on `unsafe { a.get_unchecked(i) }` | unicode-ident | ✓ |
| a parameter used only through a projection is reported unused | zmij | |

The counts of crates (of 222) that use the constructs give an idea of their weight. These are regex counts:

- an impl for `&T`, `Box`, `Option` or a slice: 112;
- `-> impl Trait`: 59;
- `dyn A + B`: 51;
- GATs: 8.

### 2.6 macro_rules, modules and attributes

- The fragments `ty` and `expr` are matched as the tokens up to the next delimiter. So in a repetition without
  separator, `$($t:ty)*` takes the whole list as one type: num-traits (`int_trait_impl!(Num for usize u8 u16 ..)`),
  wasm-bindgen (`vectors! { u8 i8 u16 .. }`). ✓
- Larger tt-munchers do not match:
  - `pin_project!` of pin-project-lite ✓, which tokio, futures-util, event-listener, throw_error, http-body-util and
    tower use;
  - `bitflags!` ✓: its definition does not even load ("unterminated item");
  - wasm-bindgen's `closures!` ✓;
  - konst's `__choose!`, bytes's `buf_try_get_impl!`, html-escape's `encode_impl!`, either_of's `tuples!`.
- The recursion limit is 64 (rustc: 128), and `#![recursion_limit]` is not read: typenum, writeable. ✓
- Macro scope:
  - `#[macro_use] extern crate x;` does not bring x's macros, though `x::m!` and `use x::m;` do: encoding_rs
    (`cfg_if!`), lock_api (`defer!`). ✓
  - `#[macro_use] mod macros;` in its own file does not either, though an inline `mod` does: httparse, zerocopy. ✓
- `pub extern crate alloc;` is not removed; only `extern crate x;` is: smallvec. ✓
- A doc comment before an item that `#[cfg]` removes is left behind, and is then an error (E0585): enumset, matchit,
  litemap, wasm-bindgen. ✓
- Modules:
  - a `mod de;` that a macro expands to is looked for next to the file that defines the macro, not where it is
    called: serde_core's `crate_root!()`;
  - `#[cfg_attr(.., path = "..")] mod` is not followed: mio.

### 2.7 std in the prelude

Each of these is missing, and each is the first error of some crate:

- `any::Any` (with `downcast_ref`), `rc::Weak` and `sync::Weak`;
- `RwLock`, `LockResult` / `PoisonError`, `LazyLock`, `Once`, `thread::Thread`;
- `panic::AssertUnwindSafe` / `UnwindSafe`, `task::RawWaker`, `io::IoSlice` / `IoSliceMut`, `vec::Drain`;
- `str::FromStr`, `time::SystemTime`, `num::NonZero*`, `marker::PhantomPinned`, `path::Ancestors`;
- `CStr::from_bytes_until_nul`, `spin_loop_hint`, and the default `ExactSizeIterator::len`.

Also, the prelude's `iter::Map` has 3 type parameters where std's has 2, so unicode-segmentation's `Map<I, F>` is
an error. How many of the 222 crates use them:

- `dyn Any`, `downcast` or `TypeId`: 28;
- `Weak`: 12;
- `RwLock`, `Mutex` or atomics: 69;
- `thread_local!`: 33;
- `OnceLock`, `LazyLock` or `OnceCell`: 23.

## 3. How deep it goes: the path through wasm-bindgen

A crate's first error is only the first. Every web API goes through unicode-ident → wasm-bindgen-shared →
wasm-bindgen. To see past the first errors on that path, scratch copies of these crates were changed one error at a
time:

| crate | error | change |
|---|---|---|
| unicode-ident | `wrapping_shr` on `unsafe { .. get_unchecked(..) }` | `>>` |
| wasm-bindgen-shared | `Option<&[impl AsRef<str>]>` ("unsupported type") | a type parameter |
| | `alloc::borrow::Cow::Borrowed(..)` ("unsupported path") | `use` and `Cow::Borrowed` |
| | `Cow<'_, str>` ("no method `bytes` on type `str`") | return a `String` |
| | `env!("CARGO_PKG_VERSION")`, `option_env!` | constants |
| wasm-bindgen | `closures!` matches none of its calls | the module removed |
| | a doc comment before a cfg'd-out item (E0585) | doc comments removed |
| | `$($t:ty)*` | `$($t:ident)*` |
| | `ref_by_copy!` makes `impl IntoWasmAbi for &&str` ("unsupported impl self type") | (stopped here) |

wasm-bindgen-shared (670 lines) needed five changes. wasm-bindgen (9.7 k lines) has at least four more, before its
proc macro and its ABI come into play. After it come js-sys (23 k lines) and web-sys, whose bindings are all
`#[wasm_bindgen]`.

## 4. What it would take

An order that removes one layer at a time. `cratebench/ui/run.py` measures steps 1 to 4 and 6: whether the crates
build. Step 5 needs a build for wasm32 and a check of the ABI (the `wasm-bindgen` CLI on the module), which it does
not do.

1. The leaves. `run.py` ranks the failing crates by how many crates are above them:
   - dioxus: serde_core 43, memchr 42, futures-core 38, unicode-ident 37, futures-sink 33, and futures-io,
     futures-task and slab 32 each;
   - leptos: memchr 29, unicode-ident 24, serde_core 23, futures-core 21.

   Most first errors are one of the std types of 2.7, the macro gaps of 2.6, or the impl rules of 2.5. A first
   goal: the futures crates, slab, memchr, unicode-ident and serde_core, none of which needs a proc macro or async.
2. Cargo as the driver (2.1): `cargo metadata`, and `cargo check --message-format=json` for the build scripts and
   the proc-macro artifacts; an edition per crate; two versions of one crate; `env!`.
3. Lowering async to state machines (2.3).
4. Proc macros (2.2, the first or the second way).
5. wasm-bindgen's ABI (2.4): 32-bit pointers on wasm32, imports and exports, the custom section and reference
   types. Then the `wasm-bindgen` CLI can make the JS glue.
6. The frameworks:
   - dioxus-core: `Rc<dyn Any>`, generational-box;
   - reactive_graph: `Arc<RwLock<..>>`, `Weak`;
   - tachys: views as types (tuples, associated types).

Steps 2 to 5 are each a large piece of work, and these two frameworks need all of them. A smaller target would reach
the browser sooner: a UI library made for zustc, with macro_rules instead of proc macros, a JS bridge of its own, and
callbacks instead of async.

## 5. Files

- `cratebench/ui/apps/dioxus`, `cratebench/ui/apps/leptos`: the counter apps, with versions pinned by Cargo.lock.
- `cratebench/ui/run.py`: the check (`--rustc` also builds the apps with cargo for wasm32 first).
- docs/language.md: async is listed as erased. The line on temporaries is updated: since the
  [RefCell report](2026-11-21-02-refcell-guards.md), temporaries of a `let` are no longer all kept to the end of the
  block.
- TODO.md: a section on UI frameworks, which points here.
