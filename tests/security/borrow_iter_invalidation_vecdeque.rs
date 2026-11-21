//! Iterator invalidation on a VecDeque (E0502): zustc's deque iterator holds a raw pointer to the deque, so only
//! the borrow checker keeps a push (which may move the elements) away from a live iterator.
//@ pattern: iterator invalidation
//@ expect: reject
use std::collections::VecDeque;
fn main() {
    let mut d: VecDeque<String> = VecDeque::new();
    d.push_back("a".to_string());
    let mut it = d.iter();
    for i in 0..100 {
        d.push_back(i.to_string()); //~ ERROR E0502
    }
    println!("{:?}", it.next());
}
