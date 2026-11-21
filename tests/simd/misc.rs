// The rest of core::arch::x86_64's subset (issue #30): shifts by a count vector and by each lane's own count,
// gathers (plain and masked), masked loads and stores, mulhi / mulhrs, sad_epu8, maddubs_epi16, dp_ps / dp_pd,
// at 128 and 256 bits, on their edge cases (counts past the width, a mask that leaves out addresses past the end
// of an array, -32768 * -32768, saturation, NaN-free dot products in Intel's order)
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

// (a function's const parameter shadows the program's constant of its name; a cast in a constant wraps)
const LANE: i32 = 0;
const CAST: i32 = 4294967298u64 as i32;

#[target_feature(enable = "avx2,fma")]
fn lane<const LANE: i32>(a: __m128i) -> i32 {
    _mm_extract_epi32::<LANE>(a)
}

// (a const parameter shadows a program constant too large for a lane, in braces)
const BIG: i64 = 1 << 40;

// (a cast to an alias of an integer type wraps as to the type)
type Lane = i32;

// (a constant of an intrinsic's name is the program's: called, not lowered)
fn add(a: i32, b: i32) -> i32 {
    a * 10 + b
}
#[allow(non_upper_case_globals)]
const _mm_add_ps: fn(i32, i32) -> i32 = add;

// (a program's type named like a vector type is another type than the vector)
#[allow(non_camel_case_types)]
struct v128;

#[target_feature(enable = "avx2,fma")]
fn blane<const BIG: i32>(a: __m128i) -> i32 {
    _mm_extract_epi32::<{ BIG }>(a)
}

#[target_feature(enable = "avx2,fma")]
fn run() {
    let l = _mm_setr_epi32(10, 11, 12, 13);
    println!("const lanes {} {} {} {}", _mm_extract_epi32::<LANE>(l), lane::<1>(l), lane::<3>(l), _mm_extract_epi32::<CAST>(l));
    println!(
        "inline casts {} {} {}",
        _mm_extract_epi32::<{ ((-1i8 as i32) < 0) as i32 }>(l),
        _mm_extract_epi32::<{ 4294967298u64 as i32 }>(l),
        blane::<3>(l)
    );
    println!("alias cast {}", _mm_extract_epi32::<{ 4294967298u64 as Lane }>(l));
    let fa = _mm_setr_pd(1.0, 2.0);
    let fb = _mm_setr_pd(3.0, 4.0);
    h("blend_pd 3", _mm_castpd_si128(_mm_blend_pd::<3>(fa, fb)));
    let pa = _mm256_set1_ps(1.0);
    let pb = _mm256_set1_ps(2.0);
    h2("256 blend_ps 255", _mm256_castps_si256(_mm256_blend_ps::<255>(pa, pb)));
    println!("callable const {}", _mm_add_ps(3, 4));
    println!("type ids {}", std::any::TypeId::of::<v128>() == std::any::TypeId::of::<__m128i>());
    let a16 = _mm_setr_epi16(1, -2, 32767, -32768, 1000, -1000, 255, -256);
    let a32 = _mm_setr_epi32(1, -2, i32::MAX, i32::MIN);
    let a64 = _mm_set_epi64x(-5, 0x1234_5678_9abc_def0);
    for c in [0i64, 1, 7, 15, 16, 31, 32, 63, 64, 1000, -1] {
        let cv = _mm_set_epi64x(0x7777, c);
        h(&format!("sll16 {c}"), _mm_sll_epi16(a16, cv));
        h(&format!("srl16 {c}"), _mm_srl_epi16(a16, cv));
        h(&format!("sra16 {c}"), _mm_sra_epi16(a16, cv));
        h(&format!("sll32 {c}"), _mm_sll_epi32(a32, cv));
        h(&format!("srl32 {c}"), _mm_srl_epi32(a32, cv));
        h(&format!("sra32 {c}"), _mm_sra_epi32(a32, cv));
        h(&format!("sll64 {c}"), _mm_sll_epi64(a64, cv));
        h(&format!("srl64 {c}"), _mm_srl_epi64(a64, cv));
    }
    let b32 = _mm256_setr_epi32(1, -2, i32::MAX, i32::MIN, 5, -6, 70000, -1);
    let b16 = _mm256_setr_epi16(1, -2, 32767, -32768, 1000, -1000, 255, -256, 9, 99, 999, 9999, -9, -99, -999, -9999);
    let b64 = _mm256_setr_epi64x(-5, 0x1234_5678_9abc_def0, 1, i64::MIN);
    for c in [3i64, 33, 70] {
        let cv = _mm_set_epi64x(0, c);
        h2(&format!("256 sll16 {c}"), _mm256_sll_epi16(b16, cv));
        h2(&format!("256 sra32 {c}"), _mm256_sra_epi32(b32, cv));
        h2(&format!("256 srl64 {c}"), _mm256_srl_epi64(b64, cv));
    }
    let cnt = _mm_setr_epi32(0, 1, 31, 32);
    h("sllv32", _mm_sllv_epi32(a32, cnt));
    h("srlv32", _mm_srlv_epi32(a32, cnt));
    h("srav32", _mm_srav_epi32(a32, _mm_setr_epi32(0, 33, -1, 31)));
    h("sllv64", _mm_sllv_epi64(a64, _mm_set_epi64x(64, 4)));
    h("srlv64", _mm_srlv_epi64(a64, _mm_set_epi64x(1, 63)));
    let cnt8 = _mm256_setr_epi32(0, 1, 2, 3, 30, 31, 32, -1);
    h2("256 sllv32", _mm256_sllv_epi32(b32, cnt8));
    h2("256 srlv32", _mm256_srlv_epi32(b32, cnt8));
    h2("256 srav32", _mm256_srav_epi32(b32, cnt8));
    h2("256 sllv64", _mm256_sllv_epi64(b64, _mm256_setr_epi64x(0, 1, 63, 64)));
    h2("256 srlv64", _mm256_srlv_epi64(b64, _mm256_setr_epi64x(64, 63, 2, 1)));

    // gathers
    let ti: Vec<i32> = (0..64).map(|i| i * 1000 + 7).collect();
    let tl: Vec<i64> = (0..64).map(|i| (i as i64) << 40 | 3).collect();
    let tf: Vec<f32> = (0..64).map(|i| i as f32 * 0.5).collect();
    let td: Vec<f64> = (0..64).map(|i| i as f64 * 0.25).collect();
    unsafe {
        let i4 = _mm_setr_epi32(5, 0, 63, 17);
        let i8 = _mm256_setr_epi32(1, 2, 3, 60, 0, 33, 7, 9);
        let j2 = _mm_set_epi64x(9, 62);
        let j4 = _mm256_setr_epi64x(3, 0, 61, 12);
        h("i32gather_epi32", _mm_i32gather_epi32::<4>(ti.as_ptr(), i4));
        h2("256 i32gather_epi32", _mm256_i32gather_epi32::<4>(ti.as_ptr(), i8));
        h("i32gather_ps", _mm_castps_si128(_mm_i32gather_ps::<4>(tf.as_ptr(), i4)));
        h2("256 i32gather_ps", _mm256_castps_si256(_mm256_i32gather_ps::<4>(tf.as_ptr(), i8)));
        h("i32gather_epi64", _mm_i32gather_epi64::<8>(tl.as_ptr(), i4));
        h2("256 i32gather_epi64", _mm256_i32gather_epi64::<8>(tl.as_ptr(), i4));
        h2("256 i32gather_pd", _mm256_castpd_si256(_mm256_i32gather_pd::<8>(td.as_ptr(), i4)));
        h("i64gather_epi64", _mm_i64gather_epi64::<8>(tl.as_ptr(), j2));
        h2("256 i64gather_pd", _mm256_castpd_si256(_mm256_i64gather_pd::<8>(td.as_ptr(), j4)));
        h("i64gather_epi32", _mm_i64gather_epi32::<4>(ti.as_ptr(), j2));
        h("256 i64gather_epi32", _mm256_i64gather_epi32::<4>(ti.as_ptr(), j4));
        h("256 i64gather_ps", _mm_castps_si128(_mm256_i64gather_ps::<4>(tf.as_ptr(), j4)));
        // (scale 1 and 2: the indices are byte / half offsets)
        h("i32gather scale 1", _mm_i32gather_epi32::<1>(ti.as_ptr(), _mm_setr_epi32(0, 4, 8, 2)));
        h("i32gather scale 2", _mm_i32gather_epi32::<2>(ti.as_ptr(), _mm_setr_epi32(0, 2, 4, 1)));
        // masked: the left-out lanes keep src, and their indices (far past the table) are never read
        let src = _mm_set1_epi32(-1);
        let far = _mm_setr_epi32(5, 1 << 28, 63, -(1 << 28));
        let m4 = _mm_setr_epi32(-1, 0, i32::MIN, 1);
        h("mask_i32gather_epi32", _mm_mask_i32gather_epi32::<4>(src, ti.as_ptr(), far, m4));
        let src8 = _mm256_set1_epi32(-7);
        let far8 = _mm256_setr_epi32(1, 1 << 28, 3, 60, 1 << 29, 33, -(1 << 29), 9);
        let m8 = _mm256_setr_epi32(-1, 0, -1, -1, 0, -1, 0, -1);
        h2("256 mask_i32gather_epi32", _mm256_mask_i32gather_epi32::<4>(src8, ti.as_ptr(), far8, m8));
        h2(
            "256 mask_i32gather_ps",
            _mm256_castps_si256(_mm256_mask_i32gather_ps::<4>(_mm256_set1_ps(9.5), tf.as_ptr(), far8, _mm256_castsi256_ps(m8))),
        );
        h2(
            "256 mask_i64gather_pd",
            _mm256_castpd_si256(_mm256_mask_i64gather_pd::<8>(
                _mm256_set1_pd(-1.0),
                td.as_ptr(),
                _mm256_setr_epi64x(3, 1 << 40, 61, 12),
                _mm256_castsi256_pd(_mm256_setr_epi64x(-1, 0, -1, 0)),
            )),
        );
        h("mask_i64gather_epi32", _mm_mask_i64gather_epi32::<4>(src, ti.as_ptr(), _mm_set_epi64x(1 << 40, 7), _mm_setr_epi32(-1, 0, -1, -1)));

        // masked loads and stores: a mask that stops before the end of a short array
        let short = [1.5f32, 2.5, 3.5];
        let mk = _mm_setr_epi32(-1, -1, -1, 0);
        h("maskload_ps", _mm_castps_si128(_mm_maskload_ps(short.as_ptr(), mk)));
        let sd = [1.25f64, 2.25, 3.25];
        h2("256 maskload_pd", _mm256_castpd_si256(_mm256_maskload_pd(sd.as_ptr(), _mm256_setr_epi64x(-1, 0, -1, 0))));
        let si = [10i32, 20, 30, 40, 50];
        h2("256 maskload_epi32", _mm256_maskload_epi32(si.as_ptr(), _mm256_setr_epi32(-1, -1, 0, -1, -1, 0, 0, 0)));
        h("maskload_epi64", _mm_maskload_epi64([7i64, 8].as_ptr(), _mm_set_epi64x(-1, 0)));
        let mut out = [0f32; 5];
        _mm_maskstore_ps(out.as_mut_ptr(), _mm_setr_epi32(-1, 0, -1, 0), _mm_setr_ps(1.0, 2.0, 3.0, 4.0));
        println!("maskstore_ps {:?}", out);
        let mut out8 = [0i32; 6];
        _mm256_maskstore_epi32(out8.as_mut_ptr(), _mm256_setr_epi32(0, -1, -1, 0, -1, -1, 0, 0), _mm256_setr_epi32(1, 2, 3, 4, 5, 6, 7, 8));
        println!("256 maskstore_epi32 {:?}", out8);
        let mut outd = [0f64; 3];
        _mm256_maskstore_pd(outd.as_mut_ptr(), _mm256_setr_epi64x(-1, 0, -1, 0), _mm256_set1_pd(6.5));
        println!("256 maskstore_pd {:?}", outd);
    }

    // multiplies, sums of differences, multiply-adds, dot products
    let x16 = _mm_setr_epi16(-32768, 32767, -1, 1000, 12345, -32768, 2, -3);
    let y16 = _mm_setr_epi16(-32768, 32767, -1, -1000, 23456, 1, 3, 4);
    h("mulhi_epi16", _mm_mulhi_epi16(x16, y16));
    h("mulhi_epu16", _mm_mulhi_epu16(x16, y16));
    h("mulhrs_epi16", _mm_mulhrs_epi16(x16, y16));
    let x16b = _mm256_setr_epi16(-32768, 32767, -1, 1000, 12345, -32768, 2, -3, 1, 2, 3, 4, 5, 6, 7, -32768);
    let y16b = _mm256_setr_epi16(-32768, 32767, -1, -1000, 23456, 1, 3, 4, 8, 7, 6, 5, 4, 3, 2, -32768);
    h2("256 mulhi_epi16", _mm256_mulhi_epi16(x16b, y16b));
    h2("256 mulhi_epu16", _mm256_mulhi_epu16(x16b, y16b));
    h2("256 mulhrs_epi16", _mm256_mulhrs_epi16(x16b, y16b));
    let p8 = _mm_setr_epi8(0, 1, 2, 3, -1, -2, -3, -4, 100, -100, 50, -50, 0, 127, -128, 1);
    let q8 = _mm_setr_epi8(-1, 1, 3, 2, 0, -2, 4, 5, -100, 100, 0, 0, 127, -128, 127, -128);
    h("sad_epu8", _mm_sad_epu8(p8, q8));
    h("maddubs_epi16", _mm_maddubs_epi16(p8, q8));
    h("maddubs sat", _mm_maddubs_epi16(_mm_set1_epi8(-1), _mm_set1_epi8(127)));
    h("maddubs sat neg", _mm_maddubs_epi16(_mm_set1_epi8(-1), _mm_set1_epi8(-128)));
    let p8b = _mm256_set_m128i(q8, p8);
    let q8b = _mm256_set_m128i(p8, q8);
    h2("256 sad_epu8", _mm256_sad_epu8(p8b, q8b));
    h2("256 maddubs_epi16", _mm256_maddubs_epi16(p8b, q8b));
    let fa = _mm_setr_ps(1.5, -2.0, 1e-8, 3.0);
    let fb = _mm_setr_ps(2.0, 0.25, 1e8, -1.0);
    h("dp_ps 0xff", _mm_castps_si128(_mm_dp_ps::<0xff>(fa, fb)));
    h("dp_ps 0x71", _mm_castps_si128(_mm_dp_ps::<0x71>(fa, fb)));
    h("dp_ps 0xa6", _mm_castps_si128(_mm_dp_ps::<0xa6>(fa, fb)));
    h("dp_ps 0x0f", _mm_castps_si128(_mm_dp_ps::<0x0f>(fa, fb)));
    // (an order that rounds: (t3 + t2) + (t1 + t0))
    let ra = _mm_setr_ps(1e8, 1.0, -1e8, 1.0);
    h("dp_ps order", _mm_castps_si128(_mm_dp_ps::<0xf1>(ra, _mm_set1_ps(1.0))));
    let da = _mm_setr_pd(1.5, 1e-20);
    let db = _mm_setr_pd(1e20, 3.0);
    h("dp_pd 0x33", _mm_castpd_si128(_mm_dp_pd::<0x33>(da, db)));
    h("dp_pd 0x12", _mm_castpd_si128(_mm_dp_pd::<0x12>(da, db)));
    let fa8 = _mm256_setr_ps(1.0, 2.0, 3.0, 4.0, -1.0, 0.5, 1e8, -1e8);
    let fb8 = _mm256_setr_ps(4.0, 3.0, 2.0, 1.0, 2.0, 2.0, 1.0, 1.0);
    h2("256 dp_ps 0xf1", _mm256_castps_si256(_mm256_dp_ps::<0xf1>(fa8, fb8)));
    h2("256 dp_ps 0x5c", _mm256_castps_si256(_mm256_dp_ps::<0x5c>(fa8, fb8)));
}

fn main() {
    unsafe { run() }
}
