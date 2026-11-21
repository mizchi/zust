//! `RefCell` checks its borrows at run time: a second `borrow_mut` (or a `borrow` while mutably
//! borrowed) must panic, or two `&mut` to one value exist in safe code. (zustc's RefCell handed out plain
//! references with no borrow flag until its `Ref` / `RefMut` guards.)
//@ rule: RefCell's run-time borrow flags
//@ expect: panic
//@ args: 0 1 2
use std::cell::RefCell;
fn main() {
    let k: u32 = std::env::args().nth(1).unwrap().parse().unwrap();
    let c = RefCell::new(vec![1]);
    match k {
        0 => {
            let _a = c.borrow_mut();
            let _b = c.borrow_mut();
        }
        1 => {
            let _a = c.borrow_mut();
            let _b = c.borrow();
        }
        _ => {
            let _a = c.borrow();
            let _b = c.borrow_mut();
        }
    }
    println!("not reached");
}
