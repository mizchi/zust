//! Moving out of a reference (E0507): both the reference's owner and the new owner would drop it.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn take(v: &Vec<String>) -> String {
    let s = v[0]; //~ ERROR E0507
    s
}
fn main() {
    let v = vec![String::from("a")];
    println!("{}", take(&v));
}
