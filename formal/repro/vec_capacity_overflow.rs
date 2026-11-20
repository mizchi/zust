// rustc and zustc: panics with "capacity overflow" (exit 101).
// Before 2026-11-20 zustc's `Vec::reserve` computed `c * size_of::<T>()` without an overflow check: 2^61 * 8
// wrapped to 0, the block was tiny while `cap` said 2^61, and the pushes wrote past it (a segmentation fault).
// Found by formal/kani-prelude; tests/alloc_limits.rs keeps it fixed.
fn main() {
    let n: usize = std::env::args().count() << 61;
    let mut v: Vec<u64> = Vec::with_capacity(n);
    for i in 0..100000 {
        v.push(i);
    }
    println!("len {}", v.len());
}
