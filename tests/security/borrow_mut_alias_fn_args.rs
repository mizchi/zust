//! Passing the same value as `&mut` and `&` to one call (E0502).
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn f(a: &mut Vec<i32>, b: &Vec<i32>) {
    a.push(b.len() as i32);
}
fn main() {
    let mut v = vec![1];
    f(&mut v, &v); //~ ERROR E0502
    println!("{:?}", v);
}
