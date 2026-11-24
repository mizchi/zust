//! A library that re-exports the proc macros (as wasm-bindgen does its own)
pub use pm::{scaled, sum as plus, Describe};

/// A function of this library that its users' derive output calls
pub fn label(name: &str, n: usize) -> String {
    format!("{name}/{n}")
}
