//! zustc's syntax layer (shared by the compiler and LSP / analysis).
//! Source → cfg / macro expansion / flattening `mod` / moving nested items (prep) → splitting into items (lexer::split) →
//! syntax tree (parse) → item summaries (ast_summary). The prelude is the stand-in for the standard library that the compiler appends.
//! (inside the files, write paths like `crate::fxhash`: the lower layers' modules are re-exported here. When zustc compiles itself,
//! everything is laid out as modules of one crate, so the same paths resolve)

pub use zust_base::*;

pub mod ast_closure;
pub mod ast_summary;
pub mod ast_types;
pub mod cfg;
pub mod ctors;
pub mod enc;
pub mod front;
pub mod lexer;
pub mod macros;
pub mod parse;
pub mod pm;
pub mod prelude;
pub mod prep;
pub mod regions;
pub mod wf;
pub mod srcmap;
pub mod stub;
pub mod test_harness;
pub mod types;
pub mod types_enc;
