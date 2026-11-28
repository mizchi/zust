//! A path call of `Builder::spawn` (`Builder::spawn(builder, f)`) checks its closure as the method call does: the
//! closure comes after the builder, and moving an `Rc` into it is not `Send` (E0277).
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
use std::thread::{self, Builder};
fn main() {
    let rc = Rc::new(5);
    let keep = rc.clone();
    let h = Builder::spawn(Builder::new(), move || drop(rc)).unwrap(); //~ ERROR E0277
    h.join().unwrap();
    println!("{} {:?}", keep, thread::current().name());
}
