//! SIMD for the Cranelift backend (issue #30).
//!
//! A vector of the IR (`Type::Vec`) is held as 128-bit Cranelift values of type I8X16, one for a 128-bit vector and
//! two for a 256-bit one (four for 512): Cranelift's x86-64 backend has no 256-bit registers yet, so a 256-bit
//! instruction is two SSE ones, as LLVM does without AVX. Each instruction bitcasts its parts to the lane type it
//! reads (I32X4, F32X4, ..) and back. The instructions have wasm's semantics (Cranelift's vector instructions are
//! wasmtime's), so the results are those of the wasm backend.

use cranelift_codegen::ir::condcodes::{FloatCC as CFloatCC, IntCC as CIntCC};
use cranelift_codegen::ir::{self, ConstantData, InstBuilder, MemFlagsData, types};
use cranelift_codegen::ir::Endianness;
use cranelift_frontend::FunctionBuilder as CBuilder;
use minclif::{Lane, Shape, Type, VOp};

/// The type of each part
pub const PART: ir::Type = types::I8X16;

/// How many 128-bit parts a value of type t is held in (1 for a scalar)
pub fn nparts(t: Type) -> usize {
    match t {
        Type::Vec(w) => (w.bits() / 128) as usize,
        _ => 1,
    }
}

/// Memory flags of a vector load or store: not trapping (as the scalar ones) but not known to be aligned
pub fn vflags() -> MemFlagsData {
    MemFlagsData::new().with_notrap()
}

fn le() -> MemFlagsData {
    MemFlagsData::new().with_endianness(Endianness::Little)
}

/// The Cranelift vector type of 128 bits of these lanes
fn vt(l: Lane) -> ir::Type {
    match l {
        Lane::I8 => types::I8X16,
        Lane::I16 => types::I16X8,
        Lane::I32 => types::I32X4,
        Lane::I64 => types::I64X2,
        Lane::F32 => types::F32X4,
        Lane::F64 => types::F64X2,
    }
}

/// The integer lanes of the same width (for masks and bit operations on float lanes)
fn int_of(l: Lane) -> Lane {
    match l {
        Lane::F32 => Lane::I32,
        Lane::F64 => Lane::I64,
        l => l,
    }
}

fn wider(l: Lane) -> Result<Lane, String> {
    Ok(match l {
        Lane::I8 => Lane::I16,
        Lane::I16 => Lane::I32,
        Lane::I32 => Lane::I64,
        _ => return Err(format!("cranelift: no lanes wider than {l:?}")),
    })
}

fn narrower(l: Lane) -> Result<Lane, String> {
    Ok(match l {
        Lane::I16 => Lane::I8,
        Lane::I32 => Lane::I16,
        Lane::I64 => Lane::I32,
        _ => return Err(format!("cranelift: no lanes narrower than {l:?}")),
    })
}

fn cast(b: &mut CBuilder, t: ir::Type, v: ir::Value) -> ir::Value {
    if b.func.dfg.value_type(v) == t { v } else { b.ins().bitcast(t, le(), v) }
}

fn canon(b: &mut CBuilder, v: ir::Value) -> ir::Value {
    cast(b, PART, v)
}

/// A 128-bit constant
pub fn vconst(b: &mut CBuilder, x: u128) -> ir::Value {
    let c = b.func.dfg.constants.insert(ConstantData::from(x.to_le_bytes().to_vec()));
    b.ins().vconst(PART, c)
}

/// bytes of (x, y) picked by the indices (< 32)
fn shuffle(b: &mut CBuilder, x: ir::Value, y: ir::Value, m: [u8; 16]) -> ir::Value {
    let imm = b.func.dfg.immediates.push(ConstantData::from(m.to_vec()));
    let (x, y) = (canon(b, x), canon(b, y));
    b.ins().shuffle(x, y, imm)
}

/// the high 64 bits of x moved to the low ones
fn high_to_low(b: &mut CBuilder, x: ir::Value) -> ir::Value {
    shuffle(b, x, x, [8, 9, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4, 5, 6, 7])
}

/// the low 64 bits of x, then the low 64 bits of y
fn low_halves(b: &mut CBuilder, x: ir::Value, y: ir::Value) -> ir::Value {
    shuffle(b, x, y, [0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23])
}

pub fn icc(c: minclif::IntCC) -> CIntCC {
    crate::icc(c)
}

pub fn fcc(c: minclif::FloatCC) -> CFloatCC {
    crate::fcc(c)
}

/// One instruction on 128-bit vectors (parts as I8X16, scalars as they are); the result as a part or a scalar
fn op128(b: &mut CBuilder, l: Lane, op: VOp, a: &[ir::Value], fma: bool) -> Result<ir::Value, String> {
    let t = vt(l);
    let f = l.is_float();
    // (the operands read as the lanes)
    let x = |b: &mut CBuilder, k: usize| cast(b, t, a[k]);
    let r = match op {
        VOp::Not => {
            let v = canon(b, a[0]);
            b.ins().bnot(v)
        }
        VOp::And | VOp::AndNot | VOp::Or | VOp::Xor => {
            let (p, q) = (canon(b, a[0]), canon(b, a[1]));
            match op {
                VOp::And => b.ins().band(p, q),
                VOp::AndNot => b.ins().band_not(p, q),
                VOp::Or => b.ins().bor(p, q),
                _ => b.ins().bxor(p, q),
            }
        }
        VOp::Bitselect | VOp::RelaxedLaneselect => {
            let (p, q, c) = (canon(b, a[0]), canon(b, a[1]), canon(b, a[2]));
            b.ins().bitselect(c, p, q)
        }
        VOp::AnyTrue => {
            let v = canon(b, a[0]);
            return Ok(b.ins().vany_true(v));
        }
        VOp::AllTrue => {
            let v = cast(b, vt(int_of(l)), a[0]);
            return Ok(b.ins().vall_true(v));
        }
        VOp::Bitmask => {
            let v = cast(b, vt(int_of(l)), a[0]);
            return Ok(b.ins().vhigh_bits(types::I32, v));
        }
        VOp::Splat => b.ins().splat(t, a[0]),
        VOp::Extract(k) => {
            let v = x(b, 0);
            return Ok(b.ins().extractlane(v, k));
        }
        VOp::Replace(k) => {
            let v = x(b, 0);
            b.ins().insertlane(v, a[1], k)
        }
        VOp::Shuffle(m) => shuffle(b, a[0], a[1], m.to_le_bytes()),
        VOp::Swizzle | VOp::RelaxedSwizzle => {
            let (p, q) = (canon(b, a[0]), canon(b, a[1]));
            b.ins().swizzle(p, q)
        }
        VOp::Add | VOp::Sub | VOp::Mul | VOp::Div => {
            let (p, q) = (x(b, 0), x(b, 1));
            match (op, f) {
                (VOp::Add, true) => b.ins().fadd(p, q),
                (VOp::Add, false) => b.ins().iadd(p, q),
                (VOp::Sub, true) => b.ins().fsub(p, q),
                (VOp::Sub, false) => b.ins().isub(p, q),
                (VOp::Mul, true) => b.ins().fmul(p, q),
                (VOp::Mul, false) => b.ins().imul(p, q),
                (_, true) => b.ins().fdiv(p, q),
                _ => return Err(format!("cranelift: {op:?} of {l:?} lanes")),
            }
        }
        VOp::Neg | VOp::Abs | VOp::Sqrt | VOp::Ceil | VOp::Floor | VOp::Trunc | VOp::Nearest => {
            let p = x(b, 0);
            match (op, f) {
                (VOp::Neg, true) => b.ins().fneg(p),
                (VOp::Neg, false) => b.ins().ineg(p),
                (VOp::Abs, true) => b.ins().fabs(p),
                (VOp::Abs, false) => b.ins().iabs(p),
                (VOp::Sqrt, true) => b.ins().sqrt(p),
                (VOp::Ceil, true) => b.ins().ceil(p),
                (VOp::Floor, true) => b.ins().floor(p),
                (VOp::Trunc, true) => b.ins().trunc(p),
                (VOp::Nearest, true) => b.ins().nearest(p),
                _ => return Err(format!("cranelift: {op:?} of {l:?} lanes")),
            }
        }
        VOp::Min(u) | VOp::Max(u) => {
            let (p, q) = (x(b, 0), x(b, 1));
            match (matches!(op, VOp::Min(_)), f, u) {
                (true, true, _) => b.ins().fmin(p, q),
                (false, true, _) => b.ins().fmax(p, q),
                (true, false, false) => b.ins().smin(p, q),
                (true, false, true) => b.ins().umin(p, q),
                (false, false, false) => b.ins().smax(p, q),
                (false, false, true) => b.ins().umax(p, q),
            }
        }
        // (pmin: b < a ? b : a; pmax: a < b ? b : a; relaxed min / max as x86's minps / maxps: a < b ? a : b,
        // a > b ? a : b)
        VOp::Pmin | VOp::Pmax | VOp::RelaxedMin | VOp::RelaxedMax => {
            let (p, q) = (x(b, 0), x(b, 1));
            let (cc, l1, r1, yes, no) = match op {
                VOp::Pmin => (CFloatCC::LessThan, q, p, q, p),
                VOp::Pmax => (CFloatCC::LessThan, p, q, q, p),
                VOp::RelaxedMin => (CFloatCC::LessThan, p, q, p, q),
                _ => (CFloatCC::GreaterThan, p, q, p, q),
            };
            let m = b.ins().fcmp(cc, l1, r1);
            let m = cast(b, t, m);
            b.ins().bitselect(m, yes, no)
        }
        VOp::AddSat(u) | VOp::SubSat(u) => {
            let (p, q) = (x(b, 0), x(b, 1));
            match (matches!(op, VOp::AddSat(_)), u) {
                (true, false) => b.ins().sadd_sat(p, q),
                (true, true) => b.ins().uadd_sat(p, q),
                (false, false) => b.ins().ssub_sat(p, q),
                (false, true) => b.ins().usub_sat(p, q),
            }
        }
        VOp::Avgr => {
            let (p, q) = (x(b, 0), x(b, 1));
            b.ins().avg_round(p, q)
        }
        VOp::Icmp(cc) => {
            let (p, q) = (x(b, 0), x(b, 1));
            b.ins().icmp(icc(cc), p, q)
        }
        VOp::Fcmp(cc) => {
            let (p, q) = (x(b, 0), x(b, 1));
            b.ins().fcmp(fcc(cc), p, q)
        }
        VOp::Shl | VOp::Shr(_) => {
            let p = x(b, 0);
            match op {
                VOp::Shl => b.ins().ishl(p, a[1]),
                VOp::Shr(true) => b.ins().ushr(p, a[1]),
                _ => b.ins().sshr(p, a[1]),
            }
        }
        VOp::Popcnt => {
            let p = canon(b, a[0]);
            b.ins().popcnt(p)
        }
        VOp::Narrow(u) => {
            let s = vt(wider(l)?);
            let (p, q) = (cast(b, s, a[0]), cast(b, s, a[1]));
            if u { b.ins().unarrow(p, q) } else { b.ins().snarrow(p, q) }
        }
        VOp::Extend(h, u) => {
            let p = cast(b, vt(narrower(l)?), a[0]);
            widen(b, p, h, u)
        }
        VOp::ExtMul(h, u) => {
            let s = vt(narrower(l)?);
            let (p, q) = (cast(b, s, a[0]), cast(b, s, a[1]));
            let (p, q) = (widen(b, p, h, u), widen(b, q, h, u));
            b.ins().imul(p, q)
        }
        // (to i64 lanes: x64 has no iadd_pairwise.i64x2; the even and the odd 32-bit lanes widened and added)
        VOp::ExtAddPairwise(u) if l == Lane::I64 => {
            let ev = shuffle(b, a[0], a[0], [0, 1, 2, 3, 8, 9, 10, 11, 0, 1, 2, 3, 8, 9, 10, 11]);
            let od = shuffle(b, a[0], a[0], [4, 5, 6, 7, 12, 13, 14, 15, 4, 5, 6, 7, 12, 13, 14, 15]);
            let (ev, od) = (cast(b, types::I32X4, ev), cast(b, types::I32X4, od));
            let (ev, od) = (widen(b, ev, false, u), widen(b, od, false, u));
            b.ins().iadd(ev, od)
        }
        VOp::ExtAddPairwise(u) => {
            let p = cast(b, vt(narrower(l)?), a[0]);
            let (lo, hi) = (widen(b, p, false, u), widen(b, p, true, u));
            b.ins().iadd_pairwise(lo, hi)
        }
        VOp::Dot => dot(b, a[0], a[1], types::I16X8),
        VOp::RelaxedDot => dot(b, a[0], a[1], types::I8X16),
        VOp::RelaxedDotAdd => {
            let d = dot(b, a[0], a[1], types::I8X16);
            let (lo, hi) = (widen(b, d, false, false), widen(b, d, true, false));
            let s = b.ins().iadd_pairwise(lo, hi);
            let c = cast(b, types::I32X4, a[2]);
            b.ins().iadd(s, c)
        }
        VOp::Q15MulrSat | VOp::RelaxedQ15Mulr => {
            let (p, q) = (cast(b, types::I16X8, a[0]), cast(b, types::I16X8, a[1]));
            b.ins().sqmul_round_sat(p, q)
        }
        VOp::FcvtFromInt(u) => {
            let p = cast(b, types::I32X4, a[0]);
            match l {
                Lane::F32 if u => b.ins().fcvt_from_uint(types::F32X4, p),
                Lane::F32 => b.ins().fcvt_from_sint(types::F32X4, p),
                Lane::F64 => {
                    let w = widen(b, p, false, u);
                    if u { b.ins().fcvt_from_uint(types::F64X2, w) } else { b.ins().fcvt_from_sint(types::F64X2, w) }
                }
                _ => return Err(format!("cranelift: {op:?} to {l:?} lanes")),
            }
        }
        VOp::FcvtToIntSat(f64, u) | VOp::RelaxedTrunc(f64, u) => {
            if f64 {
                let p = cast(b, types::F64X2, a[0]);
                // (the zero a vconst of I64X2 itself: x64 lowers only this pattern of `uunarrow`)
                let c0 = b.func.dfg.constants.insert(ConstantData::from(vec![0u8; 16]));
                let z = b.ins().vconst(types::I64X2, c0);
                if u {
                    let c = b.ins().fcvt_to_uint_sat(types::I64X2, p);
                    b.ins().uunarrow(c, z)
                } else {
                    let c = b.ins().fcvt_to_sint_sat(types::I64X2, p);
                    b.ins().snarrow(c, z)
                }
            } else {
                let p = cast(b, types::F32X4, a[0]);
                if u { b.ins().fcvt_to_uint_sat(types::I32X4, p) } else { b.ins().fcvt_to_sint_sat(types::I32X4, p) }
            }
        }
        VOp::Demote => {
            let p = cast(b, types::F64X2, a[0]);
            b.ins().fvdemote(p)
        }
        VOp::Promote => {
            let p = cast(b, types::F32X4, a[0]);
            b.ins().fvpromote_low(p)
        }
        VOp::Fma | VOp::RelaxedMadd | VOp::RelaxedNmadd => {
            let (p, q, c) = (x(b, 0), x(b, 1), x(b, 2));
            let neg = op == VOp::RelaxedNmadd;
            if fma {
                let p = if neg { b.ins().fneg(p) } else { p };
                b.ins().fma(p, q, c)
            } else if op == VOp::Fma {
                return Err("cranelift: a fused multiply-add needs a CPU with FMA".into());
            } else {
                // (relaxed: two roundings are allowed)
                let m = b.ins().fmul(p, q);
                if neg { b.ins().fsub(c, m) } else { b.ins().fadd(m, c) }
            }
        }
        VOp::Half(_) | VOp::Concat => return Err(format!("cranelift: {op:?} of a 128-bit vector")),
    };
    Ok(canon(b, r))
}

/// half the lanes widened
fn widen(b: &mut CBuilder, p: ir::Value, high: bool, u: bool) -> ir::Value {
    match (high, u) {
        (false, false) => b.ins().swiden_low(p),
        (true, false) => b.ins().swiden_high(p),
        (false, true) => b.ins().uwiden_low(p),
        (true, true) => b.ins().uwiden_high(p),
    }
}

/// sums of adjacent products, lanes twice as wide as `src`'s (the operands read as `src`)
fn dot(b: &mut CBuilder, x: ir::Value, y: ir::Value, src: ir::Type) -> ir::Value {
    let (x, y) = (cast(b, src, x), cast(b, src, y));
    let (xl, yl, xh, yh) = (widen(b, x, false, false), widen(b, y, false, false), widen(b, x, true, false), widen(b, y, true, false));
    let lo = b.ins().imul(xl, yl);
    let hi = b.ins().imul(xh, yh);
    b.ins().iadd_pairwise(lo, hi)
}

/// One instruction on vectors of any width: each operand as its parts (a scalar as one), the result as parts
pub fn lower(b: &mut CBuilder, sh: Shape, op: VOp, args: &[Vec<ir::Value>], fma: bool) -> Result<Vec<ir::Value>, String> {
    let n = (sh.bits.bits() / 128) as usize;
    let l = sh.lane;
    let per = 128 / l.bits();
    if args.len() != op.arity() {
        return Err(format!("cranelift: {op:?} with {} operands", args.len()));
    }
    // (each vector operand has its n parts; Concat's have half as many)
    let want = if op == VOp::Concat { n / 2 } else { n };
    for (k, a) in args.iter().enumerate() {
        let scalar = matches!((op, k), (VOp::Splat, 0) | (VOp::Replace(_), 1) | (VOp::Shl | VOp::Shr(_), 1));
        if !scalar && a.len() != want {
            return Err(format!("cranelift: {op:?} on {sh:?}: operand {k} has {} parts", a.len()));
        }
    }
    if n == 1 && !matches!(op, VOp::Half(_) | VOp::Concat) {
        let a: Vec<ir::Value> = args.iter().map(|a| a[0]).collect();
        return Ok(vec![op128(b, l, op, &a, fma)?]);
    }
    if op.only_128() || n == 0 {
        return Err(format!("cranelift: {op:?} of a {}-bit vector", sh.bits.bits()));
    }
    let a = &args[0];
    let mut out: Vec<ir::Value> = vec![];
    match op {
        VOp::Half(h) => out.extend_from_slice(if h { &a[n / 2..] } else { &a[..n / 2] }),
        VOp::Concat => {
            out.extend_from_slice(&args[0]);
            out.extend_from_slice(&args[1]);
        }
        VOp::Splat => {
            let p = op128(b, l, op, &[a[0]], fma)?;
            out = vec![p; n];
        }
        VOp::Extract(k) => {
            let (i, j) = (k as u32 / per, k as u32 % per);
            out.push(op128(b, l, VOp::Extract(j as u8), &[a[i as usize]], fma)?);
        }
        VOp::Replace(k) => {
            let (i, j) = (k as u32 / per, k as u32 % per);
            out = a.clone();
            out[i as usize] = op128(b, l, VOp::Replace(j as u8), &[a[i as usize], args[1][0]], fma)?;
        }
        VOp::AnyTrue => {
            let mut x = canon(b, a[0]);
            for &p in &a[1..] {
                let p = canon(b, p);
                x = b.ins().bor(x, p);
            }
            out.push(b.ins().vany_true(x));
        }
        VOp::AllTrue => {
            let mut x = op128(b, l, op, &[a[0]], fma)?;
            for &p in &a[1..] {
                let y = op128(b, l, op, &[p], fma)?;
                x = b.ins().band(x, y);
            }
            out.push(x);
        }
        VOp::Bitmask => {
            if per as usize * n > 32 {
                return Err(format!("cranelift: a bitmask of {} lanes", per as usize * n));
            }
            let mut x = op128(b, l, op, &[a[0]], fma)?;
            for (i, &p) in a.iter().enumerate().skip(1) {
                let y = op128(b, l, op, &[p], fma)?;
                let y = b.ins().ishl_imm_s(y, (per as usize * i) as i64);
                x = b.ins().bor(x, y);
            }
            out.push(x);
        }
        // (the lanes of a, then those of b, each pair of parts narrowed into one)
        VOp::Narrow(_) => {
            for v in [&args[0], &args[1]] {
                for k in 0..n / 2 {
                    out.push(op128(b, l, op, &[v[2 * k], v[2 * k + 1]], fma)?);
                }
            }
        }
        // (half of the lanes, each source part into two)
        VOp::Extend(h, u) | VOp::ExtMul(h, u) => {
            let r = if h { n / 2..n } else { 0..n / 2 };
            for k in r {
                for hh in [false, true] {
                    let o = if matches!(op, VOp::Extend(..)) { VOp::Extend(hh, u) } else { VOp::ExtMul(hh, u) };
                    let mut xs = vec![args[0][k]];
                    if matches!(op, VOp::ExtMul(..)) {
                        xs.push(args[1][k]);
                    }
                    out.push(op128(b, l, o, &xs, fma)?);
                }
            }
        }
        // (from the low half of the lanes: each source part into two)
        VOp::FcvtFromInt(_) | VOp::Promote if l == Lane::F64 => {
            for &p in &a[..n / 2] {
                out.push(op128(b, l, op, &[p], fma)?);
                let q = high_to_low(b, p);
                out.push(op128(b, l, op, &[q], fma)?);
            }
        }
        // (into the low half of the lanes, the rest 0: each source part gives 64 bits)
        VOp::Demote | VOp::FcvtToIntSat(true, _) | VOp::RelaxedTrunc(true, _) => {
            for k in 0..n / 2 {
                let x = op128(b, l, op, &[a[2 * k]], fma)?;
                let y = op128(b, l, op, &[a[2 * k + 1]], fma)?;
                out.push(low_halves(b, x, y));
            }
            let z = vconst(b, 0);
            for _ in n / 2..n {
                out.push(z);
            }
        }
        // (lane by lane, or within 128 bits: each part on its own)
        _ => {
            for k in 0..n {
                let xs: Vec<ir::Value> = args.iter().enumerate().map(|(j, v)| if v.len() == 1 && matches!((op, j), (VOp::Shl | VOp::Shr(_), 1)) { v[0] } else { v[k] }).collect();
                out.push(op128(b, l, op, &xs, fma)?);
            }
        }
    }
    Ok(out)
}
