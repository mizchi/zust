// `T: Into<u32>` / `U: From<u8>` bounds for primitives: std's lossless conversions; `U::from(x)` of a number
// parameter and `From::from(x)` to an expected number type
fn widen<T: Into<u32>>(x: T) -> u32 {
    x.into()
}
fn up<U: From<u8>>(x: u8) -> U {
    U::from(x)
}
fn main() {
    println!("{} {}", widen(7u8), widen(true));
    let a: u64 = up(3);
    let b: u16 = up(4);
    let c: u32 = From::from(4u16);
    let d: f64 = 2.5f32.into();
    println!("{} {} {} {} {}", a, b, c, d, i64::from(true));
}
