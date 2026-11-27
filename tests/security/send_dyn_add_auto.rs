//! A `Box<dyn Tr>` is not a `Box<dyn Tr + Send>` (E0308): an object may drop auto traits in a coercion, never gain
//! them, as the value behind it may not have them (an `Rc` inside).
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
fn main() {
    let rc = Rc::new(5);
    let f: Box<dyn Fn() -> i32> = Box::new(move || *rc);
    let g: Box<dyn Fn() -> i32 + Send> = f; //~ ERROR E0308
    let h = std::thread::spawn(move || g());
    println!("{}", h.join().unwrap());
}
