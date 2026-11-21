# zurgo: Cargo packages with zustc

`zurgo` builds a Cargo package with zustc, as `cargo build` builds it with rustc. It reads Cargo.toml and Cargo.lock,
finds the sources, turns on features as cargo does, and gives the graph to zustc's workspace build in the same
process. `zurgo metadata` writes the JSON of `cargo metadata`. The plan and what is left: mizchi/zust#28.

```bash
cargo install --path crates/zurgo
```

```bash
zurgo run
```

## Commands

| command | what it does |
|---|---|
| `zurgo build` | builds the package of the manifest (every member, for a virtual workspace) and its dependencies |
| `zurgo run [-- args]` | builds, then runs the binary (`--bin` chooses one when there are several) |
| `zurgo metadata` | the package graph as `cargo metadata --format-version 1` writes it |
| `zurgo clean` | removes `target/zust` |

Options:
- `--manifest-path <Cargo.toml>`: the package. Without it, zurgo uses the nearest Cargo.toml from the current
  directory up.
- `-r` / `--release`: zustc's `-O`.
- `-F` / `--features a,b`, `--all-features`, `--no-default-features`. `dep/feat` names a dependency's feature.
- `--target <triple>`: `x86_64-unknown-linux-gnu` (the default). `wasm32-unknown-unknown` is accepted, but zustc's
  workspace build does not take it yet.
- `--bin <name>`, `-j <n>`, `--time` (zustc's timings), `--no-cache`.
- `-v`: prints the workspace zurgo gives zustc, in the format of `zustc build`. It is also written to
  `target/zust/<profile>/zurgo-ws.txt`, so `zustc build` can repeat the build.
- `--filter-platform <triple>` (metadata).
- `--locked`, `--offline`, `--frozen` are accepted. zurgo always uses Cargo.lock as it is and never downloads.

Output goes to `target/zust/[<triple>/]<profile>/` (`CARGO_TARGET_DIR` moves `target`), apart from cargo's own
output. A binary is `target/zust/debug/<name>`.

## What it reads

- **Manifests.**
  - `[package]`, with the fields a workspace member inherits (`version.workspace = true` ...).
  - `[lib]` and `[[bin]]`, and the targets Cargo finds by itself: `src/lib.rs`, `src/main.rs`, `src/bin/*.rs`,
    `src/bin/*/main.rs`.
  - Dependencies of the three kinds, also under `[target.'cfg(..)']`, with renames, `optional`,
    `default-features` and `features`.
  - `[features]`, and `[workspace]` (members with `*` globs, `exclude`, `[workspace.dependencies]`).
  - The TOML parser is zurgo's own (`crates/zurgo/src/toml.rs`).
- **Cargo.lock** (versions 1 to 4). zurgo has no resolver yet, so the versions are the lock's. Without a lock, zurgo
  says to run `cargo generate-lockfile`.
- **Sources.**
  - Workspace members and path dependencies.
  - crates.io packages in `$CARGO_HOME/registry/src`, where `cargo fetch` puts them.
  - A package that is not downloaded (one for another platform) is read from cargo's index cache
    (`registry/index/*/.cache`): its dependencies and features, enough for `metadata`. Building it is an error that
    says to run `cargo fetch`.

## Features, as cargo turns them on

- **`zurgo build`** uses the rules of cargo's feature resolver (resolver 2 and 3). It is used for builds:
  - a package built for the host (a build dependency, a proc macro and what they use) has its own feature set;
  - dev-dependencies are left out;
  - a `[target.'cfg(..)']` dependency is checked against the side it is on;
  - a weak `dep?/feat` waits until something else turns `dep` on.
- **`zurgo metadata`** gives what `cargo metadata` reports, which comes from cargo's dependency resolver:
  - one feature set per package, over every kind and every platform;
  - a weak `dep?/feat` also pulls `dep` into the graph;
  - `--filter-platform` only chooses which edges are shown. An edge is shown if one of its declarations applies,
    and its `dep_kinds` list all its declarations.
- **In both:**
  - `x/f` turns on dependency `x`, and, when `x` is optional, the feature named `x` if there is one;
  - an optional dependency that no feature names with `dep:` has an implicit feature of its name.

`crates/zurgo/tests/zurgo.rs` checks that `zurgo metadata` and `cargo metadata` give the same graph for the test
packages and for cratebench/ui's dioxus and leptos apps (344 and 180 packages; also with `--filter-platform` for
wasm32 and x86-64 Linux). For dioxus, zurgo takes 39 ms and cargo 320 ms.

## Not yet (mizchi/zust#28)

- **Build scripts are not run.** A warning names each one. The cfgs they set and the files they write are missing.
- **Proc-macro dependencies are an error** (mizchi/zust#29).
- **Two versions of one library**, and **renamed dependencies**, are errors: zustc's workspace has one name per
  crate.
- **Other sources:** git dependencies, other registries, source replacement.
- **No `[patch]`, no resolver of its own, no downloads, no `test` / `doc`.**
- **Editions:** zustc does not take an edition per crate yet.
