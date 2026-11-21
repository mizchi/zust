//! Iterator invalidation on a String (E0502): `chars()` reads the buffer a `push_str` may move.
//@ pattern: iterator invalidation
//@ expect: reject
fn main() {
    let mut s = String::from("abc");
    let mut cs = s.chars();
    s.push_str("a much longer text that makes the String reallocate its buffer"); //~ ERROR E0502
    println!("{:?}", cs.next());
}
