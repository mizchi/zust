//! A closure that borrows a local and outlives it (E0373).
//@ expect: reject
fn make() -> Box<dyn Fn() -> usize> {
    let s = String::from("captured");
    Box::new(|| s.len()) //~ ERROR E0373
}
fn main() {
    println!("{}", make()());
}
