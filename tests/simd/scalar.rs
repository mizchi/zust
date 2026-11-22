// x86's scalar forms (`_ss` / `_sd`: lane 0 only, the other lanes a's), loads and stores of one element, `testz` of
// floats (sign bits only), the reference argument of `broadcast_ss`, `_mm_broadcastsi128_si256`, and a program's
// own constant that has the name of an AVX predicate (issue #30)
use std::arch::x86_64::*;

// (the program's own: shadows core::arch's 0x11)
const _CMP_LT_OQ: i32 = 0;

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

#[repr(C)]
struct Guarded {
    before: [f32; 3],
    one: f32,
    after: [f32; 3],
}

#[target_feature(enable = "avx2,fma")]
fn bcast(x: &f32) -> __m256 {
    // (safe in a function with AVX: it takes a reference)
    _mm256_broadcast_ss(x)
}

#[target_feature(enable = "avx2,fma")]
fn run() {
    let a = _mm_setr_ps(1.0, 2.0, 3.0, 4.0);
    let b = _mm_setr_ps(10.0, 20.0, 30.0, 40.0);
    let n = _mm_setr_ps(f32::NAN, -1.0, 9.0, 16.0);
    h("add_ss", _mm_castps_si128(_mm_add_ss(a, b)));
    h("sub_ss", _mm_castps_si128(_mm_sub_ss(a, b)));
    h("mul_ss", _mm_castps_si128(_mm_mul_ss(a, b)));
    h("div_ss", _mm_castps_si128(_mm_div_ss(a, b)));
    h("min_ss", _mm_castps_si128(_mm_min_ss(n, b)));
    h("max_ss", _mm_castps_si128(_mm_max_ss(b, n)));
    h("sqrt_ss", _mm_castps_si128(_mm_sqrt_ss(_mm_setr_ps(2.0, -1.0, 4.0, 9.0))));
    h("cmplt_ss", _mm_castps_si128(_mm_cmplt_ss(a, b)));
    h("cmpge_ss", _mm_castps_si128(_mm_cmpge_ss(a, b)));
    h("cmpneq_ss", _mm_castps_si128(_mm_cmpneq_ss(n, n)));
    h("cmp_ss", _mm_castps_si128(_mm_cmp_ss::<_CMP_GT_OQ>(b, a)));
    h("cmp braced", _mm_castps_si128(_mm_cmp_ps::<{ _CMP_GE_OQ }>(a, _mm_setr_ps(1.0, 9.0, 3.0, 0.0))));
    h("cmp own const", _mm_castps_si128(_mm_cmp_ps::<_CMP_LT_OQ>(a, _mm_setr_ps(1.0, 0.0, 3.0, 9.0))));
    h("fmadd_ss", _mm_castps_si128(_mm_fmadd_ss(a, b, n)));
    h("fnmsub_ss", _mm_castps_si128(_mm_fnmsub_ss(b, a, b)));
    h("move_ss", _mm_castps_si128(_mm_move_ss(a, b)));
    h("set_ss", _mm_castps_si128(_mm_set_ss(7.5)));
    let d = _mm_setr_pd(1.5, 2.5);
    let e = _mm_setr_pd(9.0, -4.0);
    h("add_sd", _mm_castpd_si128(_mm_add_sd(d, e)));
    h("mul_sd", _mm_castpd_si128(_mm_mul_sd(d, e)));
    h("min_sd", _mm_castpd_si128(_mm_min_sd(e, d)));
    h("sqrt_sd", _mm_castpd_si128(_mm_sqrt_sd(d, e)));
    h("cmplt_sd", _mm_castpd_si128(_mm_cmplt_sd(d, e)));
    h("move_sd", _mm_castpd_si128(_mm_move_sd(d, e)));
    h("set_sd", _mm_castpd_si128(_mm_set_sd(-0.5)));
    h("fmadd_sd", _mm_castpd_si128(_mm_fmadd_sd(d, e, d)));

    // one element read and written: its neighbours are not touched
    let mut g = Guarded { before: [1.0, 2.0, 3.0], one: 4.5, after: [5.0, 6.0, 7.0] };
    unsafe {
        h("load_ss", _mm_castps_si128(_mm_load_ss(&g.one)));
        _mm_store_ss(&mut g.one, b);
        let x = 0.25f64;
        h("load_sd", _mm_castpd_si128(_mm_load_sd(&x)));
        let mut y = [1.0f64, 2.0];
        _mm_store_sd(&mut y[1], e);
        println!("store_sd {:?}", y);
    }
    println!("store_ss {:?} {} {:?}", g.before, g.one, g.after);

    // testz of floats: the sign bits only
    let one = _mm_set1_ps(1.0);
    let neg = _mm_set1_ps(-1.0);
    println!("testz_ps {} {} {}", _mm_testz_ps(one, one), _mm_testz_ps(neg, neg), _mm_testz_ps(neg, one));
    let one8 = _mm256_set1_ps(1.0);
    let mix = _mm256_setr_ps(1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, -2.0);
    println!("256 testz_ps {} {}", _mm256_testz_ps(one8, one8), _mm256_testz_ps(mix, mix));
    println!("testz_pd {} {}", _mm_testz_pd(_mm_set1_pd(3.0), _mm_set1_pd(3.0)), _mm256_testz_pd(_mm256_set1_pd(-3.0), _mm256_set1_pd(-1.0)));
    println!("256 testz_si256 {}", _mm256_testz_si256(_mm256_castps_si256(one8), _mm256_castps_si256(one8)));

    let f = 2.75f32;
    h2("broadcast_ss ref", _mm256_castps_si256(bcast(&f)));
    h2("_mm_broadcastsi128_si256", _mm_broadcastsi128_si256(_mm_setr_epi32(1, 2, 3, 4)));
}

fn main() {
    unsafe { run() }
}
