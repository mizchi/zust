// zustc: segmentation fault (rustc: panics with "capacity overflow").
// `Vec::reserve` computes `c * size_of::<T>()` without an overflow check: 2^61 * 8 wraps to 0, so the block is
// tiny while `cap` says 2^61, and the pushes write past it. Found by formal/kani-prelude (bug_with_capacity_overflow).
fn main() {
    let n: usize = std::env::args().count() << 61;
    let mut v: Vec<u64> = Vec::with_capacity(n);
    for i in 0..100000 {
        v.push(i);
    }
    println!("len {}", v.len());
}
