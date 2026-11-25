//! Small building blocks used throughout zustc (no external crates; zustc itself can compile them).
//! - fxhash: FxHasher / FxMap / FxSet for name tables etc.
//! - key: cache keys (Key / KeyHasher)
//! - prof: per-stage timing

pub mod fxhash;
pub mod key;
pub mod prof;
