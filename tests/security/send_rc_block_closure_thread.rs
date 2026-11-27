//! A spawned closure given as a block (`spawn({ move || .. })`) or an `if` is checked by its type, as one written in
//! the call or held in a local: an `Rc` moved into it is not `Send` (E0277). Before, only a closure written in the call
//! or named by a local was checked, so a block let the `Rc` reach the other thread.
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
use std::thread;
fn main() {
    let rc = Rc::new(5);
    let keep = rc.clone();
    let h = thread::spawn({ move || drop(rc) }); //~ ERROR E0277
    h.join().unwrap();
    println!("{}", keep);
}
