//! zustc's type checking. acg checks types while reading the syntax tree and the type inference results, and records the instruction sequence in zust_ir::Recorder
//! (check / LSP stop here; turning it into machine code is zust-codegen's lower). build_cache is the serialization of each fn's result (Blob)

pub use zust_ir as ir;
pub use zust_sema::*;

pub mod acg;
pub mod build_cache;
