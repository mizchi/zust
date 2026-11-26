// i128 / u128: wrapping_div / wrapping_rem / overflowing_div / overflowing_rem (MIN / -1 included), isqrt and reverse_bits
// (#187: these were an internal error), with isqrt, reverse_bits and ilog10 on narrower types beside them
fn main() {
    let a: i128 = -7;
    let b: i128 = 2;
    println!("{} {}", a.wrapping_div(b), a.wrapping_rem(b));
    let c: u128 = 10;
    println!("{:?} {}", c.overflowing_div(3), c.isqrt());
    println!("{:?} {:?}", a.overflowing_div(b), a.overflowing_rem(b));
    println!("{:?} {:?}", i128::MIN.overflowing_div(-1), i128::MIN.overflowing_rem(-1));
    println!("{} {}", i128::MIN.wrapping_div(-1), i128::MIN.wrapping_rem(-1));
    println!("{} {}", u128::MAX.wrapping_div(7), u128::MAX.wrapping_rem(7));
    println!("{:?}", u128::MAX.overflowing_rem(1u128 << 100));
    println!("{} {} {}", u128::MAX.isqrt(), i128::MAX.isqrt(), (1u128 << 64).isqrt());
    println!("{} {}", 0u128.isqrt(), 99i128.isqrt());
    println!("{} {}", c.reverse_bits(), (0x0123_4567_89ab_cdef_u128 << 3).reverse_bits());
    println!("{}", (-2i128).reverse_bits());
    println!("{} {} {}", 17u8.isqrt(), 1_000_000i64.isqrt(), 255u8.reverse_bits());
    let x: i32 = 50;
    println!("{}", x.isqrt());
    // (ilog10 divides a u128 in the prelude: on a u32 it was "`__exr_udiv128` is missing from the prelude")
    let y: u32 = 12345;
    println!("{} {}", y.ilog10(), 7u8.ilog10());
}
