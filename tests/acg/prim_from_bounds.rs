// `T: Into<u32>` / `From<u8>` bounds for primitives: std's lossless conversions
fn widen<T: Into<u32>>(x: T) -> u32 { x.into() }
fn main() {
    println!("{} {}", widen(7u8), widen(true));
}
