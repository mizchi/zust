//! Assigning to a place that is borrowed (E0506): the old value is dropped under the reference.
//@ expect: reject
fn main() {
    let mut s = String::from("old");
    let r = &s;
    s = String::from("new"); //~ ERROR E0506
    println!("{r} {s}");
}
