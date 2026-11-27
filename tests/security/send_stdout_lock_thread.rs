//! `StdoutLock` is not `Send` (E0277): it holds the stdout lock for the thread that took it, and a guard moved to
//! another thread would let go a lock that thread does not own.
//@ rule: Send / Sync
//@ expect: reject
use std::io::Write;
fn main() {
    let out = std::io::stdout();
    let lock = out.lock();
    let h = std::thread::spawn(move || { //~ ERROR E0277
        let mut l = lock;
        writeln!(l, "x").unwrap();
    });
    h.join().unwrap();
}
