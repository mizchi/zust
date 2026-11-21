# Native SIMD (issue #30)

The IR's vectors (`Type::Vec`: 128 / 256 / 512 bits, instructions per lane shape: crates/ir's SIMD section) are
emitted natively by the Cranelift backend (the default; `crates/clif/src/simd.rs`):

- A vector is held as 128-bit Cranelift values (I8X16): one for a 128-bit vector, two for a 256-bit one. Cranelift's
  x86-64 backend has no 256-bit registers yet, so a 256-bit instruction is two SSE ones; the instructions that move
  lanes between the halves (half the lanes widened, narrowed, converted; a bitmask; lane access) are written from
  128-bit ones. A 256-bit value is two parts everywhere: parameters and results, variables, block parameters,
  loads and stores.
- Each instruction bitcasts its parts to the lane type it reads (I32X4, F32X4, ..). Cranelift's vector instructions
  have wasm's semantics (they are wasmtime's), so `core::arch::wasm32`'s intrinsics give the wasm results natively
  too (`tests/wasm.rs`'s `simd_natively_like_rustc_wasm`).
- The ISA leaves out AVX-512 (`ZUSTC_CLIF_AVX512=1` keeps it): no 512-bit registers are used, and with it Cranelift
  picks EVEX forms for some 128-bit instructions, which CPUs without AVX-512 and valgrind do not run. The other
  features are the host's (cranelift_native), as for scalar code: `_mm256_fmadd_ps` needs FMA (it is an error
  without), `f32x4_relaxed_madd` uses it when there.
- minclif has no SIMD: it rejects a function with a vector ("minclif has no SIMD yet").

## core::arch::x86_64

`__m128` / `__m128i` / `__m128d` are `Ty::Vec(128)`, the `__m256` ones `Ty::Vec(256)` (one type each: the lane type is
the intrinsic's). The intrinsics are lowered in place to IR instructions (acg/simd.rs, `x86_emit`; signatures in
sema/simd_sig.rs), with x86's semantics where they differ from the IR's (wasm's): `min` / `max` (`a < b ? a : b`),
shifts by the lane width or more, float to int conversions out of range (0x80000000), and the AVX2 instructions that
work within each 128-bit half (`_mm256_packs_epi32`). The subset, for `_mm_` (128) and `_mm256_` (256):

| kind | intrinsics |
|---|---|
| memory | `loadu` / `load` / `storeu` / `store` (`ps`, `pd`, `si128`, `si256`), `broadcast_ss` / `_sd` |
| constructors | `setzero`, `set1`, `set` / `setr` (every lane type), `set_m128` / `setr_m128` (and `i` / `d`) |
| float | `add`, `sub`, `mul`, `div`, `sqrt`, `min`, `max`, `and`, `or`, `xor`, `andnot`, `cmpeq` .. `cmpge`, `cmp::<_CMP_*>` (all 32 predicates), `blendv`, `fmadd` / `fmsub` / `fnmadd` / `fnmsub`, `movemask`, `cvtss_f32` / `cvtsd_f64` |
| integer | `add`, `sub`, `mullo`, `adds` / `subs` (`epi` / `epu`), `avg_epu8` / `16`, `min` / `max` (`epi` / `epu`), `abs`, `and` / `or` / `xor` / `andnot`, `cmpeq`, `cmpgt`, `cmplt`, `slli` / `srli` / `srai::<N>`, `blendv_epi8`, `packs` / `packus`, `madd_epi16`, `movemask_epi8`, `testz`, `extract::<N>`, `cvtsi128_si32` / `si64` |
| conversions | `cvtepi32_ps`, `cvtepi32_pd`, `cvtps_pd`, `cvtpd_ps`, `cvttps_epi32`, `cvtps_epi32`, `cvtepi8_epi16` .. `cvtepu32_epi64` |
| halves | `cast*` (between lane types, to the low half, from a half: zero-extended), `zext*`, `extractf128` / `extracti128::<N>`, `insertf128` / `inserti128::<N>` |
| shuffles by an immediate | `shuffle_ps` / `_pd` / `_epi32`, `shufflelo` / `shufflehi_epi16`, `unpacklo` / `unpackhi` (every lane type), `movehl_ps` / `movelh_ps`, `moveldup` / `movehdup_ps`, `movedup_pd`, `alignr_epi8`, `blend_ps` / `_pd` / `_epi16` / `_epi32`, `bslli` / `bsrli` / `slli_si128` / `srli_si128` (and `_epi128` / `_si256`), `permute_ps` / `_pd`, `permute2f128` / `permute2x128`, `permute4x64_epi64` / `_pd`, `broadcastss` / `sd` / `b` / `w` / `d` / `q` / `si128` |
| shuffles by a vector | `shuffle_epi8` (pshufb), `permutevar_ps` / `_pd`, `permutevar8x32_epi32` / `_ps` |

A shuffle by an immediate is a table of the elements it picks (Intel's definitions; the 256-bit forms of most work
within each 128-bit half), lowered to one 128-bit byte `Shuffle` per 128-bit half of the result, from the halves it
reads (two at a time). Cranelift lowers those masks to the instruction that does it (`pshufd`, `shufps`, `punpck*`,
`palignr`, `pblendw`, `pshuflw` / `pshufhw`; `pshufb` otherwise). A shuffle by a vector is a `Swizzle` with byte
indices computed from the lane indices (`permutevar8x32` swizzles each half and joins them).

Not yet: horizontal adds, `mul_epu32`, `sll` by a vector count, gathers, masked loads, AVX-512. `#[target_feature]` is accepted and changes
nothing (the intrinsics are always available), and an intrinsic needs `unsafe` only when it reads or writes through a
pointer.

## Tests

`tests/simd.rs` (needs a CPU with AVX2 and FMA): `x86_simd_like_rustc` checks that `sse.rs`, `avx.rs`,
`avx_usage.rs` and `shuffle.rs` (every intrinsic above on edge cases: NaN, ±0, saturation, shifts past the width,
every comparison predicate, all 256 immediates of the 4-lane shuffles, out-of-range shuffle indices; 256-bit values
through parameters, results, branches, loops, structs, `Vec`, closures; the zustscript kernels at 8 lanes with FMA; a
4x4 transpose) print what rustc's builds print, with and without -O. `minclif_rejects_simd`. The
binaries also run clean under valgrind.

The 256-bit kernels (`avx_usage.rs`'s, 1e6 f32, -O): sum of squares 0.31 ms against rustc's 0.23 ms (AVX2) and
zustc's scalar loop's 1.07 ms; saxpy 0.53 ms against 0.48 ms.
