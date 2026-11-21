//! Overflow in a constant (E0080) is a compile error, not a wrapped value.
//@ rule: constant evaluation
//@ todo: E0080: overflow in a const is found only when it is evaluated at run time (a panic)
//@ expect: reject
const N: u8 = 255 + 1; //~ ERROR E0080
fn main() {
    println!("{N}");
}
