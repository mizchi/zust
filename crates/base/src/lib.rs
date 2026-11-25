//! Small building blocks used throughout zustc (no external crates; zustc itself can compile them).
//! - fxhash: FxHasher / FxMap / FxSet for name tables etc.
//! - key: cache keys (Key / KeyHasher)
//! - prof: per-stage timing
//! - par: the front end's thread parallelism (an ordered parallel map on scoped threads; not self-hosted)

pub mod fxhash;
pub mod key;
pub mod par;
pub mod prof;
