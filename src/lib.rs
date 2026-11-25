//! zustc: a small Rust (subset) compiler specialized for incremental development builds.
//! The contents are split into the crates in crates/ (this crate is the CLI plus re-exports of them):
//! - `zust-base`: building blocks (FxHash tables, cache keys, serialization)
//! - `zust-syntax`: lexing, syntax trees, preprocessing (cfg / macros / `mod` / nested items), item summaries, prelude selection
//! - `zust-sema`: Program (the tables of names, types and traits), type inference
//! - `zust-codegen`: acg (code generation and type checking), Blob serialization, linking
//! - `zust-core`: session (cache, parallelization, workspaces, check). Re-exports the layers below
//! - `minclif`: a small x86-64 backend with a Cranelift-like API (no external crates)
//! - `zust-clif`: session's Cranelift backend (the default)
//! (base through core do not depend on syn / Cranelift, and zustc itself can compile them)
//! The LSP server (zust-analyzer, which uses the editor-oriented analysis zust-ide) is a separate binary, and this crate does not depend on it

pub use zust_core::*;
pub use zust_clif as clif_backend;
pub use zust_wasm as wasm_backend;
pub use zust_core::session::{Diagnostic, Options, BuildReport, Session};
