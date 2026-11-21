//! The other side of the `mem::zeroed` rows of unsafe_std_apis.txt: `MaybeUninit::zeroed` is safe, and stays
//! accepted outside `unsafe`.
//@ rule: the unsafe boundary (E0133)
//@ expect: run
use std::mem::MaybeUninit;
fn main() {
    let m: MaybeUninit<u64> = MaybeUninit::zeroed();
    let v = unsafe { m.assume_init() };
    println!("{v}");
}
