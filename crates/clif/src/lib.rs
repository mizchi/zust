//! session's Cranelift backend: rebuilds the instruction sequence recorded by minclif's FunctionBuilder (in the order acg called it)
//! as Cranelift IR and compiles it. acg, the cache and .o writing are the same ones minclif uses, so
//! the backends alone can be compared with "the same frontend, the same instruction sequence" (minclif and Cranelift).
//!
//! It returns the same shape as minclif: (machine code, relocations (name, ELF kind), trap locations and numbers).
//! zustc itself cannot compile it (it uses cranelift), so it is not part of the self-host.

use minclif::{FMap, FSet};

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::{FloatCC as CFloatCC, IntCC as CIntCC};
use cranelift_codegen::ir::{
    self, AbiParam, ExtFuncData, ExternalName, InstBuilder, MemFlagsData, Signature, StackSlotData, StackSlotKind, TrapCode,
    UserExternalName, UserFuncName, types,
};
use cranelift_codegen::isa::{CallConv, OwnedTargetIsa};
use cranelift_codegen::{Context, RelocTarget, binemit::Reloc as CReloc, settings, settings::Configurable};
use cranelift_frontend::{FunctionBuilder as CBuilder, FunctionBuilderContext, Variable};

use minclif::{FloatCC, FunctionBuilder, IntCC, Op, Reloc, StackSlot, Type};

mod simd;
use simd::{PART, nparts};

/// Cranelift ISA (opt: speed if -O, fast_ra: single_pass register allocation)
pub fn make_isa(opt: bool, fast_ra: bool) -> OwnedTargetIsa {
    let mut f = settings::builder();
    f.set("opt_level", if opt { "speed" } else { "none" }).unwrap();
    f.set("regalloc_algorithm", if fast_ra { "single_pass" } else { "backtracking" }).unwrap();
    f.set("is_pic", "true").unwrap();
    f.set("unwind_info", "false").unwrap();
    f.set("enable_llvm_abi_extensions", "true").unwrap();
    // (IR verification took 18% of compilation: off by default. ZUSTC_CLIF_VERIFY=1 turns it on)
    f.set("enable_verifier", if std::env::var_os("ZUSTC_CLIF_VERIFY").is_some() { "true" } else { "false" }).unwrap();
    let mut ib = cranelift_native::builder().unwrap();
    // (no AVX-512: no 512-bit registers are used, and with it Cranelift picks EVEX forms for some 128-bit
    // instructions (vpsraq for an i64x2 shift), which CPUs without AVX-512 and valgrind do not run.
    // ZUSTC_CLIF_AVX512=1 keeps what the CPU has)
    if std::env::var_os("ZUSTC_CLIF_AVX512").is_none() {
        for k in ["has_avx512bitalg", "has_avx512dq", "has_avx512f", "has_avx512vbmi", "has_avx512vl", "has_avx512vnni"] {
            let _ = ib.set(k, "false");
        }
    }
    ib.finish(settings::Flags::new(f)).unwrap()
}

thread_local! {
    static CTXS: std::cell::RefCell<Option<(Context, FunctionBuilderContext)>> = const { std::cell::RefCell::new(None) };
    static ISA: std::cell::RefCell<Option<(bool, bool, OwnedTargetIsa)>> = const { std::cell::RefCell::new(None) };
}

fn ty(t: Type) -> ir::Type {
    match t {
        Type::I8 => types::I8,
        Type::I16 => types::I16,
        Type::I32 => types::I32,
        Type::I64 => types::I64,
        Type::I128 => types::I128,
        Type::F32 => types::F32,
        Type::F64 => types::F64,
        // (a vector is held in 128-bit parts: simd.rs)
        Type::Vec(_) => PART,
    }
}

/// The Cranelift types a value of type t is held in (a vector: its 128-bit parts)
fn ctys(t: Type) -> Vec<ir::Type> {
    vec![ty(t); nparts(t)]
}

fn icc(c: IntCC) -> CIntCC {
    match c {
        IntCC::Equal => CIntCC::Equal,
        IntCC::NotEqual => CIntCC::NotEqual,
        IntCC::SignedLessThan => CIntCC::SignedLessThan,
        IntCC::SignedLessThanOrEqual => CIntCC::SignedLessThanOrEqual,
        IntCC::SignedGreaterThan => CIntCC::SignedGreaterThan,
        IntCC::SignedGreaterThanOrEqual => CIntCC::SignedGreaterThanOrEqual,
        IntCC::UnsignedLessThan => CIntCC::UnsignedLessThan,
        IntCC::UnsignedLessThanOrEqual => CIntCC::UnsignedLessThanOrEqual,
        IntCC::UnsignedGreaterThan => CIntCC::UnsignedGreaterThan,
        IntCC::UnsignedGreaterThanOrEqual => CIntCC::UnsignedGreaterThanOrEqual,
    }
}

fn fcc(c: FloatCC) -> CFloatCC {
    match c {
        FloatCC::Equal => CFloatCC::Equal,
        FloatCC::NotEqual => CFloatCC::NotEqual,
        FloatCC::LessThan => CFloatCC::LessThan,
        FloatCC::LessThanOrEqual => CFloatCC::LessThanOrEqual,
        FloatCC::GreaterThan => CFloatCC::GreaterThan,
        FloatCC::GreaterThanOrEqual => CFloatCC::GreaterThanOrEqual,
        FloatCC::Unordered => CFloatCC::Unordered,
    }
}

/// Makes machine code from a minclif recording. zero: slots zeroed at the entry
pub fn compile(fb: &FunctionBuilder, zero: &[StackSlot], opt: bool) -> Result<(Vec<u8>, Vec<Reloc>, Vec<(u32, u8)>), String> {
    let fast_ra = std::env::var_os("ZUSTC_CLIF_FAST_RA").is_some();
    ISA.with(|cell| {
        let mut c = cell.borrow_mut();
        if !matches!(&*c, Some((o, f, _)) if *o == opt && *f == fast_ra) {
            *c = Some((opt, fast_ra, make_isa(opt, fast_ra)));
        }
        let isa = &c.as_ref().unwrap().2;
        build(fb, zero, &**isa)
    })
}

fn build(fb: &FunctionBuilder, zero: &[StackSlot], isa: &dyn cranelift_codegen::isa::TargetIsa) -> Result<(Vec<u8>, Vec<Reloc>, Vec<(u32, u8)>), String> {
    let mut sig = Signature::new(CallConv::SystemV);
    for &p in &fb.params {
        for t in ctys(p) {
            sig.params.push(AbiParam::new(t));
        }
    }
    for &r in &fb.rets {
        for t in ctys(r) {
            sig.returns.push(AbiParam::new(t));
        }
    }
    // (FMA instructions: the target's (cranelift_native's) flag)
    let has_fma = isa.isa_flags().iter().any(|f| f.name == "has_fma" && f.as_bool() == Some(true));
    let mut func = ir::Function::with_name_signature(UserFuncName::user(0, 0), sig);
    // (Context and FunctionBuilderContext are reused across fns: the Vecs inside are not rebuilt each time)
    let (mut ctx, mut fctx) = CTXS.with(|c| c.borrow_mut().take()).unwrap_or_else(|| (Context::new(), FunctionBuilderContext::new()));
    let mut b = CBuilder::new(&mut func, &mut fctx);
    // (Name table: relocation number → name)
    let mut names: Vec<String> = vec![];
    let mut vals: FMap<u32, ir::Value> = FMap::default();
    // (a vector held in several parts: its parts after the first, which is in vals; its variables)
    let mut more: FMap<u32, Vec<ir::Value>> = FMap::default();
    let mut gvp: FMap<u32, Vec<Variable>> = FMap::default();
    let mut varsp: FMap<u32, Vec<Variable>> = FMap::default();
    let mut blocks: FMap<u32, ir::Block> = FMap::default();
    let mut vars: FMap<u32, Variable> = FMap::default();
    let mut slots: FMap<u32, ir::StackSlot> = FMap::default();
    // (minclif fn number → name. A Cranelift FuncRef is made per signature used at a call)
    let mut fnames: FMap<u32, String> = FMap::default();
    let mut frefs: FMap<(String, Vec<ir::Type>, Vec<ir::Type>), ir::FuncRef> = FMap::default();
    let mut gvs: FMap<String, ir::GlobalValue> = FMap::default();
    // Entry: the argument values (minclif values 0..) and the slots to zero. Jumps to the first block
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let ps = b.block_params(entry).to_vec();
    let mut at = 0;
    for (i, &t) in fb.params.iter().enumerate() {
        let n = nparts(t);
        vals.insert(i as u32, ps[at]);
        if n > 1 {
            more.insert(i as u32, ps[at + 1..at + n].to_vec());
        }
        at += n;
    }
    // Values used in a different segment (a range where a block was written contiguously) from their definition become Variables: acg sometimes uses values in blocks
    // they do not dominate (minclif keeps values in slots, so this is no problem). cranelift_frontend builds the SSA
    let mut gvars: FMap<u32, Variable> = FMap::default();
    // Slot addresses are rebuilt where they are used (not made long-lived values / Variables across segments: register allocation gets lighter)
    let remat = std::env::var_os("ZUSTC_CLIF_NO_REMAT").is_none();
    let fold_br = std::env::var_os("ZUSTC_CLIF_NO_FOLD_BR").is_none();
    let mut saddr: FMap<u32, (u32, i32)> = FMap::default();
    if remat {
        for (i, op) in fb.ops.iter().enumerate() {
            if let Op::StackAddr(sl, off) = op {
                saddr.insert(fb.res[i][0], (sl.0, *off));
            }
        }
    }
    if let Some(tr) = fb.track() {
        let segs = &tr.segs;
        let seg_of = |op: u32| -> usize {
            let (mut lo, mut hi) = (0usize, segs.len());
            while lo + 1 < hi {
                let m = (lo + hi) / 2;
                if segs[m].start <= op {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            lo
        };
        let mut first: FMap<u32, usize> = FMap::default();
        let mut multi: FSet<u32> = FSet::default();
        for &(id, op, _) in &tr.ev {
            if id >= 1 << 28 || id < fb.params.len() as u32 || saddr.contains_key(&id) {
                continue;
            }
            let sg = seg_of(op);
            match first.get(&id) {
                None => {
                    first.insert(id, sg);
                }
                Some(&f) if f != sg => {
                    multi.insert(id);
                }
                _ => {}
            }
        }
        let mut ids: Vec<u32> = multi.into_iter().collect();
        ids.sort_unstable();
        for id in ids {
            let t = fb.value_type(minclif::Value(id));
            if nparts(t) > 1 {
                let vs: Vec<Variable> = ctys(t).into_iter().map(|ct| b.declare_var(ct)).collect();
                gvp.insert(id, vs);
                continue;
            }
            let var = b.declare_var(ty(t));
            gvars.insert(id, var);
        }
    }
    // Turn stack slot fields into Variables (SROA: the same decision as minclif; slots whose address is used only by load /
    // store). Cranelift does not make stack slots SSA, so struct values no longer round-trip through memory
    let mut facc: FMap<u32, Variable> = FMap::default();
    if let (Some(tr), true) = (fb.track(), std::env::var_os("ZUSTC_CLIF_NO_SROA").is_none()) {
        let zs: Vec<u32> = zero.iter().map(|z| z.0).collect();
        let (prom, fields, ftys) = minclif::sroa(tr, fb.n_values(), fb.paddr(), fb.n_slots(), &zs);
        if !fields.is_empty() {
            let fv: Vec<Variable> = ftys
                .iter()
                .map(|&t| {
                    let var = b.declare_var(ty(t));
                    // (0 at the entry: paths that read before writing also get a definite value)
                    let z = match t {
                        Type::F64 => b.ins().f64const(0.0),
                        Type::F32 => b.ins().f32const(0.0),
                        t => b.ins().iconst(ty(t), 0),
                    };
                    b.def_var(var, z);
                    var
                })
                .collect();
            for &(op, sl, off, _, _) in &tr.acc {
                if prom[sl as usize] {
                    if let Some(&f) = fields.get(&(sl, off)) {
                        facc.insert(op, fv[f as usize]);
                    }
                }
            }
        }
    }
    // (Block params: those made Variables are def_var'd at the start of the block)
    let mut bparams: FMap<u32, Vec<u32>> = FMap::default();
    let mut started: FSet<u32> = FSet::default();
    let mut in_entry = true;
    // The number of predecessor blocks (jumps into it not yet written). A block that reaches 0 is sealed at once: a variable's value can be decided from the predecessors
    // on the spot, so fewer block params are placed provisionally, and branches on constant conditions can be folded
    let early_seal = std::env::var_os("ZUSTC_CLIF_LATE_SEAL").is_none();
    let mut rem: FMap<u32, u32> = FMap::default();
    if early_seal {
        b.seal_block(entry);
        let mut first = true;
        for op in &fb.ops {
            match op {
                Op::Jump(bk, _) => *rem.entry(bk.0).or_default() += 1,
                Op::Brif(_, t, _, f, _) => {
                    *rem.entry(t.0).or_default() += 1;
                    *rem.entry(f.0).or_default() += 1;
                }
                Op::SwitchToBlock(bk) if first => {
                    // (The jump from the entry)
                    *rem.entry(bk.0).or_default() += 1;
                    first = false;
                }
                _ => {}
            }
        }
    }
    let mut sealed: FSet<u32> = FSet::default();
    macro_rules! edge {
        ($bk:expr) => {
            if early_seal {
                if let Some(r) = rem.get_mut(&$bk) {
                    *r -= 1;
                    if *r == 0 && sealed.insert($bk) {
                        b.seal_block(blocks[&$bk]);
                    }
                }
            }
        };
    }
    macro_rules! v {
        ($x:expr) => {
            match gvars.get(&$x.0) {
                Some(&var) => b.use_var(var),
                None => match saddr.get(&$x.0) {
                    Some(&(sl, off)) => b.ins().stack_addr(types::I64, slots[&sl], off),
                    None => *vals.get(&$x.0).ok_or_else(|| format!("clif_backend: unknown value v{}", $x.0))?,
                },
            }
        };
    }
    // (a value as its parts: one for a scalar or a 128-bit vector)
    macro_rules! vp {
        ($x:expr) => {{
            let id = $x.0;
            match gvp.get(&id) {
                Some(vs) => vs.clone().into_iter().map(|var| b.use_var(var)).collect::<Vec<ir::Value>>(),
                None => {
                    let mut o = vec![v!($x)];
                    if let Some(m) = more.get(&id) {
                        o.extend_from_slice(m);
                    }
                    o
                }
            }
        }};
    }
    // (values, a vector's parts in a row: arguments of jumps, calls and returns)
    macro_rules! vsf {
        ($xs:expr) => {{
            let mut out: Vec<ir::Value> = vec![];
            for x in $xs.iter() {
                let ps = vp!(x);
                out.extend(ps);
            }
            out
        }};
    }
    let name_of = |n: &str, names: &mut Vec<String>| -> u32 {
        match names.iter().position(|x| x == n) {
            Some(i) => i as u32,
            None => {
                names.push(n.to_string());
                names.len() as u32 - 1
            }
        }
    };
    for (i, op) in fb.ops.iter().enumerate() {
        let res = &fb.res[i];
        // (Instructions that make values: values made Variables are def_var'd here)
        let makes_values = !matches!(
            op,
            Op::CreateBlock | Op::AppendBlockParam(..) | Op::SwitchToBlock(_) | Op::DeclareVar(_) | Op::CreateStackSlot(..) | Op::ImportFunction(_)
        );
        let set1 = |vals: &mut FMap<u32, ir::Value>, v: ir::Value| {
            vals.insert(res[0], v);
        };
        // (a result as its parts)
        let setp = |vals: &mut FMap<u32, ir::Value>, more: &mut FMap<u32, Vec<ir::Value>>, id: u32, ps: &[ir::Value]| {
            vals.insert(id, ps[0]);
            if ps.len() > 1 {
                more.insert(id, ps[1..].to_vec());
            }
        };
        match op {
            Op::CreateBlock => {
                let bl = b.create_block();
                blocks.insert(res[0], bl);
            }
            Op::AppendBlockParam(bk, t) => {
                let bl = blocks[&bk.0];
                let ps: Vec<ir::Value> = ctys(*t).into_iter().map(|ct| b.append_block_param(bl, ct)).collect();
                setp(&mut vals, &mut more, res[0], &ps);
                bparams.entry(bk.0).or_default().push(res[0]);
            }
            Op::SwitchToBlock(bk) => {
                let bl = blocks[&bk.0];
                if in_entry {
                    b.ins().jump(bl, &[]);
                    in_entry = false;
                    edge!(bk.0);
                }
                b.switch_to_block(bl);
                if early_seal && rem.get(&bk.0).copied().unwrap_or(0) == 0 && sealed.insert(bk.0) {
                    b.seal_block(bl);
                }
                if started.insert(bk.0) {
                    for id in bparams.get(&bk.0).cloned().unwrap_or_default() {
                        if let Some(&var) = gvars.get(&id) {
                            b.def_var(var, vals[&id]);
                        }
                        if let Some(vs) = gvp.get(&id) {
                            let mut ps = vec![vals[&id]];
                            ps.extend(more.get(&id).cloned().unwrap_or_default());
                            for (var, p) in vs.iter().zip(ps) {
                                b.def_var(*var, p);
                            }
                        }
                    }
                }
            }
            Op::DeclareVar(t) if nparts(*t) > 1 => {
                let vs: Vec<Variable> = ctys(*t).into_iter().map(|ct| b.declare_var(ct)).collect();
                varsp.insert(res[0], vs);
            }
            Op::DeclareVar(t) => {
                let var = b.declare_var(ty(*t));
                vars.insert(res[0], var);
            }
            Op::DefVar(var, x) if varsp.contains_key(&var.0) => {
                let ps = vp!(x);
                for (k, p) in ps.into_iter().enumerate() {
                    b.def_var(varsp[&var.0][k], p);
                }
            }
            Op::DefVar(var, x) => {
                let x = v!(x);
                b.def_var(vars[&var.0], x);
            }
            Op::UseVar(var) if varsp.contains_key(&var.0) => {
                let ps: Vec<ir::Value> = varsp[&var.0].clone().into_iter().map(|v| b.use_var(v)).collect();
                setp(&mut vals, &mut more, res[0], &ps);
            }
            Op::UseVar(var) => {
                let x = b.use_var(vars[&var.0]);
                set1(&mut vals, x);
            }
            Op::CreateStackSlot(size, align) => {
                let ss = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (*size).max(1), (*align).max(1).trailing_zeros() as u8));
                slots.insert(res[0], ss);
            }
            Op::ImportFunction(n) => {
                fnames.insert(res[0], n.clone());
            }
            Op::Iconst(t, x) => {
                let x = if *t == Type::I128 {
                    let lo = b.ins().iconst(types::I64, *x);
                    let hi = b.ins().iconst(types::I64, if *x < 0 { -1 } else { 0 });
                    b.ins().iconcat(lo, hi)
                } else {
                    b.ins().iconst(ty(*t), *x)
                };
                set1(&mut vals, x);
            }
            Op::Iadd(x, y) | Op::Iadd128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().iadd(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Isub(x, y) | Op::Isub128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().isub(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Imul(x, y) | Op::Imul128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().imul(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Band(x, y) | Op::Band128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().band(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Bor(x, y) | Op::Bor128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().bor(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Bxor(x, y) | Op::Bxor128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().bxor(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::IaddImm(x, k) => {
                let _t0 = v!(x);
                let r = b.ins().iadd_imm_s(_t0, *k);
                set1(&mut vals, r);
            }
            Op::Udiv(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().udiv(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Sdiv(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().sdiv(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Urem(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().urem(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Srem(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().srem(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Ishl(x, y) | Op::Ishl128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().ishl(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Ushr(x, y) | Op::Ushr128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().ushr(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Sshr(x, y) | Op::Sshr128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().sshr(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Ineg(x) => {
                let _t0 = v!(x);
                let r = b.ins().ineg(_t0);
                set1(&mut vals, r);
            }
            Op::Bnot(x) => {
                let _t0 = v!(x);
                let r = b.ins().bnot(_t0);
                set1(&mut vals, r);
            }
            Op::Icmp(c, x, y) | Op::Icmp128(c, x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().icmp(icc(*c), _t0, _t1);
                set1(&mut vals, r);
            }
            Op::IcmpImm(c, x, k) => {
                let _t0 = v!(x);
                let r = b.ins().icmp_imm_s(icc(*c), _t0, *k);
                set1(&mut vals, r);
            }
            Op::Select(c, x, y) if nparts(fb.value_type(*x)) > 1 => {
                let cv = v!(c);
                let (xs, ys) = (vp!(x), vp!(y));
                let ps: Vec<ir::Value> = xs.into_iter().zip(ys).map(|(p, q)| b.ins().select(cv, p, q)).collect();
                setp(&mut vals, &mut more, res[0], &ps);
            }
            Op::Select(c, x, y) => {
                let _t0 = v!(c);
                let _t1 = v!(x);
                let _t2 = v!(y);
                let r = b.ins().select(_t0, _t1, _t2);
                set1(&mut vals, r);
            }
            Op::Uextend(t, x) | Op::Sextend(t, x) | Op::Ireduce(t, x) => {
                let xv = v!(x);
                let from = b.func.dfg.value_type(xv);
                let to = ty(*t);
                let r = if from == to {
                    xv
                } else if from.bits() > to.bits() {
                    b.ins().ireduce(to, xv)
                } else if matches!(op, Op::Sextend(..)) {
                    b.ins().sextend(to, xv)
                } else {
                    b.ins().uextend(to, xv)
                };
                set1(&mut vals, r);
            }
            Op::Extend128(x, signed) => {
                let _t0 = v!(x);
                let _t1 = v!(x);
                let r = if *signed { b.ins().sextend(types::I128, _t0) } else { b.ins().uextend(types::I128, _t1) };
                set1(&mut vals, r);
            }
            Op::Reduce128(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().ireduce(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::Load(_, _, _) if facc.contains_key(&(i as u32)) => {
                let r = b.use_var(facc[&(i as u32)]);
                set1(&mut vals, r);
            }
            Op::Store(x, _, _) if facc.contains_key(&(i as u32)) => {
                let x = v!(x);
                b.def_var(facc[&(i as u32)], x);
            }
            // (a vector: its parts, at 16-byte steps; vector accesses are not known to be aligned)
            Op::Load(Type::Vec(w), a, off) => {
                let av = v!(a);
                let ps: Vec<ir::Value> = (0..w.bits() / 128).map(|k| b.ins().load(PART, simd::vflags(), av, *off + 16 * k as i32)).collect();
                setp(&mut vals, &mut more, res[0], &ps);
            }
            Op::Store(x, a, off) if matches!(fb.value_type(*x), Type::Vec(_)) => {
                let ps = vp!(x);
                let av = v!(a);
                for (k, p) in ps.into_iter().enumerate() {
                    b.ins().store(simd::vflags(), p, av, *off + 16 * k as i32);
                }
            }
            Op::Load(t, a, off) => {
                let _t0 = v!(a);
                let r = b.ins().load(ty(*t), MemFlagsData::trusted(), _t0, *off);
                set1(&mut vals, r);
            }
            Op::Store(x, a, off) => {
                let _t0 = v!(x);
                let _t1 = v!(a);
                b.ins().store(MemFlagsData::trusted(), _t0, _t1, *off);
            }
            Op::StackAddr(..) if remat => {}
            Op::StackAddr(s, off) => {
                let r = b.ins().stack_addr(types::I64, slots[&s.0], *off);
                set1(&mut vals, r);
            }
            Op::Jump(bk, args) => {
                let a: Vec<ir::BlockArg> = vsf!(args).into_iter().map(|x| x.into()).collect();
                b.ins().jump(blocks[&bk.0], &a);
                edge!(bk.0);
            }
            Op::Brif(c, tb, ta, eb, ea) => {
                let cv = v!(c);
                let ta: Vec<ir::BlockArg> = vsf!(ta).into_iter().map(|x| x.into()).collect();
                let ea: Vec<ir::BlockArg> = vsf!(ea).into_iter().map(|x| x.into()).collect();
                // (If the condition is a constant (a drop flag after a move, etc.), jump to the side taken: the other side becomes unreachable and disappears)
                let k = match b.func.dfg.value_def(b.func.dfg.resolve_aliases(cv)) {
                    ir::ValueDef::Result(inst, _) => match b.func.dfg.insts[inst] {
                        ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm } if fold_br => Some(imm.bits() != 0),
                        _ => None,
                    },
                    _ => None,
                };
                match k {
                    Some(true) => {
                        b.ins().jump(blocks[&tb.0], &ta);
                    }
                    Some(false) => {
                        b.ins().jump(blocks[&eb.0], &ea);
                    }
                    None => {
                        b.ins().brif(cv, blocks[&tb.0], &ta, blocks[&eb.0], &ea);
                    }
                }
                edge!(tb.0);
                edge!(eb.0);
            }
            Op::Return_(rs) => {
                let r: Vec<ir::Value> = vsf!(rs);
                b.ins().return_(&r);
            }
            Op::Trap => {
                b.ins().trap(TrapCode::unwrap_user(1));
            }
            Op::AtomicLoad(t, a) => {
                let _t0 = v!(a);
                let r = b.ins().atomic_load(ty(*t), MemFlagsData::trusted(), _t0);
                set1(&mut vals, r);
            }
            Op::AtomicStore(x, a) => {
                let _t0 = v!(x);
                let _t1 = v!(a);
                b.ins().atomic_store(MemFlagsData::trusted(), _t0, _t1);
            }
            Op::AtomicRmw(op, a, x) => {
                let _t0 = v!(a);
                let _t1 = v!(x);
                let o = match op {
                    minclif::AtomicOp::Xchg => ir::AtomicRmwOp::Xchg,
                    minclif::AtomicOp::Add => ir::AtomicRmwOp::Add,
                };
                let r = b.ins().atomic_rmw(ty(fb.value_type(*x)), MemFlagsData::trusted(), o, _t0, _t1);
                set1(&mut vals, r);
            }
            Op::AtomicCas(a, e, x) => {
                let _t0 = v!(a);
                let _t1 = v!(e);
                let _t2 = v!(x);
                let r = b.ins().atomic_cas(MemFlagsData::trusted(), _t0, _t1, _t2);
                set1(&mut vals, r);
            }
            Op::Fence => {
                b.ins().fence();
            }
            Op::TrapCode(c) => {
                b.ins().trap(TrapCode::unwrap_user(*c));
            }
            Op::Trapz(c) => {
                let _t0 = v!(c);
                b.ins().trapz(_t0, TrapCode::unwrap_user(1));
            }
            Op::Trapnz(c) => {
                let _t0 = v!(c);
                b.ins().trapnz(_t0, TrapCode::unwrap_user(1));
            }
            Op::TrapzCode(c, k) => {
                let _t0 = v!(c);
                b.ins().trapz(_t0, TrapCode::unwrap_user(*k));
            }
            Op::TrapnzCode(c, k) => {
                let _t0 = v!(c);
                b.ins().trapnz(_t0, TrapCode::unwrap_user(*k));
            }
            Op::Call(f, args, rets) => {
                let av: Vec<ir::Value> = vsf!(args);
                let pt: Vec<ir::Type> = av.iter().map(|&x| b.func.dfg.value_type(x)).collect();
                let rt: Vec<ir::Type> = rets.iter().flat_map(|&t| ctys(t)).collect();
                let n = fnames.get(&f.0).cloned().unwrap_or_default();
                let key = (n.clone(), pt.clone(), rt.clone());
                let fr = match frefs.get(&key) {
                    Some(&fr) => fr,
                    None => {
                        let mut s = Signature::new(CallConv::SystemV);
                        s.params = pt.iter().map(|&t| AbiParam::new(t)).collect();
                        s.returns = rt.iter().map(|&t| AbiParam::new(t)).collect();
                        let sr = b.import_signature(s);
                        let idx = name_of(&n, &mut names);
                        let nm = b.func.declare_imported_user_function(UserExternalName::new(0, idx));
                        let fr = b.import_function(ExtFuncData { name: ExternalName::user(nm), signature: sr, colocated: false, patchable: false });
                        frefs.insert(key, fr);
                        fr
                    }
                };
                let inst = b.ins().call(fr, &av);
                let rs = b.inst_results(inst).to_vec();
                let mut at = 0;
                for (k, &t) in rets.iter().enumerate() {
                    let n = nparts(t);
                    if let Some(&id) = res.get(k) {
                        setp(&mut vals, &mut more, id, &rs[at..at + n]);
                    }
                    at += n;
                }
            }
            Op::CallIndirect(p, args, rets) => {
                let av: Vec<ir::Value> = vsf!(args);
                let mut s = Signature::new(CallConv::SystemV);
                s.params = av.iter().map(|&x| AbiParam::new(b.func.dfg.value_type(x))).collect();
                s.returns = rets.iter().flat_map(|&t| ctys(t)).map(AbiParam::new).collect();
                let sr = b.import_signature(s);
                let pv = v!(p);
                let inst = b.ins().call_indirect(sr, pv, &av);
                let rs = b.inst_results(inst).to_vec();
                let mut at = 0;
                for (k, &t) in rets.iter().enumerate() {
                    let n = nparts(t);
                    if let Some(&id) = res.get(k) {
                        setp(&mut vals, &mut more, id, &rs[at..at + n]);
                    }
                    at += n;
                }
            }
            Op::Vconst(x) => {
                let r = simd::vconst(&mut b, *x);
                set1(&mut vals, r);
            }
            Op::Simd(sh, o, args) => {
                let mut xs: Vec<Vec<ir::Value>> = vec![];
                for x in args {
                    let ps = vp!(x);
                    xs.push(ps);
                }
                let ps = simd::lower(&mut b, *sh, *o, &xs, has_fma)?;
                setp(&mut vals, &mut more, res[0], &ps);
            }
            Op::SymbolAddr(n) => {
                let gv = match gvs.get(n) {
                    Some(&g) => g,
                    None => {
                        let idx = name_of(n, &mut names);
                        let nm = b.func.declare_imported_user_function(UserExternalName::new(0, idx));
                        let g = b.create_global_value(ir::GlobalValueData::Symbol {
                            name: ExternalName::user(nm),
                            offset: ir::immediates::Imm64::new(0),
                            colocated: false,
                            tls: false,
                        });
                        gvs.insert(n.clone(), g);
                        g
                    }
                };
                let r = b.ins().symbol_value(types::I64, gv);
                set1(&mut vals, r);
            }
            Op::F64const(x) => {
                let r = b.ins().f64const(*x);
                set1(&mut vals, r);
            }
            Op::F32const(x) => {
                let r = b.ins().f32const(*x);
                set1(&mut vals, r);
            }
            Op::Fadd(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fadd(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fsub(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fsub(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fmul(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fmul(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fdiv(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fdiv(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Sqrt(x) => {
                let _t0 = v!(x);
                let r = b.ins().sqrt(_t0);
                set1(&mut vals, r);
            }
            Op::Floor(x) => {
                let _t0 = v!(x);
                let r = b.ins().floor(_t0);
                set1(&mut vals, r);
            }
            Op::Ceil(x) => {
                let _t0 = v!(x);
                let r = b.ins().ceil(_t0);
                set1(&mut vals, r);
            }
            Op::Trunc(x) => {
                let _t0 = v!(x);
                let r = b.ins().trunc(_t0);
                set1(&mut vals, r);
            }
            Op::Fneg(x) => {
                let _t0 = v!(x);
                let r = b.ins().fneg(_t0);
                set1(&mut vals, r);
            }
            Op::Fabs(x) => {
                let _t0 = v!(x);
                let r = b.ins().fabs(_t0);
                set1(&mut vals, r);
            }
            Op::Fcopysign(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fcopysign(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fma(x, y, z) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let _t2 = v!(z);
                let r = b.ins().fma(_t0, _t1, _t2);
                set1(&mut vals, r);
            }
            Op::Bitcast(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().bitcast(ty(*t), MemFlagsData::new(), _t0);
                set1(&mut vals, r);
            }
            Op::Fcmp(c, x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fcmp(fcc(*c), _t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fpromote(x) => {
                let _t0 = v!(x);
                let r = b.ins().fpromote(types::F64, _t0);
                set1(&mut vals, r);
            }
            Op::Fdemote(x) => {
                let _t0 = v!(x);
                let r = b.ins().fdemote(types::F32, _t0);
                set1(&mut vals, r);
            }
            Op::FcvtFromSint(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().fcvt_from_sint(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::FcvtFromUint(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().fcvt_from_uint(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::FcvtToSintSat(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().fcvt_to_sint_sat(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::FcvtToUintSat(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().fcvt_to_uint_sat(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::Iconcat(lo, hi) => {
                let _t0 = v!(lo);
                let _t1 = v!(hi);
                let r = b.ins().iconcat(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Isplit(x) => {
                let _t0 = v!(x);
                let (lo, hi) = b.ins().isplit(_t0);
                vals.insert(res[0], lo);
                vals.insert(res[1], hi);
            }
            Op::Udiv128(x, y) | Op::Sdiv128(x, y) | Op::Urem128(x, y) | Op::Srem128(x, y) => {
                // (i128 division is libgcc: same as minclif)
                let n = match op {
                    Op::Udiv128(..) => "__udivti3",
                    Op::Sdiv128(..) => "__divti3",
                    Op::Urem128(..) => "__umodti3",
                    _ => "__modti3",
                };
                let mut s = Signature::new(CallConv::SystemV);
                s.params = vec![AbiParam::new(types::I128), AbiParam::new(types::I128)];
                s.returns = vec![AbiParam::new(types::I128)];
                let sr = b.import_signature(s);
                let idx = name_of(n, &mut names);
                let nm = b.func.declare_imported_user_function(UserExternalName::new(0, idx));
                let fr = b.import_function(ExtFuncData { name: ExternalName::user(nm), signature: sr, colocated: false, patchable: false });
                let (xv, yv) = (v!(x), v!(y));
                let inst = b.ins().call(fr, &[xv, yv]);
                let r = b.inst_results(inst)[0];
                set1(&mut vals, r);
            }
            Op::UaddOverflow(x, y) | Op::UsubOverflow(x, y) | Op::SaddOverflow(x, y) | Op::SsubOverflow(x, y) | Op::UmulOverflow(x, y) | Op::SmulOverflow(x, y) => {
                let (xv, yv) = (v!(x), v!(y));
                let (r, o) = match op {
                    Op::UaddOverflow(..) => b.ins().uadd_overflow(xv, yv),
                    Op::UsubOverflow(..) => b.ins().usub_overflow(xv, yv),
                    Op::SaddOverflow(..) => b.ins().sadd_overflow(xv, yv),
                    Op::SsubOverflow(..) => b.ins().ssub_overflow(xv, yv),
                    Op::UmulOverflow(..) => b.ins().umul_overflow(xv, yv),
                    _ => b.ins().smul_overflow(xv, yv),
                };
                vals.insert(res[0], r);
                vals.insert(res[1], o);
            }
            Op::Umulhi(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().umulhi(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Clz(x) => {
                let _t0 = v!(x);
                let r = b.ins().clz(_t0);
                set1(&mut vals, r);
            }
            Op::Ctz(x) => {
                let _t0 = v!(x);
                let r = b.ins().ctz(_t0);
                set1(&mut vals, r);
            }
            Op::Popcnt(x) => {
                let _t0 = v!(x);
                let r = b.ins().popcnt(_t0);
                set1(&mut vals, r);
            }
            Op::Bswap(x) => {
                let _t0 = v!(x);
                let r = b.ins().bswap(_t0);
                set1(&mut vals, r);
            }
            Op::Rotl(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().rotl(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Rotr(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().rotr(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Smin(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().smin(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Umin(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().umin(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Smax(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().smax(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Umax(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().umax(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Iabs(x) => {
                let _t0 = v!(x);
                let r = b.ins().iabs(_t0);
                set1(&mut vals, r);
            }
        }
        if makes_values {
            for id in res {
                if let (Some(&var), Some(&v)) = (gvars.get(id), vals.get(id)) {
                    b.def_var(var, v);
                }
                if let (Some(vs), Some(&v)) = (gvp.get(id), vals.get(id)) {
                    let mut ps = vec![v];
                    ps.extend(more.get(id).cloned().unwrap_or_default());
                    for (var, p) in vs.clone().into_iter().zip(ps) {
                        b.def_var(var, p);
                    }
                }
            }
        }
    }
    b.seal_all_blocks();
    b.finalize(isa.frontend_config());
    // (Slots zeroed at the entry (drop flags): they are created midway, so they are written at the head of the entry after assembling)
    if !zero.is_empty() {
        use cranelift_codegen::cursor::{Cursor, FuncCursor};
        let mut pos = FuncCursor::new(&mut func).at_first_insertion_point(entry);
        for z in zero {
            if let Some(&ss) = slots.get(&z.0) {
                let k = pos.ins().iconst(types::I8, 0);
                pos.ins().stack_store(types::I64, k, ss, 0);
            }
        }
    }
    // (ZUSTC_CLIF_STATS=1: prints counts per instruction kind. ZUSTC_CLIF_DUMP=1: prints the IR)
    if std::env::var_os("ZUSTC_CLIF_STATS").is_some() {
        let mut n = 0;
        for bl in func.layout.blocks() {
            for i in func.layout.block_insts(bl) {
                n += 1;
                eprintln!("CLIFOP {}", func.dfg.insts[i].opcode());
            }
        }
        eprintln!("CLIFFN insts={n} blocks={} slots={} values={}", func.layout.blocks().count(), func.sized_stack_slots.len(), func.dfg.num_values());
    }
    if std::env::var_os("ZUSTC_CLIF_DUMP").is_some() {
        eprintln!("{}", func.display());
    }
    let stat_n = if std::env::var_os("ZUSTC_CLIF_TIME").is_some() { Some((func.dfg.num_insts(), func.layout.blocks().count(), std::time::Instant::now())) } else { None };
    ctx.func = func;
    ctx.compile(isa, &mut ControlPlane::default()).map_err(|e| format!("cranelift: {:?}", e.inner))?;
    if let Some((n, bl, t)) = stat_n {
        eprintln!("CLIFTIME {n} {bl} {}", t.elapsed().as_micros());
    }
    let code = ctx.compiled_code().unwrap();
    let mut relocs = vec![];
    for r in code.buffer.relocs() {
        let name = match r.target {
            RelocTarget::ExternalName(ExternalName::User(reff)) => {
                let n = &ctx.func.params.user_named_funcs()[reff];
                names.get(n.index as usize).cloned().unwrap_or_default()
            }
            ref t => return Err(format!("cranelift: unsupported reloc target {t:?}")),
        };
        let kind = match r.kind {
            CReloc::X86CallPLTRel4 | CReloc::X86CallPCRel4 => 4,
            CReloc::X86GOTPCRel4 => 9,
            CReloc::X86PCRel4 => 2,
            CReloc::Abs8 => 1,
            k => return Err(format!("cranelift: unsupported reloc kind {k:?}")),
        };
        relocs.push(Reloc { offset: r.offset, name, kind, addend: r.addend });
    }
    let traps: Vec<(u32, u8)> = code.buffer.traps().iter().map(|t| (t.offset, t.code.as_raw().get())).collect();
    let data = code.buffer.data().to_vec();
    ctx.clear();
    CTXS.with(|c| *c.borrow_mut() = Some((ctx, fctx)));
    Ok((data, relocs, traps))
}
