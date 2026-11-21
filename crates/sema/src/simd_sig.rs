//! The signatures of `core::arch::wasm32`'s SIMD intrinsics (issue #30), for type inference (ast_infer) and for
//! acg, which lowers each call in place (acg/simd.rs). The names and types are rustc's: `v128` for vectors, the lane
//! type for scalars, `u32` for shift amounts, `bool` for `any_true` / `all_true`, raw pointers for loads and stores.

use crate::types::Ty;

/// What an intrinsic takes
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SArg {
    /// a `v128`
    V,
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
    let (pre, rest) = split(name);
    let v = |n: usize| Some((vec![V; n], Ty::V128));
    if pre == "v128" {
        return match rest {
            "not" => v(1),
            "and" | "andnot" | "or" | "xor" => v(2),
            "bitselect" => v(3),
            "any_true" => Some((vec![V], Ty::Bool)),
            "load" | "load8_splat" | "load16_splat" | "load32_splat" | "load64_splat" | "load32_zero" | "load64_zero" => Some((vec![P], Ty::V128)),
            "store" => Some((vec![P, V], Ty::Unit)),
            "load8_lane" | "load16_lane" | "load32_lane" | "load64_lane" => Some((vec![V, P], Ty::V128)),
            "store8_lane" | "store16_lane" | "store32_lane" | "store64_lane" => Some((vec![V, P], Ty::Unit)),
            _ => None,
        };
    }
    let (n, lt) = shape(pre)?;
    match rest {
        "" => Some((vec![S(lt); n as usize], Ty::V128)),
        "splat" => Some((vec![S(lt)], Ty::V128)),
        "extract_lane" => Some((vec![V], lt)),
        "replace_lane" => Some((vec![V, S(lt)], Ty::V128)),
        "all_true" => Some((vec![V], Ty::Bool)),
        "bitmask" => Some((vec![V], if n == 16 { Ty::U16 } else { Ty::U8 })),
        "shl" | "shr" => Some((vec![V, S(Ty::U32)], Ty::V128)),
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
