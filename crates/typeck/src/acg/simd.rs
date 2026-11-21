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
        // (a single name, written bare, as a type, or in braces: `::<_CMP_LT_OQ>`, `::<{ _CMP_LT_OQ }>`, `::<LANE>`)
        let single = |p: &Path| (p.segs.len() == 1 && p.segs[0].args.is_empty()).then(|| p.segs[0].name.clone());
        let one: Option<String> = match seg.args.get(k) {
            Some(GArg::Const(Expr::Path(None, p))) | Some(GArg::Ty(crate::parse::Ty::Path(None, p))) => single(p),
            Some(GArg::Const(Expr::Block(None, false, b))) if b.stmts.len() == 1 => match &b.stmts[0] {
                crate::parse::Stmt::Expr(Expr::Path(None, p), false) => single(p),
                _ => None,
            },
            _ => None,
        };
        // (a const parameter of the function shadows the program's constants and the predicates: resolved below)
        let one = one.filter(|n| !self.env.params.iter().any(|p| &p.0 == n));
        if let Some(n) = &one {
            if self.prog.const_index.contains_key(n) {
                // (a constant of the program's: its value, which must fit (not truncated to another lane))
                let v = self.prog.const_usize(n).or_else(|e| err(SP, e))?;
                return u32::try_from(v).or_else(|_| err(SP, format!("`{name}`: const argument {} is too large", k + 1)));
            }
            // (x86's comparison predicates by name: `_mm256_cmp_ps::<_CMP_LT_OQ>`)
            if let Some(v) = cmp_predicate(n) {
                return Ok(v);
            }
        }
        let ts = match seg.args.get(k) {
            Some(GArg::Const(e)) => crate::ast_types::const_arg_syn(e).or_else(|er| err(SP, er))?,
            Some(GArg::Ty(t)) => match self.parse_ty(t)? {
                Ty::Array(_, n) => return Ok(n),
                _ => return err(SP, format!("`{name}`: const argument {} is not a constant", k + 1)),
            },
            _ => return err(SP, format!("`{name}` needs {} const argument(s): `{name}::<..>(..)`", k + 1)),
        };
        // (a constant past u32 is an error here, not truncated: `extract_lane::<4294967297>` is not lane 1. Array
        // lengths elsewhere keep their wrapping: `[u8; 4294967296]` may be declared and never used)
        let big = match &ts {
            crate::types::TySyn::Array(_, crate::types::ArrLen::Lit(n)) => *n > u32::MAX as u64,
            crate::types::TySyn::Array(_, crate::types::ArrLen::Const(c)) => self.prog.const_usize(c).is_ok_and(|n| n > u32::MAX as u64),
            _ => false,
        };
        if big {
            return err(SP, format!("`{name}`: const argument {} is too large", k + 1));
        }
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
        // (a local of that name (a closure, a fn pointer), a path the program's own function has (`Ops::_mm_add_ps`),
        // or a program's function of that name is called instead)
        if p.segs.len() == 1 && self.lookup(name).is_some() {
            return Ok(None);
        }
        if !crate::simd_sig::arch_path(p.segs[..p.segs.len() - 1].iter().map(|s| s.name.as_str()), name) {
            return Ok(None);
        }
        if self.prog.fn_by_p(&PPath::of(p), self.self_sid).is_some() || self.prog.index.contains_key(name) {
            return Ok(None);
        }
        let Some((args, ret)) = simd_sig(name) else {
            // (an x86 intrinsic's name that zustc does not have, rather than "cannot find")
            if x86_split(name).is_some() {
                return err(SP, format!("`{name}` is not one of the core::arch::x86_64 intrinsics zustc supports (tests/simd/README.md)"));
            }
            return Ok(None);
        };
        let (pre, rest) = split(name);
        let plan = if let Some(x) = x86_split(name) {
            // (the const argument: a predicate, a lane, a shift, a half)
            let imm = if matches!(
                x.op,
                "cmp" | "extract" | "slli" | "srli" | "srai" | "extractf128" | "extracti128" | "insertf128" | "inserti128" | "shufflelo" | "shufflehi"
                    | "alignr" | "blend" | "bslli" | "bsrli" | "permute" | "permute2f128" | "permute2x128" | "permute4x64" | "dp"
            ) || (x.op == "shuffle" && x.suffix != "epi8")
                || crate::simd_sig::x86_gather(x).is_some()
            {
                self.simd_const(last, 0, name)?
            } else {
                0
            };
            // (the immediates' ranges, as rustc's static asserts: a gather's scale 1, 2, 4 or 8; a comparison
            // predicate 5 bits; a 128-bit half 1 bit; the other immediates 8 bits)
            let max = match x.op {
                "cmp" => 31,
                "extractf128" | "extracti128" | "insertf128" | "inserti128" => 1,
                "extract" => u32::MAX,
                _ => 255,
            };
            if crate::simd_sig::x86_gather(x).is_some() {
                if !matches!(imm, 1 | 2 | 4 | 8) {
                    return err(SP, format!("`{name}`: the scale must be 1, 2, 4 or 8, not {imm}"));
                }
            } else if imm > max {
                return err(SP, format!("`{name}`: the immediate {imm} is out of range (0..={max})"));
            }
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
        // (what reads or writes memory through a raw pointer is unsafe)
        if args.iter().any(|a| matches!(a, SArg::P(..))) {
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
                SArg::P(want, mutable) => {
                    // (`*const T`: a `*mut T`, `&T` or `&mut T` coerces; `*mut T`: a `*mut T` or `&mut T`)
                    let (x, t) = self.expr(e, Some(Ty::Ptr(mutable, intern(want))))?;
                    let ok = match t {
                        Ty::Never => true,
                        Ty::Ptr(m, i) | Ty::Ref(m, i) => *i == want && (m || !mutable),
                        _ => false,
                    };
                    if !ok {
                        let w = self.prog.tn(Ty::Ptr(mutable, intern(want)));
                        return err(SP, format!("mismatched types: `{name}` takes `{w}`, found `{}`", self.prog.tn(t)));
                    }
                    (x, t)
                }
                SArg::R(want) => {
                    let r = Ty::Ref(false, intern(want));
                    let (x, t) = self.expr(e, Some(r))?;
                    if !matches!(t, Ty::Never) && !matches!(t, Ty::Ref(_, i) if *i == want) {
                        return err(SP, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(r), self.prog.tn(t)));
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
    if let Some(g) = crate::simd_sig::x86_gather(x) {
        return Ok(Some(gather(m, g, imm, v)));
    }
    // (the scalar forms, `_ss` / `_sd`: lane 0 only. A load reads one element into a vector of zeros, a store writes
    // one; arithmetic and compares put lane 0 of the packed result into a's lanes)
    if matches!(x.suffix, "ss" | "sd") && x.op != "broadcast" {
        let sh = Shape::new(if x.suffix == "ss" { Lane::F32 } else { Lane::F64 }, w);
        let l = sh.lane;
        return Ok(match x.op {
            "load" | "loadu" => {
                let e = m.load(l.ty(), v[0], 0);
                let z = zero(m, w);
                Some(m.simd(sh, VOp::Replace(0), &[z, e]))
            }
            "store" | "storeu" => {
                let e = m.simd(sh, VOp::Extract(0), &[v[1]]);
                m.store(e, v[0], 0);
                None
            }
            "set" => {
                let z = zero(m, w);
                Some(m.simd(sh, VOp::Replace(0), &[z, v[0]]))
            }
            "move" => {
                let e = m.simd(sh, VOp::Extract(0), &[v[1]]);
                Some(m.simd(sh, VOp::Replace(0), &[v[0], e]))
            }
            "add" | "sub" | "mul" | "div" | "min" | "max" | "sqrt" | "cmpeq" | "cmpneq" | "cmplt" | "cmple" | "cmpgt" | "cmpge" | "cmp" | "fmadd"
            | "fmsub" | "fnmadd" | "fnmsub" => {
                let packed = X86 { suffix: if x.suffix == "ss" { "ps" } else { "pd" }, ..x };
                // (`_mm_sqrt_sd(a, b)`: the root of b's lane 0)
                let args = if x.op == "sqrt" && v.len() == 2 { &v[1..] } else { v };
                let Some(p) = x86_emit(m, packed, imm, args)? else { return Ok(None) };
                let e = m.simd(sh, VOp::Extract(0), &[p]);
                Some(m.simd(sh, VOp::Replace(0), &[v[0], e]))
            }
            _ => None,
        });
    }
    let Some((sh, u)) = x86_shape(x.suffix, x.bits).or(match x.suffix {
        // (suffixes of results that are not vectors: cvtss_f32, cvtsi128_si32, set_m128)
        "f32" => Some((Shape::new(Lane::F32, w), false)),
        "f64" => Some((Shape::new(Lane::F64, w), false)),
        "si32" => Some((Shape::new(Lane::I32, w), false)),
        "si64" => Some((Shape::new(Lane::I64, w), false)),
        s if s.starts_with("m128") => Some((Shape::new(Lane::I8, w), false)),
        s if s.starts_with("ps") || s.starts_with("pd") || s.starts_with("si") || s == "epi128" => Some((Shape::new(Lane::I8, w), false)),
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
        // (pmuludq / pmuldq: the even 32-bit lanes (the low half of each 64-bit lane) widened and multiplied: they are
        // moved to the low half, and multiplied as the IR's ExtMul does)
        "mul" if l == Lane::I32 => {
            let n = sh.count();
            let sel: Vec<Option<(usize, u32)>> = (0..n).map(|i| if i < n / 2 { Some((0, 2 * i)) } else { None }).collect();
            let sel_b: Vec<Option<(usize, u32)>> = sel.iter().map(|e| e.map(|(_, x)| (1, x))).collect();
            let ws = [w, w];
            let ea = pick(m, w, v, &ws, 4, &sel);
            let eb = pick(m, w, v, &ws, 4, &sel_b);
            m.simd(Shape::new(Lane::I64, w), VOp::ExtMul(false, u), &[ea, eb])
        }
        "mul" | "mullo" if l != Lane::I8 => m.simd(sh, VOp::Mul, v),
        // (hadd: a0 + a1, a2 + a3, .., then b's pairs, within each 128-bit half; hadds / hsubs saturate)
        "hadd" | "hsub" | "hadds" | "hsubs" if matches!(l, Lane::I16 | Lane::I32 | Lane::F32 | Lane::F64) => {
            let es = l.bits() / 8;
            let half_n = 16 / es;
            let n = sh.count();
            let pair = |p: u32| -> Vec<Option<(usize, u32)>> {
                (0..n)
                    .map(|i| {
                        let (base, k) = (i / half_n * half_n, i % half_n);
                        if k < half_n / 2 { Some((0, base + 2 * k + p)) } else { Some((1, base + 2 * (k - half_n / 2) + p)) }
                    })
                    .collect()
            };
            let ws = [w, w];
            let ev = pick(m, w, v, &ws, es, &pair(0));
            let od = pick(m, w, v, &ws, es, &pair(1));
            let o = match x.op {
                "hadd" => VOp::Add,
                "hsub" => VOp::Sub,
                "hadds" if l == Lane::I16 => VOp::AddSat(false),
                "hsubs" if l == Lane::I16 => VOp::SubSat(false),
                _ => return Ok(None),
            };
            m.simd(sh, o, &[ev, od])
        }
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
        "testz" if f => {
            // (vtestps / vtestpd: the sign bits of a & b only)
            let a = m.simd(sh, VOp::And, v);
            let b = m.simd(int, VOp::Bitmask, &[a]);
            let z = m.icmp_imm(IntCC::Equal, b, 0);
            m.uextend(Type::I32, z)
        }
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
        // (`_mm_slli_si128` is a byte shift: below)
        "slli" | "srli" | "srai" if !f && x.suffix.starts_with("epi") => {
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
        "maskload" | "maskstore" => {
            // (lane by lane, without a branch: a lane the mask leaves out reads and writes a scratch slot instead of
            // memory, so an address past the end of an array is never touched)
            let li = int_lane(l);
            let n = sh.count();
            let es = li.bits() / 8;
            let scratch = m.create_stack_slot(8, 8);
            let dummy = m.stack_addr(scratch, 0);
            let mut r = zero(m, w);
            for i in 0..n {
                let mk = m.simd(Shape::new(li, w), VOp::Extract(i as u8), &[v[1]]);
                let on = m.icmp_imm(IntCC::SignedLessThan, mk, 0);
                let a = m.iadd_imm(v[0], (i * es) as i64);
                let a = m.select(on, a, dummy);
                if x.op == "maskload" {
                    let e = m.load(l.ty(), a, 0);
                    let z = zero_scalar(m, l);
                    let e = m.select(on, e, z);
                    r = m.simd(sh, VOp::Replace(i as u8), &[r, e]);
                } else {
                    let e = m.simd(sh, VOp::Extract(i as u8), &[v[2]]);
                    m.store(e, a, 0);
                }
            }
            if x.op == "maskstore" {
                return Ok(None);
            }
            r
        }
        "sll" | "srl" | "sra" if !f => {
            // (by the low 64 bits of the count: the lane width or more gives 0, or every bit the sign)
            let c = m.simd(Shape::I64X2, VOp::Extract(0), &[v[1]]);
            let over = m.icmp_imm(IntCC::UnsignedGreaterThanOrEqual, c, lb as i64);
            let k = m.ireduce(Type::I32, c);
            if x.op == "sra" {
                let top = m.iconst(Type::I32, (lb - 1) as i64);
                let k = m.select(over, top, k);
                m.simd(sh, VOp::Shr(false), &[v[0], k])
            } else {
                let o = if x.op == "sll" { VOp::Shl } else { VOp::Shr(true) };
                let r = m.simd(sh, o, &[v[0], k]);
                let z = zero(m, w);
                m.select(over, z, r)
            }
        }
        "sllv" | "srlv" | "srav" if matches!(l, Lane::I32 | Lane::I64) => {
            // (each lane by its own count, lane by lane: no vector instruction for it in the IR)
            let mut r = v[0];
            for i in 0..sh.count() {
                let a = m.simd(sh, VOp::Extract(i as u8), &[v[0]]);
                let c = m.simd(sh, VOp::Extract(i as u8), &[v[1]]);
                let over = m.icmp_imm(IntCC::UnsignedGreaterThanOrEqual, c, lb as i64);
                let e = match x.op {
                    "srav" => {
                        let top = m.iconst(l.ty(), (lb - 1) as i64);
                        let c = m.select(over, top, c);
                        m.sshr(a, c)
                    }
                    op => {
                        let s = if op == "sllv" { m.ishl(a, c) } else { m.ushr(a, c) };
                        let z = m.iconst(l.ty(), 0);
                        m.select(over, z, s)
                    }
                };
                r = m.simd(sh, VOp::Replace(i as u8), &[r, e]);
            }
            r
        }
        "mulhi" if l == Lane::I16 => {
            // (the high 16 bits of each 32-bit product: products of the low and high halves, their odd 16-bit parts)
            let s32 = Shape::new(Lane::I32, w);
            let lo = m.simd(s32, VOp::ExtMul(false, u), v);
            let hi = m.simd(s32, VOp::ExtMul(true, u), v);
            let n = sh.count();
            let sel: Vec<Option<(usize, u32)>> = (0..n).map(|i| if i < n / 2 { Some((0, 2 * i + 1)) } else { Some((1, 2 * (i - n / 2) + 1)) }).collect();
            pick(m, w, &[lo, hi], &[w, w], 2, &sel)
        }
        "mulhrs" if l == Lane::I16 => {
            // (pmulhrsw: q15mulr, but -32768 * -32768 wraps to -32768 where q15mulr_sat saturates)
            let r = m.simd(sh, VOp::Q15MulrSat, v);
            let k = m.iconst(Type::I16, 0x8000);
            let k = m.simd(sh, VOp::Splat, &[k]);
            let ea = m.simd(sh, VOp::Icmp(IntCC::Equal), &[v[0], k]);
            let eb = m.simd(sh, VOp::Icmp(IntCC::Equal), &[v[1], k]);
            let both = m.simd(sh, VOp::And, &[ea, eb]);
            m.simd(sh, VOp::Bitselect, &[k, r, both])
        }
        "sad" if x.suffix == "epu8" => {
            // (psadbw: |a - b| of each byte, summed over each 64-bit lane)
            let mx = m.simd(sh, VOp::Max(true), v);
            let mn = m.simd(sh, VOp::Min(true), v);
            let d = m.simd(sh, VOp::Sub, &[mx, mn]);
            let d = m.simd(Shape::new(Lane::I16, w), VOp::ExtAddPairwise(true), &[d]);
            let d = m.simd(Shape::new(Lane::I32, w), VOp::ExtAddPairwise(true), &[d]);
            m.simd(Shape::new(Lane::I64, w), VOp::ExtAddPairwise(true), &[d])
        }
        "maddubs" if x.suffix == "epi16" => {
            // (pmaddubsw: a's bytes unsigned times b's signed, adjacent pairs summed with signed saturation)
            let s8 = Shape::new(Lane::I16, w);
            let mut prods = vec![];
            for h in [false, true] {
                let a = m.simd(s8, VOp::Extend(h, true), &[v[0]]);
                let b = m.simd(s8, VOp::Extend(h, false), &[v[1]]);
                let p = m.simd(s8, VOp::Mul, &[a, b]);
                prods.push(m.simd(Shape::new(Lane::I32, w), VOp::ExtAddPairwise(false), &[p]));
            }
            m.simd(s8, VOp::Narrow(false), &prods)
        }
        "dp" if f => {
            // (dpps / dppd within each 128-bit half: the products imm's high bits select, summed in Intel's order
            // ((t3 + t2) + (t1 + t0); t1 + t0), into the lanes imm's low bits select, 0 elsewhere)
            let es = l.bits() / 8;
            let per = 16 / es;
            let n = sh.count();
            let mask = |m: &mut crate::ir::Recorder, f: &dyn Fn(u32) -> bool| -> Value {
                let li = int_lane(l);
                let one = m.iconst(li.ty(), -1);
                let mut r = zero(m, w);
                for i in 0..n {
                    if f(i % per) {
                        r = m.simd(Shape::new(li, w), VOp::Replace(i as u8), &[r, one]);
                    }
                }
                r
            };
            let lanes = |f: &dyn Fn(u32) -> u32| -> Vec<Option<(usize, u32)>> { (0..n).map(|i| Some((0, i / per * per + f(i % per)))).collect() };
            let t = m.simd(sh, VOp::Mul, v);
            let pm = mask(m, &|j| (imm >> (4 + j)) & 1 == 1);
            let t = m.simd(sh, VOp::And, &[t, pm]);
            let sum = if per == 4 {
                let x1 = pick(m, w, &[t], &[w], es, &lanes(&|k| [3, 1, 3, 1][k as usize]));
                let y1 = pick(m, w, &[t], &[w], es, &lanes(&|k| [2, 0, 2, 0][k as usize]));
                let s1 = m.simd(sh, VOp::Add, &[x1, y1]);
                let s2 = pick(m, w, &[s1], &[w], es, &lanes(&|_| 1));
                m.simd(sh, VOp::Add, &[s1, s2])
            } else {
                let x1 = pick(m, w, &[t], &[w], es, &lanes(&|_| 1));
                m.simd(sh, VOp::Add, &[x1, t])
            };
            let b = pick(m, w, &[sum], &[w], es, &lanes(&|_| 0));
            let dm = mask(m, &|j| (imm >> j) & 1 == 1);
            m.simd(sh, VOp::And, &[b, dm])
        }
        // (pshufb: a byte's index is its low 4 bits, or 0 when its top bit is set; within each 128-bit half)
        "shuffle" if x.suffix == "epi8" => {
            let k = m.iconst(Type::I8, 0x8F);
            let k = m.simd(Shape::new(Lane::I8, w), VOp::Splat, &[k]);
            let idx = m.simd(Shape::new(Lane::I8, w), VOp::And, &[v[1], k]);
            per_half(m, w, &[v[0], idx], |m, h| m.simd(Shape::I8X16, VOp::Swizzle, h))
        }
        // (vpermilps / vpermilpd by a vector: within each 128-bit half)
        "permutevar" if f => {
            let idx = lane_bytes(m, w, l, v[1], if l == Lane::F32 { 3 } else { 2 }, 0);
            per_half(m, w, &[v[0], idx], |m, h| m.simd(Shape::I8X16, VOp::Swizzle, h))
        }
        // (vpermd / vpermps: any of the 8 lanes)
        "permutevar8x32" if w == VBits::B256 => {
            let idx = lane_bytes(m, w, Lane::I32, v[1], 7, 0);
            let (alo, ahi) = (half(m, v[0], false), half(m, v[0], true));
            let k16 = m.iconst(Type::I8, 16);
            let k16 = m.simd(Shape::I8X16, VOp::Splat, &[k16]);
            let mut hs = vec![];
            for h in [false, true] {
                // (indices 0..15 from the low half, 16..31 from the high one: swizzle gives 0 past 15)
                let i = half(m, idx, h);
                let lo = m.simd(Shape::I8X16, VOp::Swizzle, &[alo, i]);
                let i2 = m.simd(Shape::I8X16, VOp::Sub, &[i, k16]);
                let hi = m.simd(Shape::I8X16, VOp::Swizzle, &[ahi, i2]);
                hs.push(m.simd(Shape::I8X16, VOp::Or, &[lo, hi]));
            }
            m.simd(Shape::new(Lane::I8, w), VOp::Concat, &hs)
        }
        op => {
            let Some((es, sel)) = shuffle_table(op, x.suffix, x.bits, imm) else { return Ok(None) };
            let widths: Vec<VBits> = (0..v.len()).map(|k| VBits::from_bits(crate::simd_sig::x86_arg_width(x, k) as u32).unwrap()).collect();
            pick(m, w, v, &widths, es, &sel)
        }
    };
    Ok(Some(r))
}

/// Applies f to each 128-bit half of the operands (all of width w) and joins the results
fn per_half(m: &mut crate::ir::Recorder, w: VBits, v: &[Value], f: impl Fn(&mut crate::ir::Recorder, &[Value]) -> Value) -> Value {
    if w == VBits::B128 {
        return f(m, v);
    }
    let mut hs = vec![];
    for h in [false, true] {
        let xs: Vec<Value> = v.iter().map(|&a| m.simd(Shape::new(Lane::I8, w), VOp::Half(h), &[a])).collect();
        hs.push(f(m, &xs));
    }
    m.simd(Shape::new(Lane::I8, w), VOp::Concat, &hs)
}

/// Byte indices for a swizzle from lane indices in `idx` (lanes of l): each lane's index (`& mask`, then `>> shift`
/// first) times the lane's bytes, plus 0, 1, .. in its bytes
fn lane_bytes(m: &mut crate::ir::Recorder, w: VBits, l: Lane, idx: Value, mask: i64, shift: u32) -> Value {
    let li = match l {
        Lane::F32 => Lane::I32,
        Lane::F64 => Lane::I64,
        l => l,
    };
    let s = Shape::new(li, w);
    let es = li.bits() / 8;
    let splat = |m: &mut crate::ir::Recorder, x: i64| {
        let k = m.iconst(li.ty(), x);
        m.simd(s, VOp::Splat, &[k])
    };
    let mut k = idx;
    // (vpermilpd's index is bit 1 of each 64-bit lane)
    let (mask, shift) = if li == Lane::I64 { (1, 1) } else { (mask, shift) };
    if shift > 0 {
        let a = m.iconst(Type::I32, shift as i64);
        k = m.simd(s, VOp::Shr(true), &[k, a]);
    }
    let mk = splat(m, mask);
    k = m.simd(s, VOp::And, &[k, mk]);
    let a = m.iconst(Type::I32, es.trailing_zeros() as i64);
    k = m.simd(s, VOp::Shl, &[k, a]);
    // (the byte index into each byte of the lane, then + 0, 1, ..)
    let (rep, offs): (i64, i64) = if es == 4 { (0x0101_0101, 0x0302_0100) } else { (0x0101_0101_0101_0101, 0x0706_0504_0302_0100) };
    let r = splat(m, rep);
    k = m.simd(s, VOp::Mul, &[k, r]);
    let o = splat(m, offs);
    m.simd(Shape::new(Lane::I8, w), VOp::Add, &[k, o])
}

/// An immediate shuffle as elements picked from the operands: (element bytes, for each result element its
/// (operand, element) or None for 0). Intel's definitions; the 256-bit forms of most work within each 128-bit half
fn shuffle_table(op: &str, suffix: &str, bits: u16, imm: u32) -> Option<(u32, Vec<Option<(usize, u32)>>)> {
    let es: u32 = match suffix {
        "ps" | "epi32" => 4,
        "pd" | "epi64" => 8,
        "epi16" => 2,
        "epi8" | "si128" | "si256" | "epi128" => 1,
        _ => return None,
    };
    // (a register broadcast's suffix names the result; its source is 128 bits)
    let n = bits as u32 / 8 / es;
    let m = 16 / es;
    let a = |e: u32| Some((0usize, e));
    let b = |e: u32| Some((1usize, e));
    let f2 = |k: u32| (imm >> (2 * k)) & 3;
    let mut out = vec![];
    for i in 0..n {
        let (base, k) = (i / m * m, i % m);
        out.push(match op {
            "shuffle" if es == 4 && suffix == "ps" => {
                if k < 2 { a(base + f2(k)) } else { b(base + f2(k)) }
            }
            "shuffle" if es == 8 => {
                let sel = (imm >> i) & 1;
                if k == 0 { a(base + sel) } else { b(base + sel) }
            }
            "shuffle" | "permute" if es == 4 => a(base + f2(k)),
            "permute" if es == 8 => a(base + ((imm >> i) & 1)),
            "shufflelo" if es == 2 => {
                if k < 4 { a(base + f2(k)) } else { a(base + k) }
            }
            "shufflehi" if es == 2 => {
                if k >= 4 { a(base + 4 + f2(k - 4)) } else { a(base + k) }
            }
            "unpacklo" | "unpackhi" => {
                let off = if op == "unpackhi" { m / 2 } else { 0 };
                if k % 2 == 0 { a(base + off + k / 2) } else { b(base + off + k / 2) }
            }
            "movehl" if bits == 128 => [b(2), b(3), a(2), a(3)][k as usize],
            "movelh" if bits == 128 => [a(0), a(1), b(0), b(1)][k as usize],
            "alignr" if es == 1 => {
                // (b's bytes, then a's, shifted down by imm)
                let j = k + imm;
                if j < 16 {
                    b(base + j)
                } else if j < 32 {
                    a(base + j - 16)
                } else {
                    None
                }
            }
            "blend" => {
                // (blend_epi16's 8 bits are each 128-bit half's)
                let bit = if es == 2 { k } else { i };
                if (imm >> bit) & 1 == 1 { b(i) } else { a(i) }
            }
            "slli" | "bslli" if es == 1 => {
                if k >= imm { a(base + k - imm) } else { None }
            }
            "srli" | "bsrli" if es == 1 => {
                if k + imm < 16 { a(base + k + imm) } else { None }
            }
            "moveldup" if es == 4 => a(base + (k & !1)),
            "movehdup" if es == 4 => a(base + (k | 1)),
            "movedup" if es == 8 => a(base),
            "permute2f128" | "permute2x128" if bits == 256 => {
                // (each result half: one of a's or b's halves, or 0)
                let c = (imm >> (4 * (i / m))) & 15;
                if c & 8 != 0 {
                    None
                } else {
                    Some(((c as usize & 3) / 2, (c & 1) * m + k))
                }
            }
            "permute4x64" if es == 8 && bits == 256 => a(f2(i)),
            "broadcastsi128" => a(i % 16),
            op if op.starts_with("broadcast") => a(0),
            _ => return None,
        });
    }
    Some((es, out))
}

/// The elements `sel` picked from the operands (`es` bytes each; operand k of width ws[k]), as a vector of width w:
/// one 128-bit byte shuffle per 128-bit half of the result, from the halves of the operands it reads
fn pick(m: &mut crate::ir::Recorder, w: VBits, v: &[Value], ws: &[VBits], es: u32, sel: &[Option<(usize, u32)>]) -> Value {
    // (the operands' 128-bit halves, in order)
    let mut halves: Vec<Value> = vec![];
    let mut first: Vec<usize> = vec![];
    for (k, &a) in v.iter().enumerate() {
        first.push(halves.len());
        if ws[k] == VBits::B128 {
            halves.push(a);
        } else {
            for h in [false, true] {
                halves.push(m.simd(Shape::new(Lane::I8, ws[k]), VOp::Half(h), &[a]));
            }
        }
    }
    let bytes: Vec<Option<(usize, u32)>> = sel
        .iter()
        .flat_map(|e| (0..es).map(move |j| e.map(|(k, el)| (k, el * es + j))))
        .map(|e| e.map(|(k, byte)| (first[k] + (byte / 16) as usize, byte % 16)))
        .collect();
    let mut parts = vec![];
    for c in bytes.chunks(16) {
        parts.push(pick128(m, &halves, c));
    }
    if parts.len() == 1 { parts[0] } else { m.simd(Shape::new(Lane::I8, w), VOp::Concat, &parts) }
}

/// 16 bytes from 128-bit vectors (half, byte), or 0: byte shuffles of two vectors at a time
fn pick128(m: &mut crate::ir::Recorder, halves: &[Value], c: &[Option<(usize, u32)>]) -> Value {
    let mut srcs: Vec<Option<usize>> = vec![];
    for e in c {
        let s = e.map(|(h, _)| h);
        if !srcs.contains(&s) {
            srcs.push(s);
        }
    }
    let zero = if srcs.contains(&None) { Some(m.vconst(0)) } else { None };
    let val = |s: Option<usize>| match s {
        Some(h) => halves[h],
        None => zero.unwrap(),
    };
    let byte = |e: &Option<(usize, u32)>| e.map(|(_, b)| b).unwrap_or(0);
    let src_of = |e: &Option<(usize, u32)>| e.map(|(h, _)| h);
    let mask = |f: &dyn Fn(usize) -> u32| -> u128 { (0..16).fold(0u128, |acc, i| acc | ((f(i) as u128) << (8 * i))) };
    if srcs.len() == 1 {
        let x = val(srcs[0]);
        let mk = mask(&|i| byte(&c[i]));
        return m.simd(Shape::I8X16, VOp::Shuffle(mk), &[x, x]);
    }
    // (the first two sources, then one more at a time: the bytes decided so far stay in place)
    let (s0, s1) = (srcs[0], srcs[1]);
    let mk = mask(&|i| if src_of(&c[i]) == s1 { 16 + byte(&c[i]) } else if src_of(&c[i]) == s0 { byte(&c[i]) } else { i as u32 });
    let (x0, x1) = (val(s0), val(s1));
    let mut acc = m.simd(Shape::I8X16, VOp::Shuffle(mk), &[x0, x1]);
    for &s in &srcs[2..] {
        let mk = mask(&|i| if src_of(&c[i]) == s { 16 + byte(&c[i]) } else { i as u32 });
        let x = val(s);
        acc = m.simd(Shape::I8X16, VOp::Shuffle(mk), &[acc, x]);
    }
    acc
}

fn int_lane(l: Lane) -> Lane {
    match l {
        Lane::F32 => Lane::I32,
        Lane::F64 => Lane::I64,
        l => l,
    }
}

/// a scalar 0 of a lane type
fn zero_scalar(m: &mut crate::ir::Recorder, l: Lane) -> Value {
    match l {
        Lane::F32 => m.f32const(0.0),
        Lane::F64 => m.f64const(0.0),
        l => m.iconst(l.ty(), 0),
    }
}

/// A gather, lane by lane: element i from p + index_i * scale (the index sign-extended). With a mask: only the
/// lanes whose mask's top bit is set, the others src's; a lane left out reads a scratch slot, not memory
fn gather(m: &mut crate::ir::Recorder, g: crate::simd_sig::Gather, scale: u32, v: &[Value]) -> Value {
    let el = if g.elem64 { Lane::I64 } else { Lane::I32 };
    let il = if g.idx64 { Lane::I64 } else { Lane::I32 };
    let rw = VBits::from_bits(g.res_bits as u32).unwrap();
    let iw = VBits::from_bits(g.idx_bits as u32).unwrap();
    let (src, p, idx, mask) = if g.mask { (Some(v[0]), v[1], v[2], Some(v[3])) } else { (None, v[0], v[1], None) };
    let rs = Shape::new(el, rw);
    let mut r = match src {
        Some(s) => s,
        None => zero(m, rw),
    };
    let dummy = mask.map(|_| {
        let s = m.create_stack_slot(8, 8);
        m.stack_addr(s, 0)
    });
    for i in 0..g.n {
        let k = m.simd(Shape::new(il, iw), VOp::Extract(i as u8), &[idx]);
        let k = if g.idx64 { k } else { m.sextend(Type::I64, k) };
        let s = m.iconst(Type::I64, scale as i64);
        let off = m.imul(k, s);
        let mut a = m.iadd(p, off);
        let mut on = None;
        if let (Some(mk), Some(d)) = (mask, dummy) {
            let e = m.simd(rs, VOp::Extract(i as u8), &[mk]);
            let c = m.icmp_imm(IntCC::SignedLessThan, e, 0);
            a = m.select(c, a, d);
            on = Some(c);
        }
        // (a scale below the element's size makes an address that need not be aligned to it: read it byte by byte)
        let e = if scale < el.bits() / 8 { load_bytes(m, el.ty(), a) } else { m.load(el.ty(), a, 0) };
        let e = match on {
            Some(c) => {
                let old = m.simd(rs, VOp::Extract(i as u8), &[r]);
                m.select(c, e, old)
            }
            None => e,
        };
        r = m.simd(rs, VOp::Replace(i as u8), &[r, e]);
    }
    // (an i64 gather of 32-bit elements: the lanes past the n it loads are 0, also with a mask (Intel: dst[MAX:..] := 0))
    for i in g.n..rs.count() {
        let z = m.iconst(el.ty(), 0);
        r = m.simd(rs, VOp::Replace(i as u8), &[r, z]);
    }
    r
}

/// An integer of type t read a byte at a time (an address that need not be aligned to t: the IR's loads are)
fn load_bytes(m: &mut crate::ir::Recorder, t: Type, a: Value) -> Value {
    let n = t.bytes();
    let mut r = m.iconst(t, 0);
    for j in 0..n {
        let b = m.load(Type::I8, a, j as i32);
        let b = m.uextend(t, b);
        let s = m.iconst(Type::I32, (8 * j) as i64);
        let b = m.ishl(b, s);
        r = m.bor(r, b);
    }
    r
}
