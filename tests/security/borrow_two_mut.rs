//! Two live `&mut` to the same place (E0499): the aliasing rule every other guarantee rests on.
//@ expect: reject
fn main() {
    let mut v = vec![1, 2, 3];
    let a = &mut v;
    let b = &mut v; //~ ERROR E0499
    a.push(4);
    b.push(5);
    println!("{:?}", v);
}
