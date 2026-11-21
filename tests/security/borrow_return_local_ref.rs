//! Returning a reference to a local (E0515): a dangling pointer to a dead stack frame.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn dangle() -> &'static i32 {
    let x = 5;
    &x //~ ERROR E0515
}
fn main() {
    println!("{}", dangle());
}
