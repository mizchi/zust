//! CVE-2021-31162 (std, fixed in 1.52.0): the in-place specialization of `Vec::from_iter` dropped an element twice
//! when dropping one of them panicked. zustc does not unwind: the panic ends the program, and up to it every
//! element must be dropped once (a second drop prints a marker, and frees its String twice).
//@ cve: CVE-2021-31162
//@ expect: panic
use std::cell::RefCell;
thread_local! {
    static DROPPED: RefCell<Vec<u32>> = RefCell::new(Vec::new());
}
// (each element owns a String, so a second drop is also a double free that valgrind sees; a repeated id prints a
// marker on stdout, which the oracle compares)
struct D(u32, String);
impl Drop for D {
    fn drop(&mut self) {
        let again = DROPPED.with(|d| {
            let mut d = d.borrow_mut();
            let again = d.contains(&self.0);
            d.push(self.0);
            again
        });
        if again {
            println!("dropped {} twice", self.0);
        }
        if self.0 == 3 {
            panic!("drop of {} panics", self.1);
        }
    }
}
fn main() {
    let v: Vec<D> = (0..8).map(|i| D(i, format!("d{i}"))).collect();
    let w: Vec<D> = v.into_iter().filter(|d| d.0 % 2 == 0).collect();
    println!("{}", w.len());
}
