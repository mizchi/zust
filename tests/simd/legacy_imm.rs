// The immediate as the last argument (`#[rustc_legacy_const_generics]`), as chacha20 0.10's avx2 backend writes
// `_mm256_shuffle_epi32(*c, 0b_00_11_10_01)`: the same as `::<IMM>(..)`
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

const ROT: i32 = 0b_10_01_00_11;

fn main() {
    unsafe {
        let a = _mm_setr_epi32(1, 2, 3, 4);
        h("shuffle", _mm_shuffle_epi32(a, 0b_00_11_10_01));
        h("shuffle turbofish", _mm_shuffle_epi32::<0b_00_11_10_01>(a));
        h("slli", _mm_slli_epi32(a, 3));
        h("srli const", _mm_srli_epi32(a, { 1 }));
        let b = _mm256_setr_epi32(1, 2, 3, 4, 5, 6, 7, 8);
        h2("256 shuffle", _mm256_shuffle_epi32(b, ROT));
        h2("256 blend", _mm256_blend_epi32(b, _mm256_set1_epi32(9), 0b1010_0101));
        h2("256 permute2x128", _mm256_permute2x128_si256(b, b, 0x01));
    }
}
