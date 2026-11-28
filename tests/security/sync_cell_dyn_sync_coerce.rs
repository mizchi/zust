//! A value becomes a `dyn Tr + Sync` only if its type is `Sync` (E0277): a closure that borrows a `Cell` cannot be a
//! `&(dyn Fn() + Sync)`, which other threads could then call while this one sets the cell.
//@ rule: Send / Sync
//@ expect: reject
use std::cell::Cell;
fn main() {
    let c = Cell::new(1);
    let f: &(dyn Fn() -> i32 + Sync) = &|| c.get(); //~ ERROR E0277
    c.set(2);
    println!("{}", f());
}
