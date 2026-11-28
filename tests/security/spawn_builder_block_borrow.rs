//! A closure given to `Builder::spawn` as a block (`spawn({ || .. })`) that borrows a local may outlive it (E0373), as
//! one written in the call: the thread would read the string after the function freed it.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
use std::thread;
fn start() -> thread::JoinHandle<usize> {
    let s = String::from("freed");
    thread::Builder::new().spawn({ || s.len() }).unwrap() //~ ERROR E0373
}
fn main() {
    println!("{}", start().join().unwrap());
}
