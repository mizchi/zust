//! A constant index out of bounds: rustc rejects it at compile time (unconditional_panic is deny
//! by default); a panic at run time is also safe. Reading past the array is not.
//@ expect: reject-or-panic
fn main() {
    let a = [1, 2, 3];
    println!("{}", a[5]); //~ ERROR this operation will panic at runtime
}
