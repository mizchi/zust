//! Calling a function through a pointer of another signature: the call's type check traps.
fn add(a: u64, b: u64) -> u64 {
    a + b
}
fn main() {
    let f: fn(f64) -> f64 = unsafe { std::mem::transmute(add as fn(u64, u64) -> u64) };
    println!("after {}", std::hint::black_box(f)(1.0));
}
