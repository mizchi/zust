fn n<const N: usize>() -> usize {
    N
}
fn main() {
    let _ = n::<{ true }>();
}
