// core::arch::x86_64, 256 bits (AVX, AVX2, FMA): the IR's 256-bit vectors, which the Cranelift backend holds as two
// 128-bit halves (issue #30). Includes the instructions that work within each 128-bit half (packs), the ones that
// move between halves (casts, extract / insert, conversions that read or write half the lanes), and every AVX
// comparison predicate on NaN
use std::arch::x86_64::*;

fn h(name: &str, v: __m256i) {
    let mut o = [0u32; 8];
    unsafe { _mm256_storeu_si256(o.as_mut_ptr() as *mut __m256i, v) };
    let s: Vec<String> = o.iter().map(|x| format!("{x:08x}")).collect();
    println!("{name}: {}", s.join(" "));
}

fn h128(name: &str, v: __m128i) {
    let mut o = [0u32; 4];
    unsafe { _mm_storeu_si128(o.as_mut_ptr() as *mut __m128i, v) };
    println!("{name}: {:08x} {:08x} {:08x} {:08x}", o[0], o[1], o[2], o[3]);
}

#[target_feature(enable = "avx2,fma")]
fn hf(name: &str, v: __m256) {
    h(name, _mm256_castps_si256(v));
}

#[target_feature(enable = "avx2,fma")]
fn hd(name: &str, v: __m256d) {
    h(name, _mm256_castpd_si256(v));
}

#[target_feature(enable = "avx2,fma")]
fn run() {
    let a = _mm256_setr_ps(1.5, -2.25, 0.0, -0.0, 7.0, 8.5, -9.0, 1e30);
    let b = _mm256_set_ps(0.0, -0.0, f32::NAN, 3.0, -1.0, 2.0, 100.0, -1e30);
    let n = _mm256_setr_ps(f32::NAN, 1.0, f32::INFINITY, -1e30, 0.5, -0.5, f32::NAN, 0.0);
    hf("setr_ps", a);
    hf("set_ps", b);
    hf("set1_ps", _mm256_set1_ps(-7.5));
    hf("setzero_ps", _mm256_setzero_ps());
    hf("add", _mm256_add_ps(a, b));
    hf("sub", _mm256_sub_ps(a, n));
    hf("mul", _mm256_mul_ps(a, n));
    hf("div", _mm256_div_ps(n, a));
    hf("sqrt", _mm256_sqrt_ps(a));
    hf("min", _mm256_min_ps(a, b));
    hf("max", _mm256_max_ps(b, a));
    hf("min n", _mm256_min_ps(n, a));
    hf("max n", _mm256_max_ps(a, n));
    hf("and", _mm256_and_ps(a, n));
    hf("or", _mm256_or_ps(a, b));
    hf("xor", _mm256_xor_ps(a, n));
    hf("andnot", _mm256_andnot_ps(a, n));
    hf("fmadd", _mm256_fmadd_ps(a, b, n));
    hf("fmsub", _mm256_fmsub_ps(a, b, n));
    hf("fnmadd", _mm256_fnmadd_ps(a, b, n));
    hf("fnmsub", _mm256_fnmsub_ps(a, b, n));
    // (one rounding: 1 + eps squared minus one is not 2 eps)
    let e = _mm256_set1_ps(1.0 + f32::EPSILON);
    hf("fmadd rounding", _mm256_fmadd_ps(e, e, _mm256_set1_ps(-1.0)));
    hf("fmadd 128", _mm256_castps128_ps256(_mm_fmadd_ps(_mm256_castps256_ps128(a), _mm256_castps256_ps128(b), _mm256_castps256_ps128(n))));
    hf("cmp 0", _mm256_cmp_ps::<_CMP_EQ_OQ>(a, b));
    hf("cmp 1", _mm256_cmp_ps::<_CMP_LT_OS>(a, b));
    hf("cmp 2", _mm256_cmp_ps::<_CMP_LE_OS>(a, b));
    hf("cmp 3", _mm256_cmp_ps::<_CMP_UNORD_Q>(a, n));
    hf("cmp 4", _mm256_cmp_ps::<_CMP_NEQ_UQ>(a, n));
    hf("cmp 5", _mm256_cmp_ps::<_CMP_NLT_US>(a, n));
    hf("cmp 6", _mm256_cmp_ps::<_CMP_NLE_US>(a, n));
    hf("cmp 7", _mm256_cmp_ps::<_CMP_ORD_Q>(a, n));
    hf("cmp 8", _mm256_cmp_ps::<_CMP_EQ_UQ>(a, n));
    hf("cmp 9", _mm256_cmp_ps::<_CMP_NGE_US>(a, n));
    hf("cmp 10", _mm256_cmp_ps::<_CMP_NGT_US>(a, n));
    hf("cmp 11", _mm256_cmp_ps::<_CMP_FALSE_OQ>(a, n));
    hf("cmp 12", _mm256_cmp_ps::<_CMP_NEQ_OQ>(a, n));
    hf("cmp 13", _mm256_cmp_ps::<_CMP_GE_OS>(a, n));
    hf("cmp 14", _mm256_cmp_ps::<_CMP_GT_OS>(a, n));
    hf("cmp 15", _mm256_cmp_ps::<_CMP_TRUE_UQ>(a, n));
    hf("cmp 17", _mm256_cmp_ps::<_CMP_LT_OQ>(n, a));
    hf("cmp 29", _mm256_cmp_ps::<_CMP_GE_OQ>(n, a));
    hf("cmp 30", _mm256_cmp_ps::<_CMP_GT_OQ>(n, a));
    hf("cmp literal", _mm256_cmp_ps::<14>(a, b));
    println!("movemask_ps {} {}", _mm256_movemask_ps(a), _mm256_movemask_ps(n));
    println!("cvtss_f32 {}", _mm256_cvtss_f32(a));
    hf("blendv_ps", _mm256_blendv_ps(a, n, b));

    let d = _mm256_setr_pd(1.5, -0.0, f64::NAN, 1e300);
    let d2 = _mm256_set_pd(2.0, -3.0, 0.0, f64::INFINITY);
    hd("add_pd", _mm256_add_pd(d, d2));
    hd("mul_pd", _mm256_mul_pd(d, d2));
    hd("div_pd", _mm256_div_pd(d2, d));
    hd("sqrt_pd", _mm256_sqrt_pd(d2));
    hd("min_pd", _mm256_min_pd(d, d2));
    hd("max_pd", _mm256_max_pd(d2, d));
    hd("fmadd_pd", _mm256_fmadd_pd(d, d2, d2));
    hd("cmp_pd lt", _mm256_cmp_pd::<_CMP_LT_OQ>(d, d2));
    hd("cmp_pd neq", _mm256_cmp_pd::<_CMP_NEQ_UQ>(d, d));
    hd("blendv_pd", _mm256_blendv_pd(d, d2, d));
    println!("movemask_pd {} cvtsd_f64 {}", _mm256_movemask_pd(d), _mm256_cvtsd_f64(d));
    hd("set1_pd", _mm256_set1_pd(0.1));
    hd("setzero_pd", _mm256_setzero_pd());

    let x = _mm256_setr_epi32(1, -2, i32::MAX, i32::MIN, 5, -6, 70000, -1);
    let y = _mm256_setr_epi32(3, 7, 1, -1, 5, 6, -70000, 0);
    let x8 = _mm256_setr_epi8(
        1, -2, 3, -4, 127, -128, 100, -100, 0, 50, -50, 77, -1, 64, 32, 16, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, -1, -2, -3, -4, -5, -128,
    );
    let y8 = _mm256_set1_epi8(-100);
    let x16 = _mm256_setr_epi16(1, -2, 32767, -32768, 1000, -1000, 255, -256, 9, 99, 999, 9999, -9, -99, -999, -9999);
    let y16 = _mm256_set1_epi16(300);
    h("set_epi32", _mm256_set_epi32(1, 2, 3, 4, 5, 6, 7, 8));
    h("set_epi64x", _mm256_set_epi64x(-1, 2, -3, 4));
    h("setr_epi64x", _mm256_setr_epi64x(-1, 2, -3, 4));
    h("set1_epi64x", _mm256_set1_epi64x(0x1234_5678_9abc_def0));
    h("set1_epi32", _mm256_set1_epi32(-5));
    h("setzero", _mm256_setzero_si256());
    h("add_epi8", _mm256_add_epi8(x8, y8));
    h("add_epi16", _mm256_add_epi16(x16, y16));
    h("add_epi32", _mm256_add_epi32(x, y));
    h("add_epi64", _mm256_add_epi64(x, y));
    h("sub_epi8", _mm256_sub_epi8(x8, y8));
    h("sub_epi32", _mm256_sub_epi32(x, y));
    h("mullo_epi16", _mm256_mullo_epi16(x16, y16));
    h("mullo_epi32", _mm256_mullo_epi32(x, y));
    h("adds_epi8", _mm256_adds_epi8(x8, y8));
    h("adds_epu8", _mm256_adds_epu8(x8, y8));
    h("subs_epi16", _mm256_subs_epi16(x16, y16));
    h("subs_epu16", _mm256_subs_epu16(x16, y16));
    h("avg_epu8", _mm256_avg_epu8(x8, y8));
    h("min_epi8", _mm256_min_epi8(x8, y8));
    h("max_epu8", _mm256_max_epu8(x8, y8));
    h("min_epi16", _mm256_min_epi16(x16, y16));
    h("max_epu16", _mm256_max_epu16(x16, y16));
    h("min_epi32", _mm256_min_epi32(x, y));
    h("max_epu32", _mm256_max_epu32(x, y));
    h("abs_epi8", _mm256_abs_epi8(x8));
    h("abs_epi32", _mm256_abs_epi32(x));
    h("and", _mm256_and_si256(x, y));
    h("or", _mm256_or_si256(x, y));
    h("xor", _mm256_xor_si256(x, y));
    h("andnot", _mm256_andnot_si256(x, y));
    h("cmpeq_epi8", _mm256_cmpeq_epi8(x8, y8));
    h("cmpgt_epi8", _mm256_cmpgt_epi8(x8, y8));
    h("cmpgt_epi16", _mm256_cmpgt_epi16(x16, y16));
    h("cmpeq_epi32", _mm256_cmpeq_epi32(x, y));
    h("cmpgt_epi32", _mm256_cmpgt_epi32(x, y));
    h("cmpeq_epi64", _mm256_cmpeq_epi64(x, x));
    h("cmpgt_epi64", _mm256_cmpgt_epi64(x, y));
    println!("movemask_epi8 {} testz {} {}", _mm256_movemask_epi8(x8), _mm256_testz_si256(x, y), _mm256_testz_si256(x, _mm256_setzero_si256()));
    println!(
        "extract {} {} {} {} cvtsi256 {}",
        _mm256_extract_epi8::<31>(x8),
        _mm256_extract_epi16::<9>(x16),
        _mm256_extract_epi32::<6>(x),
        _mm256_extract_epi64::<3>(x),
        _mm256_cvtsi256_si32(x)
    );
    h("slli_epi16 3", _mm256_slli_epi16::<3>(x16));
    h("slli_epi16 16", _mm256_slli_epi16::<16>(x16));
    h("srli_epi32 4", _mm256_srli_epi32::<4>(x));
    h("srli_epi32 33", _mm256_srli_epi32::<33>(x));
    h("srai_epi32 3", _mm256_srai_epi32::<3>(x));
    h("srai_epi32 99", _mm256_srai_epi32::<99>(x));
    h("srai_epi16 15", _mm256_srai_epi16::<15>(x16));
    h("slli_epi64 1", _mm256_slli_epi64::<1>(x));
    h("srli_epi64 40", _mm256_srli_epi64::<40>(x));
    h("blendv_epi8", _mm256_blendv_epi8(x8, y8, x8));
    h("packs_epi16", _mm256_packs_epi16(x16, y16));
    h("packus_epi16", _mm256_packus_epi16(x16, y16));
    h("packs_epi32", _mm256_packs_epi32(x, y));
    h("packus_epi32", _mm256_packus_epi32(x, y));
    h("madd_epi16", _mm256_madd_epi16(x16, y16));

    // halves and conversions between widths
    let lo = _mm256_castsi256_si128(x);
    let hi = _mm256_extracti128_si256::<1>(x);
    h128("cast lo", lo);
    h128("extracti128 1", hi);
    h128("extracti128 0", _mm256_extracti128_si256::<0>(x));
    h("inserti128 0", _mm256_inserti128_si256::<0>(x, _mm_set1_epi32(9)));
    h("inserti128 1", _mm256_inserti128_si256::<1>(x, _mm_set1_epi32(9)));
    h("set_m128i", _mm256_set_m128i(lo, hi));
    h("setr_m128i", _mm256_setr_m128i(lo, hi));
    hf("set_m128", _mm256_set_m128(_mm_set1_ps(1.0), _mm_set1_ps(2.0)));
    hf("insertf128", _mm256_insertf128_ps::<1>(a, _mm_set1_ps(4.0)));
    hf("extractf128", _mm256_castps128_ps256(_mm256_extractf128_ps::<1>(a)));
    hd("extractf128_pd", _mm256_castpd128_pd256(_mm256_extractf128_pd::<1>(d)));
    hf("zextps128", _mm256_zextps128_ps256(_mm_set1_ps(3.0)));
    h("cvtepi8_epi16", _mm256_cvtepi8_epi16(_mm256_castsi256_si128(x8)));
    h("cvtepu8_epi16", _mm256_cvtepu8_epi16(_mm256_castsi256_si128(x8)));
    h("cvtepi16_epi32", _mm256_cvtepi16_epi32(_mm256_castsi256_si128(x16)));
    h("cvtepu16_epi32", _mm256_cvtepu16_epi32(_mm256_castsi256_si128(x16)));
    h("cvtepi32_epi64", _mm256_cvtepi32_epi64(lo));
    h("cvtepu32_epi64", _mm256_cvtepu32_epi64(lo));
    hf("cvtepi32_ps", _mm256_cvtepi32_ps(x));
    hd("cvtepi32_pd", _mm256_cvtepi32_pd(hi));
    hd("cvtps_pd", _mm256_cvtps_pd(_mm256_castps256_ps128(a)));
    h128("cvtpd_ps", _mm_castps_si128(_mm256_cvtpd_ps(d)));
    let big = _mm256_setr_ps(2.5, -2.5, 3e9, f32::NAN, -3e9, 1.5, f32::INFINITY, 2147483520.0);
    h("cvttps_epi32", _mm256_cvttps_epi32(big));
    h("cvtps_epi32", _mm256_cvtps_epi32(big));
    h("cvtps_epi32 even", _mm256_cvtps_epi32(_mm256_setr_ps(0.5, 1.5, 2.5, -0.5, -1.5, -2.5, 3.5, 4.5)));

    let mut buf = [0f32; 20];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = i as f32 * 1.25;
    }
    unsafe {
        let v = _mm256_loadu_ps(buf.as_ptr().add(1));
        hf("loadu_ps", v);
        _mm256_storeu_ps(buf.as_mut_ptr().add(9), _mm256_mul_ps(v, v));
        hf("broadcast_ss", _mm256_broadcast_ss(&buf[4]));
        hd("broadcast_sd", _mm256_broadcast_sd(&2.5));
        let w = _mm256_loadu_pd([1.0f64, 2.0, 3.0, 4.0].as_ptr());
        hd("loadu_pd", w);
        let k = _mm256_loadu_si256(buf.as_ptr() as *const __m256i);
        h("loadu_si256", k);
    }
    println!("buf {:?}", buf);
}

fn main() {
    unsafe { run() }
}
