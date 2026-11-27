//! A closure that borrows a `MutexGuard` is not `Send` (E0277): the guard must be let go on the thread that locked
//! the Mutex. `*g += 1` borrows it mutably, and a `&mut MutexGuard` is Send only if the guard is; it is Sync, not
//! Send (before, zustc took every capture held by reference as a `&T`, which is Send when the guard is Sync).
//@ rule: Send / Sync
//@ expect: reject
use std::sync::Mutex;
fn need_send<F: FnOnce() + Send>(f: F) {
    f()
}
fn main() {
    let m = Mutex::new(0);
    let mut g = m.lock().unwrap();
    need_send(|| { //~ ERROR E0277
        *g += 1;
    });
    drop(g);
    println!("{}", m.lock().unwrap());
}
