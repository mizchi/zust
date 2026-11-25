//! zustc の型の検査。acg は構文木と型推論の結果を読みながら型を検査し、命令の列を zust_ir::Recorder に記録する
//! (check / LSP はここで止める。機械語にするのは zust-codegen の lower)。build_cache は関数ごとの結果 (Blob) の直列化

pub use zust_ir as ir;
pub use zust_sema::*;

pub mod acg;
pub mod build_cache;
