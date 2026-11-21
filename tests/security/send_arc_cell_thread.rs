//! Neighbor of send_rc_thread: `Arc<Cell<_>>` is not Send (Cell is not Sync), so two threads cannot share it (E0277).
//@ pattern: Send / Sync
//@ expect: reject
use std::cell::Cell;
use std::sync::Arc;
fn main() {
    let a = Arc::new(Cell::new(0));
    let b = a.clone();
    let h = std::thread::spawn(move || { //~ ERROR E0277
        drop(b);
    });
    h.join().unwrap();
    println!("{}", std::sync::Arc::strong_count(&a));
}
