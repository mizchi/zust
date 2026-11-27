//! A closure that only reads a borrowed `Cell` is not `Send` (E0277): the `&Cell` it holds would be shared with the
//! spawned side while this thread can still set the cell. An assignment to another variable of the same name, bound
//! inside the closure, does not make the capture a `&mut` (which would need only `Cell: Send`).
//@ rule: Send / Sync
//@ expect: reject
use std::cell::Cell;
fn need_send<F: FnOnce() + Send>(f: F) {
    f()
}
fn main() {
    #[allow(unused_mut)]
    let mut c = Cell::new(1);
    need_send(|| { //~ ERROR E0277
        let seen = c.get();
        {
            let mut c = Cell::new(seen);
            c = Cell::new(c.get() + 1);
            println!("{}", c.get());
        }
    });
    c.set(3);
}
