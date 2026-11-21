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
use crate::simd_sig::{SArg, simd_sig, split};

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
}

impl Cx<'_> {
    /// The lane index or shuffle index k of a call's const generic arguments (`::<3>`)
    fn simd_const(&mut self, seg: &crate::parse::Seg, k: usize, name: &str) -> R<u32> {
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
        let plan = if pre == "v128" {
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
        if matches!(plan, Plan::Load | Plan::Store | Plan::LoadSplat(_) | Plan::LoadZero(_) | Plan::LoadExtend(..) | Plan::LoadLane(..) | Plan::StoreLane(..)) {
            self.check_unsafe_op(name, p.pos)?;
        }
        let mut vals: Vec<Value> = vec![];
        let mut never = false;
        for (e, a) in cargs.iter().zip(&args) {
            let (x, t) = match *a {
                SArg::V => {
                    let (x, t) = self.expr(e, Some(Ty::V128))?;
                    self.same(SP, t, Ty::V128)?;
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
