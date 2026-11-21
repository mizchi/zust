//! A read near address 0 (a null pointer plus a field offset): the null page traps.
fn main() {
    let p = std::hint::black_box(8usize) as *const u64;
    let v = unsafe { *p };
    println!("after {v}");
}
