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
    /// a raw pointer to this type (`*const T`; `*mut T` when the flag is set, for a store): `&T` / `&mut T` and
    /// `*mut T` for `*const T` coerce as in rustc
    P(Ty, bool),
    /// a reference `&T` (`_mm256_broadcast_ss(&f32)`: no unsafe needed)
    R(Ty),
}

/// The vector type names (core::arch's), whose width is the type: resolved after the program's own types, so a
/// program's `struct __m128` keeps its fields
pub fn vector_name(s: &str) -> Option<Ty> {
    Some(match s {
        "v128" | "__m128" | "__m128i" | "__m128d" => Ty::Vec(128),
        "__m256" | "__m256i" | "__m256d" => Ty::Vec(256),
        "__m512" | "__m512i" | "__m512d" => Ty::Vec(512),
        _ => return None,
    })
}

/// Whether a call's path may name an intrinsic: a bare name, or one qualified by core::arch's modules only
/// (`core::arch::x86_64::_mm_add_ps`). `Ops::_mm_add_ps` is the program's function
pub fn arch_path<'a>(qualifier: impl IntoIterator<Item = &'a str>) -> bool {
    qualifier.into_iter().all(|s| matches!(s, "core" | "std" | "arch" | "x86_64" | "x86" | "wasm32" | "wasm64"))
}

/// A pointer argument of a load (`*const T`) or a store (`*mut T`)
fn pc(t: Ty) -> SArg {
    SArg::P(t, false)
}
fn pm(t: Ty) -> SArg {
    SArg::P(t, true)
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
            "load" => Some((vec![pc(Ty::V128)], Ty::V128)),
            "store" => Some((vec![pm(Ty::V128), V(128)], Ty::Unit)),
            "load8_splat" | "load16_splat" | "load32_splat" | "load64_splat" | "load32_zero" | "load64_zero" => Some((vec![pc(mem_ty(rest))], Ty::V128)),
            "load8_lane" | "load16_lane" | "load32_lane" | "load64_lane" => Some((vec![V(128), pc(mem_ty(rest))], Ty::V128)),
            "store8_lane" | "store16_lane" | "store32_lane" | "store64_lane" => Some((vec![V(128), pm(mem_ty(rest))], Ty::Unit)),
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
        // (`i16x8_load_extend_i8x8(*const i8)`, `_u8x8(*const u8)`, ..)
        _ if rest.starts_with("load_extend_") => {
            let src = &rest["load_extend_".len()..];
            let t = match &src[..src.find('x')?] {
                "i8" => Ty::I8,
                "u8" => Ty::U8,
                "i16" => Ty::I16,
                "u16" => Ty::U16,
                "i32" => Ty::I32,
                "u32" => Ty::U32,
                _ => return None,
            };
            Some((vec![pc(t)], Ty::V128))
        }
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
        // (`_mm_broadcastsi128_si256` makes a 256-bit vector, as its `_mm256_` spelling)
        let r = name.strip_prefix("_mm_")?;
        (if r == "broadcastsi128_si256" { 256 } else { 128 }, r)
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
    if let Some(g) = x86_gather(x) {
        // (src, pointer, indices, mask / pointer, indices)
        let r = V(g.res_bits);
        let i = V(g.idx_bits);
        let et = match (x.suffix, g.elem64) {
            ("ps", _) => Ty::F32,
            ("pd", _) => Ty::F64,
            (_, false) => Ty::I32,
            _ => Ty::I64,
        };
        return Some((if g.mask { vec![r, pc(et), i, r] } else { vec![pc(et), i] }, Ty::Vec(g.res_bits)));
    }
    // (what a load or store points to: the lane type, or the vector type for `si128` / `si256`)
    let mem = if x.suffix.starts_with("si") { Ty::Vec(x.bits) } else { lt };
    let scalar = matches!(x.suffix, "ss" | "sd");
    match x.op {
        "loadu" | "load" if lane_suffix => Some((vec![pc(mem)], Ty::Vec(x.bits))),
        "maskload" if lane_suffix => Some((vec![pc(mem), V(x.bits)], Ty::Vec(x.bits))),
        "maskstore" if lane_suffix => Some((vec![pm(mem), V(x.bits), V(x.bits)], Ty::Unit)),
        // (`_mm_set_ss(a)`: lane 0, the rest 0; `_mm_move_ss(a, b)`; `_mm_sqrt_sd(a, b)`: sqrt of b's lane 0 into a's)
        "set" if scalar => Some((vec![S(lt)], Ty::Vec(x.bits))),
        "move" if scalar => vs(2),
        "sqrt" if x.suffix == "sd" => vs(2),
        // (shifts by the low 64 bits of a 128-bit count; by each lane's own count)
        "sll" | "srl" | "sra" if lane_suffix => Some((vec![V(x.bits), V(128)], Ty::Vec(x.bits))),
        "sllv" | "srlv" | "srav" | "mulhi" | "mulhrs" | "sad" | "maddubs" | "dp" => vs(2),
        "storeu" | "store" if lane_suffix => Some((vec![pm(mem), V(x.bits)], Ty::Unit)),
        "setzero" if lane_suffix => Some((vec![], Ty::Vec(x.bits))),
        "set1" if lane_suffix => Some((vec![S(lt)], Ty::Vec(x.bits))),
        "set" | "setr" if x.suffix.starts_with("m128") && x.bits == 256 => vs(2),
        "set" | "setr" if lane_suffix => Some((vec![S(lt); lanes as usize], Ty::Vec(x.bits))),
        "add" | "sub" | "mul" | "div" | "min" | "max" | "and" | "or" | "xor" | "andnot" | "adds" | "subs" | "mullo" | "avg" | "cmpeq" | "cmpgt"
        | "cmplt" | "cmpneq" | "cmple" | "cmpge" | "madd" | "packs" | "packus" | "cmp" => vs(2),
        "sqrt" | "abs" | "slli" | "srli" | "srai" => vs(1),
        // (the halves of a 256-bit vector)
        "insertf128" | "inserti128" if x.bits == 256 => vs(2),
        "extractf128" | "extracti128" if x.bits == 256 => vs(1),
        // (shuffles and permutes: by an immediate, or by the indices in a vector (`shuffle_epi8`, `permutevar`))
        "shuffle" if x.suffix == "epi8" => vs(2),
        "shuffle" if matches!(x.suffix, "ps" | "pd") => vs(2),
        "shuffle" | "shufflelo" | "shufflehi" | "permute" | "permute4x64" | "moveldup" | "movehdup" | "movedup" | "bslli" | "bsrli" => vs(1),
        "unpacklo" | "unpackhi" | "movehl" | "movelh" | "alignr" | "blend" | "permute2f128" | "permute2x128" | "permutevar" | "permutevar8x32" => vs(2),
        // (horizontal: adjacent pairs of a, then of b, per 128-bit half)
        "hadd" | "hsub" | "hadds" | "hsubs" => vs(2),
        op if op.starts_with("broadcast") && op != "broadcast" => vs(1),
        "blendv" | "fmadd" | "fmsub" | "fnmadd" | "fnmsub" => vs(3),
        "movemask" | "testz" => to(if x.op == "testz" { 2 } else { 1 }, Ty::I32),
        "extract" => to(1, if x.suffix == "epi64" { Ty::I64 } else { Ty::I32 }),
        "cvtss" if x.suffix == "f32" => to(1, Ty::F32),
        "cvtsd" if x.suffix == "f64" => to(1, Ty::F64),
        "cvtsi128" | "cvtsi256" => to(1, if x.suffix == "si64" { Ty::I64 } else { Ty::I32 }),
        "broadcast" if matches!(x.suffix, "ss" | "sd") => Some((vec![R(lt)], Ty::Vec(x.bits))),
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

/// A gather (`_mm256_i32gather_ps::<SCALE>(p, idx)`, `_mm_mask_i64gather_epi32`): its element and index widths, how
/// many elements it loads, the widths of its result and index vectors
#[derive(Clone, Copy, Debug)]
pub struct Gather {
    pub mask: bool,
    pub idx64: bool,
    pub elem64: bool,
    pub n: u32,
    pub res_bits: u16,
    pub idx_bits: u16,
}

pub fn x86_gather(x: X86) -> Option<Gather> {
    let (mask, op) = match x.op.strip_prefix("mask_") {
        Some(o) => (true, o),
        None => (false, x.op),
    };
    let idx64 = match op {
        "i32gather" => false,
        "i64gather" => true,
        _ => return None,
    };
    let elem64 = match x.suffix {
        "epi32" | "ps" => false,
        "epi64" | "pd" => true,
        _ => return None,
    };
    // (as many elements as the wider of the two lane types fits in the vector)
    let n = x.bits as u32 / if idx64 || elem64 { 64 } else { 32 };
    let res_bits = if elem64 || !idx64 { x.bits } else { 128 };
    let idx_bits = if idx64 || !elem64 { x.bits } else { 128 };
    Some(Gather { mask, idx64, elem64, n, res_bits, idx_bits })
}

/// What a wasm lane load or store points to (`v128_load16_splat`: u16)
fn mem_ty(rest: &str) -> Ty {
    let bits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
    match bits.as_str() {
        "8" => Ty::U8,
        "16" => Ty::U16,
        "32" => Ty::U32,
        _ => Ty::U64,
    }
}
