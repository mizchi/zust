//! A slice of a Vec kept across `clear` + refill (E0502): the slice would read dropped Strings.
//@ pattern: iterator invalidation
//@ todo: `&v[..]` is not a loan of v: a clear (dropping the elements) is accepted under it
//@ expect: reject
fn main() {
    let mut v = vec![String::from("x"), String::from("y")];
    let s = &v[..];
    v.clear(); //~ ERROR E0502
    println!("{:?}", s);
}
