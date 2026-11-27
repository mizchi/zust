//! A value becomes a `dyn Send` (an auto trait alone as the object's trait) only if its type is `Send` (E0277): a
//! `Box<dyn Send>` holding an `Rc` would let another thread count it.
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
fn main() {
    let rc = Rc::new(5);
    let keep = rc.clone();
    let b: Box<dyn Send> = Box::new(rc); //~ ERROR E0277
    let h = std::thread::spawn(move || drop(b));
    h.join().unwrap();
    println!("{}", keep);
}
