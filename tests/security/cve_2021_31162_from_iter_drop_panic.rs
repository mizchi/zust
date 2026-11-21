//! CVE-2021-31162 (std, fixed in 1.52.0): the in-place specialization of `Vec::from_iter` dropped an element twice
//! when dropping one of them panicked. zustc does not unwind: the panic ends the program, and up to it every
//! element must be dropped once (valgrind: no double free).
//@ cve: CVE-2021-31162
//@ expect: panic
use std::cell::Cell;
thread_local! {
    static DROPS: Cell<u32> = Cell::new(0);
}
struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        let n = DROPS.with(|d| {
            d.set(d.get() + 1);
            d.get()
        });
        if self.0 == 3 {
            panic!("drop panics after {n} drops");
        }
    }
}
fn main() {
    let v: Vec<D> = (0..8).map(D).collect();
    let w: Vec<D> = v.into_iter().filter(|d| d.0 % 2 == 0).collect();
    println!("{}", w.len());
}
