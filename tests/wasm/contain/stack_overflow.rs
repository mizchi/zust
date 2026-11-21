//! Unbounded recursion with large frames: the shadow stack (or the engine's stack) runs out and the run traps;
//! the stack never grows into the data below it.
fn deep(n: u64) -> u64 {
    let buf = [n; 512];
    std::hint::black_box(&buf);
    deep(n + 1) + buf[3]
}
fn main() {
    println!("after {}", deep(0));
}
