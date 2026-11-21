//! Moving a RefCell (here into `drop`) while a guard of it is alive (E0505): the guard would point into freed memory.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
use std::cell::RefCell;
fn main() {
    let c = RefCell::new(vec![String::from("a")]);
    let g = c.borrow_mut();
    drop(c); //~ ERROR E0505
    println!("{}", g[0]);
}
