// core::arch::wasm32's relaxed SIMD (issue #30). Engines may give either of the allowed results: each case
// prints a value both allow (inputs where they agree), or whether the result is one of the allowed ones
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
    // exact products: fused or not, the same
    let a = f32x4(1.5, -2.0, 4.0, 0.25);
    let b = f32x4(2.0, 3.0, -0.5, 8.0);
    let c = f32x4(1.0, 1.0, 1.0, -2.0);
    h("f32 madd", f32x4_relaxed_madd(a, b, c));
    h("f32 nmadd", f32x4_relaxed_nmadd(a, b, c));
    let a2 = f64x2(1.5, -2.0);
    let b2 = f64x2(4.0, 0.125);
    let c2 = f64x2(0.5, 3.0);
    h("f64 madd", f64x2_relaxed_madd(a2, b2, c2));
    h("f64 nmadd", f64x2_relaxed_nmadd(a2, b2, c2));
    // an inexact product: one rounding or two
    let x = 1.0f32 + f32::EPSILON;
    let m = f32x4_extract_lane::<0>(f32x4_relaxed_madd(f32x4_splat(x), f32x4_splat(x), f32x4_splat(-1.0)));
    let fused = x.mul_add(x, -1.0);
    let unfused = x * x - 1.0;
    println!("f32 madd rounding allowed: {}", m == fused || m == unfused);
    // min / max without NaN or zeros
    h("f32 min", f32x4_relaxed_min(a, b));
    h("f32 max", f32x4_relaxed_max(a, b));
    h("f64 min", f64x2_relaxed_min(a2, b2));
    h("f64 max", f64x2_relaxed_max(a2, b2));
    // laneselect with all-ones / all-zeros lanes
    let p = u32x4(0x1111_1111, 0x2222_2222, 0x3333_3333, 0x4444_4444);
    let q = u32x4(0xaaaa_aaaa, 0xbbbb_bbbb, 0xcccc_cccc, 0xdddd_dddd);
    h("i8 laneselect", i8x16_relaxed_laneselect(p, q, i8x16(-1, 0, -1, 0, 0, 0, -1, -1, 0, -1, 0, -1, -1, -1, -1, 0)));
    h("u8 laneselect", u8x16_relaxed_laneselect(p, q, u8x16_splat(0xff)));
    h("i16 laneselect", i16x8_relaxed_laneselect(p, q, i16x8(-1, 0, 0, -1, -1, -1, 0, 0)));
    h("u16 laneselect", u16x8_relaxed_laneselect(p, q, u16x8_splat(0)));
    h("i32 laneselect", i32x4_relaxed_laneselect(p, q, i32x4(0, -1, 0, -1)));
    h("u32 laneselect", u32x4_relaxed_laneselect(p, q, u32x4(!0, 0, !0, 0)));
    h("i64 laneselect", i64x2_relaxed_laneselect(p, q, i64x2(-1, 0)));
    h("u64 laneselect", u64x2_relaxed_laneselect(p, q, u64x2(0, !0)));
    // swizzle with indices < 16
    let t = u8x16(10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25);
    h("swizzle", i8x16_relaxed_swizzle(t, u8x16(15, 0, 14, 1, 13, 2, 12, 3, 11, 4, 10, 5, 9, 6, 8, 7)));
    h("swizzle u", u8x16_relaxed_swizzle(t, u8x16_splat(3)));
    // truncation in range
    h("i32 trunc f32", i32x4_relaxed_trunc_f32x4(f32x4(1.9, -1.9, 1e9, -0.5)));
    h("u32 trunc f32", u32x4_relaxed_trunc_f32x4(f32x4(1.9, 3e9, 0.0, 7.5)));
    h("i32 trunc f64", i32x4_relaxed_trunc_f64x2_zero(f64x2(-2.5, 2e9)));
    h("u32 trunc f64", u32x4_relaxed_trunc_f64x2_zero(f64x2(4e9, 1.0)));
    // q15mulr away from -32768 * -32768
    h("q15mulr", i16x8_relaxed_q15mulr(i16x8(16384, -16384, 32767, 100, -5, 0, 1, 2), i16x8(16384, 16384, 32767, -200, 7, 9, 32767, -32768)));
    h("q15mulr u", u16x8_relaxed_q15mulr(i16x8_splat(8192), i16x8_splat(4)));
    // the dot products, the second operand in i7 (0..=127)
    let s = i8x16(1, -2, 3, -4, 5, -6, 7, -8, 127, -128, 50, -50, 0, 1, -1, 100);
    let u = i8x16(1, 2, 3, 4, 5, 6, 7, 8, 127, 127, 0, 1, 2, 3, 4, 5);
    h("dot", i16x8_relaxed_dot_i8x16_i7x16(s, u));
    h("dot u", u16x8_relaxed_dot_i8x16_i7x16(s, u));
    h("dot add", i32x4_relaxed_dot_i8x16_i7x16_add(s, u, i32x4(1000, -1000, 0, i32::MAX - 100000)));
    h("dot add u", u32x4_relaxed_dot_i8x16_i7x16_add(s, u, u32x4_splat(7)));
}
