//! CVE-2018-1000810 (`str::repeat`) and the size computations of every growable buffer: a length
//! times an element size that overflows `usize` must panic ("capacity overflow"), not wrap to a
//! small allocation that is then written past its end.
//@ pattern: capacity arithmetic (the class of CVE-2018-1000810: a size computed with a wrapping multiply)
//@ expect: panic
//@ args: 0 1 2 3 4 5 6 7 8
use std::collections::VecDeque;
fn arg() -> usize {
    std::env::args().nth(1).unwrap().parse().unwrap()
}
fn main() {
    let k = arg();
    let huge = usize::MAX / 2 + 1 + (k & 0);
    match k {
        0 => println!("{}", "ab".repeat(huge).len()),
        1 => println!("{}", String::from("abc").repeat(huge / 2).len()),
        2 => println!("{}", Vec::<u64>::with_capacity(usize::MAX / 4 + 1).capacity()),
        3 => {
            let mut v = vec![1u8];
            v.reserve(usize::MAX);
            println!("{}", v.capacity());
        }
        4 => println!("{}", String::with_capacity(usize::MAX - (k & 0)).capacity()),
        5 => println!("{}", vec![0u32; huge].len()),
        6 => println!("{}", VecDeque::<u64>::with_capacity(usize::MAX / 4 + 1).len()),
        7 => {
            let mut d: VecDeque<u64> = VecDeque::new();
            d.push_back(1);
            let mut e: VecDeque<u64> = VecDeque::with_capacity(usize::MAX / 8 + 1);
            d.append(&mut e);
            println!("{}", d.len());
        }
        _ => {
            let mut s = String::from("x");
            s.push_str(&String::with_capacity(usize::MAX / 2 + 1));
            println!("{}", s.len());
        }
    }
    println!("not reached");
}
