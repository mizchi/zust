// a v128 store to an address past 4 GiB traps: narrowed to 32 bits it would land on the stack or the data (issue #30)
use std::arch::wasm32::*;

fn main() {
    let x = [0u8; 64];
    let a = x.as_ptr() as usize + (1usize << 32);
    unsafe { v128_store(std::hint::black_box(a) as *mut v128, i32x4_splat(-1)) };
    println!("after {}", x[0]);
}
