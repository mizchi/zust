//! rust-lang/rust#10184 ("floating point to integer casts can cause undefined behaviour", I-unsound, closed by
//! rust-lang/rust#71269): `f as i32` on NaN or an out-of-range float was LLVM poison, so safe code could produce an
//! undefined integer. Casts now saturate (NaN is 0).
//@ issue: rust-lang/rust#10184
//@ expect: run
fn main() {
    let args = std::env::args().count() as f64; // 1.0 at run time
    let nan = (args - 1.0) / (args - 1.0);
    let big = 1e20 * args;
    let inf = f64::INFINITY * args;
    println!("{} {} {}", nan as i32, nan as u8, nan as i64);
    println!("{} {} {}", big as i32, -big as i32, big as u8);
    println!("{} {} {}", -big as u8, inf as u64, -inf as i64);
    println!("{} {}", (300.7 * args) as u8 as u32, (-1.5 * args) as u32);
    let f = 3e9f32 * args as f32;
    println!("{} {} {}", f as i32, f as u32, (f32::NAN * args as f32) as i16);
}
