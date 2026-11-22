// Constant evaluation with integer types (array lengths are evaluated at compile time): a value keeps its type's
// width, `!` and `<<` keep its bits, a named constant has its declared type, a cast wraps, methods count at the
// receiver's width, u128 is unsigned
type Byte = i8;
type Lane = u16;

const X: i8 = -1;
const M: u8 = !0;
const SH: u8 = 0xF0u8 << 4;
const NOT_U8: usize = ((!0u8 as u16) >> 8) as usize;
const ONES_NAMED: usize = X.count_ones() as usize;
const ONES_CAST: usize = (255u16 as Byte).count_ones() as usize;
const ONES_NEG: usize = (-1i8).count_ones() as usize;
const ZEROS: usize = 0u16.trailing_zeros() as usize;
const LEAD: usize = 1u32.leading_zeros() as usize;
const CZ: usize = 7u8.count_zeros() as usize;
const ALIAS: usize = (70000u32 as Lane) as usize;
const BIG: u128 = u128::MAX;
const BIG_HALF: usize = ((BIG >> 120) as usize) + (BIG > 0) as usize;
const M_PLUS: usize = M as usize + 1;
const NEG_SHR: usize = ((-16i32 >> 2) + 8) as usize;
const WRAP: usize = (300u32 as u8) as usize;
const POW: usize = 3u8.pow(5) as usize;
// (review of #49: a block keeps its tail's type; `-` of a literal is the type's minimum; an unsuffixed operand is the
// other's type; u128's pow is unsigned; a layout sees the constant's declared type)
const BLOCK_SHL: usize = ({ let _unused = 0; 255u8 } << 1) as usize;
const NEG_MIN: usize = (-128i8).count_ones() as usize + (-(128i8)).count_ones() as usize;
const NOT_AND: usize = (!0 & 7u8) as usize;
const POW128: usize = (2u128.pow(127) >> 127) as usize;
const LET_TYPED: usize = { let x: u8 = !0; x as usize };
const LAYOUT_N: usize = X.count_ones() as usize;
struct S {
    a: [u8; LAYOUT_N],
}

fn main() {
    println!("{}", std::mem::size_of::<[u8; M as usize]>());
    println!("{}", std::mem::size_of::<[u8; SH as usize]>());
    println!("{}", std::mem::size_of::<[u8; NOT_U8]>());
    println!("{}", std::mem::size_of::<[u8; ONES_NAMED]>());
    println!("{}", std::mem::size_of::<[u8; ONES_CAST]>());
    println!("{}", std::mem::size_of::<[u8; ONES_NEG]>());
    println!("{}", std::mem::size_of::<[u8; ZEROS]>());
    println!("{}", std::mem::size_of::<[u8; LEAD]>());
    println!("{}", std::mem::size_of::<[u8; CZ]>());
    println!("{}", std::mem::size_of::<[u8; ALIAS]>());
    println!("{}", std::mem::size_of::<[u8; BIG_HALF]>());
    println!("{}", std::mem::size_of::<[u8; M_PLUS]>());
    println!("{}", std::mem::size_of::<[u8; NEG_SHR]>());
    println!("{}", std::mem::size_of::<[u8; WRAP]>());
    println!("{}", std::mem::size_of::<[u8; POW]>());
    println!("{}", std::mem::size_of::<[u8; BLOCK_SHL]>());
    println!("{}", std::mem::size_of::<[u8; NEG_MIN]>());
    println!("{}", std::mem::size_of::<[u8; NOT_AND]>());
    println!("{}", std::mem::size_of::<[u8; POW128]>());
    println!("{}", std::mem::size_of::<[u8; LET_TYPED]>());
    println!("{}", std::mem::size_of::<S>());
    let s = S { a: [0; LAYOUT_N] };
    println!("{}", s.a.len());
}
