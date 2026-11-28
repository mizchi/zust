//! `thread::Builder::spawn` asks what `thread::spawn` asks: a closure that moves an `Rc` in, or one that returns an
//! `Rc`, is not `Send` (E0277). Either way two threads would count the same `Rc` without atomics.
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
use std::thread;
fn main() {
    let rc = Rc::new(5);
    let keep = rc.clone();
    let h = thread::Builder::new().spawn(move || *rc).unwrap(); //~ ERROR E0277
    println!("{} {}", h.join().unwrap(), keep);
}
