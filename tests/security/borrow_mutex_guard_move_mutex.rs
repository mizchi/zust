//! Moving a Mutex while its guard is alive (E0505). `lock()` returns `Result<MutexGuard<T>, _>`: the guard inside the
//! Result must keep the Mutex's loan, as an `Option<Iter<T>>` keeps its Vec's.
//@ pattern: borrow_refcell_guard_move_cell (the same guard, from a Mutex)
//@ expect: reject
use std::sync::Mutex;
fn main() {
    let m = Mutex::new(vec![String::from("a")]);
    let g = m.lock().unwrap();
    drop(m); //~ ERROR E0505
    println!("{}", g[0]);
}
