//! `core::arch::wasm32`'s SIMD intrinsics (issue #30): `v128` is a primitive here (`Ty::Vec(128)`, one IR value of
//! `Type::Vec(B128)`), and each intrinsic is lowered in place to one SIMD instruction of the IR (`VOp` with the
//! `Shape` its name gives: `f32x4_add` is `Add` on f32 lanes of 128 bits), or a few. Wider targets' intrinsics
//! (`core::arch::x86_64`'s `_mm256_add_ps`) would be the same instructions on a 256-bit shape. They are
//! not prelude functions: a program that names none of them pays nothing (#6), and there is no call per lane op.
//!
//! The names are rustc's (`f32x4_add`, `u8x16_shr`, `v128_load`, `i32x4_extract_lane::<N>`, the relaxed ones), and
//! so are the types: `v128` for vectors, the lane type for scalars, `u32` for shift amounts, `bool` for
//! `any_true` / `all_true`. A function of the program with the same name is called instead.

use super::*;
use crate::ir::{FloatCC, Lane, Shape, VBits, VOp};
use crate::parse::GArg;
use crate::simd_sig::{SArg, X86, simd_sig, split, x86_split};

/// A lane-shape prefix: (the 128-bit shape, unsigned)
fn shape(p: &str) -> Option<(Shape, bool)> {
    let (l, u) = match p {
        "i8x16" => (Lane::I8, false),
        "u8x16" => (Lane::I8, true),
        "i16x8" => (Lane::I16, false),
        "u16x8" => (Lane::I16, true),
        "i32x4" => (Lane::I32, false),
        "u32x4" => (Lane::I32, true),
        "i64x2" => (Lane::I64, false),
        "u64x2" => (Lane::I64, true),
        "f32x4" => (Lane::F32, false),
        "f64x2" => (Lane::F64, false),
        _ => return None,
    };
    Some((Shape::new(l, VBits::B128), u))
}

/// The memory access of a `v128_load8_lane` .. `v128_store64_lane` and the like: its shape from the width in the name
fn mem_shape(rest: &str) -> Shape {
    let bits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
    match bits.as_str() {
        "8" => Shape::I8X16,
        "16" => Shape::I16X8,
        "32" => Shape::I32X4,
        _ => Shape::I64X2,
    }
}

/// How its result is made from the operands
enum Plan {
    /// one op
    Op(Shape, VOp),
    /// a constructor: splat lane 0, replace the others
    Build(Shape),
    /// a load of the pointer (a lane, or 64 bits), then an op: splat / replace into zero / widen
    LoadSplat(Shape),
    LoadZero(Shape),
    LoadExtend(Shape, bool),
    /// `v128_load` / `v128_store`
    Load,
    Store,
    /// (vector, pointer): a lane from memory / to memory
    LoadLane(Shape, u8),
    StoreLane(Shape, u8),
    /// an x86 intrinsic (its const argument, if it has one)
    X86(u32),
}

impl Cx<'_> {
    /// The lane index or shuffle index k of a call's const generic arguments (`::<3>`)
    fn simd_const(&mut self, seg: &crate::parse::Seg, k: usize, name: &str) -> R<u32> {
        // (x86's comparison predicates by name: `_mm256_cmp_ps::<_CMP_LT_OQ>`)
        let named = match seg.args.get(k) {
            Some(GArg::Const(Expr::Path(None, p))) | Some(GArg::Ty(crate::parse::Ty::Path(None, p))) if p.segs.len() == 1 => cmp_predicate(&p.segs[0].name),
            _ => None,
        };
        if let Some(n) = named {
            return Ok(n);
        }
        let ts = match seg.args.get(k) {
            Some(GArg::Const(e)) => crate::ast_types::const_arg_syn(e).or_else(|er| err(SP, er))?,
            Some(GArg::Ty(t)) => match self.parse_ty(t)? {
                Ty::Array(_, n) => return Ok(n),
                _ => return err(SP, format!("`{name}`: const argument {} is not a constant", k + 1)),
            },
            _ => return err(SP, format!("`{name}` needs {} const argument(s): `{name}::<..>(..)`", k + 1)),
        };
        match self.prog.resolve_env(&ts, &self.env).or_else(|er| err(SP, er))? {
            Ty::Array(_, n) => Ok(n),
            _ => err(SP, format!("`{name}`: const argument {} is not a constant", k + 1)),
        }
    }

    fn simd_lane(&mut self, seg: &crate::parse::Seg, l: Shape, name: &str) -> R<u8> {
        let n = self.simd_const(seg, 0, name)?;
        if n >= l.count() {
            return err(SP, format!("`{name}`: lane {n} is out of range (it has {} lanes)", l.count()));
        }
        Ok(n as u8)
    }

    /// A call of a `core::arch::wasm32` SIMD intrinsic, lowered in place (None: not one). The arguments and the
    /// result are crate::simd_sig's; the instruction is chosen here
    pub(crate) fn simd_call(&mut self, p: &Path, cargs: &[Expr]) -> R<Option<V>> {
        let last = p.segs.last().unwrap();
        let name = last.name.as_str();
        if self.prog.index.contains_key(name) {
            return Ok(None);
        }
        let Some((args, ret)) = simd_sig(name) else { return Ok(None) };
        let (pre, rest) = split(name);
        let plan = if let Some(x) = x86_split(name) {
            // (the const argument: a predicate, a lane, a shift, a half)
            let imm = if matches!(x.op, "cmp" | "extract" | "slli" | "srli" | "srai" | "extractf128" | "extracti128" | "insertf128" | "inserti128") {
                self.simd_const(last, 0, name)?
            } else {
                0
            };
            Plan::X86(imm)
        } else if pre == "v128" {
            match rest {
                "not" => Plan::Op(Shape::I8X16, VOp::Not),
                "and" => Plan::Op(Shape::I8X16, VOp::And),
                "andnot" => Plan::Op(Shape::I8X16, VOp::AndNot),
                "or" => Plan::Op(Shape::I8X16, VOp::Or),
                "xor" => Plan::Op(Shape::I8X16, VOp::Xor),
                "bitselect" => Plan::Op(Shape::I8X16, VOp::Bitselect),
                "any_true" => Plan::Op(Shape::I8X16, VOp::AnyTrue),
                "load" => Plan::Load,
                "store" => Plan::Store,
                "load32_zero" | "load64_zero" => Plan::LoadZero(mem_shape(rest)),
                _ if rest.ends_with("_splat") => Plan::LoadSplat(mem_shape(rest)),
                _ => {
                    let l = mem_shape(rest);
                    let k = self.simd_lane(last, l, name)?;
                    if rest.starts_with("load") { Plan::LoadLane(l, k) } else { Plan::StoreLane(l, k) }
                }
            }
        } else {
            let Some((l, u)) = shape(pre) else { return Ok(None) };
            let cmp = |s: IntCC, uu: IntCC| if u { uu } else { s };
            // (the source shape's letter in `extend_low_u8x16` and the like: unsigned)
            let src_u = |s: &str| s.starts_with('u');
            let f = l.is_float();
            let o = match rest {
                // (a constructor: made a Build below)
                "" | "splat" => VOp::Splat,
                "extract_lane" => VOp::Extract(self.simd_lane(last, l, name)?),
                "replace_lane" => VOp::Replace(self.simd_lane(last, l, name)?),
                "shuffle" => {
                    let (n, w) = (l.count(), 16 / l.count());
                    let mut m = 0u128;
                    for k in 0..n {
                        let i = self.simd_const(last, k as usize, name)?;
                        if i >= 2 * n {
                            return err(SP, format!("`{name}`: index {i} is out of range (the two vectors have {} lanes)", 2 * n));
                        }
                        for j in 0..w {
                            m |= ((i * w + j) as u128) << (8 * (k * w + j));
                        }
                    }
                    VOp::Shuffle(m)
                }
                "swizzle" if l.lane == Lane::I8 => VOp::Swizzle,
                "relaxed_swizzle" if l.lane == Lane::I8 => VOp::RelaxedSwizzle,
                "eq" if f => VOp::Fcmp(FloatCC::Equal),
                "ne" if f => VOp::Fcmp(FloatCC::NotEqual),
                "lt" if f => VOp::Fcmp(FloatCC::LessThan),
                "gt" if f => VOp::Fcmp(FloatCC::GreaterThan),
                "le" if f => VOp::Fcmp(FloatCC::LessThanOrEqual),
                "ge" if f => VOp::Fcmp(FloatCC::GreaterThanOrEqual),
                "eq" => VOp::Icmp(IntCC::Equal),
                "ne" => VOp::Icmp(IntCC::NotEqual),
                "lt" => VOp::Icmp(cmp(IntCC::SignedLessThan, IntCC::UnsignedLessThan)),
                "gt" => VOp::Icmp(cmp(IntCC::SignedGreaterThan, IntCC::UnsignedGreaterThan)),
                "le" => VOp::Icmp(cmp(IntCC::SignedLessThanOrEqual, IntCC::UnsignedLessThanOrEqual)),
                "ge" => VOp::Icmp(cmp(IntCC::SignedGreaterThanOrEqual, IntCC::UnsignedGreaterThanOrEqual)),
                "add" => VOp::Add,
                "sub" => VOp::Sub,
                "mul" => VOp::Mul,
                "div" if f => VOp::Div,
                "neg" => VOp::Neg,
                "abs" => VOp::Abs,
                "sqrt" if f => VOp::Sqrt,
                "ceil" if f => VOp::Ceil,
                "floor" if f => VOp::Floor,
                "trunc" if f => VOp::Trunc,
                "nearest" if f => VOp::Nearest,
                "min" => VOp::Min(u),
                "max" => VOp::Max(u),
                "pmin" if f => VOp::Pmin,
                "pmax" if f => VOp::Pmax,
                "add_sat" => VOp::AddSat(u),
                "sub_sat" => VOp::SubSat(u),
                "avgr" => VOp::Avgr,
                "popcnt" if l.lane == Lane::I8 => VOp::Popcnt,
                "all_true" => VOp::AllTrue,
                "bitmask" => VOp::Bitmask,
                "shl" => VOp::Shl,
                "shr" => VOp::Shr(u),
                "q15mulr_sat" if l.lane == Lane::I16 => VOp::Q15MulrSat,
                "dot_i16x8" if l.lane == Lane::I32 => VOp::Dot,
                "narrow_i16x8" | "narrow_i32x4" => VOp::Narrow(u),
                "trunc_sat_f32x4" => VOp::FcvtToIntSat(false, u),
                "trunc_sat_f64x2_zero" => VOp::FcvtToIntSat(true, u),
                "convert_i32x4" | "convert_u32x4" if l.lane == Lane::F32 => VOp::FcvtFromInt(rest == "convert_u32x4"),
                "convert_low_i32x4" | "convert_low_u32x4" if l.lane == Lane::F64 => VOp::FcvtFromInt(rest == "convert_low_u32x4"),
                "demote_f64x2_zero" if l.lane == Lane::F32 => VOp::Demote,
                "promote_low_f32x4" if l.lane == Lane::F64 => VOp::Promote,
                "relaxed_trunc_f32x4" => VOp::RelaxedTrunc(false, u),
                "relaxed_trunc_f64x2_zero" => VOp::RelaxedTrunc(true, u),
                "relaxed_madd" if f => VOp::RelaxedMadd,
                "relaxed_nmadd" if f => VOp::RelaxedNmadd,
                "relaxed_min" if f => VOp::RelaxedMin,
                "relaxed_max" if f => VOp::RelaxedMax,
                "relaxed_laneselect" if !f => VOp::RelaxedLaneselect,
                "relaxed_q15mulr" if l.lane == Lane::I16 => VOp::RelaxedQ15Mulr,
                "relaxed_dot_i8x16_i7x16" if l.lane == Lane::I16 => VOp::RelaxedDot,
                "relaxed_dot_i8x16_i7x16_add" if l.lane == Lane::I32 => VOp::RelaxedDotAdd,
                _ => {
                    if let Some(s) = rest.strip_prefix("extend_low_") {
                        VOp::Extend(false, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("extend_high_") {
                        VOp::Extend(true, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("extmul_low_") {
                        VOp::ExtMul(false, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("extmul_high_") {
                        VOp::ExtMul(true, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("extadd_pairwise_") {
                        VOp::ExtAddPairwise(src_u(s))
                    } else if let Some(s) = rest.strip_prefix("load_extend_") {
                        VOp::Extend(false, src_u(s))
                    } else {
                        return Ok(None);
                    }
                }
            };
            // (the two that are not one op: a constructor, `i16x8_load_extend_i8x8` and the like)
            match o {
                VOp::Splat if rest.is_empty() => Plan::Build(l),
                VOp::Extend(_, u) if rest.starts_with("load_extend_") => Plan::LoadExtend(l, u),
                o => Plan::Op(l, o),
            }
        };
        if cargs.len() != args.len() {
            return err(SP, format!("`{name}` takes {} argument(s), found {}", args.len(), cargs.len()));
        }
        // (what reads or writes memory through a pointer is unsafe)
        if args.contains(&SArg::P) {
            self.check_unsafe_op(name, p.pos)?;
        }
        let mut vals: Vec<Value> = vec![];
        let mut never = false;
        for (e, a) in cargs.iter().zip(&args) {
            let (x, t) = match *a {
                SArg::V(w) => {
                    let (x, t) = self.expr(e, Some(Ty::Vec(w)))?;
                    self.same(SP, t, Ty::Vec(w))?;
                    (x, t)
                }
                SArg::S(want) => {
                    let (x, t) = self.expr(e, Some(want))?;
                    self.same(SP, t, want)?;
                    (x, t)
                }
                SArg::P => {
                    let (x, t) = self.expr(e, None)?;
                    if !matches!(t, Ty::Ptr(..) | Ty::Ref(..) | Ty::Never) || t.is_fat() {
                        return err(SP, format!("`{name}` takes a raw pointer, found `{}`", self.prog.tn(t)));
                    }
                    (x, t)
                }
            };
            match x {
                Some(x) => vals.push(x),
                None if t == Ty::Never => never = true,
                None => return err(SP, format!("`{name}`: an argument has no value")),
            }
        }
        if never {
            return Ok(Some((None, Ty::Never)));
        }
        let m = &mut self.b.m;
        let r = match plan {
            Plan::X86(imm) => {
                let x = x86_split(name).unwrap();
                return match x86_emit(m, x, imm, &vals)? {
                    Some(r) => Ok(Some((Some(r), ret))),
                    None if ret == Ty::Unit => Ok(Some((None, Ty::Unit))),
                    None => err(SP, format!("`{name}` is not supported yet")),
                };
            }
            // (a bitmask is an i32 in the IR: u16 / u8 in Rust)
            Plan::Op(l, o @ VOp::Bitmask) => {
                let r = m.simd(l, o, &vals);
                m.ireduce(if l.lane == Lane::I8 { Type::I16 } else { Type::I8 }, r)
            }
            Plan::Op(l, o) => m.simd(l, o, &vals),
            Plan::Build(l) => {
                let mut x = m.simd(l, VOp::Splat, &vals[..1]);
                for (k, &a) in vals.iter().enumerate().skip(1) {
                    x = m.simd(l, VOp::Replace(k as u8), &[x, a]);
                }
                x
            }
            Plan::Load => m.load(Type::Vec(VBits::B128), vals[0], 0),
            Plan::Store => {
                m.store(vals[1], vals[0], 0);
                return Ok(Some((None, Ty::Unit)));
            }
            Plan::LoadSplat(l) => {
                let s = m.load(l.lane.ty(), vals[0], 0);
                m.simd(l, VOp::Splat, &[s])
            }
            Plan::LoadZero(l) => {
                let s = m.load(l.lane.ty(), vals[0], 0);
                let z = m.vconst(0);
                m.simd(l, VOp::Replace(0), &[z, s])
            }
            Plan::LoadExtend(l, u) => {
                let s = m.load(Type::I64, vals[0], 0);
                let x = m.simd(Shape::I64X2, VOp::Splat, &[s]);
                m.simd(l, VOp::Extend(false, u), &[x])
            }
            Plan::LoadLane(l, k) => {
                let s = m.load(l.lane.ty(), vals[1], 0);
                m.simd(l, VOp::Replace(k), &[vals[0], s])
            }
            Plan::StoreLane(l, k) => {
                let s = m.simd(l, VOp::Extract(k), &vals[..1]);
                m.store(s, vals[1], 0);
                return Ok(Some((None, Ty::Unit)));
            }
        };
        Ok(Some((Some(r), ret)))
    }
}

/// An AVX comparison predicate's value (`_CMP_LT_OQ`)
fn cmp_predicate(n: &str) -> Option<u32> {
    const P: [&str; 32] = [
        "EQ_OQ", "LT_OS", "LE_OS", "UNORD_Q", "NEQ_UQ", "NLT_US", "NLE_US", "ORD_Q", "EQ_UQ", "NGE_US", "NGT_US", "FALSE_OQ", "NEQ_OQ", "GE_OS",
        "GT_OS", "TRUE_UQ", "EQ_OS", "LT_OQ", "LE_OQ", "UNORD_S", "NEQ_US", "NLT_UQ", "NLE_UQ", "ORD_S", "EQ_US", "NGE_UQ", "NGT_UQ", "FALSE_OS",
        "NEQ_OS", "GE_OQ", "GT_OQ", "TRUE_US",
    ];
    let k = n.strip_prefix("_CMP_")?;
    P.iter().position(|&p| p == k).map(|i| i as u32)
}

/// The lanes of an x86 suffix, as an IR lane type (unsigned)
fn x86_shape(suffix: &str, bits: u16) -> Option<(Shape, bool)> {
    let (l, u) = match suffix {
        "ps" | "ss" => (Lane::F32, false),
        "pd" | "sd" => (Lane::F64, false),
        "epi8" | "si128" | "si256" => (Lane::I8, false),
        "epu8" => (Lane::I8, true),
        "epi16" => (Lane::I16, false),
        "epu16" => (Lane::I16, true),
        "epi32" => (Lane::I32, false),
        "epu32" => (Lane::I32, true),
        "epi64" | "epi64x" => (Lane::I64, false),
        _ => return None,
    };
    Some((Shape::new(l, VBits::from_bits(bits as u32)?), u))
}

/// A vector of zeros
fn zero(m: &mut crate::ir::Recorder, w: VBits) -> Value {
    if w == VBits::B128 {
        return m.vconst(0);
    }
    let z = m.iconst(Type::I64, 0);
    m.simd(Shape::new(Lane::I64, w), VOp::Splat, &[z])
}

/// An x86 intrinsic in IR instructions (None: no value, or not supported). x86's semantics where they differ from
/// the IR's (wasm's): min / max (`a < b ? a : b`), shifts past the lane width, out-of-range float to int conversions
/// (0x80000000), and the instructions that work within each 128-bit half of a 256-bit vector (`packs`)
fn x86_emit(m: &mut crate::ir::Recorder, x: X86, imm: u32, v: &[Value]) -> R<Option<Value>> {
    let w = VBits::from_bits(x.bits as u32).unwrap();
    let Some((sh, u)) = x86_shape(x.suffix, x.bits).or(match x.suffix {
        // (suffixes of results that are not vectors: cvtss_f32, cvtsi128_si32, set_m128)
        "f32" => Some((Shape::new(Lane::F32, w), false)),
        "f64" => Some((Shape::new(Lane::F64, w), false)),
        "si32" => Some((Shape::new(Lane::I32, w), false)),
        "si64" => Some((Shape::new(Lane::I64, w), false)),
        s if s.starts_with("m128") => Some((Shape::new(Lane::I8, w), false)),
        s if s.starts_with("ps") || s.starts_with("pd") || s.starts_with("si") => Some((Shape::new(Lane::I8, w), false)),
        _ => None,
    }) else {
        return Ok(None);
    };
    let l = sh.lane;
    let f = l.is_float();
    let lb = l.bits();
    let int = Shape::new(
        match l {
            Lane::F32 => Lane::I32,
            Lane::F64 => Lane::I64,
            l => l,
        },
        w,
    );
    let half = |m: &mut crate::ir::Recorder, a: Value, h: bool| m.simd(Shape::new(Lane::I8, w), VOp::Half(h), &[a]);
    // (a 128-bit vector as the low half of a 256-bit one, the high half a copy: for conversions that read the low half)
    let both = |m: &mut crate::ir::Recorder, a: Value| m.simd(Shape::new(Lane::I8, VBits::B256), VOp::Concat, &[a, a]);
    let r = match x.op {
        "loadu" | "load" => m.load(Type::Vec(w), v[0], 0),
        "storeu" | "store" => {
            m.store(v[1], v[0], 0);
            return Ok(None);
        }
        "setzero" => zero(m, w),
        "set1" => m.simd(sh, VOp::Splat, v),
        "set" | "setr" if x.suffix.starts_with("m128") => {
            let (lo, hi) = if x.op == "set" { (v[1], v[0]) } else { (v[0], v[1]) };
            m.simd(sh, VOp::Concat, &[lo, hi])
        }
        "set" | "setr" => {
            // (`set` takes the lanes from the highest down)
            let xs: Vec<Value> = if x.op == "set" { v.iter().rev().copied().collect() } else { v.to_vec() };
            let mut r = m.simd(sh, VOp::Splat, &xs[..1]);
            for (k, &a) in xs.iter().enumerate().skip(1) {
                r = m.simd(sh, VOp::Replace(k as u8), &[r, a]);
            }
            r
        }
        "add" => m.simd(sh, VOp::Add, v),
        "sub" => m.simd(sh, VOp::Sub, v),
        "mul" | "mullo" if l != Lane::I8 => m.simd(sh, VOp::Mul, v),
        "div" if f => m.simd(sh, VOp::Div, v),
        "adds" => m.simd(sh, VOp::AddSat(u), v),
        "subs" => m.simd(sh, VOp::SubSat(u), v),
        "avg" => m.simd(sh, VOp::Avgr, v),
        // (minps: a < b ? a : b, the IR's pmin with the operands swapped; maxps: a > b ? a : b)
        "min" if f => m.simd(sh, VOp::Pmin, &[v[1], v[0]]),
        "max" if f => m.simd(sh, VOp::Pmax, &[v[1], v[0]]),
        "min" => m.simd(sh, VOp::Min(u), v),
        "max" => m.simd(sh, VOp::Max(u), v),
        "and" => m.simd(sh, VOp::And, v),
        "or" => m.simd(sh, VOp::Or, v),
        "xor" => m.simd(sh, VOp::Xor, v),
        // (!a & b)
        "andnot" => m.simd(sh, VOp::AndNot, &[v[1], v[0]]),
        "sqrt" if f => m.simd(sh, VOp::Sqrt, v),
        "abs" if !f => m.simd(sh, VOp::Abs, v),
        "cmpeq" if f => m.simd(sh, VOp::Fcmp(FloatCC::Equal), v),
        "cmpneq" if f => m.simd(sh, VOp::Fcmp(FloatCC::NotEqual), v),
        "cmplt" if f => m.simd(sh, VOp::Fcmp(FloatCC::LessThan), v),
        "cmple" if f => m.simd(sh, VOp::Fcmp(FloatCC::LessThanOrEqual), v),
        "cmpgt" if f => m.simd(sh, VOp::Fcmp(FloatCC::GreaterThan), v),
        "cmpge" if f => m.simd(sh, VOp::Fcmp(FloatCC::GreaterThanOrEqual), v),
        "cmpeq" => m.simd(sh, VOp::Icmp(IntCC::Equal), v),
        "cmpgt" => m.simd(sh, VOp::Icmp(IntCC::SignedGreaterThan), v),
        "cmplt" => m.simd(sh, VOp::Icmp(IntCC::SignedLessThan), v),
        "cmp" if f => {
            // (the predicate's low 4 bits: signalling or quiet does not change the result)
            let c = |m: &mut crate::ir::Recorder, cc: FloatCC| m.simd(sh, VOp::Fcmp(cc), v);
            let not = |m: &mut crate::ir::Recorder, a: Value| m.simd(sh, VOp::Not, &[a]);
            match imm & 15 {
                0 => c(m, FloatCC::Equal),
                1 => c(m, FloatCC::LessThan),
                2 => c(m, FloatCC::LessThanOrEqual),
                3 => c(m, FloatCC::Unordered),
                4 => c(m, FloatCC::NotEqual),
                5 => {
                    let a = c(m, FloatCC::LessThan);
                    not(m, a)
                }
                6 => {
                    let a = c(m, FloatCC::LessThanOrEqual);
                    not(m, a)
                }
                7 => {
                    let a = c(m, FloatCC::Unordered);
                    not(m, a)
                }
                8 | 12 => {
                    // (equal or unordered; 12: its negation, ordered and not equal)
                    let (a, b) = (c(m, FloatCC::Equal), c(m, FloatCC::Unordered));
                    let r = m.simd(sh, VOp::Or, &[a, b]);
                    if imm & 15 == 12 { not(m, r) } else { r }
                }
                9 => {
                    let a = c(m, FloatCC::GreaterThanOrEqual);
                    not(m, a)
                }
                10 => {
                    let a = c(m, FloatCC::GreaterThan);
                    not(m, a)
                }
                11 => zero(m, w),
                13 => c(m, FloatCC::GreaterThanOrEqual),
                14 => c(m, FloatCC::GreaterThan),
                _ => {
                    let z = zero(m, w);
                    not(m, z)
                }
            }
        }
        "movemask" => m.simd(int, VOp::Bitmask, v),
        "testz" => {
            let a = m.simd(sh, VOp::And, v);
            let t = m.simd(sh, VOp::AnyTrue, &[a]);
            let t = m.uextend(Type::I32, t);
            let one = m.iconst(Type::I32, 1);
            m.bxor(t, one)
        }
        "extract" => {
            if imm >= sh.count() {
                return err(SP, format!("lane {imm} is out of range (the vector has {} lanes)", sh.count()));
            }
            let e = m.simd(int, VOp::Extract(imm as u8), v);
            // (i8 / i16 lanes zero-extended to i32, as pextrb / pextrw)
            if lb < 32 { m.uextend(Type::I32, e) } else { e }
        }
        "cvtss" | "cvtsd" | "cvtsi128" | "cvtsi256" => m.simd(sh, VOp::Extract(0), v),
        "slli" | "srli" | "srai" if !f => {
            // (a shift by the lane width or more: 0, or every bit the sign)
            if imm >= lb && x.op != "srai" {
                zero(m, w)
            } else {
                let k = m.iconst(Type::I32, imm.min(lb - 1) as i64);
                let o = match x.op {
                    "slli" => VOp::Shl,
                    "srli" => VOp::Shr(true),
                    _ => VOp::Shr(false),
                };
                m.simd(sh, o, &[v[0], k])
            }
        }
        "blendv" => {
            // (b's lanes where the mask's top bit is set)
            let k = m.iconst(Type::I32, (lb - 1) as i64);
            let mask = m.simd(int, VOp::Shr(false), &[v[2], k]);
            m.simd(int, VOp::Bitselect, &[v[1], v[0], mask])
        }
        "fmadd" | "fmsub" | "fnmadd" | "fnmsub" if f => {
            let (mut a, b, mut c) = (v[0], v[1], v[2]);
            if x.op.starts_with("fn") {
                a = m.simd(sh, VOp::Neg, &[a]);
            }
            if x.op.ends_with("sub") {
                c = m.simd(sh, VOp::Neg, &[c]);
            }
            m.simd(sh, VOp::Fma, &[a, b, c])
        }
        "broadcast" => {
            let s = m.load(l.ty(), v[0], 0);
            m.simd(sh, VOp::Splat, &[s])
        }
        op if op.starts_with("cast") || op.starts_with("zext") => {
            let src = crate::simd_sig::x86_width(op).unwrap_or(x.bits);
            let dst = crate::simd_sig::x86_width(x.suffix).unwrap_or(x.bits);
            match src.cmp(&dst) {
                std::cmp::Ordering::Equal => v[0],
                // (to the low half)
                std::cmp::Ordering::Greater => m.simd(Shape::new(Lane::I8, VBits::from_bits(src as u32).unwrap()), VOp::Half(false), v),
                // (from a half: the rest 0, as `_mm256_zextps128_ps256`; `cast`'s is undefined)
                std::cmp::Ordering::Less => {
                    let z = zero(m, VBits::from_bits(src as u32).unwrap());
                    m.simd(Shape::new(Lane::I8, VBits::from_bits(dst as u32).unwrap()), VOp::Concat, &[v[0], z])
                }
            }
        }
        "extractf128" | "extracti128" => half(m, v[0], imm & 1 == 1),
        "insertf128" | "inserti128" => {
            let s = Shape::new(Lane::I8, w);
            if imm & 1 == 1 {
                let lo = half(m, v[0], false);
                m.simd(s, VOp::Concat, &[lo, v[1]])
            } else {
                let hi = half(m, v[0], true);
                m.simd(s, VOp::Concat, &[v[1], hi])
            }
        }
        "cvtepi32" if x.suffix == "ps" => m.simd(sh, VOp::FcvtFromInt(false), v),
        "cvtepi32" | "cvtps" if x.suffix == "pd" => {
            let a = if w == VBits::B256 { both(m, v[0]) } else { v[0] };
            let o = if x.op == "cvtps" { VOp::Promote } else { VOp::FcvtFromInt(false) };
            m.simd(sh, o, &[a])
        }
        "cvtpd" if x.suffix == "ps" => {
            let d = m.simd(Shape::new(Lane::F32, w), VOp::Demote, v);
            if w == VBits::B256 { half(m, d, false) } else { d }
        }
        "cvttps" | "cvtps" if x.suffix == "epi32" => {
            // (x86: a NaN or a value out of range is 0x80000000; rounding is to the nearest for cvtps)
            let fs = Shape::new(Lane::F32, w);
            let a = if x.op == "cvtps" { m.simd(fs, VOp::Nearest, v) } else { v[0] };
            let r = m.simd(sh, VOp::FcvtToIntSat(false, false), &[a]);
            let nan = m.simd(fs, VOp::Fcmp(FloatCC::NotEqual), &[a, a]);
            let big = m.f32const(2147483648.0);
            let big = m.simd(fs, VOp::Splat, &[big]);
            let over = m.simd(fs, VOp::Fcmp(FloatCC::GreaterThanOrEqual), &[a, big]);
            let bad = m.simd(sh, VOp::Or, &[nan, over]);
            let k = m.iconst(Type::I32, i32::MIN as u32 as i64);
            let k = m.simd(sh, VOp::Splat, &[k]);
            m.simd(sh, VOp::Bitselect, &[k, r, bad])
        }
        "cvtepi8" | "cvtepu8" | "cvtepi16" | "cvtepu16" | "cvtepi32" | "cvtepu32" if !f => {
            // (only to lanes twice as wide: pmovsxbw, pmovzxwd, ..)
            let from: u32 = x.op[6..].parse().unwrap_or(0);
            if from * 2 != lb {
                return Ok(None);
            }
            let a = if w == VBits::B256 { both(m, v[0]) } else { v[0] };
            m.simd(sh, VOp::Extend(false, x.op.starts_with("cvtepu")), &[a])
        }
        "packs" | "packus" => {
            // (to lanes half as wide: epi16 -> i8, epi32 -> i16; on 256 bits within each 128-bit half)
            let nl = match l {
                Lane::I16 => Lane::I8,
                Lane::I32 => Lane::I16,
                _ => return Ok(None),
            };
            let o = VOp::Narrow(x.op == "packus");
            if w == VBits::B128 {
                m.simd(Shape::new(nl, w), o, v)
            } else {
                let (al, bl, ah, bh) = (half(m, v[0], false), half(m, v[1], false), half(m, v[0], true), half(m, v[1], true));
                let lo = m.simd(Shape::new(nl, VBits::B128), o, &[al, bl]);
                let hi = m.simd(Shape::new(nl, VBits::B128), o, &[ah, bh]);
                m.simd(Shape::new(nl, w), VOp::Concat, &[lo, hi])
            }
        }
        "madd" if l == Lane::I16 => m.simd(Shape::new(Lane::I32, w), VOp::Dot, v),
        _ => return Ok(None),
    };
    Ok(Some(r))
}
