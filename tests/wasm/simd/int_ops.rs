// core::arch::wasm32: integer lanes (issue #30): arithmetic, saturation, compares, shifts, narrowing and widening
use std::arch::wasm32::*;

fn h(name: &str, v: v128) {
    println!(
        "{name}: {:08x} {:08x} {:08x} {:08x}",
        u32x4_extract_lane::<0>(v),
        u32x4_extract_lane::<1>(v),
        u32x4_extract_lane::<2>(v),
        u32x4_extract_lane::<3>(v)
    );
}

fn main() {
    let a8 = i8x16(1, -2, 3, -4, 127, -128, 100, -100, 0, 50, -50, 77, -1, 64, 32, 16);
    let b8 = i8x16(1, 2, -3, -4, 1, -1, 100, -100, 0, -50, 120, 77, -128, 64, -32, 16);
    let a16 = i16x8(1, -2, 32767, -32768, 1000, -1000, 255, -256);
    let b16 = i16x8(3, 2, 1, -1, 1000, 30000, -255, 256);
    let a32 = i32x4(1, -2, i32::MAX, i32::MIN);
    let b32 = i32x4(3, 7, 1, -1);
    let a64 = i64x2(i64::MAX, -5);
    let b64 = i64x2(1, -7);

    for (n, a, b) in [("8", a8, b8), ("8r", b8, a8)] {
        h(&format!("i8 add {n}"), i8x16_add(a, b));
        h(&format!("i8 sub {n}"), i8x16_sub(a, b));
        h(&format!("i8 add_sat {n}"), i8x16_add_sat(a, b));
        h(&format!("u8 add_sat {n}"), u8x16_add_sat(a, b));
        h(&format!("i8 sub_sat {n}"), i8x16_sub_sat(a, b));
        h(&format!("u8 sub_sat {n}"), u8x16_sub_sat(a, b));
        h(&format!("i8 min {n}"), i8x16_min(a, b));
        h(&format!("u8 min {n}"), u8x16_min(a, b));
        h(&format!("i8 max {n}"), i8x16_max(a, b));
        h(&format!("u8 max {n}"), u8x16_max(a, b));
        h(&format!("u8 avgr {n}"), u8x16_avgr(a, b));
        h(&format!("i8 eq {n}"), i8x16_eq(a, b));
        h(&format!("i8 ne {n}"), i8x16_ne(a, b));
        h(&format!("i8 lt {n}"), i8x16_lt(a, b));
        h(&format!("u8 lt {n}"), u8x16_lt(a, b));
        h(&format!("i8 gt {n}"), i8x16_gt(a, b));
        h(&format!("u8 gt {n}"), u8x16_gt(a, b));
        h(&format!("i8 le {n}"), i8x16_le(a, b));
        h(&format!("u8 le {n}"), u8x16_le(a, b));
        h(&format!("i8 ge {n}"), i8x16_ge(a, b));
        h(&format!("u8 ge {n}"), u8x16_ge(a, b));
    }
    h("u8 add", u8x16_add(a8, b8));
    h("u8 sub", u8x16_sub(a8, b8));
    h("u8 eq", u8x16_eq(a8, b8));
    h("u8 ne", u8x16_ne(a8, b8));
    h("i8 abs", i8x16_abs(a8));
    h("i8 neg", i8x16_neg(a8));
    h("i8 popcnt", i8x16_popcnt(a8));
    h("u8 popcnt", u8x16_popcnt(b8));
    for s in [0u32, 1, 3, 7, 8, 9, 33] {
        h(&format!("i8 shl {s}"), i8x16_shl(a8, s));
        h(&format!("u8 shl {s}"), u8x16_shl(a8, s));
        h(&format!("i8 shr {s}"), i8x16_shr(a8, s));
        h(&format!("u8 shr {s}"), u8x16_shr(a8, s));
    }
    h("i8 narrow", i8x16_narrow_i16x8(a16, b16));
    h("u8 narrow", u8x16_narrow_i16x8(a16, b16));

    h("i16 add", i16x8_add(a16, b16));
    h("u16 add", u16x8_add(a16, b16));
    h("i16 sub", i16x8_sub(a16, b16));
    h("u16 sub", u16x8_sub(a16, b16));
    h("i16 mul", i16x8_mul(a16, b16));
    h("u16 mul", u16x8_mul(a16, b16));
    h("i16 add_sat", i16x8_add_sat(a16, b16));
    h("u16 add_sat", u16x8_add_sat(a16, b16));
    h("i16 sub_sat", i16x8_sub_sat(a16, b16));
    h("u16 sub_sat", u16x8_sub_sat(a16, b16));
    h("i16 min", i16x8_min(a16, b16));
    h("u16 min", u16x8_min(a16, b16));
    h("i16 max", i16x8_max(a16, b16));
    h("u16 max", u16x8_max(a16, b16));
    h("u16 avgr", u16x8_avgr(a16, b16));
    h("i16 abs", i16x8_abs(a16));
    h("i16 neg", i16x8_neg(a16));
    h("i16 q15mulr_sat", i16x8_q15mulr_sat(a16, b16));
    h("i16 eq", i16x8_eq(a16, b16));
    h("u16 eq", u16x8_eq(a16, b16));
    h("i16 ne", i16x8_ne(a16, b16));
    h("u16 ne", u16x8_ne(a16, b16));
    h("i16 lt", i16x8_lt(a16, b16));
    h("u16 lt", u16x8_lt(a16, b16));
    h("i16 gt", i16x8_gt(a16, b16));
    h("u16 gt", u16x8_gt(a16, b16));
    h("i16 le", i16x8_le(a16, b16));
    h("u16 le", u16x8_le(a16, b16));
    h("i16 ge", i16x8_ge(a16, b16));
    h("u16 ge", u16x8_ge(a16, b16));
    for s in [0u32, 1, 15, 16, 17] {
        h(&format!("i16 shl {s}"), i16x8_shl(a16, s));
        h(&format!("u16 shl {s}"), u16x8_shl(a16, s));
        h(&format!("i16 shr {s}"), i16x8_shr(a16, s));
        h(&format!("u16 shr {s}"), u16x8_shr(a16, s));
    }
    h("i16 narrow", i16x8_narrow_i32x4(a32, b32));
    h("u16 narrow", u16x8_narrow_i32x4(a32, b32));
    h("i16 ext lo s", i16x8_extend_low_i8x16(a8));
    h("i16 ext hi s", i16x8_extend_high_i8x16(a8));
    h("i16 ext lo u", i16x8_extend_low_u8x16(a8));
    h("i16 ext hi u", i16x8_extend_high_u8x16(a8));
    h("u16 ext lo u", u16x8_extend_low_u8x16(a8));
    h("u16 ext hi u", u16x8_extend_high_u8x16(a8));
    h("i16 extmul lo s", i16x8_extmul_low_i8x16(a8, b8));
    h("i16 extmul hi s", i16x8_extmul_high_i8x16(a8, b8));
    h("i16 extmul lo u", i16x8_extmul_low_u8x16(a8, b8));
    h("i16 extmul hi u", i16x8_extmul_high_u8x16(a8, b8));
    h("u16 extmul lo u", u16x8_extmul_low_u8x16(a8, b8));
    h("u16 extmul hi u", u16x8_extmul_high_u8x16(a8, b8));
    h("i16 extadd s", i16x8_extadd_pairwise_i8x16(a8));
    h("i16 extadd u", i16x8_extadd_pairwise_u8x16(a8));
    h("u16 extadd u", u16x8_extadd_pairwise_u8x16(b8));

    h("i32 add", i32x4_add(a32, b32));
    h("u32 add", u32x4_add(a32, b32));
    h("i32 sub", i32x4_sub(a32, b32));
    h("u32 sub", u32x4_sub(a32, b32));
    h("i32 mul", i32x4_mul(a32, b32));
    h("u32 mul", u32x4_mul(a32, b32));
    h("i32 min", i32x4_min(a32, b32));
    h("u32 min", u32x4_min(a32, b32));
    h("i32 max", i32x4_max(a32, b32));
    h("u32 max", u32x4_max(a32, b32));
    h("i32 abs", i32x4_abs(a32));
    h("i32 neg", i32x4_neg(a32));
    h("i32 dot", i32x4_dot_i16x8(a16, b16));
    h("i32 eq", i32x4_eq(a32, b32));
    h("u32 eq", u32x4_eq(a32, a32));
    h("i32 ne", i32x4_ne(a32, b32));
    h("u32 ne", u32x4_ne(a32, b32));
    h("i32 lt", i32x4_lt(a32, b32));
    h("u32 lt", u32x4_lt(a32, b32));
    h("i32 gt", i32x4_gt(a32, b32));
    h("u32 gt", u32x4_gt(a32, b32));
    h("i32 le", i32x4_le(a32, b32));
    h("u32 le", u32x4_le(a32, b32));
    h("i32 ge", i32x4_ge(a32, b32));
    h("u32 ge", u32x4_ge(a32, b32));
    for s in [0u32, 1, 31, 32, 35] {
        h(&format!("i32 shl {s}"), i32x4_shl(a32, s));
        h(&format!("u32 shl {s}"), u32x4_shl(a32, s));
        h(&format!("i32 shr {s}"), i32x4_shr(a32, s));
        h(&format!("u32 shr {s}"), u32x4_shr(a32, s));
    }
    h("i32 ext lo s", i32x4_extend_low_i16x8(a16));
    h("i32 ext hi s", i32x4_extend_high_i16x8(a16));
    h("i32 ext lo u", i32x4_extend_low_u16x8(a16));
    h("i32 ext hi u", i32x4_extend_high_u16x8(a16));
    h("u32 ext lo u", u32x4_extend_low_u16x8(a16));
    h("u32 ext hi u", u32x4_extend_high_u16x8(a16));
    h("i32 extmul lo s", i32x4_extmul_low_i16x8(a16, b16));
    h("i32 extmul hi s", i32x4_extmul_high_i16x8(a16, b16));
    h("i32 extmul lo u", i32x4_extmul_low_u16x8(a16, b16));
    h("i32 extmul hi u", i32x4_extmul_high_u16x8(a16, b16));
    h("u32 extmul lo u", u32x4_extmul_low_u16x8(a16, b16));
    h("u32 extmul hi u", u32x4_extmul_high_u16x8(a16, b16));
    h("i32 extadd s", i32x4_extadd_pairwise_i16x8(a16));
    h("i32 extadd u", i32x4_extadd_pairwise_u16x8(a16));
    h("u32 extadd u", u32x4_extadd_pairwise_u16x8(b16));

    h("i64 add", i64x2_add(a64, b64));
    h("u64 add", u64x2_add(a64, b64));
    h("i64 sub", i64x2_sub(a64, b64));
    h("u64 sub", u64x2_sub(a64, b64));
    h("i64 mul", i64x2_mul(a64, b64));
    h("u64 mul", u64x2_mul(a64, b64));
    h("i64 abs", i64x2_abs(i64x2(i64::MIN, -3)));
    h("i64 neg", i64x2_neg(a64));
    h("i64 eq", i64x2_eq(a64, b64));
    h("u64 eq", u64x2_eq(a64, a64));
    h("i64 ne", i64x2_ne(a64, b64));
    h("u64 ne", u64x2_ne(a64, b64));
    h("i64 lt", i64x2_lt(a64, b64));
    h("i64 gt", i64x2_gt(a64, b64));
    h("i64 le", i64x2_le(a64, a64));
    h("i64 ge", i64x2_ge(b64, a64));
    for s in [0u32, 1, 63, 64, 65] {
        h(&format!("i64 shl {s}"), i64x2_shl(a64, s));
        h(&format!("u64 shl {s}"), u64x2_shl(a64, s));
        h(&format!("i64 shr {s}"), i64x2_shr(b64, s));
        h(&format!("u64 shr {s}"), u64x2_shr(b64, s));
    }
    h("i64 ext lo s", i64x2_extend_low_i32x4(a32));
    h("i64 ext hi s", i64x2_extend_high_i32x4(a32));
    h("i64 ext lo u", i64x2_extend_low_u32x4(a32));
    h("i64 ext hi u", i64x2_extend_high_u32x4(a32));
    h("u64 ext lo u", u64x2_extend_low_u32x4(a32));
    h("u64 ext hi u", u64x2_extend_high_u32x4(a32));
    h("i64 extmul lo s", i64x2_extmul_low_i32x4(a32, b32));
    h("i64 extmul hi s", i64x2_extmul_high_i32x4(a32, b32));
    h("i64 extmul lo u", i64x2_extmul_low_u32x4(a32, b32));
    h("i64 extmul hi u", i64x2_extmul_high_u32x4(a32, b32));
    h("u64 extmul lo u", u64x2_extmul_low_u32x4(a32, b32));
    h("u64 extmul hi u", u64x2_extmul_high_u32x4(a32, b32));
}
