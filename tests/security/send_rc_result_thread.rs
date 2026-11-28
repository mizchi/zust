//! A spawned closure's result goes to the thread that joins it, so it must be `Send` too (E0277): an `Rc` made on the
//! new thread and handed back would share its count between two threads.
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
fn main() {
    let h = std::thread::spawn(|| Rc::new(5)); //~ ERROR E0277
    println!("{}", h.join().unwrap());
}
