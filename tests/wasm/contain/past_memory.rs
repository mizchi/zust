//! A write past the end of linear memory (the memory never grows past 2 GiB).
fn main() {
    let p = std::hint::black_box(0xFFFF_FF00usize) as *mut u64;
    unsafe { *p = 1 };
    println!("after");
}
