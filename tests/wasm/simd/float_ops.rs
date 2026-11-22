// core::arch::wasm32: float lanes (issue #30): NaN, signed zeros, infinities, rounding and conversions
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

/// NaN's sign and payload are not specified by wasm for arithmetic results: compare NaNs as "NaN"
fn f4(name: &str, v: v128) {
    let l = [f32x4_extract_lane::<0>(v), f32x4_extract_lane::<1>(v), f32x4_extract_lane::<2>(v), f32x4_extract_lane::<3>(v)];
    let s: Vec<String> = l.iter().map(|x| if x.is_nan() { "NaN".to_string() } else { format!("{:08x}", x.to_bits()) }).collect();
    println!("{name}: {}", s.join(" "));
}

fn f2(name: &str, v: v128) {
    let l = [f64x2_extract_lane::<0>(v), f64x2_extract_lane::<1>(v)];
    let s: Vec<String> = l.iter().map(|x| if x.is_nan() { "NaN".to_string() } else { format!("{:016x}", x.to_bits()) }).collect();
    println!("{name}: {}", s.join(" "));
}

fn main() {
    let sets32 = [
        (f32x4(1.5, -2.25, 0.0, -0.0), f32x4(-0.5, 3.0, -0.0, 0.0)),
        (f32x4(f32::NAN, 1.0, f32::INFINITY, f32::NEG_INFINITY), f32x4(2.0, f32::NAN, -1.0, f32::INFINITY)),
        (f32x4(2.5, -2.5, 3.5, 1e30), f32x4(0.4999, -0.5, 1e-30, -1e30)),
    ];
    for (k, &(a, b)) in sets32.iter().enumerate() {
        f4(&format!("f32 add {k}"), f32x4_add(a, b));
        f4(&format!("f32 sub {k}"), f32x4_sub(a, b));
        f4(&format!("f32 mul {k}"), f32x4_mul(a, b));
        f4(&format!("f32 div {k}"), f32x4_div(a, b));
        f4(&format!("f32 min {k}"), f32x4_min(a, b));
        f4(&format!("f32 max {k}"), f32x4_max(a, b));
        f4(&format!("f32 pmin {k}"), f32x4_pmin(a, b));
        f4(&format!("f32 pmax {k}"), f32x4_pmax(a, b));
        f4(&format!("f32 sqrt {k}"), f32x4_sqrt(a));
        f4(&format!("f32 neg {k}"), f32x4_neg(a));
        f4(&format!("f32 abs {k}"), f32x4_abs(b));
        f4(&format!("f32 ceil {k}"), f32x4_ceil(a));
        f4(&format!("f32 floor {k}"), f32x4_floor(a));
        f4(&format!("f32 trunc {k}"), f32x4_trunc(b));
        f4(&format!("f32 nearest {k}"), f32x4_nearest(a));
        h(&format!("f32 eq {k}"), f32x4_eq(a, b));
        h(&format!("f32 ne {k}"), f32x4_ne(a, b));
        h(&format!("f32 lt {k}"), f32x4_lt(a, b));
        h(&format!("f32 gt {k}"), f32x4_gt(a, b));
        h(&format!("f32 le {k}"), f32x4_le(a, b));
        h(&format!("f32 ge {k}"), f32x4_ge(a, b));
        h(&format!("i32 trunc_sat {k}"), i32x4_trunc_sat_f32x4(a));
        h(&format!("u32 trunc_sat {k}"), u32x4_trunc_sat_f32x4(b));
        f2(&format!("f64 promote {k}"), f64x2_promote_low_f32x4(a));
    }
    let sets64 = [
        (f64x2(1.5, -0.0), f64x2(-0.5, 0.0)),
        (f64x2(f64::NAN, f64::INFINITY), f64x2(2.0, -1.0)),
        (f64x2(2.5, -3.5), f64x2(1e-300, f64::NAN)),
        (f64x2(1e300, -1e300), f64x2(3e9, -3e9)),
    ];
    for (k, &(a, b)) in sets64.iter().enumerate() {
        f2(&format!("f64 add {k}"), f64x2_add(a, b));
        f2(&format!("f64 sub {k}"), f64x2_sub(a, b));
        f2(&format!("f64 mul {k}"), f64x2_mul(a, b));
        f2(&format!("f64 div {k}"), f64x2_div(a, b));
        f2(&format!("f64 min {k}"), f64x2_min(a, b));
        f2(&format!("f64 max {k}"), f64x2_max(a, b));
        f2(&format!("f64 pmin {k}"), f64x2_pmin(a, b));
        f2(&format!("f64 pmax {k}"), f64x2_pmax(a, b));
        f2(&format!("f64 sqrt {k}"), f64x2_sqrt(a));
        f2(&format!("f64 neg {k}"), f64x2_neg(a));
        f2(&format!("f64 abs {k}"), f64x2_abs(a));
        f2(&format!("f64 ceil {k}"), f64x2_ceil(a));
        f2(&format!("f64 floor {k}"), f64x2_floor(a));
        f2(&format!("f64 trunc {k}"), f64x2_trunc(a));
        f2(&format!("f64 nearest {k}"), f64x2_nearest(a));
        h(&format!("f64 eq {k}"), f64x2_eq(a, b));
        h(&format!("f64 ne {k}"), f64x2_ne(a, b));
        h(&format!("f64 lt {k}"), f64x2_lt(a, b));
        h(&format!("f64 gt {k}"), f64x2_gt(a, b));
        h(&format!("f64 le {k}"), f64x2_le(a, b));
        h(&format!("f64 ge {k}"), f64x2_ge(a, b));
        h(&format!("i32 trunc_sat_zero {k}"), i32x4_trunc_sat_f64x2_zero(a));
        h(&format!("u32 trunc_sat_zero {k}"), u32x4_trunc_sat_f64x2_zero(b));
        f4(&format!("f32 demote {k}"), f32x4_demote_f64x2_zero(a));
    }
    let ints = i32x4(-1, i32::MAX, i32::MIN, 16777217);
    f4("f32 convert s", f32x4_convert_i32x4(ints));
    f4("f32 convert u", f32x4_convert_u32x4(ints));
    f2("f64 convert s", f64x2_convert_low_i32x4(ints));
    f2("f64 convert u", f64x2_convert_low_u32x4(ints));
    f2("f64 convert s hi", f64x2_convert_low_i32x4(i32x4_shuffle::<2, 3, 0, 1>(ints, ints)));
}
