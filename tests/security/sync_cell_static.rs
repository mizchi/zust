//! A `static` must be `Sync`: a `Cell` in a static could be mutated from two threads at once (E0277).
//@ rule: Send / Sync (a static must be Sync; CVE-2017-20004 was a missing Sync bound)
//@ expect: reject
use std::cell::Cell;
static C: Cell<u32> = Cell::new(0); //~ ERROR E0277
fn main() {
    C.set(1);
    println!("{}", C.get());
}
