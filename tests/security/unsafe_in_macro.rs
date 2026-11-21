//! Neighbor of unsafe_in_closure: a raw pointer dereference that a `macro_rules!` writes into safe code is still
//! safe code (E0133). zustc skips checks at positions it does not know: a macro's expansion must not be one.
//@ pattern: E0133 context
//@ expect: reject
macro_rules! read {
    ($p:expr) => {
        *$p
    };
}
fn main() {
    let p = 0x40 as *const u32;
    let v = read!(p); //~ ERROR E0133
    println!("{v}");
}
