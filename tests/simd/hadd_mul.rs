// core::arch::x86_64's horizontal adds and subtracts (SSE3, SSSE3, AVX, AVX2) and the 32 x 32 -> 64-bit multiplies
// (pmuludq / pmuldq), at 128 and 256 bits (issue #30): wrapping, saturation, NaN, the order of each pair
use std::arch::x86_64::*;

fn h(name: &str, v: __m128i) {
    let mut o = [0u32; 4];
    unsafe { _mm_storeu_si128(o.as_mut_ptr() as *mut __m128i, v) };
    println!("{name}: {:08x} {:08x} {:08x} {:08x}", o[0], o[1], o[2], o[3]);
}

fn h2(name: &str, v: __m256i) {
    let mut o = [0u32; 8];
    unsafe { _mm256_storeu_si256(o.as_mut_ptr() as *mut __m256i, v) };
    let s: Vec<String> = o.iter().map(|x| format!("{x:08x}")).collect();
    println!("{name}: {}", s.join(" "));
}

#[target_feature(enable = "avx2")]
fn dot4(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    // (a dot product with hadd, as SSE3 code does)
    let p = unsafe { _mm_mul_ps(_mm_loadu_ps(a.as_ptr()), _mm_loadu_ps(b.as_ptr())) };
    let s = _mm_hadd_ps(p, p);
    _mm_cvtss_f32(_mm_hadd_ps(s, s))
}

#[target_feature(enable = "avx2")]
fn run() {
    let fa = _mm_setr_ps(1.5, -2.25, f32::NAN, 4.0);
    let fb = _mm_setr_ps(1e30, 1e30, -0.0, 0.0);
    h("hadd_ps", _mm_castps_si128(_mm_hadd_ps(fa, fb)));
    h("hsub_ps", _mm_castps_si128(_mm_hsub_ps(fa, fb)));
    h("hsub_ps ba", _mm_castps_si128(_mm_hsub_ps(fb, fa)));
    let da = _mm_setr_pd(1.5, -0.25);
    let db = _mm_setr_pd(f64::INFINITY, -1e300);
    h("hadd_pd", _mm_castpd_si128(_mm_hadd_pd(da, db)));
    h("hsub_pd", _mm_castpd_si128(_mm_hsub_pd(da, db)));
    let a16 = _mm_setr_epi16(1, -2, 32767, 1, -32768, -1, 300, 400);
    let b16 = _mm_setr_epi16(-32768, -32768, 32767, 32767, 7, -8, 100, -100);
    h("hadd_epi16", _mm_hadd_epi16(a16, b16));
    h("hsub_epi16", _mm_hsub_epi16(a16, b16));
    h("hadds_epi16", _mm_hadds_epi16(a16, b16));
    h("hsubs_epi16", _mm_hsubs_epi16(a16, b16));
    let a32 = _mm_setr_epi32(i32::MAX, 1, -5, 7);
    let b32 = _mm_setr_epi32(i32::MIN, -1, 100, -100);
    h("hadd_epi32", _mm_hadd_epi32(a32, b32));
    h("hsub_epi32", _mm_hsub_epi32(a32, b32));
    println!("dot4 {}", dot4(&[1.0, 2.0, 3.0, 4.0], &[0.5, -1.0, 2.0, 0.25]));

    let fa2 = _mm256_setr_ps(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, f32::NAN, 8.0);
    let fb2 = _mm256_setr_ps(10.0, 20.0, 30.0, 40.0, -50.0, 60.0, 70.0, -0.0);
    h2("256 hadd_ps", _mm256_castps_si256(_mm256_hadd_ps(fa2, fb2)));
    h2("256 hsub_ps", _mm256_castps_si256(_mm256_hsub_ps(fa2, fb2)));
    let da2 = _mm256_setr_pd(1.0, 2.0, 3.0, 4.5);
    let db2 = _mm256_setr_pd(-10.0, 20.0, f64::NAN, 40.0);
    h2("256 hadd_pd", _mm256_castpd_si256(_mm256_hadd_pd(da2, db2)));
    h2("256 hsub_pd", _mm256_castpd_si256(_mm256_hsub_pd(da2, db2)));
    let a16x = _mm256_setr_epi16(1, -2, 32767, 1, -32768, -1, 300, 400, 9, 8, 7, 6, 5, 4, 3, 2);
    let b16x = _mm256_setr_epi16(-32768, -32768, 32767, 32767, 7, -8, 100, -100, 1, 1, 2, 2, 3, 3, -32768, 1);
    h2("256 hadd_epi16", _mm256_hadd_epi16(a16x, b16x));
    h2("256 hsub_epi16", _mm256_hsub_epi16(a16x, b16x));
    h2("256 hadds_epi16", _mm256_hadds_epi16(a16x, b16x));
    h2("256 hsubs_epi16", _mm256_hsubs_epi16(a16x, b16x));
    let a32x = _mm256_setr_epi32(i32::MAX, 1, -5, 7, 1, 2, 3, 4);
    let b32x = _mm256_setr_epi32(i32::MIN, -1, 100, -100, 50, 60, 70, 80);
    h2("256 hadd_epi32", _mm256_hadd_epi32(a32x, b32x));
    h2("256 hsub_epi32", _mm256_hsub_epi32(a32x, b32x));

    // 32 x 32 -> 64: the even lanes, unsigned / signed
    let x = _mm_setr_epi32(-1, 0x7777_7777, -2, 0x1234_5678);
    let y = _mm_setr_epi32(-1, 5, 3, -9);
    h("mul_epu32", _mm_mul_epu32(x, y));
    h("mul_epi32", _mm_mul_epi32(x, y));
    let x2 = _mm256_setr_epi32(-1, 1, -2, 2, i32::MIN, 3, i32::MAX, 4);
    let y2 = _mm256_setr_epi32(-1, 9, 3, 9, i32::MIN, 9, 2, 9);
    h2("256 mul_epu32", _mm256_mul_epu32(x2, y2));
    h2("256 mul_epi32", _mm256_mul_epi32(x2, y2));
    // (a 64-bit product sum, as big-number code uses it)
    let mut acc = _mm256_setzero_si256();
    for i in 0..100 {
        let p = _mm256_mul_epu32(_mm256_set1_epi32(i * 40503 - 7), _mm256_setr_epi32(1, 0, 2, 0, -3, 0, 4, 0));
        acc = _mm256_add_epi64(acc, p);
    }
    h2("mul_epu32 sum", acc);
}

fn main() {
    unsafe { run() }
}
