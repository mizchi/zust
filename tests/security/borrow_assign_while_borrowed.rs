//! Assigning to a place that is borrowed (E0506): the old value is dropped under the reference.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn main() {
    let mut s = String::from("old");
    let r = &s;
    s = String::from("new"); //~ ERROR E0506
    println!("{r} {s}");
}
