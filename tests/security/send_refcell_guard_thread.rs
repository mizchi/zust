//! Neighbor of send_rc_thread: a RefCell's guard is not Send (E0277). Its drop changes the cell's borrow flag, which
//! is not atomic, so a guard released on another thread races with the borrows of this one.
//@ pattern: Send / Sync
//@ expect: reject
use std::cell::{RefCell, RefMut};
fn main() {
    let cell: &'static RefCell<i32> = Box::leak(Box::new(RefCell::new(1)));
    let g: RefMut<'static, i32> = cell.borrow_mut();
    let h = std::thread::spawn(move || { //~ ERROR E0277
        println!("{}", *g);
    });
    h.join().unwrap();
    println!("{}", cell.borrow());
}
