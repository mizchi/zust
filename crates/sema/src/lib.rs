//! zustc's semantic layer (shared by the compiler and LSP / analysis).
//! Item summaries → Program (the tables of fns, types, traits and impls, the instances of generic fns) → type inference of bodies (ast_infer).

pub use zust_syntax::*;

pub mod ast_consteval;
pub mod ast_infer;
pub mod program;
pub mod simd_sig;
pub mod solver;
pub mod traits;
