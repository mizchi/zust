//! CVE-2019-1010299 (std, fixed in 1.30.0): the Debug impl of `vec_deque::Iter` printed the deque's whole buffer, so
//! formatting an iterator over an empty (or partly popped) deque showed uninitialized memory. It must show exactly
//! the elements still to come. (std prints the two halves of the ring buffer, zustc one list: the case compares the
//! numbers printed, which is what the CVE was about.)
//@ cve: CVE-2019-1010299
//@ expect: run
use std::collections::VecDeque;
fn shown<T: std::fmt::Debug>(x: T) -> String {
    let s = format!("{:?}", x);
    s.split(|c: char| !c.is_ascii_digit()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}
fn main() {
    let mut d: VecDeque<u32> = VecDeque::with_capacity(8);
    println!("[{}]", shown(d.iter()));
    for i in 100..106 {
        d.push_back(i);
    }
    for _ in 0..6 {
        d.pop_front();
    }
    println!("[{}]", shown(d.iter()));
    d.push_back(7);
    d.push_back(8);
    let mut it = d.iter();
    println!("[{}]", shown(&it));
    it.next();
    println!("[{}]", shown(&it));
}
