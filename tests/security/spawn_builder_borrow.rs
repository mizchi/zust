//! A closure given to `thread::Builder::spawn` is checked as one given to `thread::spawn`: one that borrows a local of
//! the function may outlive it (E0373), and on an OS thread it would read the string after the function freed it.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
use std::thread;
fn start() -> thread::JoinHandle<usize> {
    let s = String::from("freed");
    thread::Builder::new().spawn(|| s.len()).unwrap() //~ ERROR E0373
}
fn main() {
    println!("{}", start().join().unwrap());
}
