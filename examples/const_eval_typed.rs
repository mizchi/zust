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
// (a static of an integer constant or expression is its type's: 15, not the low byte of an untyped -1 >> 4; a u128
// local past i128::MAX and i128::MIN substitute as their values)
const M2: u8 = !0 >> 4;
static SB: u8 = M2;
static SC: u8 = !0 >> 4;
const U128_SHR: usize = {
    let x: u128 = u128::MAX;
    (x >> 127) as usize
};
// (third review of #49)
const SHADOW: usize = {
    let x = 1u8;
    {
        let x = 255u8;
        x.count_ones() as usize
    }
};
const LET_INFER: u8 = {
    let x = !0 >> 4;
    x
};
const BYTE_LZ: usize = b'\x01'.leading_zeros() as usize;
const IF_COMMON: usize = (((if true { !0 } else { 1u8 }) >> 4) as u8) as usize;
const ARR: [u8; 1] = [!0];
const TUP: (u8,) = (!0,);
const PROJ: usize = ARR[0].count_ones() as usize + TUP.0.count_ones() as usize;
const PEER: usize = ((4 / ((!0 >> 7) + 1)) & 7u8) as usize;
const SHORT: usize = if true || (u8::MAX + 1 == 0) { 1 } else { 2 };
const POW_U32: usize = 2u8.pow(!0 >> 31) as usize;
const ABS: usize = (-127i8).abs() as usize;
const REM: usize = (i8::MIN % -2i8) as usize;
const I128_MIN_OK: usize = {
    let m = i128::MIN;
    (m == i128::MIN) as usize + 1
};

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
    println!("{} {}", SB, SC);
    println!("{}", std::mem::size_of::<[u8; U128_SHR]>());
    println!("{}", std::mem::size_of::<[u8; I128_MIN_OK]>());
    println!("{}", std::mem::size_of::<[u8; SHADOW]>());
    println!("{}", LET_INFER);
    println!("{}", std::mem::size_of::<[u8; LET_INFER as usize]>());
    println!("{}", std::mem::size_of::<[u8; BYTE_LZ]>());
    println!("{}", std::mem::size_of::<[u8; IF_COMMON]>());
    println!("{}", std::mem::size_of::<[u8; PROJ]>());
    println!("{}", std::mem::size_of::<[u8; PEER]>());
    println!("{}", std::mem::size_of::<[u8; SHORT]>());
    println!("{}", std::mem::size_of::<[u8; POW_U32]>());
    println!("{}", std::mem::size_of::<[u8; ABS]>());
    println!("{} {}", REM, ARR[0]);
    println!("{} {}", std::mem::size_of::<G>(), US);
    println!("{}", std::mem::size_of::<[u8; I128_LIT_MIN]>());
    println!("{} {} {}", std::mem::size_of::<[u8; REF_IDX]>(), std::mem::size_of::<[u8; USER_BITS]>(), EXP);
    println!("{}", FLIP);
    println!("{}", std::mem::size_of::<PL>());
    println!("{} {} {}", FLIP2, WIDE, std::mem::size_of::<[u8; ADD_U16]>());
    println!("{}", DIV_LATE);
}

// (a generic alias resolves before layout; an unsafe block evaluates; the i128::MIN literal negates)
type Id<T> = T;
const GX: Id<i8> = -1;
const GEN_N: usize = GX.count_ones() as usize;
struct G {
    a: [u8; GEN_N],
}
const UNSAFE_C: u8 = unsafe { 7 };
static US: u8 = UNSAFE_C;
const I128_LIT_MIN: usize = (-170141183460469231731687303715884105728i128).count_ones() as usize;
// (through a reference; a user's `BITS`; untyped lets that double; a const fn's result as its use's type)
const R: &[u8; 1] = &[!0];
const REF_IDX: usize = R[0].count_ones() as usize;
struct SB2;
impl SB2 {
    const BITS: u8 = 7;
}
const USER_BITS: usize = ((4 / ((!0 >> 7) + 1)) & SB2::BITS) as usize;
const EXP: usize = { let x0 = 0; let x1 = x0 + x0; let x2 = x1 + x1; let x3 = x2 + x2; let x4 = x3 + x3; let x5 = x4 + x4; let x6 = x5 + x5; let x7 = x6 + x6; let x8 = x7 + x7; let x9 = x8 + x8; let x10 = x9 + x9; let x11 = x10 + x10; let x12 = x11 + x11; let x13 = x12 + x12; let x14 = x13 + x13; let x15 = x14 + x14; let x16 = x15 + x15; let x17 = x16 + x16; let x18 = x17 + x17; let x19 = x18 + x18; let x20 = x19 + x19; let x21 = x20 + x20; let x22 = x21 + x21; let x23 = x22 + x22; let x24 = x23 + x23; let x25 = x24 + x24; x25 };
const fn flip(x: u8) -> u8 {
    !x
}
const FLIP: u8 = flip(255) + 1;
// (an element or field of an aggregate constant in a layout, before the constants' types are resolved)
const PARR: [u8; 1] = [!0];
const PTUP: (u8,) = (!0,);
const PL_N: usize = PARR[0].count_ones() as usize + PTUP.0.count_ones() as usize;
struct PL {
    bytes: [u8; PL_N],
}
// (a const fn's parameters and result are its signature's types; a long untyped chain keeps the type of its use)
const fn flip2(x: u8) -> u8 {
    !x >> 4
}
const FLIP2: u8 = flip2(255) + 1;
const WIDE: u8 = { let y0 = !0 >> 1; let y1 = y0 & y0; let y2 = y1 & y1; let y3 = y2 & y2; let y4 = y3 & y3; let y5 = y4 & y4; let y6 = y5 & y5; let y7 = y6 & y6; let y8 = y7 & y7; let y9 = y8 & y8; let y10 = y9 & y9; let y11 = y10 & y10; let y12 = y11 & y11; let y13 = y12 & y12; let y14 = y13 & y13; let y15 = y14 & y14; let y16 = y15 & y15; let y17 = y16 & y16; let y18 = y17 & y17; let y19 = y18 & y18; let y20 = y19 & y19; let y21 = y20 & y20; let y22 = y21 & y21; let y23 = y22 & y22; let y24 = y23 & y23; let y25 = y24 & y24; let y26 = y25 & y25; let y27 = y26 & y26; let y28 = y27 & y27; let y29 = y28 & y28; let y30 = y29 & y29; let y31 = y30 & y30; let y32 = y31 & y31; let y33 = y32 & y32; let y34 = y33 & y33; let y35 = y34 & y34; let y36 = y35 & y35; let y37 = y36 & y36; let y38 = y37 & y37; let y39 = y38 & y38; y39 };
const fn add16(a: u16, b: u16) -> u16 {
    a + b
}
const ADD_U16: usize = (add16(65535, 0) >> 8) as usize;
// (an untyped local whose untyped value fails (a division by 0) but whose use's type succeeds)
const DIV_LATE: u8 = {
    let x = 1 / ((!0 >> 7) + 1);
    x
};
