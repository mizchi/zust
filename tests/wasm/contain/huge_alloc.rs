//! An allocation larger than the sandbox's memory: the allocator returns null and the program aborts.
fn main() {
    let n = std::hint::black_box(3usize << 30);
    let v: Vec<u8> = Vec::with_capacity(n);
    println!("after {}", v.capacity());
}
