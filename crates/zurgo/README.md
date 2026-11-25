# zurgo

A Cargo-compatible build driver for zustc (#28). It reads Cargo.toml and Cargo.lock, finds the sources (workspace
members, path dependencies, the crates.io packages that `cargo fetch` unpacked), activates features as cargo does,
and builds the graph with zustc's workspace build, in process.

| module | what |
|---|---|
| `toml`, `json` | a TOML parser and a JSON reader (no crates.io dependency) |
| `manifest` | Cargo.toml: targets, dependencies, features, workspace inheritance |
| `lock` | Cargo.lock, and where the locked sources are |
| `semver` | versions and requirements |
| `platform` | targets and `cfg(..)` platform keys |
| `resolve` | the package graph and feature activation |
| `build_script` | runs build scripts and reads their directives |
| `proc_macro_stub` | what stands in for a proc-macro crate |
| `compile` | the crate graph to zustc's workspace build |
| `metadata` | `zurgo metadata`: the JSON of `cargo metadata` |
| `test_runner` | `zurgo test`: one process per test, as nextest does |

Depends on: zustc. Tests: `tests/zurgo.rs` with `tests/fixtures`, and `cratebench/test.py` (real crates' tests
against `cargo test`).
