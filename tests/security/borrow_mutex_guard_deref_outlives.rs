//! A reference through a MutexGuard that outlives the guard (E0505). After the guard is dropped the Mutex is free, so
//! a new lock may clear the Vec while the reference still points into its buffer.
//@ pattern: borrow_refcell_guard_deref_outlives (what a guard's deref gives borrows the guard)
//@ expect: reject
use std::sync::Mutex;
fn main() {
    let m = Mutex::new(vec![String::from("a")]);
    let g = m.lock().unwrap();
    let r = &g[0];
    drop(g); //~ ERROR E0505
    m.lock().unwrap().clear();
    println!("{r}");
}
