//! Taking `&mut` of an immutable binding (E0596).
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn main() {
    let v = vec![1];
    let m = &mut v; //~ ERROR E0596
    m.push(2);
    println!("{:?}", v);
}
