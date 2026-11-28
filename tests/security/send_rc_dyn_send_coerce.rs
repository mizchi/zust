//! A value becomes a `dyn Tr + Send` only if its type is `Send` (E0277): a closure that holds an `Rc` cannot be a
//! `Box<dyn Fn() + Send>`, which a thread would then take with the `Rc` (its count updated by two threads at once).
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
fn main() {
    let rc = Rc::new(5);
    let f: Box<dyn Fn() -> i32 + Send> = Box::new(move || *rc); //~ ERROR E0277
    let h = std::thread::spawn(move || f());
    println!("{}", h.join().unwrap());
}
