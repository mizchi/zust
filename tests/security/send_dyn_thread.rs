//! A `Box<dyn Fn()>` without `+ Send` may hold anything (an `Rc`): it cannot move to another thread (E0277).
//@ pattern: Send / Sync
//@ expect: reject
use std::rc::Rc;
fn main() {
    let rc = Rc::new(5);
    let f: Box<dyn Fn() -> i32> = Box::new(move || *rc);
    let h = std::thread::spawn(move || f()); //~ ERROR E0277
    println!("{}", h.join().unwrap());
}
