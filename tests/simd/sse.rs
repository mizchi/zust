// core::arch::x86_64, 128 bits (SSE .. SSE4.1): what the Cranelift backend emits for the IR's SIMD (issue #30).
// x86's semantics where they differ from wasm's: min / max, shifts past the lane width, float to int conversions
use std::arch::x86_64::*;

fn h(name: &str, v: __m128i) {
    let mut o = [0u32; 4];
    unsafe { _mm_storeu_si128(o.as_mut_ptr() as *mut __m128i, v) };
    println!("{name}: {:08x} {:08x} {:08x} {:08x}", o[0], o[1], o[2], o[3]);
}

fn hf(name: &str, v: __m128) {
    h(name, unsafe { _mm_castps_si128(v) });
}

fn hd(name: &str, v: __m128d) {
    h(name, unsafe { _mm_castpd_si128(v) });
}

#[target_feature(enable = "sse4.2")]
fn run() {
    let a = _mm_setr_ps(1.5, -2.25, 0.0, -0.0);
    let b = _mm_set_ps(0.0, -0.0, f32::NAN, 3.0);
    let n = _mm_setr_ps(f32::NAN, 1.0, f32::INFINITY, -1e30);
    hf("setr_ps", a);
    hf("set_ps", b);
    hf("set1_ps", _mm_set1_ps(-7.5));
    hf("setzero_ps", _mm_setzero_ps());
    hf("add", _mm_add_ps(a, n));
    hf("sub", _mm_sub_ps(a, n));
    hf("mul", _mm_mul_ps(a, n));
    hf("div", _mm_div_ps(n, a));
    hf("sqrt", _mm_sqrt_ps(n));
    hf("min ab", _mm_min_ps(a, b));
    hf("min ba", _mm_min_ps(b, a));
    hf("max ab", _mm_max_ps(a, b));
    hf("max ba", _mm_max_ps(b, a));
    hf("min n", _mm_min_ps(n, a));
    hf("max n", _mm_max_ps(a, n));
    hf("and", _mm_and_ps(a, n));
    hf("or", _mm_or_ps(a, b));
    hf("xor", _mm_xor_ps(a, n));
    hf("andnot", _mm_andnot_ps(a, n));
    for (k, (x, y)) in [(a, b), (n, a), (b, n)].into_iter().enumerate() {
        hf(&format!("cmpeq {k}"), _mm_cmpeq_ps(x, y));
        hf(&format!("cmpneq {k}"), _mm_cmpneq_ps(x, y));
        hf(&format!("cmplt {k}"), _mm_cmplt_ps(x, y));
        hf(&format!("cmple {k}"), _mm_cmple_ps(x, y));
        hf(&format!("cmpgt {k}"), _mm_cmpgt_ps(x, y));
        hf(&format!("cmpge {k}"), _mm_cmpge_ps(x, y));
    }
    println!("movemask_ps {} {}", _mm_movemask_ps(a), _mm_movemask_ps(n));
    println!("cvtss_f32 {} extract_ps {:08x}", _mm_cvtss_f32(a), _mm_extract_ps::<1>(a) as u32);

    let d = _mm_setr_pd(1.5, -0.0);
    let e = _mm_set_pd(f64::NAN, -3.0);
    hd("set_pd", e);
    hd("add_pd", _mm_add_pd(d, e));
    hd("mul_pd", _mm_mul_pd(d, e));
    hd("div_pd", _mm_div_pd(e, d));
    hd("sqrt_pd", _mm_sqrt_pd(_mm_set1_pd(2.0)));
    hd("min_pd", _mm_min_pd(d, e));
    hd("max_pd", _mm_max_pd(e, d));
    hd("cmplt_pd", _mm_cmplt_pd(e, d));
    hd("cmpneq_pd", _mm_cmpneq_pd(e, e));
    println!("movemask_pd {} cvtsd_f64 {}", _mm_movemask_pd(e), _mm_cvtsd_f64(d));

    let x = _mm_setr_epi32(1, -2, i32::MAX, i32::MIN);
    let y = _mm_setr_epi32(3, 7, 1, -1);
    let x8 = _mm_setr_epi8(1, -2, 3, -4, 127, -128, 100, -100, 0, 50, -50, 77, -1, 64, 32, 16);
    let y8 = _mm_set1_epi8(-100);
    let x16 = _mm_setr_epi16(1, -2, 32767, -32768, 1000, -1000, 255, -256);
    let y16 = _mm_set_epi16(3, 2, 1, -1, 1000, 30000, -255, 256);
    h("set_epi32", _mm_set_epi32(1, 2, 3, 4));
    h("set_epi64x", _mm_set_epi64x(-1, 2));
    h("set1_epi64x", _mm_set1_epi64x(0x1234_5678_9abc_def0));
    h("set1_epi16", _mm_set1_epi16(-3));
    h("setzero", _mm_setzero_si128());
    h("add_epi8", _mm_add_epi8(x8, y8));
    h("add_epi16", _mm_add_epi16(x16, y16));
    h("add_epi32", _mm_add_epi32(x, y));
    h("add_epi64", _mm_add_epi64(x, y));
    h("sub_epi32", _mm_sub_epi32(x, y));
    h("sub_epi64", _mm_sub_epi64(x, y));
    h("mullo_epi16", _mm_mullo_epi16(x16, y16));
    h("mullo_epi32", _mm_mullo_epi32(x, y));
    h("adds_epi8", _mm_adds_epi8(x8, y8));
    h("adds_epu8", _mm_adds_epu8(x8, y8));
    h("subs_epi16", _mm_subs_epi16(x16, y16));
    h("subs_epu16", _mm_subs_epu16(x16, y16));
    h("avg_epu8", _mm_avg_epu8(x8, y8));
    h("avg_epu16", _mm_avg_epu16(x16, y16));
    h("min_epi8", _mm_min_epi8(x8, y8));
    h("min_epu8", _mm_min_epu8(x8, y8));
    h("max_epi16", _mm_max_epi16(x16, y16));
    h("max_epu16", _mm_max_epu16(x16, y16));
    h("min_epi32", _mm_min_epi32(x, y));
    h("min_epu32", _mm_min_epu32(x, y));
    h("max_epi32", _mm_max_epi32(x, y));
    h("max_epu32", _mm_max_epu32(x, y));
    h("abs_epi8", _mm_abs_epi8(x8));
    h("abs_epi16", _mm_abs_epi16(x16));
    h("abs_epi32", _mm_abs_epi32(x));
    h("and", _mm_and_si128(x, y));
    h("or", _mm_or_si128(x, y));
    h("xor", _mm_xor_si128(x, y));
    h("andnot", _mm_andnot_si128(x, y));
    h("cmpeq_epi8", _mm_cmpeq_epi8(x8, y8));
    h("cmpgt_epi8", _mm_cmpgt_epi8(x8, y8));
    h("cmplt_epi16", _mm_cmplt_epi16(x16, y16));
    h("cmpeq_epi32", _mm_cmpeq_epi32(x, x));
    h("cmpgt_epi32", _mm_cmpgt_epi32(x, y));
    h("cmplt_epi32", _mm_cmplt_epi32(x, y));
    h("cmpeq_epi64", _mm_cmpeq_epi64(x, y));
    h("cmpgt_epi64", _mm_cmpgt_epi64(x, y));
    println!("movemask_epi8 {} testz {} {}", _mm_movemask_epi8(x8), _mm_testz_si128(x, y), _mm_testz_si128(x, _mm_setzero_si128()));
    println!(
        "extract {} {} {} {} cvtsi128 {} {}",
        _mm_extract_epi8::<5>(x8),
        _mm_extract_epi16::<3>(x16),
        _mm_extract_epi32::<3>(x),
        _mm_extract_epi64::<1>(x),
        _mm_cvtsi128_si32(x),
        _mm_cvtsi128_si64(x)
    );
    h("slli_epi16 3", _mm_slli_epi16::<3>(x16));
    h("slli_epi16 16", _mm_slli_epi16::<16>(x16));
    h("srli_epi16 15", _mm_srli_epi16::<15>(x16));
    h("srai_epi16 20", _mm_srai_epi16::<20>(x16));
    h("slli_epi32 31", _mm_slli_epi32::<31>(x));
    h("slli_epi32 32", _mm_slli_epi32::<32>(x));
    h("srli_epi32 4", _mm_srli_epi32::<4>(x));
    h("srai_epi32 1", _mm_srai_epi32::<1>(x));
    h("srai_epi32 40", _mm_srai_epi32::<40>(x));
    h("slli_epi64 63", _mm_slli_epi64::<63>(x));
    h("srli_epi64 64", _mm_srli_epi64::<64>(x));
    h("blendv_epi8", _mm_blendv_epi8(x8, y8, x8));
    hf("blendv_ps", _mm_blendv_ps(a, n, _mm_castsi128_ps(x)));
    hd("blendv_pd", _mm_blendv_pd(d, e, _mm_castsi128_pd(x)));
    h("packs_epi16", _mm_packs_epi16(x16, y16));
    h("packus_epi16", _mm_packus_epi16(x16, y16));
    h("packs_epi32", _mm_packs_epi32(x, y));
    h("packus_epi32", _mm_packus_epi32(x, y));
    h("madd_epi16", _mm_madd_epi16(x16, y16));
    h("cvtepi8_epi16", _mm_cvtepi8_epi16(x8));
    h("cvtepu8_epi16", _mm_cvtepu8_epi16(x8));
    h("cvtepi16_epi32", _mm_cvtepi16_epi32(x16));
    h("cvtepu16_epi32", _mm_cvtepu16_epi32(x16));
    h("cvtepi32_epi64", _mm_cvtepi32_epi64(x));
    h("cvtepu32_epi64", _mm_cvtepu32_epi64(x));
    hf("cvtepi32_ps", _mm_cvtepi32_ps(x));
    hd("cvtepi32_pd", _mm_cvtepi32_pd(x));
    let big = _mm_setr_ps(2.5, -2.5, 3e9, f32::NAN);
    let big2 = _mm_setr_ps(-3e9, 1.5, f32::INFINITY, 2147483520.0);
    h("cvttps_epi32", _mm_cvttps_epi32(big));
    h("cvttps_epi32 2", _mm_cvttps_epi32(big2));
    h("cvtps_epi32", _mm_cvtps_epi32(big));
    h("cvtps_epi32 2", _mm_cvtps_epi32(_mm_setr_ps(0.5, 1.5, -0.5, -1.5)));
    hd("cvtps_pd", _mm_cvtps_pd(a));
    hf("cvtpd_ps", _mm_cvtpd_ps(e));
    let mut buf = [0f32; 8];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = i as f32 * 1.25;
    }
    unsafe {
        let v = _mm_loadu_ps(buf.as_ptr().add(1));
        hf("loadu_ps", v);
        _mm_storeu_ps(buf.as_mut_ptr().add(3), _mm_mul_ps(v, v));
        hf("broadcast_ss", _mm_broadcast_ss(&buf[4]));
        let w = _mm_loadu_pd([1.0f64, 2.0].as_ptr());
        hd("loadu_pd", w);
    }
    println!("buf {:?}", buf);
}

fn main() {
    unsafe { run() }
}
