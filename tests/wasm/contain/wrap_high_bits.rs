//! An address past 4 GiB: narrowed to 32 bits it would land on the stack (address 64). It must trap instead.
fn main() {
    let p = (1usize << 32) + 4096 + 64;
    unsafe { *(std::hint::black_box(p) as *mut u64) = 0xDEAD };
    println!("after");
}
