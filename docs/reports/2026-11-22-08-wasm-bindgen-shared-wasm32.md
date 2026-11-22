# W0: wasm-bindgen-shared for wasm32, with zurgo

Date: 2026-11-22

Step W0 of the wasm-bindgen roadmap (mizchi/zust#38) was to build wasm-bindgen-shared for wasm32 with zurgo.
wasm-bindgen-shared is the part of wasm-bindgen with no proc macro. The step is done: an app that calls the whole
public API of wasm-bindgen-shared 0.2.130 is built with `zurgo run --target wasm32-unknown-unknown`, and under node it
prints what its cargo build prints. The previous milestone is the [zurgo M1 report](2026-11-22-04-zurgo-build-scripts.md).
Usage: [zurgo.md](../zurgo.md).

## 1. What changed

### 1.1 Workspace builds for wasm32

`zustc build --target wasm32-unknown-unknown` and `zurgo --target wasm32-unknown-unknown` used to stop with "workspace
builds are not supported yet".

- **The problem.** On x86-64, each crate is an ELF object and the system linker joins them. There is no wasm linker
  here: zustc's module writer writes a module in one go, from all its functions and data.
- **What a crate's `.o` holds now (wasm32).**
  - A crate's object is its *parts*: the functions and data that it would give the module writer, serialized
    (`zust-wasm-parts 1`).
  - The bin's link step reads its own parts and those of its dependencies, then gives each crate's local symbols a
    name of their own (`name@crate`).
  - It then calls the module writer once and writes `<bin>.wasm`.
- **The cache and incremental builds** work as on x86-64. An edit in one crate rewrites that crate's parts, and the
  bin is linked again.
- **`zurgo run`** runs a wasm bin as cargo does: with the program in `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER`,
  then the module and the arguments. In this repository the runner is `node tests/wasm/run.mjs`. Without the variable
  it is an error that says so.
- **Build scripts** are still built for the host and run on it. They see wasm32's `CARGO_CFG_*`, as under cargo.
- **The wasm backend** had no byte swap of a 128-bit integer (`u128::swap_bytes`, `to_be`), which byteorder uses.
  It swaps the two 64-bit halves now, each with its bytes reversed.
- **Two fixes found by the test of that swap** (examples/int128.rs):
  - minclif swapped only the low 64 bits of a 128-bit integer, so the high half was lost.
  - A single-file build skipped its link when its object was unchanged and an executable was there, even an
    executable older than the object. `-o x.cold` and `-o x` share `x.o`, so tests/compare.sh's warm build kept an
    old executable. Now the executable must not be older than its object.

### 1.2 The compiler gaps on the way

wasm-bindgen-shared hit the first three. The UI report lists the others for the crates after it
([section 3](2026-11-21-03-ui-frameworks-wasm.md)).

- **`alloc::borrow::Cow::Borrowed(..)`.** A value through an `alloc::` path is now found as through `std::` or
  `core::`.
- **`Cow<'_, str>`.**
  - zustc's prelude has two Cows: `Cow<[T]>`, and `__CowStr` for `Cow<str>`.
  - `Cow::Borrowed(s)` with a `&str` or `String` argument is now the str one without an annotation. This holds where
    types are inferred and in code generation. Before, inference took the slice one, and `name` became `&[T]`:
    "no method `bytes` on type `str`".
  - A pattern `Cow::Borrowed(s)` on a `Cow<str>` is the str one too.
  - `Cow<str>` has `Display` and `Debug`, and as an argument it coerces to `&str`.
- **`x.as_ref()`** follows the `AsRef` bounds in scope:
  - it is `&str` for `T: AsRef<str>`, and `&[u8]` for `T: AsRef<[u8]>`;
  - this also holds in a closure (`.map(|s| s.as_ref())`), whose return type is now inferred;
  - it works on a `&&str` too, which used to be "cannot index into a value of type `str`".
- **`$($t:ty)*`.** A type now ends where the next one starts, as rustc splits them:
  - the list `u8 u16 Vec<u8> (bool, char) fn(u8) -> u8 m!(u8)` is six types;
  - parentheses go on a type only after a name (`Fn(u8)`) or a type macro's `!`.

  Before, the repetition took the whole list as one type: wasm-bindgen's `vectors!`, num-traits'
  `int_trait_impl!`.
- **A crate or a module under another name.** After `use util as u;`, or `extern crate util as u;`, the path
  `u::f()` is `util::f()`.

## 2. Results

- `crates/zurgo/tests/zurgo.rs`:
  - **`registry_crates_run_like_cargo`** now includes wasm-bindgen-shared 0.2.130. The app calls its whole public
    API, and on x86-64 it prints what its cargo build prints. Its build script is compiled by zustc.
  - **`wasm32_runs_like_native`** (new). Each fixture is built with `--target wasm32-unknown-unknown` and run under
    node, and prints what its x86-64 build prints. The fixtures are:
    - the path workspace and the build-script package;
    - fnv, adler2, hex, itoa, rustc-hash, cfg-if, thiserror and wasm-bindgen-shared.

    The two differences are those of the target: the build script's `CARGO_CFG_*` line, and cfg-if's `cfg(unix)` arm.
- **`tests/wasm.rs`'s `wasm_workspace_runs_like_native`** (new). tests/workspace (three crates) is built as one module.
  On every path it prints what its x86-64 build prints, with the same exit code. The paths include a panic in another
  crate's generic function, an `unwrap` of `None`, and a division by zero in a dependency.
- **`tests/probe/cow_asref_ty_frags.rs`** prints what rustc's build prints. It covers the gaps of 1.2.
- **tests/workspace** calls `util` through `use util as u;`. It still matches rustc's build there.
- **`cratebench/ui/run.py`** now builds for wasm32, and gives each crate cargo's variables (`CARGO_PKG_*`). Build
  scripts still do not run there. The results, against the same zustc with the x86-64 cfg and no variables:

  | app | crates that pass | before |
  |---|---:|---:|
  | dioxus | 19 | 18 |
  | leptos | 18 | 17 |

  - wasm-bindgen-shared passes, and wasm-bindgen stops on `closures!`.
  - byteorder needed the 128-bit byte swap.
  - memchr's wasm32 SIMD module stops on `impl Vector for v128`. Its x86-64 module stops on the same kind of impl,
    for `__m128i`.

The wasm-bindgen-shared app, built cold (the median of 3, 4 cores). Each build includes the build script and
unicode-ident:

| build | time | output |
|---|---:|---|
| zurgo, x86-64 | 0.19 s | an executable of 59 KB |
| zurgo, wasm32 | 0.14 s | a module of 164 KB |
| cargo, x86-64 (debug) | 0.59 s | an executable of 5.4 MB |

The wasm32 build is the faster of the two zurgo builds: it does not run the system linker.

The KPI is the no-op rebuild of each crate of cratebench, in instructions, measured as `bench/ci_bench.py` measures
it. Over all the crates, main takes 10.102 billion instructions and this branch 10.057 (−0.4%). No crate moved by
more than 1%.

## 3. Next

W1 of mizchi/zust#38: wasm-bindgen itself. These are the gaps known so far:

- `closures!` (a tt-muncher) matches none of its calls;
- `ref_by_copy!` makes `impl IntoWasmAbi for &&str`, an impl for a reference to a reference;
- its proc macro (`#[wasm_bindgen]`, mizchi/zust#29) and its ABI.

The rest of W0's list in #38 moves on with it:

- **a crate's identity apart from its extern name.** Two versions of one crate, and a dependency renamed in
  Cargo.toml (`package = ".."`), are still errors;
- **a crate's own edition;**
- **`stringify!` of a type.** zustc spaces it as tokens (`Vec < u8 >`), while rustc writes `Vec<u8>`.
