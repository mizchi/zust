fn widen<T: Into<u32>>(x: T) -> u32 {
    x.into()
}
fn main() {
    let _ = widen(7u64);
}
