// a no_std crate that links std: `[T; N] == [T]` (`assert_eq!(a, b[..2])`), as either's tests compare
#![no_std]
extern crate std;
use std::println;
fn main() { let a = [1u8; 2]; let b = [1u8, 1, 3]; std::assert_eq!(a, b[..2]); std::assert_ne!(a, b[1..]); println!("{} {}", a == b[..2], b[..2] == a); }
