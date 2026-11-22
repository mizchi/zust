// core::arch::x86_64's shuffles and permutes (issue #30): by an immediate (one byte shuffle per 128-bit half of the
// result) and by the indices in a vector (pshufb, vpermilps, vpermd), at 128 and 256 bits, for every immediate of
// the 4-lane forms
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

/// a checksum of a 128-bit result (to cover all 256 immediates in few lines)
fn ck(v: __m128i) -> u64 {
    let mut o = [0u32; 4];
    unsafe { _mm_storeu_si128(o.as_mut_ptr() as *mut __m128i, v) };
    o.iter().enumerate().fold(0u64, |a, (i, &x)| a.wrapping_mul(1_000_003).wrapping_add(x as u64 * (i as u64 + 1)))
}

fn ck2(v: __m256i) -> u64 {
    let mut o = [0u32; 8];
    unsafe { _mm256_storeu_si256(o.as_mut_ptr() as *mut __m256i, v) };
    o.iter().enumerate().fold(0u64, |a, (i, &x)| a.wrapping_mul(1_000_003).wrapping_add(x as u64 * (i as u64 + 1)))
}

macro_rules! each_imm {
    ($name:expr, $f:ident, $a:expr, $b:expr, $ck:ident) => {{
        let parts = [
            seq16!($f, $a, $b, $ck, 0),
            seq16!($f, $a, $b, $ck, 16),
            seq16!($f, $a, $b, $ck, 32),
            seq16!($f, $a, $b, $ck, 48),
            seq16!($f, $a, $b, $ck, 64),
            seq16!($f, $a, $b, $ck, 80),
            seq16!($f, $a, $b, $ck, 96),
            seq16!($f, $a, $b, $ck, 112),
            seq16!($f, $a, $b, $ck, 128),
            seq16!($f, $a, $b, $ck, 144),
            seq16!($f, $a, $b, $ck, 160),
            seq16!($f, $a, $b, $ck, 176),
            seq16!($f, $a, $b, $ck, 192),
            seq16!($f, $a, $b, $ck, 208),
            seq16!($f, $a, $b, $ck, 224),
            seq16!($f, $a, $b, $ck, 240),
        ];
        println!("{} all immediates: {:?}", $name, parts);
    }};
}

// (256 immediates: rustc needs constants; a macro writes the calls out, 16 to a checksum)
macro_rules! seq16 {
    ($f:ident, $a:expr, $b:expr, $ck:ident, $base:expr) => {
        [
            $ck($f::<{ $base + 0 }>($a, $b)),
            $ck($f::<{ $base + 1 }>($a, $b)),
            $ck($f::<{ $base + 2 }>($a, $b)),
            $ck($f::<{ $base + 3 }>($a, $b)),
            $ck($f::<{ $base + 4 }>($a, $b)),
            $ck($f::<{ $base + 5 }>($a, $b)),
            $ck($f::<{ $base + 6 }>($a, $b)),
            $ck($f::<{ $base + 7 }>($a, $b)),
            $ck($f::<{ $base + 8 }>($a, $b)),
            $ck($f::<{ $base + 9 }>($a, $b)),
            $ck($f::<{ $base + 10 }>($a, $b)),
            $ck($f::<{ $base + 11 }>($a, $b)),
            $ck($f::<{ $base + 12 }>($a, $b)),
            $ck($f::<{ $base + 13 }>($a, $b)),
            $ck($f::<{ $base + 14 }>($a, $b)),
            $ck($f::<{ $base + 15 }>($a, $b)),
        ]
        .iter()
        .fold(0u64, |s, &x| s.wrapping_mul(31).wrapping_add(x))
    };
}

// (two-operand wrappers with the operands in vector registers, for the macro)
#[target_feature(enable = "avx2")]
fn shuf_ps<const I: i32>(a: __m128i, b: __m128i) -> __m128i {
    _mm_castps_si128(_mm_shuffle_ps::<I>(_mm_castsi128_ps(a), _mm_castsi128_ps(b)))
}
#[target_feature(enable = "avx2")]
fn shuf_epi32<const I: i32>(a: __m128i, _b: __m128i) -> __m128i {
    _mm_shuffle_epi32::<I>(a)
}
#[target_feature(enable = "avx2")]
fn shuflo<const I: i32>(a: __m128i, _b: __m128i) -> __m128i {
    _mm_shufflelo_epi16::<I>(a)
}
#[target_feature(enable = "avx2")]
fn shufhi<const I: i32>(a: __m128i, _b: __m128i) -> __m128i {
    _mm_shufflehi_epi16::<I>(a)
}
#[target_feature(enable = "avx2")]
fn blend16<const I: i32>(a: __m128i, b: __m128i) -> __m128i {
    _mm_blend_epi16::<I>(a, b)
}
#[target_feature(enable = "avx2")]
fn shuf_ps256<const I: i32>(a: __m256i, b: __m256i) -> __m256i {
    _mm256_castps_si256(_mm256_shuffle_ps::<I>(_mm256_castsi256_ps(a), _mm256_castsi256_ps(b)))
}
#[target_feature(enable = "avx2")]
fn perm4x64<const I: i32>(a: __m256i, _b: __m256i) -> __m256i {
    _mm256_permute4x64_epi64::<I>(a)
}
#[target_feature(enable = "avx2")]
fn perm2f128<const I: i32>(a: __m256i, b: __m256i) -> __m256i {
    _mm256_permute2x128_si256::<I>(a, b)
}
#[target_feature(enable = "avx2")]
fn blend32x8<const I: i32>(a: __m256i, b: __m256i) -> __m256i {
    _mm256_blend_epi32::<I>(a, b)
}

#[target_feature(enable = "avx2")]
fn run() {
    let a = _mm_setr_epi32(0x03020100, 0x07060504, 0x0b0a0908, 0x0f0e0d0c);
    let b = _mm_setr_epi32(0x13121110, 0x17161514, 0x1b1a1918, 0x1f1e1d1c);
    let fa = _mm_castsi128_ps(a);
    let fb = _mm_castsi128_ps(b);
    let da = _mm_castsi128_pd(a);
    let db = _mm_castsi128_pd(b);
    let a2 = _mm256_setr_epi32(0x03020100, 0x07060504, 0x0b0a0908, 0x0f0e0d0c, 0x13121110, 0x17161514, 0x1b1a1918, 0x1f1e1d1c);
    let b2 = _mm256_setr_epi32(0x23222120, 0x27262524, 0x2b2a2928, 0x2f2e2d2c, 0x33323130, 0x37363534, 0x3b3a3938, 0x3f3e3d3c);
    let fa2 = _mm256_castsi256_ps(a2);
    let fb2 = _mm256_castsi256_ps(b2);
    let da2 = _mm256_castsi256_pd(a2);
    let db2 = _mm256_castsi256_pd(b2);

    // every immediate of the 4-lane forms
    each_imm!("shuffle_ps", shuf_ps, a, b, ck);
    each_imm!("shuffle_epi32", shuf_epi32, a, b, ck);
    each_imm!("shufflelo_epi16", shuflo, a, b, ck);
    each_imm!("shufflehi_epi16", shufhi, a, b, ck);
    each_imm!("blend_epi16", blend16, a, b, ck);
    each_imm!("mm256_shuffle_ps", shuf_ps256, a2, b2, ck2);
    each_imm!("permute4x64_epi64", perm4x64, a2, b2, ck2);
    each_imm!("permute2x128", perm2f128, a2, b2, ck2);
    each_imm!("blend_epi32 256", blend32x8, a2, b2, ck2);

    // 128 bits
    h("shuffle_ps 0x1b", _mm_castps_si128(_mm_shuffle_ps::<0x1b>(fa, fb)));
    h("shuffle_pd 0", _mm_castpd_si128(_mm_shuffle_pd::<0>(da, db)));
    h("shuffle_pd 3", _mm_castpd_si128(_mm_shuffle_pd::<3>(da, db)));
    h("shuffle_pd 1", _mm_castpd_si128(_mm_shuffle_pd::<1>(da, db)));
    h("shuffle_epi32 0x4e", _mm_shuffle_epi32::<0x4e>(a));
    h("shufflelo 0x1b", _mm_shufflelo_epi16::<0x1b>(a));
    h("shufflehi 0x1b", _mm_shufflehi_epi16::<0x1b>(a));
    h("unpacklo_epi8", _mm_unpacklo_epi8(a, b));
    h("unpackhi_epi8", _mm_unpackhi_epi8(a, b));
    h("unpacklo_epi16", _mm_unpacklo_epi16(a, b));
    h("unpackhi_epi16", _mm_unpackhi_epi16(a, b));
    h("unpacklo_epi32", _mm_unpacklo_epi32(a, b));
    h("unpackhi_epi32", _mm_unpackhi_epi32(a, b));
    h("unpacklo_epi64", _mm_unpacklo_epi64(a, b));
    h("unpackhi_epi64", _mm_unpackhi_epi64(a, b));
    h("unpacklo_ps", _mm_castps_si128(_mm_unpacklo_ps(fa, fb)));
    h("unpackhi_ps", _mm_castps_si128(_mm_unpackhi_ps(fa, fb)));
    h("unpacklo_pd", _mm_castpd_si128(_mm_unpacklo_pd(da, db)));
    h("unpackhi_pd", _mm_castpd_si128(_mm_unpackhi_pd(da, db)));
    h("movehl_ps", _mm_castps_si128(_mm_movehl_ps(fa, fb)));
    h("movelh_ps", _mm_castps_si128(_mm_movelh_ps(fa, fb)));
    h("moveldup_ps", _mm_castps_si128(_mm_moveldup_ps(fa)));
    h("movehdup_ps", _mm_castps_si128(_mm_movehdup_ps(fa)));
    h("movedup_pd", _mm_castpd_si128(_mm_movedup_pd(da)));
    h("alignr 0", _mm_alignr_epi8::<0>(a, b));
    h("alignr 5", _mm_alignr_epi8::<5>(a, b));
    h("alignr 16", _mm_alignr_epi8::<16>(a, b));
    h("alignr 20", _mm_alignr_epi8::<20>(a, b));
    h("alignr 32", _mm_alignr_epi8::<32>(a, b));
    h("blend_ps 5", _mm_castps_si128(_mm_blend_ps::<5>(fa, fb)));
    h("blend_pd 2", _mm_castpd_si128(_mm_blend_pd::<2>(da, db)));
    h("blend_epi32 9", _mm_blend_epi32::<9>(a, b));
    h("bslli 3", _mm_bslli_si128::<3>(a));
    h("slli_si128 15", _mm_slli_si128::<15>(a));
    h("slli_si128 16", _mm_slli_si128::<16>(a));
    h("bsrli 3", _mm_bsrli_si128::<3>(a));
    h("srli_si128 7", _mm_srli_si128::<7>(a));
    h("permute_ps 0xe4", _mm_castps_si128(_mm_permute_ps::<0xe4>(fa)));
    h("permute_ps 0x39", _mm_castps_si128(_mm_permute_ps::<0x39>(fa)));
    h("permute_pd 1", _mm_castpd_si128(_mm_permute_pd::<1>(da)));
    let sel = _mm_setr_epi8(15, 0, 14, 1, -1, 13, 2, 12, 3, 0x80u8 as i8, 0x8fu8 as i8, 0x13, 0x7f, 16, 17, 5);
    h("shuffle_epi8", _mm_shuffle_epi8(a, sel));
    h("permutevar_ps", _mm_castps_si128(_mm_permutevar_ps(fa, _mm_setr_epi32(3, 2, 5, -1))));
    h("permutevar_pd", _mm_castpd_si128(_mm_permutevar_pd(da, _mm_set_epi64x(0, 2))));
    h("broadcastss_ps", _mm_castps_si128(_mm_broadcastss_ps(fb)));
    h("broadcastb_epi8", _mm_broadcastb_epi8(b));
    h("broadcastw_epi16", _mm_broadcastw_epi16(b));
    h("broadcastd_epi32", _mm_broadcastd_epi32(b));
    h("broadcastq_epi64", _mm_broadcastq_epi64(b));

    // 256 bits: within each 128-bit half, or across
    h2("256 shuffle_ps 0xb1", _mm256_castps_si256(_mm256_shuffle_ps::<0xb1>(fa2, fb2)));
    h2("256 shuffle_pd 5", _mm256_castpd_si256(_mm256_shuffle_pd::<5>(da2, db2)));
    h2("256 shuffle_pd 10", _mm256_castpd_si256(_mm256_shuffle_pd::<10>(da2, db2)));
    h2("256 shuffle_epi32 0x1b", _mm256_shuffle_epi32::<0x1b>(a2));
    h2("256 shufflelo 0x1b", _mm256_shufflelo_epi16::<0x1b>(a2));
    h2("256 shufflehi 0xb1", _mm256_shufflehi_epi16::<0xb1>(a2));
    h2("256 unpacklo_epi8", _mm256_unpacklo_epi8(a2, b2));
    h2("256 unpackhi_epi8", _mm256_unpackhi_epi8(a2, b2));
    h2("256 unpacklo_epi16", _mm256_unpacklo_epi16(a2, b2));
    h2("256 unpackhi_epi32", _mm256_unpackhi_epi32(a2, b2));
    h2("256 unpacklo_epi64", _mm256_unpacklo_epi64(a2, b2));
    h2("256 unpackhi_epi64", _mm256_unpackhi_epi64(a2, b2));
    h2("256 unpacklo_ps", _mm256_castps_si256(_mm256_unpacklo_ps(fa2, fb2)));
    h2("256 unpackhi_ps", _mm256_castps_si256(_mm256_unpackhi_ps(fa2, fb2)));
    h2("256 unpacklo_pd", _mm256_castpd_si256(_mm256_unpacklo_pd(da2, db2)));
    h2("256 unpackhi_pd", _mm256_castpd_si256(_mm256_unpackhi_pd(da2, db2)));
    h2("256 moveldup", _mm256_castps_si256(_mm256_moveldup_ps(fa2)));
    h2("256 movehdup", _mm256_castps_si256(_mm256_movehdup_ps(fa2)));
    h2("256 movedup_pd", _mm256_castpd_si256(_mm256_movedup_pd(da2)));
    h2("256 alignr 4", _mm256_alignr_epi8::<4>(a2, b2));
    h2("256 alignr 17", _mm256_alignr_epi8::<17>(a2, b2));
    h2("256 blend_ps 0x96", _mm256_castps_si256(_mm256_blend_ps::<0x96>(fa2, fb2)));
    h2("256 blend_pd 6", _mm256_castpd_si256(_mm256_blend_pd::<6>(da2, db2)));
    h2("256 blend_epi16 0x0f", _mm256_blend_epi16::<0x0f>(a2, b2));
    h2("256 bslli 5", _mm256_bslli_epi128::<5>(a2));
    h2("256 slli_si256 9", _mm256_slli_si256::<9>(a2));
    h2("256 bsrli 5", _mm256_bsrli_epi128::<5>(a2));
    h2("256 srli_si256 15", _mm256_srli_si256::<15>(a2));
    h2("256 permute_ps 0x1b", _mm256_castps_si256(_mm256_permute_ps::<0x1b>(fa2)));
    h2("256 permute_pd 9", _mm256_castpd_si256(_mm256_permute_pd::<9>(da2)));
    h2("256 permute2f128_ps 0x21", _mm256_castps_si256(_mm256_permute2f128_ps::<0x21>(fa2, fb2)));
    h2("256 permute2f128_pd 0x83", _mm256_castpd_si256(_mm256_permute2f128_pd::<0x83>(da2, db2)));
    h2("256 permute2f128_si256 0x30", _mm256_permute2f128_si256::<0x30>(a2, b2));
    h2("256 permute4x64_pd 0x1b", _mm256_castpd_si256(_mm256_permute4x64_pd::<0x1b>(da2)));
    let sel2 = _mm256_setr_epi8(
        15, 0, 14, 1, -1, 13, 2, 12, 3, 0x80u8 as i8, 0x8fu8 as i8, 0x13, 0x7f, 16, 17, 5, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
    );
    h2("256 shuffle_epi8", _mm256_shuffle_epi8(a2, sel2));
    h2("256 permutevar_ps", _mm256_castps_si256(_mm256_permutevar_ps(fa2, _mm256_setr_epi32(3, 2, 5, -1, 0, 1, 2, 3))));
    h2("256 permutevar_pd", _mm256_castpd_si256(_mm256_permutevar_pd(da2, _mm256_setr_epi64x(2, 0, 0, 2))));
    let idx = _mm256_setr_epi32(7, 0, 6, 1, 5, 2, 4, -5);
    h2("permutevar8x32_epi32", _mm256_permutevar8x32_epi32(a2, idx));
    h2("permutevar8x32_ps", _mm256_castps_si256(_mm256_permutevar8x32_ps(fa2, _mm256_setr_epi32(1, 1, 1, 1, 9, 10, 11, 12))));
    h2("256 broadcastss_ps", _mm256_castps_si256(_mm256_broadcastss_ps(fb)));
    h2("256 broadcastsd_pd", _mm256_castpd_si256(_mm256_broadcastsd_pd(db)));
    h2("256 broadcastb_epi8", _mm256_broadcastb_epi8(b));
    h2("256 broadcastw_epi16", _mm256_broadcastw_epi16(b));
    h2("256 broadcastd_epi32", _mm256_broadcastd_epi32(b));
    h2("256 broadcastq_epi64", _mm256_broadcastq_epi64(b));
    h2("256 broadcastsi128", _mm256_broadcastsi128_si256(b));
    // a transpose of a 4x4 block (shuffles together)
    let r0 = _mm_setr_ps(1.0, 2.0, 3.0, 4.0);
    let r1 = _mm_setr_ps(5.0, 6.0, 7.0, 8.0);
    let r2 = _mm_setr_ps(9.0, 10.0, 11.0, 12.0);
    let r3 = _mm_setr_ps(13.0, 14.0, 15.0, 16.0);
    let t0 = _mm_unpacklo_ps(r0, r1);
    let t1 = _mm_unpacklo_ps(r2, r3);
    let t2 = _mm_unpackhi_ps(r0, r1);
    let t3 = _mm_unpackhi_ps(r2, r3);
    let c0 = _mm_movelh_ps(t0, t1);
    let c1 = _mm_movehl_ps(t1, t0);
    let c2 = _mm_movelh_ps(t2, t3);
    let c3 = _mm_movehl_ps(t3, t2);
    let mut o = [0f32; 16];
    unsafe {
        _mm_storeu_ps(o.as_mut_ptr(), c0);
        _mm_storeu_ps(o.as_mut_ptr().add(4), c1);
        _mm_storeu_ps(o.as_mut_ptr().add(8), c2);
        _mm_storeu_ps(o.as_mut_ptr().add(12), c3);
    }
    println!("transpose {:?}", o);
    // a horizontal sum with shuffles
    let v = _mm256_setr_ps(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.5);
    let s = _mm_add_ps(_mm256_castps256_ps128(v), _mm256_extractf128_ps::<1>(v));
    let s = _mm_add_ps(s, _mm_movehl_ps(s, s));
    let s = _mm_add_ps(s, _mm_shuffle_ps::<1>(s, s));
    println!("hsum {}", _mm_cvtss_f32(s));
}

fn main() {
    unsafe { run() }
}
