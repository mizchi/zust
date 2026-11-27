//! A closure that only reads a borrowed `Cell` is not `Send` (E0277), also where another variable of the same name is
//! bound and assigned in a `let .. else` block: that assignment is not to the capture, which stays a shared borrow.
//@ rule: Send / Sync
//@ expect: reject
use std::cell::Cell;
fn need_send<F: FnOnce() + Send>(f: F) {
    f()
}
fn main() {
    #[allow(unused_mut)]
    let mut c = Cell::new(0);
    need_send(|| { //~ ERROR E0277
        let Some(_) = Some(0) else {
            let mut c = 0;
            c += 1;
            println!("{}", c);
            return;
        };
        c.set(1);
    });
    c.set(2);
}
