//! CVE-2017-20004 (std, fixed in 1.19.0): `MutexGuard<T>` was Sync for any T, so a `MutexGuard<Cell<i32>>` could be
//! shared between threads and the Cell mutated from two of them at once. A guard is Sync only if T is.
//@ cve: CVE-2017-20004
//@ expect: reject
use std::cell::Cell;
use std::sync::MutexGuard;
fn is_sync<T: Sync>() {}
fn main() {
    is_sync::<MutexGuard<'static, Cell<i32>>>(); //~ ERROR E0277
}
