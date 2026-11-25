//! The zustc driver. driver2 (Session2) brings together analysis, check (LSP diagnostics) and the cache, and with feature `build` (the default),
//! workspace builds (parallelization, machine code, linking).
//! Layers: base → syntax → sema → typeck (type checking, recording instructions) → codegen (machine code, linking: only with build) → core (here).
//! The LSP is zust-analyzer (uses this without build), the Cranelift backend is zust-clif. The modules of the lower layers can also be reached from here,
//! as in `zust_core::lexer`

pub use zust_typeck::*;
#[cfg(feature = "build")]
pub use zust_codegen::{link, lower, minclif};

pub mod driver2;
#[doc(hidden)]
pub mod testgen;
