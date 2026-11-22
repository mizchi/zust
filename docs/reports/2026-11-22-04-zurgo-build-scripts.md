# zurgo M1: build scripts, `env!`, and proc-macro stubs

Date: 2026-11-22

M0 built Cargo packages without running their build scripts, and stopped on any proc-macro dependency. This is the
second milestone of mizchi/zust#28 and the start of step W0 of the wasm-bindgen roadmap (mizchi/zust#38). Usage:
[zurgo.md](../zurgo.md).

## 1. What changed

- **zustc: a crate's own cfgs and compile-time environment.**
  - `env!` / `option_env!` are expanded when a file is read. They read the crate's environment first, then zustc's
    own, as rustc reads its own.
  - `include!(concat!(env!("OUT_DIR"), "/x.rs"))` therefore works.
  - Each crate of a workspace can carry cfgs (`cfg=name`, `cfg=name=value`) and environment variables
    (`env=KEY=VALUE`).
- **zurgo: build scripts.**
  - A build script is compiled with its build-dependencies, by zustc when it can and by rustc otherwise.
  - It runs with cargo's variables, and its `cargo:` / `cargo::` output is read.
  - It runs again only when something it depends on changes.
- **zurgo: cargo's variables for every crate.** `CARGO_PKG_*`, `CARGO_MANIFEST_DIR`, `CARGO_CRATE_NAME`, and
  `CARGO_BIN_NAME` for a bin.
- **zurgo: proc-macro dependencies are stubs.** A proc-macro crate is built as an empty library of its name, so a
  crate that only re-exports it builds. Using one of its macros is an error naming mizchi/zust#29, not an attribute
  dropped silently.
- **zustc fixes found on the way:**
  - `cfg!` inside the arguments of std's format macros (`println!("{}", cfg!(unix))`) was never evaluated;
  - `compile_error!`;
  - `dyn Error + Send + Sync + UnwindSafe` (only `Send` and `Sync` counted as auto traits);
  - `std::path::Display`;
  - `fmt::Pointer`.

## 2. Decisions

These were open questions in mizchi/zust#28:

- **`RUSTC` for build scripts is the real rustc** (question 1). A script's probes (thiserror's and anyhow's nightly
  probes, autocfg) and its version checks (rustversion, version_check) then see what they see under cargo. A crate
  takes the same paths as in its cargo build. zustc follows a recent stable rustc, so those are the paths it must
  support.
- **The build-script compiler is zustc, with rustc as the fallback** (question 3, `--build-script-compiler auto`).
  - Of the scripts tried, zustc compiles the fixture's `gen` script only. thiserror's, anyhow's, libc's, serde_core's
    and the fixture's app script stop on std functions zustc lacks: `env::vars()`, `Command` with an `OsString`,
    `process::Output`, `fs::create_dir`.
  - rustc builds those, so every script runs.
  - zurgo remembers which compiler built a script until its sources change, so a script zustc cannot build is not
    tried again on every build.
- **Proc-macro dependencies are stubs, with a check.**
  - zustc drops an unknown attribute silently, so the stub alone could miscompile code that uses an attribute macro.
  - zurgo therefore reads the macros a proc-macro crate defines (`#[proc_macro_derive(..)]`,
    `#[proc_macro_attribute]`, `#[proc_macro]`). A crate of the build that reaches the proc-macro crate and whose
    source uses one of its derives or attributes stops the build before anything is compiled.
  - Function-like macros expand to a `compile_error!`.

## 3. Results

`crates/zurgo/tests/zurgo.rs`:

- **`build_scripts_match_cargo`.** A fixture package compared with `cargo run` under both `rustc` and `auto`. The
  output is the same. The package has:
  - a build script with a build-dependency, that reads `DEP_GEN_*` and writes code into `OUT_DIR`;
  - `rustc-cfg` (also `name="value"`) and `rustc-env`;
  - `CARGO_PKG_*` with a pre-release version, `CARGO_CFG_*` and `PROFILE`;
  - a dependency with `links` whose script emits metadata in both forms and a cfg that depends on a feature.

  The test also checks that a second build runs no script, and that an edit of the file the app's script names in
  `rerun-if-changed` runs only that script. A failing script stops the build with its output.
- **thiserror 2.0.21's library builds.** An app that depends on it prints what its cargo build prints, and so it joins
  `registry_crates_run_like_cargo`. The library needs:
  - its build script, which writes `OUT_DIR/private.rs` from `CARGO_PKG_VERSION_PATCH` and probes `$RUSTC`;
  - `include!(concat!(env!("OUT_DIR"), "/private.rs"))`;
  - the stub for thiserror-impl;
  - the four zustc fixes above.

  `#[derive(thiserror::Error)]` is the #29 error.
- **`errors`.** A proc-macro dependency that is not used builds. Its attribute used is the #29 error, and so is its
  function-like macro.

Crates whose build scripts now run, and where they stop next. All of these are zustc's own gaps:

| crate | build script | the crate itself |
|---|---|---|
| thiserror 2.0.21 | runs (rustc) | builds |
| anyhow 1.0.104 | runs (rustc) | `error.rs:808`: a missing lifetime specifier (E0106) |
| libc 0.2.191 | runs (rustc) | `mod types;` inside the `prelude!` macro |
| serde_core 1.0.229 | runs (rustc) | `mod de;` inside the `crate_root!` macro |

For the last two, rustc looks for the module's file where the macro is expanded (the crate root). zustc looks
beside the file that defines the macro.

## 4. Next

For W0 of mizchi/zust#38:
- workspace builds for wasm32 (zustc's workspace build is x86-64 only);
- crate identity apart from the extern name (two versions of one crate, renamed dependencies);
- the compiler gaps on the way to wasm-bindgen-shared (the UI report, sections 3, 2.5 and 2.6).

Then W1: proc macros themselves (mizchi/zust#29). The rustc path for build scripts already builds proc-macro crates
as dylibs, which is backend A of #29.
