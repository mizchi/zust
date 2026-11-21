//! A reference through a RefCell's guard that outlives the guard (E0505). Once the guard is dropped the flag says the
//! cell is free, so a `borrow_mut` may clear the Vec while the reference still points into its buffer.
//@ rule: RefCell's run-time borrow flags
//@ expect: reject
use std::cell::RefCell;
fn main() {
    let c = RefCell::new(vec![String::from("a")]);
    let g = c.borrow();
    let r = &g[0];
    drop(g); //~ ERROR E0505
    c.borrow_mut().clear();
    println!("{r}");
}
