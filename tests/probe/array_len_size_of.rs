// Array lengths that call size_of / align_of on primitives and type aliases of them (libc 0.2's
// `[u8; 128 - 2 * size_of::<c_int>()]`, `[c_ulong; 128 / size_of::<c_ulong>()]`).
#![allow(non_camel_case_types)]
use std::mem::{align_of, size_of};

type c_int = i32;
type c_long = i64;
type c_ulong = u64;

const FD_SETSIZE: usize = 1024;

// (a const fn call is still an array length, beside size_of: tests/ui consts/issue-29927.rs)
const fn five() -> usize {
    5
}

struct SockStorage {
    family: u16,
    pad: [u8; 128 - 2 * size_of::<c_int>() - size_of::<c_long>()],
    align: [c_long; 0],
}

struct FdSet {
    bits: [c_ulong; FD_SETSIZE / 8 / core::mem::size_of::<c_ulong>()],
}

fn main() {
    let s = SockStorage { family: 1, pad: [0; 128 - 2 * size_of::<c_int>() - size_of::<c_long>()], align: [] };
    let f = FdSet { bits: [0; FD_SETSIZE / 8 / std::mem::size_of::<c_ulong>()] };
    println!("{} {} {}", s.family, s.pad.len(), s.align.len());
    println!("{}", f.bits.len());
    let p: [u8; size_of::<*const u8>() + align_of::<u16>()] = [7; 10];
    println!("{} {}", p.len(), p[9]);
    let q = [0u8; five()];
    println!("{}", q.len());
}
