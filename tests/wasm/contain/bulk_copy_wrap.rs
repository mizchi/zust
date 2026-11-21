//! A bulk copy to an address past 4 GiB: narrowed it would land on the stack. memcpy checks its operands as a store
//! does, and traps.
fn main() {
    let src = [7u8; 16];
    let dst = std::hint::black_box((1usize << 32) + 4096 + 64) as *mut u8;
    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), dst, 16) };
    println!("after");
}
