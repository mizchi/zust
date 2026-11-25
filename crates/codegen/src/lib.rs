//! zustc's backend side. lower reassembles the instruction sequence that acg recorded (zust_ir::Recorder) into minclif and
//! turns it into machine code (for Cranelift, it is passed to zust-clif). link is the linker invocation

pub use zust_typeck::*;
pub use minclif;

pub mod link;
pub mod lower;
