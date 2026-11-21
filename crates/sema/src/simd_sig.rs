//! The signatures of the SIMD intrinsics (issue #30), for type inference (ast_infer) and for acg, which lowers each
//! call in place (acg/simd.rs): `core::arch::wasm32`'s, and a subset of `core::arch::x86_64`'s (SSE .. SSE4.1, AVX,
//! AVX2, FMA). The names and types are rustc's: `v128` / `__m128` / `__m256` (and the `i` / `d` forms) for vectors,
//! the lane type for scalars, `u32` for wasm's shift amounts, `bool` for `any_true` / `all_true`, raw pointers for
//! loads and stores.

use crate::types::Ty;

/// What an intrinsic takes
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SArg {
    /// a vector of this many bits (`v128`, `__m256`)
    V(u16),
    /// a scalar of this type
    S(Ty),
    /// a raw pointer (any pointee)
    P,
}

/// A lane-shape prefix: (lane count, the Rust type of a lane)
pub fn shape(p: &str) -> Option<(u32, Ty)> {
    Some(match p {
        "i8x16" => (16, Ty::I8),
        "u8x16" => (16, Ty::U8),
        "i16x8" => (8, Ty::I16),
        "u16x8" => (8, Ty::U16),
        "i32x4" => (4, Ty::I32),
        "u32x4" => (4, Ty::U32),
        "i64x2" => (2, Ty::I64),
        "u64x2" => (2, Ty::U64),
        "f32x4" => (4, Ty::F32),
        "f64x2" => (2, Ty::F64),
        _ => return None,
    })
}

/// Whether a name may be a SIMD intrinsic (a cheap test before [`simd_sig`])
pub fn maybe_simd(name: &str) -> bool {
    let b = name.as_bytes();
    if b.len() > 4 && b[0] == b'_' && name.starts_with("_mm") {
        return true;
    }
    b.len() >= 4
        && matches!(b[0], b'i' | b'u' | b'f' | b'v')
        && b[1].is_ascii_digit()
        && (name.starts_with("v128") || shape(&name[..name.find('_').unwrap_or(name.len())]).is_some())
}

/// The (prefix, rest) of a name: `f32x4_add` → (`f32x4`, `add`), `f32x4` → (`f32x4`, ``)
pub fn split(name: &str) -> (&str, &str) {
    match name.find('_') {
        Some(i) => (&name[..i], &name[i + 1..]),
        None => (name, ""),
    }
}

/// An intrinsic's (arguments, result), or None if it is not one. (Which instruction it is, acg decides: a name this
/// accepts but acg does not know is an ordinary call, an error.) A name the program defines is not an intrinsic: the
/// caller checks that
pub fn simd_sig(name: &str) -> Option<(Vec<SArg>, Ty)> {
    use SArg::*;
    if !maybe_simd(name) {
        return None;
    }
    if let Some(x) = x86_split(name) {
        return x86_sig(x);
    }
    let (pre, rest) = split(name);
    let v = |n: usize| Some((vec![V(128); n], Ty::V128));
    if pre == "v128" {
        return match rest {
            "not" => v(1),
            "and" | "andnot" | "or" | "xor" => v(2),
            "bitselect" => v(3),
            "any_true" => Some((vec![V(128)], Ty::Bool)),
            "load" | "load8_splat" | "load16_splat" | "load32_splat" | "load64_splat" | "load32_zero" | "load64_zero" => Some((vec![P], Ty::V128)),
            "store" => Some((vec![P, V(128)], Ty::Unit)),
            "load8_lane" | "load16_lane" | "load32_lane" | "load64_lane" => Some((vec![V(128), P], Ty::V128)),
            "store8_lane" | "store16_lane" | "store32_lane" | "store64_lane" => Some((vec![V(128), P], Ty::Unit)),
            _ => None,
        };
    }
    let (n, lt) = shape(pre)?;
    match rest {
        "" => Some((vec![S(lt); n as usize], Ty::V128)),
        "splat" => Some((vec![S(lt)], Ty::V128)),
        "extract_lane" => Some((vec![V(128)], lt)),
        "replace_lane" => Some((vec![V(128), S(lt)], Ty::V128)),
        "all_true" => Some((vec![V(128)], Ty::Bool)),
        "bitmask" => Some((vec![V(128)], if n == 16 { Ty::U16 } else { Ty::U8 })),
        "shl" | "shr" => Some((vec![V(128), S(Ty::U32)], Ty::V128)),
        "neg" | "abs" | "sqrt" | "ceil" | "floor" | "trunc" | "nearest" | "popcnt" | "trunc_sat_f32x4" | "trunc_sat_f64x2_zero" | "convert_i32x4"
        | "convert_u32x4" | "convert_low_i32x4" | "convert_low_u32x4" | "demote_f64x2_zero" | "promote_low_f32x4" | "relaxed_trunc_f32x4"
        | "relaxed_trunc_f64x2_zero" => v(1),
        "relaxed_madd" | "relaxed_nmadd" | "relaxed_laneselect" | "relaxed_dot_i8x16_i7x16_add" => v(3),
        _ if rest.starts_with("extend_") || rest.starts_with("extadd_pairwise_") => v(1),
        _ if rest.starts_with("load_extend_") => Some((vec![P], Ty::V128)),
        // (the binary ops: add, eq, min, shuffle, narrow_i16x8, extmul_low_i8x16, relaxed_min, ..)
        _ => v(2),
    }
}

// --- core::arch::x86_64 ------------------------------------------------------------------------------------------

/// An x86 intrinsic's name in parts: `_mm256_add_ps` is (256, `add`, `ps`), `_mm_cvtepi32_pd` (128, `cvtepi32`, `pd`)
#[derive(Clone, Copy, Debug)]
pub struct X86<'a> {
    pub bits: u16,
    pub op: &'a str,
    pub suffix: &'a str,
}

pub fn x86_split(name: &str) -> Option<X86<'_>> {
    let (bits, rest) = if let Some(r) = name.strip_prefix("_mm256_") {
        (256, r)
    } else {
        (128, name.strip_prefix("_mm_")?)
    };
    let i = rest.rfind('_')?;
    Some(X86 { bits, op: &rest[..i], suffix: &rest[i + 1..] })
}

/// The lanes a suffix names: (lane type, unsigned). `si128` / `si256` (bits, any lanes): i8
pub fn x86_lane(suffix: &str) -> Option<(Ty, bool)> {
    Some(match suffix {
        "ps" | "ss" => (Ty::F32, false),
        "pd" | "sd" => (Ty::F64, false),
        "epi8" => (Ty::I8, false),
        "epu8" => (Ty::U8, true),
        "epi16" => (Ty::I16, false),
        "epu16" => (Ty::U16, true),
        "epi32" => (Ty::I32, false),
        "epu32" => (Ty::U32, true),
        "epi64" | "epi64x" => (Ty::I64, false),
        "si128" | "si256" => (Ty::I8, false),
        _ => return None,
    })
}

/// The width a cast's part names, if it names one (`castps256` / `si128` / `m128`)
pub fn x86_width(part: &str) -> Option<u16> {
    if part.ends_with("256") {
        Some(256)
    } else if part.ends_with("128") || part.starts_with("m128") {
        Some(128)
    } else {
        None
    }
}

fn x86_sig(x: X86) -> Option<(Vec<SArg>, Ty)> {
    use SArg::*;
    let (lt, _) = x86_lane(x.suffix).unwrap_or((Ty::I8, false));
    let lanes = x.bits as u32 / lt.bits();
    let lane_suffix = x86_lane(x.suffix).is_some();
    // (n vector operands, then the result vector)
    let vs = |n: usize| -> Option<(Vec<SArg>, Ty)> { Some(((0..n).map(|k| V(x86_arg_width(x, k))).collect(), Ty::Vec(x86_ret_width(x)))) };
    let to = |n: usize, r: Ty| -> Option<(Vec<SArg>, Ty)> { Some(((0..n).map(|k| V(x86_arg_width(x, k))).collect(), r)) };
    match x.op {
        "loadu" | "load" if lane_suffix => Some((vec![P], Ty::Vec(x.bits))),
        "storeu" | "store" if lane_suffix => Some((vec![P, V(x.bits)], Ty::Unit)),
        "setzero" if lane_suffix => Some((vec![], Ty::Vec(x.bits))),
        "set1" if lane_suffix => Some((vec![S(lt)], Ty::Vec(x.bits))),
        "set" | "setr" if x.suffix.starts_with("m128") && x.bits == 256 => vs(2),
        "set" | "setr" if lane_suffix => Some((vec![S(lt); lanes as usize], Ty::Vec(x.bits))),
        "add" | "sub" | "mul" | "div" | "min" | "max" | "and" | "or" | "xor" | "andnot" | "adds" | "subs" | "mullo" | "avg" | "cmpeq" | "cmpgt"
        | "cmplt" | "cmpneq" | "cmple" | "cmpge" | "madd" | "packs" | "packus" | "cmp" | "insertf128" | "inserti128" => vs(2),
        "sqrt" | "abs" | "slli" | "srli" | "srai" | "extractf128" | "extracti128" => vs(1),
        // (shuffles and permutes: by an immediate, or by the indices in a vector (`shuffle_epi8`, `permutevar`))
        "shuffle" if x.suffix == "epi8" => vs(2),
        "shuffle" if matches!(x.suffix, "ps" | "pd") => vs(2),
        "shuffle" | "shufflelo" | "shufflehi" | "permute" | "permute4x64" | "moveldup" | "movehdup" | "movedup" | "bslli" | "bsrli" => vs(1),
        "unpacklo" | "unpackhi" | "movehl" | "movelh" | "alignr" | "blend" | "permute2f128" | "permute2x128" | "permutevar" | "permutevar8x32" => vs(2),
        op if op.starts_with("broadcast") && op != "broadcast" => vs(1),
        "blendv" | "fmadd" | "fmsub" | "fnmadd" | "fnmsub" => vs(3),
        "movemask" | "testz" => to(if x.op == "testz" { 2 } else { 1 }, Ty::I32),
        "extract" => to(1, if x.suffix == "epi64" { Ty::I64 } else { Ty::I32 }),
        "cvtss" if x.suffix == "f32" => to(1, Ty::F32),
        "cvtsd" if x.suffix == "f64" => to(1, Ty::F64),
        "cvtsi128" | "cvtsi256" => to(1, if x.suffix == "si64" { Ty::I64 } else { Ty::I32 }),
        "broadcast" if matches!(x.suffix, "ss" | "sd") => Some((vec![P], Ty::Vec(x.bits))),
        // (casts: between lane types of one width, or to the low half / from a half)
        op if op.starts_with("cast") || op.starts_with("zext") => vs(1),
        "cvtepi32" | "cvtps" | "cvttps" | "cvtpd" | "cvtepi8" | "cvtepu8" | "cvtepi16" | "cvtepu16" | "cvtepu32" => vs(1),
        _ => None,
    }
}

/// The width of an x86 intrinsic's vector operand k (a cast's or conversion's may be a half: `_mm256_cvtepi32_pd`
/// takes an `__m128i`)
pub fn x86_arg_width(x: X86, k: usize) -> u16 {
    let half = x.bits / 2;
    match x.op {
        op if op.starts_with("cast") || op.starts_with("zext") => x86_width(op).unwrap_or(x.bits),
        "set" | "setr" if x.suffix.starts_with("m128") => 128,
        "insertf128" | "inserti128" if k == 1 => 128,
        // (a register broadcast reads lane 0 of a 128-bit vector)
        op if op.starts_with("broadcast") && op != "broadcast" => 128,
        "cvtepi32" if x.suffix == "pd" && x.bits == 256 => half,
        "cvtps" if x.suffix == "pd" && x.bits == 256 => half,
        "cvtepi8" | "cvtepu8" | "cvtepi16" | "cvtepu16" | "cvtepu32" if x.bits == 256 => half,
        "cvtepi32" if x.suffix.starts_with("epi") && x.bits == 256 => half,
        _ => x.bits,
    }
}

/// The width of an x86 intrinsic's result vector
pub fn x86_ret_width(x: X86) -> u16 {
    match x.op {
        op if op.starts_with("cast") || op.starts_with("zext") => x86_width(x.suffix).unwrap_or(x.bits),
        "extractf128" | "extracti128" => 128,
        "cvtpd" if x.suffix == "ps" && x.bits == 256 => 128,
        _ => x.bits,
    }
}
