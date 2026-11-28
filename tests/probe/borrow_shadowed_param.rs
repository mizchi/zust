// `key.borrow()` of a local that shadows the parameter `key: &K` (#261): the local's type decides (the annotated
// `&String`), not K's bound `K: Borrow<str>`
use std::borrow::Borrow;

fn f<K: Borrow<str>>(key: &K) -> usize {
    let first: &str = key.borrow();
    let key = String::from("xy");
    let r: &String = key.borrow();
    first.len() * 10 + r.len()
}

fn main() {
    println!("{}", f(&"abc"));
    println!("{}", f(&String::from("abcd")));
}
