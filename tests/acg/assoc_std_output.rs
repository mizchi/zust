// std's associated types the prelude does not write out: a number's operator Output (also through a reference) and a
// reference's or Box's Deref Target
use std::ops::{Add, Deref, Mul};
fn add<T>(x: T) -> <i32 as Add<T>>::Output where i32: Add<T> {
    42i32 + x
}
fn mul(x: &f64) -> <&f64 as Mul<f64>>::Output {
    x * 2.0
}
fn first(b: Box<u8>) -> <Box<u8> as Deref>::Target {
    *b
}
fn main() {
    let f: fn(&i32) -> <&i32 as Deref>::Target = |x| *x + 1;
    println!("{} {} {} {}", add(1i32), mul(&1.5), first(Box::new(7)), f(&9));
}
