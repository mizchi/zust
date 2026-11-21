//! A RefCell's guard that outlives the cell (E0597). `Ref` holds raw pointers to the value and to the borrow flag, so
//! the borrow checker must count it as a loan of the cell: otherwise the read after the block uses freed memory, and
//! the guard's drop writes to the freed flag.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
use std::cell::RefCell;
fn main() {
    let g = {
        let c = RefCell::new(vec![String::from("short")]);
        c.borrow() //~ ERROR E0597
    };
    println!("{}", g[0]);
}
