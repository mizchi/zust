//! A reference into a Vec kept across a push (E0502): the push may move the elements.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn main() {
    let mut v = vec![10, 20];
    let first = &v[0];
    v.push(30); //~ ERROR E0502
    println!("{first}");
}
