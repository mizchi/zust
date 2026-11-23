// Literals at their types' bounds (and casts, patterns, an allow) are not out of range
const N: usize = 4;
fn takes(x: u8) -> u8 { x }
fn main() {
    let a = 255 as u8;
    let b: u8 = (300 - 100) as u8;
    let c: u64 = 1 << 40;
    let d: i8 = -128;
    let e = -128i8;
    let f: u32 = 0xFFFF_FFFF;
    let g: i32 = 0xFFFF_FFFF_u32 as i32;
    let h: u8 = b'a';
    let x: u8 = 255;
    let m = match x { 255 => 1, 0 => 2, _ => 3 };
    let i = 9223372036854775807i64;
    let j: i64 = -9223372036854775808;
    let k: u128 = 340282366920938463463374607431768211455;
    let l: i128 = -170141183460469231731687303715884105728;
    let arr = [0u8; N];
    let s = takes(200);
    let mask = !0u8;
    let big: usize = 18446744073709551615;
    let t: f64 = 1e300;
    println!("{a} {b} {c} {d} {e} {f} {g} {h} {m} {i} {j} {k} {l} {} {s} {mask} {big} {t}", arr.len());
}
