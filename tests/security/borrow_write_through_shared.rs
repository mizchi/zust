//! Writing through a shared reference (E0594).
//@ expect: reject
fn set(x: &i32) {
    *x = 1; //~ ERROR E0594
}
fn main() {
    let a = 0;
    set(&a);
    println!("{a}");
}
