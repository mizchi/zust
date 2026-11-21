//! CVE-2018-1000657 (std, fixed in 1.22.0): `VecDeque::reserve` had a buffer overflow when it grew a deque whose
//! ring buffer had wrapped. Every growth path (reserve, and the pushes at both ends) of a wrapped deque must keep
//! the elements, in order.
//@ cve: CVE-2018-1000657
//@ expect: run
use std::collections::VecDeque;
fn wrapped(cap_hint: usize, n: usize) -> VecDeque<String> {
    let mut d = VecDeque::with_capacity(cap_hint);
    for i in 0..n {
        d.push_back(format!("e{i}"));
    }
    // move the head round: pop some from the front, push as many at the back
    for i in 0..n / 2 {
        d.pop_front();
        d.push_back(format!("w{i}"));
    }
    d
}
fn show(d: &VecDeque<String>) -> String {
    d.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")
}
fn main() {
    for (cap, n) in [(4, 4), (8, 7), (16, 13), (5, 5)] {
        let mut d = wrapped(cap, n);
        println!("{}", show(&d));
        d.reserve(100);
        println!("{} {}", show(&d), d.capacity() >= d.len() + 100);
        for i in 0..100 {
            d.push_back(format!("g{i}"));
        }
        println!("{}", show(&d));
        let mut d = wrapped(cap, n);
        for i in 0..20 {
            d.push_front(format!("f{i}"));
        }
        println!("{}", show(&d));
        let mut d = wrapped(cap, n);
        let b = d.pop_back();
        d.push_front(format!("{:?}", b));
        println!("{} {:?} {:?} {:?}", show(&d), d.front(), d.back(), d.get(1));
        let mut d = wrapped(cap, n);
        d.truncate(2);
        d.push_back("x".to_string());
        d.push_back("y".to_string());
        println!("{}", show(&d));
    }
}
