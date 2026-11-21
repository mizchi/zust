// a v128 load from the null page traps, as a scalar one does (issue #30)
use std::arch::wasm32::*;

fn main() {
    let p = std::hint::black_box(32usize) as *const v128;
    let v = unsafe { v128_load(p) };
    println!("after {}", i32x4_extract_lane::<0>(v));
}
