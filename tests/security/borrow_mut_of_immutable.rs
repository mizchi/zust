//! Taking `&mut` of an immutable binding (E0596).
//@ expect: reject
fn main() {
    let v = vec![1];
    let m = &mut v; //~ ERROR E0596
    m.push(2);
    println!("{:?}", v);
}
