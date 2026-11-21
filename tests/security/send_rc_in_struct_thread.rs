//! An `Rc` inside a struct inside a Vec is still not Send (E0277).
//@ pattern: Send / Sync
//@ expect: reject
use std::rc::Rc;
struct Node {
    next: Option<Rc<u32>>,
}
fn main() {
    let v = vec![Node { next: Some(Rc::new(1)) }];
    let h = std::thread::spawn(move || { //~ ERROR E0277
        println!("{}", v.len());
    });
    h.join().unwrap();
}
