//! CVE-2015-20001 (std, fixed in 1.2.0): BinaryHeap was not panic safe. A comparison that panicked inside
//! `sift_up` / `sift_down_range` left the heap holding a zeroed or duplicated element, which was later dropped.
//! Here the comparison panics in the middle of a push: the panic must end the program (exit 101) with every
//! element still owned once (valgrind: no double free, no use of freed memory).
//@ cve: CVE-2015-20001
//@ expect: panic
use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
thread_local! {
    static CMPS: Cell<u32> = Cell::new(0);
}
#[derive(PartialEq, Eq)]
struct P(String);
impl PartialOrd for P {
    fn partial_cmp(&self, o: &P) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for P {
    fn cmp(&self, o: &P) -> Ordering {
        let n = CMPS.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        if n == 40 {
            panic!("comparison panics");
        }
        self.0.cmp(&o.0)
    }
}
fn main() {
    let mut h = BinaryHeap::new();
    for i in 0..30 {
        h.push(P(format!("{:03}", (i * 17) % 30)));
    }
    println!("pushed {}", h.len());
}
