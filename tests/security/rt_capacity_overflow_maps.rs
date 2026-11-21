//! Same pattern as CVE-2018-1000810 (a size computed without overflow checks) in the hash collections:
//! `with_capacity` of a count whose table size does not fit must panic ("capacity overflow"), not wrap to a small
//! table or loop forever.
//@ pattern: capacity arithmetic
//@ expect: panic
//@ args: 0 1 2
use std::collections::{HashMap, HashSet};
fn main() {
    let k: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    let huge = usize::MAX / 4 + (k & 0);
    match k {
        0 => println!("{}", HashMap::<u64, u64>::with_capacity(huge).len()),
        1 => println!("{}", HashSet::<u64>::with_capacity(huge).len()),
        _ => println!("{}", HashMap::<u8, u8>::with_capacity(usize::MAX - (k & 0)).len()),
    }
    println!("not reached");
}
