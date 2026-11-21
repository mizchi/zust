//! `core::arch::wasm32`'s SIMD intrinsics (issue #30): `v128` is a primitive here (`Ty::V128`, one IR value of
//! `Type::V128`), and each intrinsic is lowered in place to one SIMD instruction of the IR (`VOp`), or a few. They are
//! not prelude functions: a program that names none of them pays nothing (#6), and there is no call per lane op.
//!
//! The names are rustc's (`f32x4_add`, `u8x16_shr`, `v128_load`, `i32x4_extract_lane::<N>`, the relaxed ones), and
//! so are the types: `v128` for vectors, the lane type for scalars, `u32` for shift amounts, `bool` for
//! `any_true` / `all_true`. A function of the program with the same name is called instead.

use super::*;
use crate::ir::{FloatCC, Lanes, VOp};
use crate::parse::GArg;
use crate::simd_sig::{SArg, simd_sig, split};

/// A lane-shape prefix: (lanes, unsigned)
fn lanes(p: &str) -> Option<(Lanes, bool)> {
    Some(match p {
        "i8x16" => (Lanes::I8x16, false),
        "u8x16" => (Lanes::I8x16, true),
        "i16x8" => (Lanes::I16x8, false),
        "u16x8" => (Lanes::I16x8, true),
        "i32x4" => (Lanes::I32x4, false),
        "u32x4" => (Lanes::I32x4, true),
        "i64x2" => (Lanes::I64x2, false),
        "u64x2" => (Lanes::I64x2, true),
        "f32x4" => (Lanes::F32x4, false),
        "f64x2" => (Lanes::F64x2, false),
        _ => return None,
    })
}

/// The memory access of a `v128_load8_lane` .. `v128_store64_lane` and the like: its lanes from the width in the name
fn mem_lanes(rest: &str) -> Lanes {
    let bits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
    match bits.as_str() {
        "8" => Lanes::I8x16,
        "16" => Lanes::I16x8,
        "32" => Lanes::I32x4,
        _ => Lanes::I64x2,
    }
}

/// How its result is made from the operands
enum Plan {
    /// one op
    Op(VOp),
    /// a constructor: splat lane 0, replace the others
    Build(Lanes),
    /// a load of the pointer (a scalar of this type), then an op: splat / replace into zero / widen
    LoadSplat(Lanes),
    LoadZero(Lanes),
    LoadExtend(Lanes, bool),
    /// `v128_load` / `v128_store`
    Load,
    Store,
    /// (vector, pointer): a lane from memory / to memory
    LoadLane(Lanes, u8),
    StoreLane(Lanes, u8),
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

    fn simd_lane(&mut self, seg: &crate::parse::Seg, l: Lanes, name: &str) -> R<u8> {
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
                "not" => Plan::Op(VOp::Not),
                "and" => Plan::Op(VOp::And),
                "andnot" => Plan::Op(VOp::AndNot),
                "or" => Plan::Op(VOp::Or),
                "xor" => Plan::Op(VOp::Xor),
                "bitselect" => Plan::Op(VOp::Bitselect),
                "any_true" => Plan::Op(VOp::AnyTrue),
                "load" => Plan::Load,
                "store" => Plan::Store,
                "load32_zero" | "load64_zero" => Plan::LoadZero(mem_lanes(rest)),
                _ if rest.ends_with("_splat") => Plan::LoadSplat(mem_lanes(rest)),
                _ => {
                    let l = mem_lanes(rest);
                    let k = self.simd_lane(last, l, name)?;
                    if rest.starts_with("load") { Plan::LoadLane(l, k) } else { Plan::StoreLane(l, k) }
                }
            }
        } else {
            let Some((l, u)) = lanes(pre) else { return Ok(None) };
            let cmp = |s: IntCC, uu: IntCC| if u { uu } else { s };
            // (the source shape's letter in `extend_low_u8x16` and the like: unsigned)
            let src_u = |s: &str| s.starts_with('u');
            let f = l.is_float();
            Plan::Op(match rest {
                // (a constructor: made a Build below)
                "" | "splat" => VOp::Splat(l),
                "extract_lane" => VOp::Extract(l, self.simd_lane(last, l, name)?),
                "replace_lane" => VOp::Replace(l, self.simd_lane(last, l, name)?),
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
                "swizzle" if l == Lanes::I8x16 => VOp::Swizzle,
                "relaxed_swizzle" if l == Lanes::I8x16 => VOp::RelaxedSwizzle,
                "eq" if f => VOp::Fcmp(l, FloatCC::Equal),
                "ne" if f => VOp::Fcmp(l, FloatCC::NotEqual),
                "lt" if f => VOp::Fcmp(l, FloatCC::LessThan),
                "gt" if f => VOp::Fcmp(l, FloatCC::GreaterThan),
                "le" if f => VOp::Fcmp(l, FloatCC::LessThanOrEqual),
                "ge" if f => VOp::Fcmp(l, FloatCC::GreaterThanOrEqual),
                "eq" => VOp::Icmp(l, IntCC::Equal),
                "ne" => VOp::Icmp(l, IntCC::NotEqual),
                "lt" => VOp::Icmp(l, cmp(IntCC::SignedLessThan, IntCC::UnsignedLessThan)),
                "gt" => VOp::Icmp(l, cmp(IntCC::SignedGreaterThan, IntCC::UnsignedGreaterThan)),
                "le" => VOp::Icmp(l, cmp(IntCC::SignedLessThanOrEqual, IntCC::UnsignedLessThanOrEqual)),
                "ge" => VOp::Icmp(l, cmp(IntCC::SignedGreaterThanOrEqual, IntCC::UnsignedGreaterThanOrEqual)),
                "add" => VOp::Add(l),
                "sub" => VOp::Sub(l),
                "mul" => VOp::Mul(l),
                "div" if f => VOp::Div(l),
                "neg" => VOp::Neg(l),
                "abs" => VOp::Abs(l),
                "sqrt" if f => VOp::Sqrt(l),
                "ceil" if f => VOp::Ceil(l),
                "floor" if f => VOp::Floor(l),
                "trunc" if f => VOp::Trunc(l),
                "nearest" if f => VOp::Nearest(l),
                "min" => VOp::Min(l, u),
                "max" => VOp::Max(l, u),
                "pmin" if f => VOp::Pmin(l),
                "pmax" if f => VOp::Pmax(l),
                "add_sat" => VOp::AddSat(l, u),
                "sub_sat" => VOp::SubSat(l, u),
                "avgr" => VOp::Avgr(l),
                "popcnt" if l == Lanes::I8x16 => VOp::Popcnt,
                "all_true" => VOp::AllTrue(l),
                "bitmask" => VOp::Bitmask(l),
                "shl" => VOp::Shl(l),
                "shr" => VOp::Shr(l, u),
                "q15mulr_sat" if l == Lanes::I16x8 => VOp::Q15MulrSat,
                "dot_i16x8" if l == Lanes::I32x4 => VOp::Dot,
                "narrow_i16x8" | "narrow_i32x4" => VOp::Narrow(l, u),
                "trunc_sat_f32x4" => VOp::FcvtToIntSat(Lanes::F32x4, u),
                "trunc_sat_f64x2_zero" => VOp::FcvtToIntSat(Lanes::F64x2, u),
                "convert_i32x4" | "convert_u32x4" if l == Lanes::F32x4 => VOp::FcvtFromInt(l, rest == "convert_u32x4"),
                "convert_low_i32x4" | "convert_low_u32x4" if l == Lanes::F64x2 => VOp::FcvtFromInt(l, rest == "convert_low_u32x4"),
                "demote_f64x2_zero" if l == Lanes::F32x4 => VOp::Demote,
                "promote_low_f32x4" if l == Lanes::F64x2 => VOp::Promote,
                "relaxed_trunc_f32x4" => VOp::RelaxedTrunc(Lanes::F32x4, u),
                "relaxed_trunc_f64x2_zero" => VOp::RelaxedTrunc(Lanes::F64x2, u),
                "relaxed_madd" if f => VOp::RelaxedMadd(l),
                "relaxed_nmadd" if f => VOp::RelaxedNmadd(l),
                "relaxed_min" if f => VOp::RelaxedMin(l),
                "relaxed_max" if f => VOp::RelaxedMax(l),
                "relaxed_laneselect" if !f => VOp::RelaxedLaneselect(l),
                "relaxed_q15mulr" if l == Lanes::I16x8 => VOp::RelaxedQ15Mulr,
                "relaxed_dot_i8x16_i7x16" if l == Lanes::I16x8 => VOp::RelaxedDot,
                "relaxed_dot_i8x16_i7x16_add" if l == Lanes::I32x4 => VOp::RelaxedDotAdd,
                _ => {
                    if let Some(s) = rest.strip_prefix("extend_low_") {
                        VOp::Extend(l, false, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("extend_high_") {
                        VOp::Extend(l, true, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("extmul_low_") {
                        VOp::ExtMul(l, false, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("extmul_high_") {
                        VOp::ExtMul(l, true, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("extadd_pairwise_") {
                        VOp::ExtAddPairwise(l, src_u(s))
                    } else if let Some(s) = rest.strip_prefix("load_extend_") {
                        VOp::Extend(l, false, src_u(s))
                    } else {
                        return Ok(None);
                    }
                }
            })
        };
        // (the two that are not one op: a constructor, `i16x8_load_extend_i8x8` and the like)
        let plan = match plan {
            Plan::Op(VOp::Splat(l)) if rest.is_empty() => Plan::Build(l),
            Plan::Op(VOp::Extend(l, _, u)) if rest.starts_with("load_extend_") => Plan::LoadExtend(l, u),
            p => p,
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
            Plan::Op(o @ VOp::Bitmask(l)) => {
                let r = m.simd(o, &vals);
                m.ireduce(if l == Lanes::I8x16 { Type::I16 } else { Type::I8 }, r)
            }
            Plan::Op(o) => m.simd(o, &vals),
            Plan::Build(l) => {
                let mut x = m.simd(VOp::Splat(l), &vals[..1]);
                for (k, &a) in vals.iter().enumerate().skip(1) {
                    x = m.simd(VOp::Replace(l, k as u8), &[x, a]);
                }
                x
            }
            Plan::Load => m.load(Type::V128, vals[0], 0),
            Plan::Store => {
                m.store(vals[1], vals[0], 0);
                return Ok(Some((None, Ty::Unit)));
            }
            Plan::LoadSplat(l) => {
                let s = m.load(l.lane(), vals[0], 0);
                m.simd(VOp::Splat(l), &[s])
            }
            Plan::LoadZero(l) => {
                let s = m.load(l.lane(), vals[0], 0);
                let z = m.vconst(0);
                m.simd(VOp::Replace(l, 0), &[z, s])
            }
            Plan::LoadExtend(l, u) => {
                let s = m.load(Type::I64, vals[0], 0);
                let x = m.simd(VOp::Splat(Lanes::I64x2), &[s]);
                m.simd(VOp::Extend(l, false, u), &[x])
            }
            Plan::LoadLane(l, k) => {
                let s = m.load(l.lane(), vals[1], 0);
                m.simd(VOp::Replace(l, k), &[vals[0], s])
            }
            Plan::StoreLane(l, k) => {
                let s = m.simd(VOp::Extract(l, k), &vals[..1]);
                m.store(s, vals[1], 0);
                return Ok(Some((None, Ty::Unit)));
            }
        };
        Ok(Some((Some(r), ret)))
    }
}
