//! `thread::Builder::spawn` asks of the closure's value what `thread::spawn` asks: an `Rc` made on the spawned thread
//! and given back through `join` is not `Send` (E0277), as the thread may still hold a clone of it.
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
use std::thread;
fn main() {
    let h = thread::Builder::new().spawn(|| Rc::new(5)).unwrap(); //~ ERROR E0277
    println!("{}", h.join().unwrap());
}
