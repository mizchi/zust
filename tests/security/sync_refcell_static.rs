//! `RefCell` is not `Sync` either (E0277): its borrow flag is not atomic.
//@ rule: Send / Sync (a static must be Sync)
//@ expect: reject
use std::cell::RefCell;
static R: RefCell<Vec<u32>> = RefCell::new(Vec::new()); //~ ERROR E0277
fn main() {
    R.borrow_mut().push(1);
    println!("{}", R.borrow().len());
}
