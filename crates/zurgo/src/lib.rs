//! zurgo: a Cargo-compatible build driver for zustc (mizchi/zust#28).
//!
//! It reads Cargo.toml and Cargo.lock, finds the sources (workspace members, path dependencies, and crates.io
//! packages that `cargo fetch` unpacked), activates features as cargo does, and builds the graph with zustc's
//! workspace build, in process. `zurgo metadata` writes the JSON of `cargo metadata`.
//!
//! - `toml`: the TOML parser (no crates.io dependency); `json`: a JSON reader, for cargo's index cache
//! - `manifest`: Cargo.toml (targets, dependencies, features, workspace inheritance)
//! - `lock`: Cargo.lock, and where locked sources are
//! - `semver`: versions and requirements
//! - `platform`: targets and `cfg(..)` platform keys
//! - `resolve`: the package graph and feature activation
//! - `metadata`: `zurgo metadata`
//! - `compile`: `zurgo build` / `run`

pub mod build_script;
pub mod compile;
pub mod json;
pub mod lock;
pub mod manifest;
pub mod metadata;
pub mod platform;
pub mod proc_macro_stub;
pub mod resolve;
pub mod semver;
pub mod toml;
