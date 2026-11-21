//! Two `&mut` into one Vec through `get_mut` / `last_mut` (E0499).
//@ pattern: two &mut
//@ expect: reject
fn main() {
    let mut v = vec![1, 2, 3];
    let a = v.get_mut(0).unwrap();
    let b = v.last_mut().unwrap(); //~ ERROR E0499
    *a += 1;
    *b += 1;
    println!("{:?}", v);
}
