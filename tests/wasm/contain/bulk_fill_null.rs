//! A bulk fill of the null page traps, as a store there does.
fn main() {
    let p = std::hint::black_box(8usize) as *mut u8;
    unsafe { std::ptr::write_bytes(p, 0, 16) };
    println!("after");
}
