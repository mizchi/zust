//! Primitive traits (fixture).
pub trait Copy: Clone {}
/// Derive macro generating an impl of the trait `Copy`.
pub macro Copy($item:item) {
    /* compiler built-in */
}
