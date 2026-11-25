//! Analysis for editors (used by zust-analyzer; the compiler (zustc) does not depend on this crate).
//! It uses neither the syntax tree nor type checking and reads from zust-syntax's token sequence: even for a source broken mid-edit it answers as far as it can read.
//!
//! - `symbols`: symbols for documentSymbol / workspace/symbol (items and members)
//! - `navigate`: the per-file index (FileIndex), and definition / references / hover / completion

pub mod navigate;
pub mod symbols;
