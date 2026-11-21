//! A `Box<dyn Fn()>` without `+ Send` may hold anything (an `Rc`): it cannot move to another thread (E0277).
//@ pattern: Send / Sync
//@ todo: `dyn Trait` types do not keep `+ Send`, so a `Box<dyn Fn()>` (maybe holding an Rc) moves to a thread
//@ expect: reject
use std::rc::Rc;
fn main() {
    let rc = Rc::new(5);
    let f: Box<dyn Fn() -> i32> = Box::new(move || *rc);
    let h = std::thread::spawn(move || f()); //~ ERROR E0277
    println!("{}", h.join().unwrap());
}
