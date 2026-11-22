//! codegen.rs (syn の構文木 → Cranelift) を、自前の構文木 (src/parse.rs) → minclif (src/minclif.rs) に移したもの。
//! Cranelift と同じ形の API は clif.rs が minclif の上に作る。syn の構文木の代わりは parse.rs の構文木、
//! syn 版の型推論 (infer) の代わりは ast_infer。外部クレートに依存しないので zustc 自身でもコンパイルできる。
//!
//! 値の表現は codegen.rs と同じ:
//! - 整数 / bool / 参照: 値 (参照はポインタ)
//! - struct / enum / 配列 / fat pointer: メモリ上にあり「値」はそのアドレス。let / 代入 / 引数 / return でコピーする。
//!   引数は呼び出し側が一時領域にコピーしてそのアドレスを渡し、戻り値は隠れ第 1 引数 (sret) の場所に書く。
//! - ローカル変数: アドレスを取られないスカラーは Variable、それ以外はスタックスロット。
//!
//! 出力は Blob (機械語 + シンボリックな再配置)。呼び出し先などは名前 `\x01f<id>` のように番号で書き、
//! Blob を作るときに Target に直す。
//!
//! 移植の決まり (codegen.rs との対応):
//! - syn の Span は使わない (Sp = u32 の 0)。エラーの場所は今は出さない
//! - syn の個別のノード (`&syn::ExprMethodCall` など) を受けていたメソッドは、式全体 (`&Expr`) を受けて中で分解する

#![allow(clippy::too_many_arguments)]

pub mod borrowck;
pub mod regionck;
pub mod clif;
pub mod lit;
mod p1_expr;
mod p2_call;
mod p3_ops;
mod p4_methods;
mod p5_loops_macros;
mod p6_match_drop;
mod p7_fmt_hash_cmp;
mod p8_misc;
mod p9_inline;
mod exhaust;
mod simd;
pub use p9_inline::{inline_candidate, Inline, MAX_TOKENS};
#[allow(unused_imports)]
pub use p1_expr::*;
#[allow(unused_imports)]
pub use p2_call::*;
#[allow(unused_imports)]
pub use p3_ops::*;
#[allow(unused_imports)]
pub use p4_methods::*;
#[allow(unused_imports)]
pub use p5_loops_macros::*;
#[allow(unused_imports)]
pub use p6_match_drop::*;
#[allow(unused_imports)]
pub use p7_fmt_hash_cmp::*;
#[allow(unused_imports)]
pub use p8_misc::*;

use crate::fxhash::{FxMap as HashMap, FxSet as HashSet};
use std::sync::Mutex;

use crate::acg::clif::{AbiParam, FuncRef, FunctionBuilder, Signature, TrapCode, types};
use crate::ir::{Block, IntCC, StackSlot, Type, Value, Variable};
pub use crate::ir::FloatCC;
use lit::Lit;

use crate::parse::{Expr, Pat, Path, Stmt};
use crate::program::{FnSig, Insts, PPath, Program, TyEnv};
use crate::types::{Ty, intern};

pub type R<T> = Result<T, String>;

/// ポインタ型 (x86-64 前提)
pub const PTR: Type = types::I64;

/// エラーの場所 (今は使わない)
/// ソース上の位置 (アイテムの中の行・桁)。分からないところは SP (0 行目)
pub type Sp = crate::parse::Pos;
pub const SP: Sp = crate::parse::Pos { line: 0, col: 0 };

pub fn err<T>(_: Sp, msg: impl std::fmt::Display) -> R<T> {
    // (ACG_PANIC_ERR=1: エラーの場所をバックトレースで調べる)
    if std::env::var_os("ACG_PANIC_ERR").is_some() {
        panic!("{msg}");
    }
    Err(msg.to_string())
}

pub fn mf() -> clif::MemFlagsData {
    clif::MemFlagsData::trusted()
}

// ---------------------------------------------------------------------------
// 出力形式 (codegen.rs の Blob と同じ)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Blob {
    pub align: u32,
    pub code: Vec<u8>,
    pub relocs: Vec<BlobReloc>,
    pub datas: Vec<Vec<u8>>,
    pub vtables: Vec<Vec<u32>>,
    pub traps: Vec<(u32, u32, u32)>,
    pub line_base: Option<u32>,
    pub code_ref: Option<(u32, u64)>,
}

#[derive(Clone, Debug)]
pub struct BlobReloc {
    pub offset: u32,
    /// codegen::RELOC_KINDS の番号 (4 = X86CallPLTRel4, 5 = X86GOTPCRel4)
    pub kind: u8,
    pub addend: i64,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub enum Target {
    Func(u32),
    SelfOffset(u32),
    Data(u32),
    Static(u32),
    Vtable(u32),
    /// 外部の名前 (libgcc の `__multi3` など)
    Sym(String),
}

/// 再配置の名前 (`\x01f12` / `\x01d0` / `\x01s3` / `\x01v1`)
/// values larger than this are copied / zeroed by a libc call
pub(crate) const BIG_COPY: u32 = 4096;

fn sym_name(kind: char, i: u32) -> String {
    format!("\x01{kind}{i}")
}

fn target_of(name: &str) -> Option<Target> {
    let r = name.strip_prefix('\x01')?;
    let (k, n) = r.split_at(1);
    let i: u32 = n.parse().ok()?;
    Some(match k {
        "f" => Target::Func(i),
        "d" => Target::Data(i),
        "s" => Target::Static(i),
        "v" => Target::Vtable(i),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// シグネチャ
// ---------------------------------------------------------------------------

pub fn clif_ty(t: Ty) -> Option<Type> {
    Some(match t {
        t if t.is_agg() => PTR,
        Ty::Ref(..) | Ty::Ptr(..) => PTR,
        Ty::F32 => types::F32,
        Ty::F64 => types::F64,
        Ty::Vec(b) => Type::Vec(crate::ir::VBits::from_bits(b as u32)?),
        t => match t.bits() {
            0 => return None,
            8 => types::I8,
            16 => types::I16,
            32 => types::I32,
            128 => types::I128,
            _ => types::I64,
        },
    })
}

fn abi_param(t: Ty) -> AbiParam {
    AbiParam::new(clif_ty(t).unwrap())
}

pub fn is_sret(s: &FnSig) -> bool {
    s.ret.is_agg() && !s.is_main
}

pub fn clif_sig(s: &FnSig) -> Signature {
    let mut sig = Signature::default();
    if is_sret(s) {
        sig.params.push(AbiParam::new(PTR));
    }
    for &p in &s.params {
        if !p.is_agg() && clif_ty(p).is_none() {
            continue;
        }
        sig.params.push(abi_param(p));
    }
    if s.is_main {
        sig.returns.push(AbiParam::new(types::I32));
    } else if !is_sret(s) && clif_ty(s.ret).is_some() {
        sig.returns.push(abi_param(s.ret));
    }
    sig
}

// ---------------------------------------------------------------------------
// 関数のコンパイル
// ---------------------------------------------------------------------------

/// 関数 1 個 (自前の構文木)
#[derive(Clone, Debug)]
pub struct ItemFn {
    pub attrs: Vec<String>,
    pub sig: crate::parse::FnSig,
    pub block: crate::parse::Block,
}

/// panic の trap の最初のコード (1〜4 はほかの trap)。関数ごとに 245 か所まで場所を覚える
pub const TRAP_SITE_BASE: u8 = 5;
pub const TRAP_SITE_MAX: usize = 245;

/// 1 関数をコンパイルする。返り値は (Blob, 要求したインスタンス)
pub fn compile(prog: &Program, insts: &Mutex<Insts>, id: u32, f: &ItemFn, line_base: u32, inl: &Inline) -> R<(Compiled, Vec<u32>)> {
    // (a region check begun late (`'static` required where the gate left the function out) that met a local bound
    // before it began, with no tree: the function again, checked from its start)
    // (rerun out of band: the result is compile's own, returned as it is, not copied into another)
    let mut rerun = false;
    let r = compile_once(prog, insts, id, f, line_base, inl, false, &mut rerun);
    if rerun && r.is_ok() {
        if regionck::debug() {
            eprintln!("regionck: {} again, checked from its start", f.sig.name);
        }
        return compile_once(prog, insts, id, f, line_base, inl, true, &mut rerun);
    }
    r
}

#[allow(clippy::too_many_arguments)]
fn compile_once(prog: &Program, insts: &Mutex<Insts>, id: u32, f: &ItemFn, line_base: u32, inl: &Inline, force_rg: bool, rerun_out: &mut bool) -> R<(Compiled, Vec<u32>)> {
    let (sig, env, body_id, fnptr, inst_args, clo_rg, lenient) = {
        let g = insts.lock().unwrap();
        let b = g.body_of(id);
        (g.sig(prog, id).clone(), g.env(prog, id), b, g.get(id).is_some_and(|i| i.fnptr), g.get(id).map(|i| i.args.clone()), g.closure_rg.get(&b).cloned(), g.lenient)
    };
    // (zustc check's last round: instances with placeholders for type parameters, and those they request (only
    // there are placeholders): see below)
    let placeholder = lenient;
    // (a closure's fn-pointer instance: the body without the `__env` parameter (it has no captures))
    let f_np;
    let f = if fnptr {
        let mut g = f.clone();
        if !g.sig.params.is_empty() {
            g.sig.params.remove(0);
        }
        f_np = g;
        &f_np
    } else {
        f
    };
    // クロージャの本体: キャプチャ (名前, 変数の型, by-ref か, オフセット)
    let mut caps: Vec<(String, Ty, bool, u32)> = vec![];
    if let Some(cd) = prog.closures.get(&body_id) {
        if let Some(Ty::Ptr(_, Ty::Struct(sid))) = sig.params.first() {
            let sd = &prog.structs[*sid as usize];
            for (i, (n, (_, ft, off))) in cd.captures.iter().zip(&sd.fields).enumerate() {
                match (cd.moved.get(i).copied().unwrap_or(false), ft) {
                    (false, Ty::Ptr(_, t)) => caps.push((n.clone(), **t, true, *off)),
                    _ => caps.push((n.clone(), *ft, false, *off)),
                }
            }
        }
    }
    let (root, base_k) = match prog.closures.get(&body_id) {
        Some(cd) => (cd.parent, cd.k + 1),
        None => (body_id, 0),
    };
    let mut closure_ix = HashMap::default();
    let mut taken = addr_taken(&f.block, &prog.prim_mut_methods);
    for (i, (a, t)) in crate::ast_closure::closure_slots(&f.block).into_iter().enumerate() {
        let k = base_k + i as u32;
        // (本体に書かれたクロージャはアドレスで、マクロの引数の中のものは式の文字列で引く)
        // (複製された式のために文字列でも引けるようにする: 同じ文字列なら最初のもの)
        if let Some(a) = a {
            closure_ix.insert(format!("@{a}"), k);
        }
        closure_ix.entry(t).or_insert(k);
        if let Some(cd) = prog.closure_by.get(&(root, k)).and_then(|i| prog.closures.get(i)) {
            for (i, n) in cd.captures.iter().enumerate() {
                if !cd.moved.get(i).copied().unwrap_or(false) {
                    taken.insert(n.clone());
                }
            }
        }
    }
    let extra: Vec<(String, Ty)> = caps.iter().map(|c| (c.0.clone(), c.1)).collect();
    let user_code = prog.item_of.get(body_id as usize).copied().flatten().is_some_and(|i| i < prog.prelude_from);
    let ((lets, _, _), margs, texts) = crate::ast_infer::infer_ctx_margs(prog, f.sig.recv.is_some(), &f.sig.params, &f.block, &sig, &env, &extra, Some(sig.ret), Some((root, base_k)));
    let bk_level = if user_code { borrowck::level() } else { 0 };
    let mut cx = Cx {
        b: FunctionBuilder::new(&clif_sig(&sig)),
        bk: borrowck::Bk { on: bk_level > 0, level: bk_level, line_base, ..Default::default() },
        // (a function's own body, not a generic one's instance nor a closure's: see regionck)
        // (a generic function's instance: the generic signature, its type parameters' regions not followed)
        // (a closure's: with the region signature the bound it is passed to (or its annotations) gives it)
        rg: regionck::Rg {
            on: bk_level >= 2 && !fnptr && ((caps.is_empty() && prog.closures.get(&body_id).is_none()) || clo_rg.is_some()) && regionck::enabled(),
            base: body_id,
            inst_args: if clo_rg.is_some() { vec![] } else { inst_args.unwrap_or_default() },
            closure: clo_rg,
            force: force_rg,
            ..Default::default()
        },
        prog,
        scopes: vec![vec![]],
        loops: vec![],
        ret: sig.ret,
        is_main: sig.is_main,
        user_code,
        raw_ref: 0,
        const_hint: None,
        // (a closure's body is checked too: one written inside `unsafe { }` comes wrapped in one, see synth_closure)
        unsafe_ok: (!user_code || sig.is_unsafe) as u32,
        overflow_checks: prog.overflow_checks && (user_code || f.attrs.iter().any(|a| a.contains("rustc_inherit_overflow_checks"))),
        traps: vec![],
        line_base,
        fn_aliases: vec![],
        stmt_i: 0,
        loc_used: false,
        index_panic_block: None,
        caller_loc: matches!(
            prog.names.get(body_id as usize).map(|n| n.as_str()),
            Some(
                "Option::unwrap" | "Option::expect" | "Result::unwrap" | "Result::expect" | "Result::unwrap_err" | "Result::expect_err"
                    | "RefCell::borrow" | "RefCell::borrow_mut"
            )
        ) && prog.item_of.get(body_id as usize).copied().flatten().is_some_and(|i| i >= prog.prelude_from),
        caller_locs: vec![],
        self_sid: env.self_sid,
        sret: None,
        fresh: Default::default(),
        ret_call: None,
        dst_hint: None,
        adt_targs: None,
        frefs: HashMap::default(),
        lets,
        addr_taken: taken,
        caps,
        root,
        generic_args: generic_args_of(prog, &f.sig.params, root),
        closure_ix,
        explicit_traits: HashMap::default(),
        explicit_trait: None,
        keep_overload: std::cell::Cell::new(None),
        closure_hint: None,
        closure_bound: None,
        closure_prelude: false,
        closure_fn_only: false,
        hr_lost: vec![],
        clo_ret_param: vec![],
        seg_self: None,
        loop_bounds: vec![],
        pending_bound: None,
        pending_pair: None,
        checked: (None, vec![]),
        hoisted: vec![],
        in_fmt_args: 0,
        late_flags: vec![],
        owned: vec![],
        temps: vec![],
        owned_binds: 0,
        last_scrut: None,
        env,
        insts,
        requested: vec![],
        datas: vec![],
        vtables: vec![],
        cmp_depth: 0,
        pending_label: None,
        keep: vec![],
        margs,
        texts,
        mbase: Default::default(),
        mstart: Default::default(),
        inl,
        inline_ret: None,
        inline_depth: 0,
        inlined: 0,
        try_into_hint: None,
    };
    let r = if prog.index.get(crate::prelude::DROP_GLUE) == Some(&body_id) {
        cx.drop_glue_body(&sig)
    } else if prog.index.get("__exr_hash") == Some(&body_id) {
        cx.hash_glue_body(&sig)
    } else if prog.index.get("__exr_cmp") == Some(&body_id) {
        cx.cmp_glue_body(&sig)
    } else if prog.index.get("__exr_eq") == Some(&body_id) {
        cx.eq_glue_body(&sig)
    } else if prog.index.get("__exr_clone") == Some(&body_id) {
        cx.clone_glue_body(&sig)
    } else if prog.index.get("__exr_default") == Some(&body_id) {
        cx.default_glue_body(&sig)
    } else if prog.index.get("__exr_display") == Some(&body_id) {
        cx.fmt_glue_body(&sig, false)
    } else if prog.index.get("__exr_debug") == Some(&body_id) {
        cx.fmt_glue_body(&sig, true)
    } else {
        cx.body(f, &sig)
    };
    // (the struct types' implied relations, for the next functions: taken when the check began)
    if cx.rg.imp_taken {
        let ic = std::mem::take(&mut cx.rg.imp_cache);
        let mut g = insts.lock().unwrap();
        if g.imp_cache.len() < ic.len() {
            g.imp_cache = ic;
        }
    }
    // (an instance with placeholders for type parameters (zustc check of a generic fn nothing calls) whose typeck
    // fails, as a placeholder cannot do all its bounds allow (a trait's method, a coercion to `dyn`): not checked
    // further, nothing reported. Its borrow and region checks' errors are)
    // (not an error of the borrow check's kind lowering itself returns: `m[k] = v` of a HashMap is E0594 whatever T is)
    let borrow_kind = |e: &str| {
        ["cannot assign", "cannot borrow", "cannot move out", "use of moved", "borrow of moved", "does not live long enough", "may not live long enough", "(E0594)", "(E0596)", "(E0384)"]
            .iter()
            .any(|k| e.contains(k))
    };
    if placeholder && r.as_ref().is_err_and(|e| !borrow_kind(e)) {
        if regionck::debug() {
            eprintln!("regionck: placeholder instance not checked: {}", r.as_ref().err().map(|e| e.as_str()).unwrap_or(""));
        }
        let b = cx.b;
        return Ok((Compiled { rec: b.m, late: vec![], datas: vec![], vtables: vec![], infos: vec![], loc_used: false, line_base }, vec![]));
    }
    r?;
    cx.bk_check()?;
    let rerun = cx.rg.rerun && !force_rg;
    *rerun_out = rerun;
    if !rerun {
        cx.rg_check()?;
    }
    // (入口以外で作った drop フラグは入口で 0 にする: minclif はフラグのスロットを関数の最初に 0 にする)
    let late = std::mem::take(&mut cx.late_flags);
    let requested = std::mem::take(&mut cx.requested);
    let datas = std::mem::take(&mut cx.datas);
    let vtables = std::mem::take(&mut cx.vtables);
    let infos = std::mem::take(&mut cx.traps);
    let loc_used = cx.loc_used;
    let b = cx.b;
    Ok((Compiled { rec: b.m, late, datas, vtables, infos, loc_used, line_base }, requested))
}

/// コード生成の結果: 記録した命令の列と、Blob を作るのに要るもの。機械語にするのはバックエンド
/// (codegen の lower)、check はしない (empty_blob)
pub struct Compiled {
    pub rec: crate::ir::Recorder,
    /// 入口で 0 にする drop フラグのスロット
    pub late: Vec<StackSlot>,
    pub datas: Vec<Vec<u8>>,
    pub vtables: Vec<Vec<u32>>,
    infos: Vec<(u32, u32)>,
    loc_used: bool,
    line_base: u32,
}

impl Compiled {
    /// 機械語なしの Blob (check の検査済みの印)
    pub fn empty_blob() -> Blob {
        Blob { align: 16, code: vec![], relocs: vec![], datas: vec![], vtables: vec![], traps: vec![], line_base: None, code_ref: None }
    }

    /// バックエンドが作った (機械語, 再配置, trap) から Blob を作る
    pub fn into_blob(self, code: Vec<u8>, relocs: Vec<crate::ir::Reloc>, traps_raw: Vec<(u32, u8)>) -> Blob {
        let Compiled { datas, vtables, infos, loc_used, line_base, .. } = self;
        let mut out = vec![];
        for r in relocs {
            let target = target_of(&r.name).unwrap_or_else(|| Target::Sym(r.name.clone()));
            // (ELF の種類 → Blob の番号: PLT32 4、GOTPCREL 5、PC32 6、64 7)
            let kind = match r.kind {
                9 => 5,
                2 => 6,
                1 => 7,
                _ => 4,
            };
            out.push(BlobReloc { offset: r.offset, kind, addend: r.addend, target });
        }
        let mut traps = vec![];
        for (off, c) in traps_raw {
            let c = c as usize;
            if (TRAP_SITE_BASE as usize..TRAP_SITE_BASE as usize + infos.len()).contains(&c) && off < 1 << 20 {
                let (info, msg) = infos[c - TRAP_SITE_BASE as usize];
                traps.push((off, info, msg));
            }
        }
        Blob { align: 16, code, relocs: out, datas, vtables, traps, line_base: loc_used.then_some(line_base), code_ref: None }
    }
}

/// `&x` / `&mut x.f` などでアドレスを取られるローカル変数名 (shadowing は無視した過大近似)
pub(crate) fn addr_taken(b: &crate::parse::Block, mut_methods: &crate::fxhash::FxSet<String>) -> HashSet<String> {
    fn root(e: &Expr) -> Option<String> {
        match e {
            Expr::Path(None, p) => ident_of(p).map(|s| s.to_string()),
            Expr::Field(b, _, _) => root(b),
            Expr::Paren(x) => root(x),
            _ => None,
        }
    }
    // (`let ref mut y = x;`: y points at x itself. A shared `ref` binds a copy in memory (pat_bind): x does not
    // change while y is in use)
    fn ref_lets(b: &crate::parse::Block, out: &mut HashSet<String>) {
        for st in &b.stmts {
            if let Stmt::Let(pat, _, Some(init), ..) = st {
                // (most bind a name by value: no walk of the pattern)
                if matches!(pat, Pat::Ident(false, _, _, None) | Pat::Wild) {
                    continue;
                }
                if borrowck::pat_binds_ref_mut(pat) {
                    if let Some(n) = root(init) {
                        out.insert(n);
                    }
                }
            }
        }
    }
    fn visit(e: &Expr, out: &mut HashSet<String>, mm: &crate::fxhash::FxSet<String>) {
        match e {
            Expr::Ref(_, x) | Expr::RawRef(_, x) => {
                if let Some(n) = root(x) {
                    out.insert(n);
                }
            }
            Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) | Expr::While(_, _, b) | Expr::For(_, _, _, b) | Expr::If(_, b, _) => ref_lets(b, out),
            // (`match x { ref mut y => .. }`, `if let Some(ref mut y) = x`: the same)
            Expr::Match(x, arms) if arms.iter().any(|a| borrowck::pat_binds_ref_mut(&a.pat)) => {
                if let Some(n) = root(x) {
                    out.insert(n);
                }
            }
            Expr::Let(pat, x) if borrowck::pat_binds_ref_mut(pat) => {
                if let Some(n) = root(x) {
                    out.insert(n);
                }
            }
            // (`x.inc()` with `fn inc(&mut self)` of a number: x is borrowed mutably)
            Expr::MethodCall(r, m, ..) if !mm.is_empty() && mm.contains(m.as_str()) => {
                if let Some(n) = root(r) {
                    out.insert(n);
                }
            }
            Expr::Macro(p, g, _) => {
                let name = p.segs.last().map(|s| s.name.as_str()).unwrap_or("");
                if matches!(name, "addr_of" | "addr_of_mut") {
                    if let Some(es) = crate::ast_closure::macro_args(g) {
                        if let Some(n) = es.first().and_then(root) {
                            out.insert(n);
                        }
                    }
                }
                if matches!(name, "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne") {
                    if let Some(es) = crate::ast_closure::macro_args(g) {
                        for e in es.iter().take(2) {
                            if let Some(n) = root(e) {
                                out.insert(n);
                            }
                        }
                    }
                }
                let es = crate::ast_closure::macro_exprs(g);
                if es.is_empty() {
                    if let Some(es) = crate::ast_closure::macro_args(g) {
                        for e in &es {
                            match e {
                                Expr::Ref(_, x) => visit(x, out, mm),
                                Expr::Path(..) | Expr::Lit(_, _) => {}
                                e => visit(e, out, mm),
                            }
                        }
                    }
                }
                for e in &es {
                    visit(e, out, mm);
                }
                return;
            }
            _ => {}
        }
        crate::ast_closure::each_child_pub(e, &mut |c| visit(c, out, mm));
    }
    let mut out = HashSet::default();
    let blk = Expr::Block(None, false, b.clone());
    visit(&blk, &mut out, mut_methods);
    out
}

/// エラーの場所にする位置: メソッド呼び出しはメソッド名 (rustc・syn 版と同じ)、ほかは式の先頭
pub fn err_pos(e: &Expr) -> Sp {
    match e {
        Expr::MethodCall(_, _, _, _, s) | Expr::Field(_, _, s) => s.at,
        e => expr_start(e),
    }
}

/// 式の先頭のソース上の位置 (syn の `e.span()` の始まり)。位置を持つ式か、その一番左の子から。分からなければ SP
pub fn expr_start(e: &Expr) -> Sp {
    match e {
        Expr::Unary(_, _, s) | Expr::Binary(_, _, _, s) | Expr::AssignOp(_, _, _, s) | Expr::MethodCall(_, _, _, _, s) | Expr::Index(_, _, s) | Expr::Macro(_, _, s) => s.start,
        Expr::Field(_, _, s) => s.start,
        Expr::Call(x, _) | Expr::Cast(x, _) | Expr::Try(x) | Expr::Paren(x) | Expr::Assign(x, _) => expr_start(x),
        Expr::Range(Some(x), _, _) => expr_start(x),
        Expr::Path(_, p) | Expr::Struct(p, _, _) => p.pos,
        Expr::Lit(_, p) => *p,
        Expr::Closure(.., p) => *p,
        // (no position of their own: that of their first part, usually on the same line)
        Expr::Ref(_, x) | Expr::RawRef(_, x) | Expr::Repeat(x, _) | Expr::Let(_, x) | Expr::Match(x, _) | Expr::If(x, _, _) => expr_start(x),
        Expr::While(_, x, _) | Expr::For(_, _, x, _) | Expr::Return(Some(x)) | Expr::Break(_, Some(x), _) => expr_start(x),
        // (`break` / `continue` / `break 'a`: the keyword, or the label when there is one)
        Expr::Break(_, None, p) | Expr::Continue(_, p) => *p,
        Expr::Array(es) | Expr::Tuple(es) => es.first().map_or(SP, expr_start),
        _ => SP,
    }
}

/// メッセージが `行:桁: ` で始まるか
pub fn has_loc(m: &str) -> bool {
    // (数字の並び `:` 数字の並び `:` で始まる)
    let b = m.as_bytes();
    let mut i = 0;
    for _ in 0..2 {
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == st || i >= b.len() || b[i] != b':' {
            return false;
        }
        i += 1;
    }
    true
}

/// 1 つの名前だけのパス (`x`)
/// `mut x` (for the borrow checker)
pub fn pat_mut(p: &Pat) -> Option<bool> {
    match p {
        Pat::Ident(false, m, _, None) => Some(*m),
        _ => None,
    }
}

pub fn ident_of(p: &Path) -> Option<&str> {
    if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].fn_args.is_none() {
        Some(&p.segs[0].name)
    } else {
        None
    }
}

/// パスが 1 つの名前 n か (`p.path.is_ident(n)`)
pub fn is_ident(p: &Path, n: &str) -> bool {
    ident_of(p) == Some(n)
}

/// 名前だけのパス
pub fn path_of(n: &str) -> Path {
    Path { global: false, segs: vec![crate::parse::Seg { name: n.to_string(), args: vec![], fn_args: None, text: n.to_string() }], pos: Default::default() }
}

/// 名前だけのパスの式 (`x`)
pub fn path_expr(n: &str) -> Expr {
    Expr::Path(None, path_of(n))
}

/// メソッド呼び出しの式を作る
pub fn mcall(recv: Expr, name: &str, args: Vec<Expr>) -> Expr {
    Expr::MethodCall(Box::new(recv), name.to_string(), vec![], args, crate::parse::Span::default())
}

/// 添字の検査を覚えるときの長さ
#[derive(Clone, Copy, PartialEq)]
pub enum LenKey {
    Const(i64),
    Val(Value),
}

/// 式の評価結果。Unit / Never のとき値は None。struct のときはアドレス。
pub type V = (Option<Value>, Ty);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Loc {
    Var(Variable),
    Slot(StackSlot),
    Ptr(Value),
    /// (番号は借用検査が変数を見分けるため)
    Zst(u32),
}

#[derive(Clone, Copy)]
pub enum Place {
    Var(Variable, Ty),
    Mem(Value, i32, Ty),
    Zst(Ty),
}

impl Place {
    pub fn ty(&self) -> Ty {
        match *self {
            Place::Var(_, t) | Place::Mem(_, _, t) | Place::Zst(t) => t,
        }
    }
}

pub struct Loop {
    pub cont: Block,
    pub brk: Block,
    pub breaks: usize,
    pub label: Option<String>,
    pub val: Option<Ty>,
    pub is_block: bool,
    pub depth: usize,
    pub temps: usize,
    /// a `while` / `for` loop (and a while's condition): the kind it names in E0571, for a `break` with a value
    /// (None: a `loop` or a labeled block, which `break v` may leave)
    pub no_value: Option<&'static str>,
}

#[derive(Clone, Copy)]
pub struct Owned {
    pub loc: Loc,
    pub ty: Ty,
    pub flag: StackSlot,
    pub depth: usize,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BindMode {
    /// 値として束縛 (struct / enum はコピー)
    Move,
    /// 参照を match したときのデフォルト束縛モード: フィールドへの参照として束縛
    Ref(bool),
}

pub struct Cx<'p> {
    pub b: FunctionBuilder,
    /// 借用検査の記録
    pub bk: borrowck::Bk,
    /// Region checking against the signature
    pub rg: regionck::Rg,
    pub prog: &'p Program,
    pub scopes: Vec<Vec<(String, Loc, Ty)>>,
    pub loops: Vec<Loop>,
    pub pending_label: Option<String>,
    pub ret: Ty,
    pub is_main: bool,
    pub user_code: bool,
    /// > 0 inside `unsafe { }`, an `unsafe fn`, or code not checked for E0133 (the prelude)
    pub unsafe_ok: u32,
    /// > 0 under `&raw const` / `&raw mut` / `addr_of!` (a raw reference to a `static mut` is safe)
    pub raw_ref: u32,
    /// the expected type at a use of a const whose type names its impl's parameters (`impl<T> S<T> { const C: Option<T> = None; }`)
    pub const_hint: Option<Ty>,
    pub overflow_checks: bool,
    pub traps: Vec<(u32, u32)>,
    pub line_base: u32,
    /// `let f1 = f;` with f a generic fn: f1 stands for the path f (each call picks its own instance)
    pub fn_aliases: Vec<(String, &'static Expr)>,
    /// the index of the statement being compiled in the innermost block, for errors that carry no position of their own
    pub stmt_i: usize,
    pub loc_used: bool,
    pub index_panic_block: Option<Block>,
    pub caller_loc: bool,
    /// The method calls being lowered that store their location for a panic in the callee (`unwrap`, a RefCell's
    /// `borrow`): (the method's name, line << 16 | column)
    pub caller_locs: Vec<(String, i64)>,
    pub self_sid: Option<u32>,
    pub sret: Option<Value>,
    /// 呼び出しの戻り値を書かせた一時領域 (まだ誰も使っていない): 値渡し・let ではコピーせずにそのまま使う
    pub fresh: crate::fxhash::FxMap<u32, StackSlot>,
    /// 戻り値になる呼び出しの式 (関数の末尾・return の式): その呼び出しには sret の場所に直接書かせる
    pub ret_call: Option<usize>,
    /// call_fn の入口で取る: この呼び出しの戻り値を書く先
    pub dst_hint: Option<Value>,
    /// `Ok::<_, ()>(x)`: the turbofish's type arguments (None for `_`), for the next adt_value
    pub adt_targs: Option<Vec<Option<Ty>>>,
    pub frefs: HashMap<u32, FuncRef>,
    pub lets: crate::ast_infer::LetTypes,
    pub addr_taken: HashSet<String>,
    pub env: TyEnv,
    pub insts: &'p Mutex<Insts>,
    pub requested: Vec<u32>,
    pub datas: Vec<Vec<u8>>,
    pub vtables: Vec<Vec<u32>>,
    pub cmp_depth: u32,
    pub caps: Vec<(String, Ty, bool, u32)>,
    pub root: u32,
    /// The fn's parameters declared as one of its type parameters (`x: T` / `x: &T`): (name, type parameter)
    pub generic_args: Vec<(String, String)>,
    /// クロージャ式 (Debug の文字列) → 番号
    pub closure_ix: HashMap<String, u32>,
    /// `a::M::m(&x)` rewritten as `x.m()`: the kept method call (by address) -> the trait it named, so the call takes
    /// that trait's impl, whatever traits are in scope
    pub explicit_traits: HashMap<usize, String>,
    /// while a method call that named its trait is being resolved: (the method, the trait)
    pub explicit_trait: Option<(String, String)>,
    /// an overload already chosen (`X::__ref_eq#1`, PartialEq's among a reference's `eq`s): pick_overload keeps it once
    pub keep_overload: std::cell::Cell<Option<u32>>,
    pub closure_hint: Option<(Vec<Ty>, Option<Ty>)>,
    /// The bound the closure being passed is checked against (`FnOnce(&u32) -> &u32`), with the callee's lifetimes and
    /// type parameters (regionck)
    pub closure_bound: Option<Box<(String, Vec<String>, Vec<String>)>>,
    /// The closure being passed goes to a function of the prelude (whose bounds write no lifetimes)
    pub closure_prelude: bool,
    /// The closure being lowered is passed where a bound wants `Fn`: it may not mutate what it captures
    pub closure_fn_only: bool,
    /// Closures made where nothing was expected (their kind, signature: from the body), by their definition: the
    /// parameters with no type written that are references (one region each, not higher-ranked)
    pub hr_lost: Vec<(u32, Vec<usize>)>,
    /// Of those, the ones whose body is one of their parameters (`|y| y`): that parameter's type argument
    pub clo_ret_param: Vec<(u32, usize)>,
    pub seg_self: Option<Ty>,
    pub loop_bounds: Vec<(Loc, u64)>,
    pub pending_bound: Option<u64>,
    pub pending_pair: Option<(Value, Option<Value>, Ty)>,
    pub checked: (Option<Block>, Vec<(Value, LenKey)>),
    pub hoisted: Vec<(String, Value, Value, Ty, Option<(Value, Value, Value)>)>,
    pub in_fmt_args: u32,
    pub late_flags: Vec<StackSlot>,
    pub owned: Vec<Owned>,
    pub temps: Vec<Owned>,
    pub owned_binds: usize,
    pub last_scrut: Option<StackSlot>,
    /// 書き換えで作った式 (コード生成の間アドレスを動かさないために持っておく)
    pub keep: Vec<Box<Expr>>,
    /// 型推論が読んだマクロの引数 (同じ構文木を使うと、型の記録をアドレスで引ける)
    pub margs: crate::ast_infer::MacroArgs,
    /// 式の形 → 型 (複製した式の型を引く)
    pub texts: crate::ast_infer::TextTypes,
    /// 今コード生成しているマクロの引数 (開き括弧) のソース上の位置 (引数を読み直すときの位置の基準)
    pub mbase: crate::parse::Pos,
    /// 今コード生成しているマクロの先頭 (syn の `mac.span()`: panic! / assert! の場所)
    pub mstart: crate::parse::Pos,
    /// 展開してよい関数の本体 (p9_inline)
    pub inl: &'p Inline,
    /// 展開している本体の続きのブロック (return / 末尾の値はここへ飛ぶ)
    pub inline_ret: Option<Block>,
    pub inline_depth: u32,
    pub inlined: u32,
    /// `x.try_into().unwrap()` の unwrap の期待型 (型推論が try_into の行き先を記録していないとき)
    pub try_into_hint: Option<Ty>,
}

/// 間接呼び出しの呼び先
#[derive(Clone, Copy)]
pub enum Indirect {
    Vtable(Value, usize),
    Ptr(Value),
}

/// 静的データに昇格できる定数式か
pub fn is_promotable(cx: &Cx, e: &Expr) -> bool {
    match e {
        Expr::Lit(l, _) => !matches!(lit::lit(l), Some(Lit::Str(_) | Lit::ByteStr(_))),
        Expr::Paren(p) => is_promotable(cx, p),
        Expr::Unary(op, x, _) => op == "-" && is_promotable(cx, x),
        Expr::Struct(_, fs, rest) => rest.is_none() && fs.iter().all(|f| is_promotable(cx, &f.1)),
        Expr::Tuple(t) => t.iter().all(|x| is_promotable(cx, x)),
        Expr::Array(a) => a.iter().all(|x| is_promotable(cx, x)),
        Expr::Call(f, args) => {
            matches!(&**f, Expr::Path(None, p)
                if cx.prog.variant_by_p(&PPath::of(p), cx.self_sid).is_some()
                    || cx.prog.struct_by_p(&PPath::of(p), cx.self_sid).is_some())
                && args.iter().all(|x| is_promotable(cx, x))
        }
        Expr::Path(_, p) => {
            cx.const_of(e).is_some() || (cx.prog.variant_by_p(&PPath::of(p), cx.self_sid).is_some() && ident_of(p).is_none_or(|i| cx.lookup(i).is_none()))
        }
        _ => false,
    }
}

/// `_` の穴を含む型か
/// Whether a type as written names a path of std / core / alloc (`std::num::NonZero<u64>`), which zustc may not have
pub fn names_std_path(t: &crate::parse::Ty) -> bool {
    use crate::parse::{GArg, Ty as P};
    match t {
        P::Path(None, p) => {
            p.segs.first().is_some_and(|s| matches!(s.name.as_str(), "std" | "core" | "alloc"))
                || p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, GArg::Ty(x) if names_std_path(x))))
        }
        P::Ref(_, _, x) | P::Ptr(_, x) | P::Slice(x) | P::Array(x, _) => names_std_path(x),
        P::Tuple(xs) => xs.iter().any(names_std_path),
        _ => false,
    }
}

pub fn has_hole(prog: &Program, t: Ty) -> bool {
    match t {
        Ty::Param(i) => i == crate::program::HOLE,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => has_hole(prog, *i),
        // (`_` は型パラメータなので、型パラメータの残らないインスタンス (symbolic でない) には無い: 辿らない)
        Ty::Struct(sid) => prog.structs[sid as usize].symbolic && prog.structs[sid as usize].args.iter().any(|&a| has_hole(prog, a)),
        _ => false,
    }
}

/// `Iterator<Item = T>` の (Item, T)
pub fn assoc_constraint(b: &str) -> Option<(String, crate::types::TySyn)> {
    if !b.contains('=') || crate::ast_summary::parse_fn_bound(b).is_some() {
        return None;
    }
    crate::ast_summary::assoc_bindings(b).into_iter().next()
}

pub fn strip_paren(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(p) => strip_paren(p),
        e => e,
    }
}

pub fn is_place_expr(e: &Expr) -> bool {
    match e {
        Expr::Path(None, p) => ident_of(p).is_some(),
        Expr::Field(..) => true,
        Expr::Index(_, ix, _) => !matches!(&**ix, Expr::Range(..)),
        Expr::Unary(op, _, _) => op == "*",
        Expr::Paren(p) => is_place_expr(p),
        _ => false,
    }
}

/// `let x` / `let mut x` / `let _` のような単純な let か
impl Cx<'_> {
    /// (`let false = 22;` / `for 'a' in 0..3`: a bool / char literal pattern of another type is a mismatch, before any
    /// E0005)
    pub(crate) fn lit_pat_type_check(&self, pat: &Pat, vt: Ty) -> R<()> {
        let lit_ty = match pat {
            Pat::Lit(e) => match &**e {
                Expr::Lit(t, _) if t == "true" || t == "false" => Some(Ty::Bool),
                Expr::Lit(t, _) if t.starts_with('\'') => Some(Ty::Char),
                _ => None,
            },
            _ => None,
        };
        if let Some(pt) = lit_ty {
            if vt != pt && (vt.is_int() || vt.is_float() || matches!(vt, Ty::Bool | Ty::Char)) {
                return err(SP, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(vt), self.prog.tn(pt)));
            }
        }
        // (`let (false,) = (22u8,);`: a tuple's elements against its fields)
        if let (Pat::Tuple(ps), Ty::Struct(sid)) = (pat, vt) {
            if self.prog.is_tuple(self.prog.base_of(sid)) && !ps.iter().any(|p| matches!(p, Pat::Rest)) {
                let fts: Vec<Ty> = self.prog.structs[sid as usize].fields.iter().map(|f| f.1).collect();
                for (p, ft) in ps.iter().zip(fts) {
                    self.lit_pat_type_check(p, ft)?;
                }
            }
        }
        Ok(())
    }

    /// `let None = ..` / `let MAX = ..`: an identifier pattern naming a unit variant or a constant
    pub(crate) fn names_variant_or_const(&self, p: &Pat) -> bool {
        let Pat::Ident(false, false, n, None) = p else { return false };
        if !n.starts_with(|c: char| c.is_uppercase()) || self.lookup(n).is_some() {
            return false;
        }
        // (a constant, not a static: module scopes are flattened away, so a static of another module (`mod config {
        // static SIZES }`) must not turn `let SIZES = ..` in main into a pattern)
        if let Some(&ci) = self.prog.const_index.get(n.as_str()) {
            return self.prog.consts[ci as usize].stat.is_none();
        }
        let Expr::Path(None, path) = path_expr(n) else { return false };
        self.prog.variant_by_p(&PPath::of(&path), self.self_sid).is_some()
    }
}

pub fn is_simple_let(p: &Pat) -> bool {
    match p {
        Pat::Ident(by_ref, _, _, sub) => sub.is_none() && !*by_ref,
        Pat::Wild => true,
        _ => false,
    }
}

/// rustc's temporary lifetime extension by the initializer's syntax (the Reference, "Extending based on
/// expressions"): e borrows a temporary that then lives to the end of the block, `&temp()`, also inside a tuple, an
/// array, a cast, a braced struct, a block's tail, an if / else or a match arm (`let p: *const S = &S::new();`)
pub fn extends_temps(e: &Expr) -> bool {
    let tail = |b: &crate::parse::Block| match b.stmts.last() {
        Some(Stmt::Expr(t, false)) => extends_temps(t),
        _ => false,
    };
    match e {
        Expr::Ref(..) => true,
        Expr::Paren(x) | Expr::Cast(x, _) => extends_temps(x),
        Expr::Tuple(xs) | Expr::Array(xs) => xs.iter().any(extends_temps),
        Expr::Struct(_, fs, _) => fs.iter().any(|f| extends_temps(&f.1)),
        Expr::Block(_, _, b) => tail(b),
        Expr::If(_, t, els) => tail(t) || els.as_deref().is_some_and(extends_temps),
        Expr::Match(_, arms) => arms.iter().any(|a| extends_temps(&a.body)),
        _ => false,
    }
}

/// 識別子のパターンが const (初期値がリテラル) を指すなら、そのリテラルのパターン
pub fn const_lit_pat(prog: &Program, p: &Pat) -> Option<Pat> {
    let Pat::Ident(false, false, n, None) = p else { return None };
    if !n.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let &ci = prog.const_index.get(n)?;
    let c = &prog.consts[ci as usize];
    if c.stat.is_some() {
        return None;
    }
    match crate::parse::parse_expr(&c.expr).ok()? {
        e @ Expr::Lit(_, _) => Some(Pat::Lit(Box::new(e))),
        _ => None,
    }
}

/// An identifier pattern naming a `const` (any value, not only a literal): the pattern compares with it
pub fn const_ident_pat<'a>(prog: &Program, p: &'a Pat) -> Option<&'a str> {
    let Pat::Ident(false, false, n, None) = p else { return None };
    if !n.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let &ci = prog.const_index.get(n)?;
    prog.consts[ci as usize].stat.is_none().then_some(n.as_str())
}

/// 識別子のパターンが unit バリアント (`None` など) に解決されるなら、そのパス
pub fn unit_variant_ident(prog: &Program, p: &Pat) -> Option<Path> {
    let Pat::Ident(false, false, n, None) = p else { return None };
    let path = path_of(n);
    let Some((sid, vi)) = prog.variant_by_p(&PPath::of(&path), None) else {
        // (a const whose value is a unit variant, `const C: E = E::A;`: the pattern is that variant)
        let ci = *prog.const_index.get(n.as_str())?;
        let c = &prog.consts[ci as usize];
        if c.stat.is_some() || !c.expr.trim_start().starts_with(|ch: char| ch.is_ascii_uppercase()) {
            return None;
        }
        let Ok(Expr::Path(None, cp)) = crate::parse::parse_expr(&c.expr) else { return None };
        let (sid, vi) = prog.variant_by_p(&PPath::of(&cp), None)?;
        return (prog.structs[sid as usize].variants[vi].kind == crate::types::VariantKind::Unit).then_some(cp);
    };
    (prog.structs[sid as usize].variants[vi].kind == crate::types::VariantKind::Unit).then_some(path)
}

pub fn has_bindings(p: &Pat) -> bool {
    match p {
        Pat::Ident(_, _, n, None) if n == "None" => false,
        Pat::Ident(..) => true,
        Pat::Ref(_, x) => has_bindings(x),
        Pat::Or(cs) => cs.iter().any(has_bindings),
        Pat::TupleStruct(_, es) | Pat::Tuple(es) | Pat::Slice(es) => es.iter().any(has_bindings),
        Pat::Struct(_, fs, _) => fs.iter().any(|f| has_bindings(&f.1)),
        _ => false,
    }
}

pub fn is_cmp(op: &str) -> bool {
    matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=")
}

pub fn pat_name(p: &Pat) -> R<String> {
    match p {
        Pat::Ident(false, _, n, None) => Ok(n.clone()),
        Pat::Wild => Ok("_".into()),
        _ => err(SP, "unsupported pattern"),
    }
}

/// A parameter's name: `_` the k-th is bound as `_#k` (no program's name), so two `_` parameters are two locals the
/// region check finds each of
fn param_name(k: usize, p: &Pat) -> R<String> {
    let n = pat_name(p)?;
    Ok(if n == "_" { format!("_#{k}") } else { n })
}

impl Cx<'_> {
    pub fn same(&self, span: Sp, got: Ty, want: Ty) -> R<()> {
        if got == want || got == Ty::Never {
            Ok(())
        } else {
            if std::env::var_os("ACG_PANIC_ERR").is_some() {
                panic!("mismatched types");
            }
            Err(self.at(span, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(want), self.prog.tn(got))))
        }
    }

    /// Whether two types differ only in the lifetimes their names show (`dyn Foo<Bar = &'a u32>` and `.. &'static u32>`)
    pub fn apart_in_lifetimes(&self, a: Ty, b: Ty) -> bool {
        let (x, y) = (self.prog.tn(a), self.prog.tn(b));
        if !x.contains('\'') && !y.contains('\'') {
            return false;
        }
        // (the names without their `'a`, and their spaces)
        let plain = |s: &str| {
            let mut out = String::new();
            let mut it = s.chars().peekable();
            while let Some(c) = it.next() {
                if c == '\'' {
                    while it.peek().is_some_and(|d| d.is_alphanumeric() || *d == '_') {
                        it.next();
                    }
                } else if !c.is_whitespace() {
                    out.push(c);
                }
            }
            out
        };
        plain(&x) == plain(&y)
    }

    /// 書かれた型 (`Self` と型パラメータは env から)
    pub fn parse_ty(&self, t: &crate::parse::Ty) -> R<Ty> {
        let ts = crate::ast_types::tysyn(t, Some(&crate::types::TySyn::Named("Self".into())))?;
        self.prog.resolve_env(&ts, &self.env)
    }

    /// マクロの引数。型推論が読んだものがあればその構文木 (型の記録がある)、なければ読み直す
    pub fn macro_args_of(&self, g: &str) -> Option<Vec<&'static Expr>> {
        if let Some(v) = self.margs.get(&(g.as_ptr() as usize)) {
            // SAFETY: margs の中身は Cx が生きている間動かない
            return Some(v.iter().map(|e| unsafe { &*(e as *const Expr) }).collect());
        }
        None
    }

    /// 書き換えで作った式をコード生成の間持っておき、その参照を返す
    /// `*x.deref_mut()` that acg makes to reach a place through Deref: the borrow checker sees a shared borrow
    /// (as for `index_mut`)
    pub(crate) fn deref_shared(&mut self, d: &Expr) {
        if self.bk.place_mut {
            return;
        }
        if let Expr::Unary(_, c, _) = d {
            self.bk.shared_recv = &**c as *const Expr as usize;
        }
    }

    pub fn keep_expr(&mut self, e: Expr) -> &'static Expr {
        self.keep.push(Box::new(e));
        // SAFETY: keep の Box の中身は Cx が生きている間動かない
        unsafe { &*(&**self.keep.last().unwrap() as *const Expr) }
    }

    // --- メモリ操作 ------------------------------------------------------

    pub fn slot(&mut self, t: Ty) -> StackSlot {
        let (size, align) = self.prog.size_align(t);
        self.b.create_stack_slot(size.max(1), align.max(1))
    }

    /// 一時領域を確保してそのアドレスを返す
    pub fn temp(&mut self, t: Ty) -> Value {
        let ss = self.slot(t);
        self.b.ins().stack_addr(PTR, ss, 0)
    }

    /// 呼び出しの戻り値の一時領域 (fresh に入れる)
    pub fn temp_fresh(&mut self, t: Ty) -> Value {
        let ss = self.slot(t);
        let a = self.b.ins().stack_addr(PTR, ss, 0);
        if std::env::var_os("ZUSTC_NO_FRESH").is_none() {
            self.fresh.insert(a.0, ss);
        }
        a
    }

    /// v がまだ誰も使っていない一時領域なら、そのスロット (取り出す: 2 度目からはコピーする)。
    /// drop の予定に入っているもの (文の終わりで drop する一時値) は使わない
    pub fn take_fresh(&mut self, v: Value) -> Option<StackSlot> {
        let ss = *self.fresh.get(&v.0)?;
        self.fresh.remove(&v.0);
        if self.temps.iter().chain(self.owned.iter()).any(|o| o.loc == Loc::Ptr(v)) {
            return None;
        }
        Some(ss)
    }

    pub fn addr_plus(&mut self, a: Value, off: i32) -> Value {
        if off == 0 { a } else { self.b.ins().iadd_imm_s(a, off as i64) }
    }

    /// struct のコピー
    /// a call of a libc function by its symbol (`memcpy` / `memset`), with one pointer-sized result
    pub(crate) fn libc_call(&mut self, name: &str, args: &[Value]) {
        let f = self.b.import_function(name, vec![PTR]);
        self.b.ins().call(f, args);
    }

    pub fn copy(&mut self, dst: Value, src: Value, t: Ty) {
        self.copy_at(dst, src, t, 0, 0);
    }

    /// src + so にある t を dst + doff に写す。struct (enum でない) はフィールドごとにその型で写す
    /// (minclif がスタックの struct のフィールドをレジスタに置けるように: 読み書きの幅をフィールドと揃える)
    fn copy_at(&mut self, dst: Value, src: Value, t: Ty, doff: u32, soff: u32) {
        if let Ty::Struct(sid) = t {
            let sd = &self.prog.structs[sid as usize];
            if !sd.is_enum && sd.generics.is_empty() && sd.fields.len() <= 16 {
                let fs = sd.fields.clone();
                for (_, ft, off) in fs {
                    self.copy_at(dst, src, ft, doff + off, soff + off);
                }
                return;
            }
        }
        // (スカラーの小さい配列は要素ごと: 要素の幅で読み書きする)
        if let Ty::Array(e, n) = t {
            if n <= 16 && !e.is_agg() && !e.is_fat() && clif_ty(*e).is_some() {
                let (es, _) = self.prog.size_align(*e);
                for i in 0..n {
                    self.copy_at(dst, src, *e, doff + i * es, soff + i * es);
                }
                return;
            }
        }
        if !t.is_agg() && !t.is_fat() {
            if let Some(ct) = clif_ty(t) {
                let v = self.b.ins().load(ct, mf(), src, soff as i32);
                self.b.ins().store(mf(), v, dst, doff as i32);
            }
            return;
        }
        let (size, _) = self.prog.size_align(t);
        // (a large value: one libc memcpy, not a load and a store per word)
        if size > BIG_COPY {
            let d = if doff == 0 { dst } else { self.b.ins().iadd_imm(dst, doff as i64) };
            let s = if soff == 0 { src } else { self.b.ins().iadd_imm(src, soff as i64) };
            let n = self.b.ins().iconst(types::I64, size as i64);
            self.libc_call("memcpy", &[d, s, n]);
            return;
        }
        let mut off = 0;
        for (n, ty) in [(8, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
            while off + n <= size {
                let v = self.b.ins().load(ty, mf(), src, (soff + off) as i32);
                self.b.ins().store(mf(), v, dst, (doff + off) as i32);
                off += n;
            }
        }
    }

    pub fn loc_value(&mut self, l: Loc) -> Option<Value> {
        match l {
            Loc::Var(v) => Some(self.b.use_var(v)),
            Loc::Slot(ss) => Some(self.b.ins().stack_load(PTR, ss, 0)),
            Loc::Ptr(a) => Some(self.b.ins().load(PTR, mf(), a, 0)),
            Loc::Zst(_) => None,
        }
    }

    pub fn loc_addr(&mut self, l: Loc) -> Option<Value> {
        match l {
            Loc::Slot(ss) => Some(self.b.ins().stack_addr(PTR, ss, 0)),
            Loc::Ptr(v) => Some(v),
            Loc::Var(_) | Loc::Zst(_) => None,
        }
    }

    pub fn read(&mut self, p: Place) -> V {
        match p {
            Place::Var(v, t) => (Some(self.b.use_var(v)), t),
            Place::Zst(t) => (None, t),
            Place::Mem(a, o, t) if t.is_agg() => (Some(self.addr_plus(a, o)), t),
            Place::Mem(a, o, t) => match clif_ty(t) {
                Some(ct) => (Some(self.b.ins().load(ct, mf(), a, o)), t),
                None => (None, t),
            },
        }
    }

    pub fn write(&mut self, p: Place, v: Option<Value>) {
        let Some(v) = v else { return };
        match p {
            Place::Var(var, _) => self.b.def_var(var, v),
            Place::Zst(_) => {}
            Place::Mem(a, o, t) if t.is_agg() => {
                let dst = self.addr_plus(a, o);
                self.copy(dst, v, t);
            }
            Place::Mem(a, o, _) => {
                self.b.ins().store(mf(), v, a, o);
            }
        }
    }

    // --- 変数 -----------------------------------------------------------

    /// ローカル変数を作る。v はスカラー値か、struct ならコピー元のアドレス。
    pub fn bind(&mut self, name: String, ty: Ty, v: Option<Value>) {
        if !self.fn_aliases.is_empty() {
            self.fn_aliases.retain(|a| a.0 != name);
        }
        let loc = match ty {
            t if t.is_agg() => {
                // (呼び出しの戻り値の一時領域はそのまま変数の場所にする)
                if let Some(ss) = v.and_then(|v| self.take_fresh(v)) {
                    Loc::Slot(ss)
                } else {
                    let ss = self.slot(ty);
                    let dst = self.b.ins().stack_addr(PTR, ss, 0);
                    if let Some(v) = v {
                        self.copy(dst, v, ty);
                    }
                    Loc::Slot(ss)
                }
            }
            _ => match clif_ty(ty) {
                None => Loc::Zst(self.bk.fresh_zst()),
                Some(_) if self.addr_taken.contains(&name) => {
                    let ss = self.slot(ty);
                    if let Some(v) = v {
                        self.b.ins().stack_store(v, ss, 0);
                    }
                    Loc::Slot(ss)
                }
                Some(ct) => {
                    let var = self.b.declare_var(ct);
                    if let Some(v) = v {
                        self.b.def_var(var, v);
                    }
                    Loc::Var(var)
                }
            },
        };
        self.bk_local(loc, &name, ty);
        self.scopes.last_mut().unwrap().push((name, loc, ty));
        if self.needs_drop(ty) {
            self.owned_binds += 1;
            self.own_local(loc, ty);
        }
    }

    pub fn lookup(&self, name: &str) -> Option<(Loc, Ty)> {
        self.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(n, _, _)| n == name).map(|&(_, l, t)| (l, t))
    }

    fn body(&mut self, f: &ItemFn, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        let mut params = self.b.func_params();
        if is_sret(sig) {
            self.sret = Some(params.remove(0));
        }
        let mut params = params.into_iter();
        let mut names: Vec<String> = vec![];
        if f.sig.recv.is_some() {
            names.push("self".to_string());
        }
        for (k, (p, _)) in f.sig.params.iter().enumerate() {
            names.push(param_name(k, p)?);
        }
        // (`mut x` parameters, for the borrow checker; `self` is not known)
        let mut muts: Vec<Option<bool>> = vec![];
        if f.sig.recv.is_some() {
            muts.push(None);
        }
        for (p, _) in &f.sig.params {
            muts.push(pat_mut(p));
        }
        let mut muts = muts.into_iter();
        for (name, &ty) in names.into_iter().zip(&sig.params) {
            self.bk.mut_next = muts.next().flatten();
            if !ty.is_agg() && clif_ty(ty).is_none() {
                self.bind(name, ty, None);
                continue;
            }
            let Some(v) = params.next() else {
                return Err("internal: parameter count mismatch".into());
            };
            if ty.is_agg() {
                // 呼び出し側がコピー済みなので、そのメモリをそのまま使う (値渡しの引数は呼び出された側が drop する)
                self.bk_local(Loc::Ptr(v), &name, ty);
                self.scopes.last_mut().unwrap().push((name, Loc::Ptr(v), ty));
                self.own_local(Loc::Ptr(v), ty);
            } else {
                self.bind(name, ty, Some(v));
            }
        }
        self.bk.params = (0..self.bk.locals.len() as u32).collect();
        // (the gate, before any work: a signature with fewer than two regions and no lifetime named has no two
        // universal regions to relate)
        // (or one region and a trait object in the result, 'static by default: `fn f(x: &u8) -> Box<dyn Tr>`)
        self.rg.eligible = self.rg.on && self.rg.closure.is_none();
        if self.rg.on && sig.regions.is_none() && self.rg.closure.is_none() && !self.rg.force {
            let n: usize = sig.params.iter().map(|&t| regionck::count(self.prog, t, 0).unwrap_or(0)).sum();
            // (a placeholder's region is universal, whether a parameter's type has it or not (`f::<T>(x)`))
            let n = n + self.rg.inst_args.iter().filter(|&&t| matches!(t, Ty::Struct(s) if self.prog.is_placeholder(s))).count();
            // (an `impl Trait` result of a fn with no region in its signature: its hidden type captures none, its
            // regions are 'static)
            let opaque = n == 0 && !sig.is_generic() && self.prog.opaque.contains(&self.rg.base) && regionck::count(self.prog, sig.ret, 0).is_some_and(|c| c > 0);
            if n < 2 && !(n == 1 && regionck::has_dyn(self.prog, sig.ret, 0)) && !opaque {
                self.rg.on = false;
                let se = f.sig.recv.as_ref().is_some_and(|r| r.starts_with('&'));
                self.rg.late = Some((se, f.sig.recv.is_some() && !se));
            }
        }
        // (a closure's body: its parameters but the environment)
        if self.rg.on && self.rg.closure.is_some() {
            let mut ps = vec![];
            for (k, (p, _)) in f.sig.params.iter().enumerate() {
                let n = param_name(k, p)?;
                if n != "__env" {
                    ps.push(n);
                }
            }
            let ps: Vec<(Loc, Ty)> = ps.iter().filter_map(|n| self.lookup(n)).collect();
            self.rg_begin_closure(&ps);
        } else if self.rg.on {
            let mut ps = vec![];
            if f.sig.recv.is_some() {
                ps.push("self".to_string());
            }
            for (k, (p, _)) in f.sig.params.iter().enumerate() {
                ps.push(param_name(k, p)?);
            }
            let ps: Vec<(Loc, Ty)> = ps.iter().filter_map(|n| self.lookup(n)).collect();
            if ps.len() == sig.params.len() {
                // (elision from the receiver: `&self` / `&mut self`, or a receiver type that names `Self`)
                let self_elides = match (&f.sig.recv, &f.sig.recv_ty) {
                    (Some(r), _) if r.starts_with('&') => true,
                    // (`self: &Struct` names the self type as Self does)
                    (Some(_), Some(t)) => {
                        let d = format!("{t:?}");
                        let own = self.self_sid.map(|s| self.prog.structs[self.prog.base_of(s) as usize].name.rsplit("::").next().unwrap_or("").to_string());
                        d.contains("Ref(") && (d.contains("\"Self\"") || own.is_some_and(|n| !n.is_empty() && d.contains(&format!("\"{n}\""))))
                    }
                    _ => false,
                };
                // (any other receiver with elided regions in the result: not followed)
                // (when the result has elided regions: `fn set(self, s: &str) -> Flag<'a>` names its own)
                let ret_elides = sig.regions.as_deref().and_then(|r| r.slots.last()).is_none_or(|s| s.iter().any(|&c| c == crate::types::R_ELIDED || c == crate::types::R_ELIDED_ADT));
                let other_self = f.sig.recv.is_some() && !self_elides && ret_elides;
                let force = self.rg.force;
                self.rg_begin(sig.regions.as_deref(), &ps, self_elides, force);
                if other_self {
                    self.rg.ret = None;
                }
            } else {
                self.rg.on = false;
            }
        }
        // クロージャ: キャプチャを `__env` のフィールド (by-ref なら指す先) として見せる
        if !self.caps.is_empty() {
            let env = match self.lookup("__env") {
                Some((l, _)) => self.loc_value(l),
                None => None,
            };
            let env = env.ok_or("closure without environment")?;
            for (ci, (n, t, by_ref, off)) in self.caps.clone().into_iter().enumerate() {
                let a = if by_ref { self.b.ins().load(PTR, mf(), env, off as i32) } else { self.addr_plus(env, off as i32) };
                let loc = if clif_ty(t).is_none() && !t.is_agg() { Loc::Zst(self.bk.fresh_zst()) } else { Loc::Ptr(a) };
                // (a capture held by value is a local of the body for the borrow checker: moving it twice, or in a
                // loop, is a use of a moved value)
                if !by_ref {
                    self.bk_local(loc, &n, t);
                }
                self.rg_capture(ci, loc, t);
                self.scopes.last_mut().unwrap().push((n, loc, t));
            }
        }
        if self.is_main && self.prog.panic_handler.is_none() && self.prog.index.contains_key("__exr_rt_init") {
            let _ = self.call_named("__exr_rt_init", vec![Ty::Unit], &[], SP);
        }
        if let Some(Stmt::Expr(e, false)) = f.block.stmts.last() {
            self.mark_ret_call(e);
            self.bk.tail = e as *const Expr as usize;
        }
        let (v, t) = self.block(&f.block, Some(self.ret))?;
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        // (エラーの場所は末尾の式 (なければ関数の先頭))
        let tail = match f.block.stmts.last() {
            Some(Stmt::Expr(e, false)) => err_pos(e),
            _ => SP,
        };
        self.same(tail, t, self.ret)?;
        self.drop_for_exit(0, 0);
        self.emit_return(v);
        Ok(())
    }

    /// e が呼び出しなら、戻り値を sret の場所に直接書かせる印を付ける
    pub fn mark_ret_call(&mut self, e: &Expr) {
        let e = strip_paren(e);
        if self.sret.is_some() && self.inline_ret.is_none() && matches!(e, Expr::Call(..) | Expr::MethodCall(..) | Expr::Struct(..)) && std::env::var_os("ZUSTC_NO_RET_DST").is_none() {
            self.ret_call = Some(e as *const Expr as usize);
        }
    }

    pub fn emit_return(&mut self, v: Option<Value>) {
        // (展開した本体: 続きのブロックへ)
        if let Some(cont) = self.inline_ret {
            if let Some(dst) = self.sret {
                if let Some(v) = v {
                    self.copy(dst, v, self.ret);
                }
                self.b.ins().jump(cont, &[]);
            } else if self.b.block_params(cont).is_empty() {
                self.b.ins().jump(cont, &[]);
            } else {
                match v {
                    Some(v) => self.b.ins().jump(cont, &[v]),
                    None => self.b.ins().trap(TrapCode::unwrap_user(1)),
                }
            }
            return;
        }
        if self.is_main {
            // `fn main() -> Result<(), E>`: Err なら `Error: {:?}` を出して 1 で終わる
            if let (Ty::Struct(rs), Some(v)) = (self.ret, v) {
                let rd = &self.prog.structs[rs as usize];
                if rd.is_enum && rd.base.is_some_and(|b| self.prog.structs[b as usize].name == "Result") {
                    if let Some(ev) = rd.variants.iter().position(|x| x.name == "Err") {
                        let (_, et, off) = rd.variants[ev].fields[0];
                        let ok = self.b.create_block();
                        if self.test_variant(Place::Mem(v, 0, self.ret), rs, ev, SP, ok).is_ok() {
                            let pa = self.addr_plus(v, off as i32);
                            let _ = self.call_named("__exr_main_err", vec![et], &[(pa, Ty::Ref(false, intern(et)))], SP);
                            let one = self.b.ins().iconst(types::I32, 1);
                            self.b.ins().return_(&[one]);
                        }
                        self.b.switch_to_block(ok);
                    }
                }
            }
            let z = self.b.ins().iconst(types::I32, 0);
            self.b.ins().return_(&[z]);
        } else if let Some(dst) = self.sret {
            // (呼び出しが sret の場所に直接書いたものはコピーしない)
            if v != Some(dst) {
                self.copy(dst, v.unwrap(), self.ret);
            }
            self.b.ins().return_(&[]);
        } else {
            match v {
                Some(v) => self.b.ins().return_(&[v]),
                None => self.b.ins().return_(&[]),
            };
        }
    }

    /// エラーのメッセージに場所 (ファイルの中の `行:桁: `) を付ける。もう付いているか、位置が分からなければそのまま
    pub fn at(&self, sp: Sp, m: String) -> String {
        if sp.line == 0 || has_loc(&m) {
            return m;
        }
        format!("{}:{}: {m}", self.line_base + sp.line, sp.col + 1)
    }

    /// `&&T` (`&&&T` ..) を `&T` の場所に渡すときの deref の強制変換
    pub fn deref_coerce(&mut self, mut v: Option<Value>, mut t: Ty, want: Ty) -> (Option<Value>, Ty) {
        while let (Ty::Ref(..), Ty::Ref(_, inner @ Ty::Ref(..)), Some(val)) = (want, t, v) {
            if t == want || t.is_fat() {
                break;
            }
            // (`&&str` の値は `&str` (fat) の置き場所のアドレスで、`&str` の表現と同じ: 読まない)
            v = Some(if inner.is_fat() { val } else { self.b.ins().load(PTR, mf(), val, 0) });
            t = *inner;
        }
        // (`&String` を `&str` に)
        if let (Ty::Ref(false, Ty::Str), Ty::Ref(_, Ty::Struct(sid)), Some(val)) = (want, t, v) {
            let sd = &self.prog.structs[*sid as usize];
            if sd.name == "String" && sd.base.is_none() && self.prog.index.contains_key("String::as_str") {
                if let Ok((Some(r), rt)) = self.call_named("String::as_str", vec![], &[(val, Ty::Ref(false, intern(Ty::Struct(*sid))))], SP) {
                    return (Some(r), rt);
                }
            }
        }
        (v, t)
    }

    /// 終端命令の後に続くコード用の到達不能ブロックへ移る。
    pub fn dead(&mut self) -> V {
        let blk = self.b.create_block();
        self.b.switch_to_block(blk);
        (None, Ty::Never)
    }

    pub fn block(&mut self, blk: &crate::parse::Block, expected: Option<Ty>) -> R<V> {
        let saved = self.stmt_i;
        let r = self.block_stmts(blk, expected);
        match r {
            // (back to the statement that holds the block)
            Ok(v) => {
                self.stmt_i = saved;
                Ok(v)
            }
            // (an error with no position of its own: at the statement it came from)
            Err(m) if !has_loc(&m) => {
                let sp = match blk.stmts.get(self.stmt_i) {
                    Some(Stmt::Let(.., p)) => *p,
                    Some(Stmt::Expr(e, _)) => expr_start(e),
                    _ => SP,
                };
                // (none here either: the block's own statement in the block around it decides)
                self.stmt_i = saved;
                Err(self.at(sp, m))
            }
            Err(m) => Err(m),
        }
    }

    fn block_stmts(&mut self, blk: &crate::parse::Block, expected: Option<Ty>) -> R<V> {
        // (`let V(s) = cap else { .. };` with cap held by value by the closure: `let t = cap;` first, as for match)
        if !self.caps.is_empty()
            && blk.stmts.iter().any(|st| matches!(st, Stmt::Let(pat, _, Some(init), els, ..) if (!is_simple_let(pat) || els.is_some()) && self.takes_capture(init, &[pat])))
        {
            let mut stmts = vec![];
            for (k, st) in blk.stmts.iter().enumerate() {
                match st {
                    Stmt::Let(pat, ann, Some(init), els, lp) if (!is_simple_let(pat) || els.is_some()) && self.takes_capture(init, &[pat]) => {
                        let tk = format!("__exr_tk{k}");
                        stmts.push(Stmt::Let(Pat::Ident(false, false, tk.clone(), None), None, Some(init.clone()), None, *lp));
                        stmts.push(Stmt::Let(pat.clone(), ann.clone(), Some(path_expr(&tk)), els.clone(), *lp));
                    }
                    st => stmts.push(st.clone()),
                }
            }
            let nb: &'static crate::parse::Block = Box::leak(Box::new(crate::parse::Block { stmts }));
            return self.block_stmts(nb, expected);
        }
        self.scopes.push(vec![]);
        let mut diverged = false;
        let mut result: V = (None, Ty::Unit);
        let n = blk.stmts.len();
        // (the borrowed temporaries of each statement end with it; the tail expression's, with the enclosing one)
        let bk_mark = self.bk.stmt_temps.len();
        for (i, st) in blk.stmts.iter().enumerate() {
            self.bk_temps_end(bk_mark);
            let last = i + 1 == n;
            let t0 = self.temps.len();
            self.stmt_i = i;
            match st {
                // (`let None = x;`: a unit variant or a constant, a pattern to test (E0005), not a new name)
                Stmt::Let(pat, ann_ty, init, els, ..) if !is_simple_let(pat) || els.is_some() || self.names_variant_or_const(pat) => {
                    // 分解する let (`let S { x, y } = s;`): 反駁不能パターンとして束縛だけ行う
                    let Some(init) = init else {
                        return err(SP, "`let` without initializer is not supported");
                    };
                    let ann = ann_ty.as_ref().and_then(|t| self.parse_ty(t).ok());
                    // (destructuring reads the parts the pattern binds, not the whole value)
                    // (a `let`'s temporary bound by `ref` lives as long as the block: not one of the statement)
                    self.bk.let_scrut = true;
                    // (`let &Foo(ref x) = &f();`: the temporary lives as long as the block)
                    let ext1 = self.bk.extended.len();
                    if self.bk_on() {
                        borrowck::direct_ref(init, &mut self.bk.extended);
                    }
                    let sc = self.bk_scrutinee(init, ann, &[pat]);
                    self.bk.extended.truncate(ext1);
                    self.bk.let_scrut = false;
                    let Some(place) = sc? else {
                        diverged = true;
                        continue;
                    };
                    // (E0005: without `else`, the pattern must cover the value's type)
                    if els.is_none() && self.user_code && !plainly_irrefutable(pat) {
                        // (`let false = 22;`: the literal's type first)
                        self.lit_pat_type_check(pat, place.ty())?;
                        match self.coverage(&[pat], place.ty()) {
                            Some(Some(w)) => return err(SP, format!("refutable pattern in local binding: `{w}` not covered (E0005)")),
                            Some(None) => {}
                            // (a pattern coverage does not model (`let [Some(x)] = xs;`): tested at run time, a trap where it
                            // does not match, so nothing binds from a payload that is not there)
                            None if els.is_none() => {
                                let trap = self.b.create_block();
                                self.pat_test(pat, place, trap)?;
                                let cur = self.b.create_block();
                                self.b.ins().jump(cur, &[]);
                                self.b.switch_to_block(trap);
                                self.b.ins().trap(TrapCode::unwrap_user(2));
                                self.b.switch_to_block(cur);
                            }
                            None => {}
                        }
                    }
                    // `let PAT = e else { .. };`: 合わなければ else の (発散する) ブロックへ
                    // (the else block may match other scrutinees: the pattern binds from this one)
                    let scrut_saved = (self.bk.scrut.clone(), self.bk.scrut_place.clone(), self.rg.scrut.clone());
                    if let Some(else_b) = els {
                        let else_blk = self.b.create_block();
                        self.pat_test(pat, place, else_blk)?;
                        let cur = self.b.create_block();
                        self.b.ins().jump(cur, &[]);
                        self.b.switch_to_block(else_blk);
                        // (the scrutinee's temporaries are dropped before the else block, as in rustc: `let Some(x) =
                        // c.borrow().first().copied() else { c.borrow_mut().push(1); return }` does not panic)
                        self.drop_for_exit(usize::MAX, t0);
                        let (_, et) = self.block(else_b, Some(Ty::Never))?;
                        if et != Ty::Never {
                            return err(SP, "the `else` block of `let ... else` must diverge (return / break / panic)");
                        }
                        self.b.ins().trap(TrapCode::unwrap_user(2));
                        self.b.switch_to_block(cur);
                    }
                    self.bk.scrut = scrut_saved.0;
                    self.bk.scrut_place = scrut_saved.1;
                    self.rg.scrut = scrut_saved.2;
                    let ob = self.owned_binds;
                    let pre = if is_place_expr(init) { self.moved_flag(init) } else { None };
                    self.bk_bind_begin();
                    let r = self.pat_bind(pat, place, BindMode::Move);
                    self.bk_bind_end();
                    r?;
                    if self.owned_binds > ob {
                        self.deref_move_check(init)?;
                        match pre {
                            Some(f) => self.clear_flag(f),
                            None => {
                                let f = self.last_scrut;
                                self.release(init, f)?;
                            }
                        }
                    }
                    // (as for a plain let: what the bindings may borrow (a `ref` binding, a reference in the value)
                    // lives to the end of the block, the rest is dropped here: `let (n, _) = (c.borrow().len(), 0);`)
                    if extends_temps(init) || borrowck::has_refs(self.prog, place.ty(), 0) || format!("{pat:?}").contains("Ident(true") {
                        self.extend_temps(t0);
                    } else {
                        self.drop_temps(t0);
                    }
                }
                Stmt::Let(Pat::Wild, ann_ty, init, None, let_at) => {
                    // `let _ = e;`: 場所ならムーブしない。値ならすぐ drop する
                    let Some(init) = init else {
                        // (`let _: T;`: T must name a type (a well-formed one); nothing else happens)
                        if let Some(t) = ann_ty {
                            self.parse_ty(t)?;
                            self.rg_let_uninit(None, t, *let_at);
                        }
                        continue;
                    };
                    let let_sp0 = std::mem::replace(&mut self.rg.let_sp, *let_at);
                    if !self.rg.on && self.rg.eligible && ann_ty.as_ref().is_some_and(crate::regions::mentions_static) {
                        self.rg_late_on();
                    }
                    // (`let _ = Foo;` with Foo a unit struct (or a const) is a value: dropped at once)
                    let local = !matches!(init, Expr::Path(None, p) if ident_of(p).is_some_and(|n| self.lookup(n).is_none()
                        && !self.prog.const_index.get(n).is_some_and(|&ci| self.prog.consts[ci as usize].stat.is_some())));
                    // (`let _: u8 = "s";`: the annotation names a type, and the value is checked against it as a named
                    // binding's is: the expected type, then a deref coercion (`let _: *const u8 = &a;` with `a: Foo`
                    // stays an error))
                    // (a type zustc cannot read yet (`[(); foo()]`, `dyn Send`, `std::num::NonZero<u64>`) is not checked;
                    // a name of none of the program's is an error)
                    let ann = match ann_ty.as_ref().map(|t| (t, self.parse_ty(t))) {
                        Some((_, Ok(a))) => Some(a),
                        Some((t, Err(e))) if e.contains("cannot find type") && !names_std_path(t) => return Err(e),
                        _ => None,
                    };
                    let full = ann.filter(|&a| !has_hole(self.prog, a) && !self.prog.has_param(a));
                    // (a place of another type than the annotation's is coerced: a value, read)
                    let coerced = || full.is_some_and(|a| self.peek(init).is_some_and(|pt| pt != a));
                    if is_place_expr(init) && local && !coerced() {
                        // (`let _ = x;` does not read x)
                        self.bk.place_depth += 1;
                        let r = self.place(init);
                        self.bk.place_depth -= 1;
                        let _ = r?;
                    } else {
                        // (`let _: [(); 0] = [];`: an array literal takes its element type from it, holes and all)
                        let exp = match ann {
                            Some(a) if matches!(strip_paren(init), Expr::Array(_) | Expr::Repeat(..)) => Some(a).filter(|&a| !has_hole(self.prog, a)),
                            _ => full,
                        };
                        let (mut v, mut t) = self.expr(init, exp)?;
                        if t == Ty::Never {
                            diverged = true;
                            continue;
                        }
                        if let Some(a) = full.filter(|&a| a != t) {
                            let (v2, t2) = self.deref_coerce(v, t, a);
                            if t2 == a {
                                v = v2;
                                t = t2;
                            }
                            // (`*mut T` where `*const T` is written: the same pointer. Types apart in their lifetimes only
                            // (`dyn Foo<Bar = &'a u32>`, which keeps the text of its bounds) are the region check's)
                            let ptr = matches!((t, a), (Ty::Ptr(true, x), Ty::Ptr(false, y)) if x == y);
                            if !ptr && !self.apart_in_lifetimes(t, a) {
                                self.same(SP, t, a).map_err(|m| self.at(expr_start(init), m))?;
                            }
                        }
                        if let Some(v) = v {
                            self.own_temp(v, t);
                        }
                    }
                    // (`let _: Covariant<'short> = c;`: the value flows into the annotation's type)
                    if let Some(a) = ann_ty {
                        self.rg_let_wild(a, init);
                    }
                    self.rg.let_sp = let_sp0;
                    self.drop_temps(t0);
                }
                Stmt::Let(pat, ann_ty, init, _, let_at) => {
                    let name = pat_name(pat)?;
                    if let Some(t) = ann_ty {
                        self.body_lifetimes(t, *let_at)?;
                    }
                    let ann = match ann_ty {
                        // (`let v: Vec<Box<_>> = Vec::new();`: the holes are what inference found, when it did)
                        Some(t) => {
                            let a = self.parse_ty(t)?;
                            match self.lets.get(&crate::ast_infer::stmt_key(st)).copied() {
                                // (`&[_]` that inference unified with the array it is initialized from: the slice of its elements)
                                Some(Ty::Ref(_, Ty::Array(e, _))) if matches!(a, Ty::Ref(_, Ty::Slice(_))) => {
                                    let Ty::Ref(m, _) = a else { unreachable!() };
                                    Some(Ty::Ref(m, intern(Ty::Slice(*e))))
                                }
                                // (only an instance of the annotation: `Box<_>` is not filled with a `bool`)
                                Some(inf) if has_hole(self.prog, a) && !has_hole(self.prog, inf) && !self.prog.has_param(inf) && std::mem::discriminant(&inf) == std::mem::discriminant(&a)
                                    && !matches!((inf, a), (Ty::Struct(x), Ty::Struct(y)) if self.prog.base_of(x) != self.prog.base_of(y)) => Some(inf),
                                _ => Some(a),
                            }
                        }
                        // (型推論は強制変換の行き先 `[T]` などを入れてしまうことがあるので、サイズ不定の型は使わない)
                        None => self
                            .lets
                            .get(&crate::ast_infer::stmt_key(st))
                            .copied()
                            .filter(|t| !matches!(t, Ty::Slice(_) | Ty::Str | Ty::Dyn(_))),
                    };
                    // (assert_eq! の展開: 右の値の型は左と同じ)
                    let hintable = |e: &Expr| {
                        let e = match e {
                            Expr::Ref(_, x) => strip_paren(x),
                            e => e,
                        };
                        !matches!(e, Expr::Lit(t, _) if matches!(lit::lit(t), Some(Lit::Str(_))))
                    };
                    let other = match name.as_str() {
                        "__exr_r" => Some("__exr_l"),
                        "__exr_l" => Some("__exr_r"),
                        _ => None,
                    };
                    let hint = match (init, other) {
                        (Some(i), Some(o)) if ann.is_none() && hintable(i) => self.lookup(o).map(|x| x.1),
                        _ => None,
                    };
                    // `let x;` (後で代入する): 0 で埋めて置く
                    let Some(init) = init else {
                        let ann = ann.filter(|&a| !has_hole(self.prog, a));
                        let Some(ty) = ann.filter(|&t| !self.prog.has_param(t)) else {
                            return err(SP, "cannot infer the type of this `let`; annotate it");
                        };
                        // (the zeroed value stands for "not initialized yet", which a value that is dropped must be
                        // able to: a build cannot make it of such a type. A check runs nothing: the zeroed value is
                        // only a placeholder)
                        if self.needs_drop(ty) && !self.zeroable(ty) && !self.inl.check_only {
                            return err(SP, "`let` without initializer is not supported for this type");
                        }
                        let v = if ty.is_agg() {
                            let a = self.temp(ty);
                            self.zero(a, ty);
                            Some(a)
                        } else {
                            match clif_ty(ty) {
                                None => None,
                                Some(ct) if ct == types::F64 => Some(self.b.ins().f64const(0.0)),
                                Some(ct) if ct == types::F32 => Some(self.b.ins().f32const(0.0)),
                                Some(ct) if ct == types::I128 => Some(self.int_const(Ty::I128, 0)),
                                Some(ct) => Some(self.b.ins().iconst(ct, 0)),
                            }
                        };
                        self.bk.uninit_next = true;
                        self.bk.mut_next = pat_mut(pat);
                        self.bind(name.clone(), ty, v);
                        // (the annotation's type is well formed; the value assigned later flows into it)
                        if let Some(t) = ann_ty {
                            self.rg_let_uninit(Some(&name), t, *let_at);
                        }
                        continue;
                    };
                    // (`let f1 = f;` with f a generic fn and no annotation: f1 is f, instantiated at each call; so is a fn
                    // with lifetime parameters, whose calls the region check sees then)
                    if let (None, Some(id)) = (ann_ty, self.fn_item(init)) {
                        let tf = matches!(init, Expr::Path(_, ip) if ip.segs.last().is_some_and(|s| !s.args.is_empty()));
                        let s = &self.prog.sigs[id as usize];
                        let lts = self.rg.on && s.regions.as_deref().is_some_and(|r| !r.lts.is_empty());
                        if (s.is_generic() || lts) && !tf && !matches!(pat, Pat::Ident(_, true, ..)) {
                            let a: &Expr = self.keep_expr(init.clone());
                            self.fn_aliases.retain(|x| x.0 != name);
                            self.fn_aliases.push((name.clone(), a));
                            continue;
                        }
                    }
                    // (`let g = Tr::m;`: a trait's method, whose impl only the calls choose: g is `Tr::m` at each call)
                    if let (None, Expr::Path(None, ip)) = (ann_ty, init) {
                        if ip.segs.len() == 2
                            && ip.segs.iter().all(|s| s.args.is_empty())
                            && self.prog.traits.contains_key(&ip.segs[0].name)
                            && self.lookup(&ip.segs[0].name).is_none()
                            && !matches!(pat, Pat::Ident(_, true, ..))
                            && self.fn_item(init).is_none()
                        {
                            let a: &Expr = self.keep_expr(init.clone());
                            self.fn_aliases.retain(|x| x.0 != name);
                            self.fn_aliases.push((name.clone(), a));
                            continue;
                        }
                    }
                    // (`let x: &[_] = &[1, 2];`: the slice of the array's element type)
                    let ann = match ann {
                        Some(Ty::Ref(m, Ty::Slice(e))) if has_hole(self.prog, **e) => match self.peek(init) {
                            Some(Ty::Ref(_, Ty::Array(ae, _))) => Some(Ty::Ref(m, intern(Ty::Slice(*ae)))),
                            _ => ann,
                        },
                        a => a,
                    };
                    self.bk_frame();
                    self.bk.move_mut_next = ann_ty.is_none();
                    let ext0 = self.bk.direct.len();
                    let ext1 = self.bk.extended.len();
                    if self.bk_on() {
                        borrowck::direct_temps(init, &mut self.bk.direct);
                        // (`let x = &f();`: the temporary lives as long as x's block)
                        borrowck::direct_ref(init, &mut self.bk.extended);
                    }
                    // (a function the gate left out, with `let x: &'static T = ..`: from here on, a borrow of what it
                    // owns must not have to be 'static. What came before has no trees and adds no constraint)
                    let static_cast = || matches!(strip_paren(init), Expr::Cast(_, t) if crate::regions::mentions_static(t));
                    if !self.rg.on && self.rg.eligible && (static_cast() || ann_ty.as_ref().is_some_and(crate::regions::mentions_static)) {
                        self.rg_late_on();
                    }
                    let let_sp0 = std::mem::replace(&mut self.rg.let_sp, *let_at);
                    let ml0 = std::mem::replace(&mut self.bk.macro_let, name.starts_with("__exr_"));
                    let r = self.expr(init, ann.or(hint));
                    self.bk.macro_let = ml0;
                    self.bk.direct.truncate(ext0);
                    self.bk.extended.truncate(ext1);
                    let srcs = self.bk_frame_end();
                    let (mut v, mut t) = r?;
                    if t == Ty::Never {
                        // (the code after it is unreachable but still checked: `let y: u32 = x;` with `x: !`)
                        diverged = true;
                        // (with an annotation, the binding has that type: `let n: Option<!> = loop {};`)
                        match ann.filter(|&a| !has_hole(self.prog, a) && !self.prog.has_param(a) && !self.needs_drop(a)) {
                            Some(a) => {
                                let v = if a.is_agg() {
                                    let tmp = self.temp(a);
                                    self.zero(tmp, a);
                                    Some(tmp)
                                } else {
                                    clif_ty(a).map(|ct| if ct == types::I128 { self.int_const(Ty::I128, 0) } else if ct.is_float() { if ct == types::F64 { self.b.ins().f64const(0.0) } else { self.b.ins().f32const(0.0) } } else { self.b.ins().iconst(ct, 0) })
                                };
                                self.bind(name, a, v);
                            }
                            None => self.bind(name, Ty::Never, None),
                        }
                        continue;
                    }
                    let mut ann = ann.filter(|&a| !has_hole(self.prog, a));
                    // (型推論の結果 (注釈なし) と違う: `&&str` → `&str` は deref の強制変換、それでも違えば実際の型)
                    if let Some(a) = ann.filter(|&a| a != t) {
                        let (v2, t2) = self.deref_coerce(v, t, a);
                        if t2 == a {
                            v = v2;
                            t = t2;
                        } else if ann_ty.is_none() {
                            ann = None;
                        }
                    }
                    let ty = ann.unwrap_or(t);
                    // (`*mut T` where `*const T` is written: the same pointer)
                    if let (Ty::Ptr(true, x), Ty::Ptr(false, y)) = (t, ty) {
                        if x == y {
                            t = ty;
                        }
                    }
                    self.same(SP, t, ty).map_err(|m| self.at(expr_start(init), m))?;
                    self.bk.mut_next = pat_mut(pat);
                    let rg_init = if self.rg_on() { self.rt_expr(init) } else { None };
                    self.bind(name.clone(), ty, v);
                    self.bk_def_named(&name, &srcs, init);
                    self.rg_let(&name, ann_ty.as_ref(), init, rg_init);
                    self.rg.let_sp = let_sp0;
                    // (rustc drops a let initializer's temporaries at the end of the statement, unless the initializer
                    // extends them (`let r = &f();`, `let p: *const S = &S::new();`). zustc also keeps them when the
                    // bound value may hold a reference, which rustc rejects when it borrows a temporary (E0716) and
                    // zustc does not see yet. A RefCell guard in `let n = *c.borrow();` must not outlive the let)
                    if extends_temps(init) || borrowck::has_refs(self.prog, ty, 0) {
                        self.extend_temps(t0);
                    } else {
                        self.drop_temps(t0);
                    }
                }
                // (`;` の付いたマクロの文。末尾の `vec![]` などは式として期待型を渡す)
                Stmt::Expr(Expr::Macro(p, g, sp), semi) if *semi => {
                    let saved = std::mem::replace(&mut self.mbase, sp.at);
                    let saved_s = std::mem::replace(&mut self.mstart, sp.start);
                    let r = self.macro_(p, g, None);
                    self.mbase = saved;
                    self.mstart = saved_s;
                    let r = r?;
                    if r.1 == Ty::Never {
                        diverged = true;
                    }
                    if last && !*semi {
                        result = r;
                        self.drop_tail_temps(t0, result);
                    } else {
                        self.drop_temps(t0);
                    }
                }
                Stmt::Expr(e, semi) => {
                    let exp = if last && !*semi { expected } else { None };
                    // (the value of the function body: what it borrows must outlive the function)
                    let tail = last && !*semi && self.bk.tail == e as *const Expr as usize && self.bk_on();
                    if tail {
                        self.bk_frame();
                    }
                    self.bk.move_mut_next = last && !*semi;
                    // (a statement's temporaries borrowed by its outermost call, or by an assignment (`x = &f();`: no
                    // `let` extends them): they go away at its end)
                    let ext0 = self.bk.direct.len();
                    if *semi && self.bk_on() {
                        match strip_paren(e) {
                            Expr::Assign(_, v) => borrowck::direct_ref(v, &mut self.bk.direct),
                            // (a method of a local that may hold references: `v.push(&f());`)
                            e @ Expr::MethodCall(r, ..) => {
                                borrowck::direct_temps(e, &mut self.bk.direct);
                                let holds = || matches!(strip_paren(r), Expr::Path(None, p) if p.segs.len() == 1 && self.lookup(&p.segs[0].name).is_some_and(|(_, t)| borrowck::has_refs(self.prog, t, 0)));
                                if self.bk.direct.len() > ext0 && !holds() {
                                    self.bk.direct.truncate(ext0);
                                }
                            }
                            _ => {}
                        }
                    }
                    let r = self.expr(e, exp);
                    self.bk.direct.truncate(ext0);
                    if last && !*semi && self.bk.tail == e as *const Expr as usize && r.is_ok() {
                        self.rg_ret(e);
                    }
                    // (a block's value: its tree, while the block's locals are in scope)
                    if last && !*semi && r.is_ok() {
                        self.rg_keep(e);
                    }
                    if tail {
                        let srcs = self.bk_frame_end();
                        if borrowck::has_refs(self.prog, self.ret, 0) {
                            { let _ = &srcs; self.bk_ret(e); }
                        }
                    }
                    let r = r?;
                    if r.1 == Ty::Never {
                        diverged = true;
                    }
                    if last && !*semi {
                        result = r;
                        self.drop_tail_temps(t0, result);
                    } else {
                        if !*semi && !matches!(r.1, Ty::Unit | Ty::Never) {
                            return err(SP, "expected `;` after non-unit expression");
                        }
                        // (unused_must_use: a `Result` dropped by its statement)
                        if self.prog.deny_must_use && self.user_code {
                            // (`foo();` with `#[must_use] fn foo() -> T`)
                            if let Expr::Call(f, _) = strip_paren(e) {
                                if let Expr::Path(None, p) = &**f {
                                    if let Some(n) = p.segs.last().map(|s| s.name.as_str()).filter(|n| self.prog.must_use_fns.iter().any(|m| m == n)) {
                                        return Err(self.at(expr_start(e), format!("unused return value of `{n}` that must be used")));
                                    }
                                }
                            }
                            if let Ty::Struct(sid) = r.1 {
                                if self.prog.structs[self.prog.base_of(sid) as usize].name == "Result" {
                                    return Err(self.at(expr_start(e), "unused `Result` that must be used".into()));
                                }
                            }
                        }
                        if let (Some(v), t) = r {
                            if t != Ty::Never {
                                self.own_temp(v, t);
                            }
                        }
                        self.drop_temps(t0);
                    }
                }
                // (a `use` in a body, left by a macro's expansion: names are resolved by their last segment here)
                Stmt::Item(it) if matches!(it.kind, crate::parse::ItemKind::Use(_)) => continue,
                Stmt::Item(it) => {
                    let k = format!("{:?}", it.kind);
                    let k = k.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_string();
                    return err(SP, format!("nested items are not supported ({k})"));
                }
            }
        }
        self.pop_scope();
        if diverged && result.1 == Ty::Unit {
            result = (None, Ty::Never);
        }
        Ok(result)
    }

    /// コード生成せずに型だけ推測する (整数リテラルの型決定用)。
    pub fn peek(&self, e: &Expr) -> Option<Ty> {
        // (組み込みの AsRef: 型推論の結果より先に)
        if let Expr::MethodCall(r, m, _, a, _) = e {
            if m == "as_ref" && a.is_empty() {
                if let Some(t) = self.as_ref_ty(r, None) {
                    return Some(t);
                }
            }
        }
        if let Some(&t) = self.lets.get(&(e as *const Expr as usize)) {
            return Some(t);
        }
        // (書き換えで複製した式は、同じ形の式の型で引く。形が同じで型の違う式があれば使わない)
        if !self.texts.is_empty() && !matches!(e, Expr::Path(..)) {
            if let Some(&Some(t)) = self.texts.get(&crate::ast_infer::expr_hash(e)) {
                return Some(t);
            }
        }
        self.peek_syn(e)
    }

    pub fn peek_syn(&self, e: &Expr) -> Option<Ty> {
        match e {
            // `f64::NAN` / `std::f64::consts::PI`
            Expr::Path(None, p)
                if p.segs.len() >= 2
                    && p.segs.iter().any(|s| s.name == "f64" || s.name == "f32")
                    && p.segs.last().is_some_and(|s| s.name.chars().all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())) =>
            {
                Some(if p.segs.iter().any(|s| s.name == "f32") { Ty::F32 } else { Ty::F64 })
            }
            // `u64::MAX` / `i128::MIN` / `u32::BITS`
            Expr::Path(None, p) if p.segs.len() == 2 && matches!(p.segs[1].name.as_str(), "MAX" | "MIN" | "BITS") && Ty::from_suffix(&p.segs[0].name).is_some() => {
                if p.segs[1].name == "BITS" { Some(Ty::U32) } else { Ty::from_suffix(&p.segs[0].name) }
            }
            Expr::Path(None, p) if p.segs.len() == 2 && self.const_of(e).is_some() => Some(self.prog.consts[self.const_of(e)? as usize].ty),
            Expr::Path(None, _) if self.const_generic(e).is_some() => Some(self.const_generic_ty(e).unwrap_or(Ty::Usize)),
            Expr::Try(x) => {
                let Ty::Struct(sid) = self.peek(x)? else { return None };
                let sd = &self.prog.structs[sid as usize];
                sd.variants.iter().find(|v| v.name == "Some" || v.name == "Ok").and_then(|v| v.fields.first()).map(|f| f.1)
            }
            Expr::Lit(l, _) => match lit::lit(l)? {
                Lit::Int(i) if i.suffix() == "f32" => Some(Ty::F32),
                Lit::Int(i) if i.suffix() == "f64" => Some(Ty::F64),
                Lit::Float(f) => Some(if f.suffix() == "f32" { Ty::F32 } else { Ty::F64 }),
                Lit::Int(i) => Ty::from_suffix(i.suffix()),
                Lit::Bool(_) => Some(Ty::Bool),
                Lit::Byte(_) => Some(Ty::U8),
                Lit::Char(_) => Some(Ty::Char),
                Lit::Str(_) => Some(Ty::Ref(false, intern(Ty::Str))),
                Lit::ByteStr(b) => Some(Ty::Ref(false, intern(Ty::Array(intern(Ty::U8), b.len() as u32)))),
                _ => None,
            },
            Expr::Path(None, p) if ident_of(p).is_some_and(|id| self.lookup(id).is_none() && self.prog.const_index.contains_key(id)) => {
                let ci = self.prog.const_index[ident_of(p).unwrap()];
                Some(self.prog.consts[ci as usize].ty)
            }
            // `m::S` (a static in a module): the type of `S`, as `expr` / `place` read it
            Expr::Path(None, p) if p.segs.len() >= 2 && self.static_of(e).is_some() => Some(self.prog.consts[self.static_of(e)? as usize].ty),
            Expr::Path(None, p) if p.segs.len() == 2 => self.prog.variant_by_p(&PPath::of(p), self.self_sid).map(|v| Ty::Struct(v.0)),
            // タプル構造体の構築 `V(1, 2)` (ジェネリックでないもの)
            Expr::Call(f, _)
                if matches!(&**f, Expr::Path(None, p) if p.segs.len() == 1
                    && self.lookup(&p.segs[0].name).is_none()
                    && self.prog.struct_by_p(&PPath::of(p), self.self_sid)
                        .is_some_and(|s| self.prog.structs[s as usize].generics.is_empty())) =>
            {
                let Expr::Path(None, p) = &**f else { unreachable!() };
                self.prog.struct_by_p(&PPath::of(p), self.self_sid).map(Ty::Struct)
            }
            Expr::Call(f, _) if matches!(&**f, Expr::Path(None, p) if p.segs.len() == 2) => {
                let Expr::Path(None, p) = &**f else { unreachable!() };
                match self.prog.variant_by_p(&PPath::of(p), self.self_sid) {
                    Some((sid, _)) => Some(Ty::Struct(sid)),
                    None => self.callee(f).map(|(_, s)| s.ret),
                }
            }
            Expr::Path(None, p) => ident_of(p).and_then(|id| self.lookup(id)).map(|(_, t)| t).or_else(|| self.float_const_name(e).map(|_| Ty::F64)),
            Expr::Call(..) if self.prim_method_call(e).is_some() => self.peek(&self.prim_method_call(e)?),
            Expr::Call(f, _) => match self.callee(f) {
                Some((_, s)) => Some(s.ret).filter(|&t| !self.prog.has_param(t)),
                None => {
                    let mut t = self.peek(f)?;
                    while let Ty::Ref(_, i) = t {
                        t = *i;
                    }
                    self.closure_sig(t).map(|(_, r)| r).filter(|&t| !self.prog.has_param(t))
                }
            },
            Expr::Cast(_, ty) => self.parse_ty(ty).ok(),
            Expr::Index(b, ix, _) if !matches!(&**ix, Expr::Range(..)) => {
                let mut t = self.peek(b)?;
                loop {
                    match t {
                        Ty::Array(e, _) => return Some(*e),
                        Ty::Ref(_, Ty::Slice(e)) => return Some(**e),
                        Ty::Ref(_, i) => t = *i,
                        _ => return None,
                    }
                }
            }
            Expr::Array(a) => {
                let e = a.iter().find_map(|e| self.peek(e))?;
                Some(Ty::Array(intern(e), a.len() as u32))
            }
            Expr::MethodCall(recv, name, _, args, _) => {
                let mut t = self.peek(recv)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                if name == "to_string" && args.is_empty() {
                    if let Some(&s) = self.prog.struct_index.get("String") {
                        return Some(Ty::Struct(s));
                    }
                }
                if t == Ty::Str {
                    return match name.as_str() {
                        "trim" | "as_str" => Some(Ty::Ref(false, intern(Ty::Str))),
                        "starts_with" | "ends_with" | "contains" | "is_empty" => Some(Ty::Bool),
                        "len" => Some(Ty::Usize),
                        _ => None,
                    };
                }
                if t.is_int() || t.is_float() {
                    return match name.as_str() {
                        "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" | "leading_ones" | "trailing_ones" => Some(Ty::U32),
                        "is_power_of_two" | "is_positive" | "is_negative" | "is_nan" | "is_finite" | "is_infinite" | "is_sign_negative"
                        | "is_sign_positive" => Some(Ty::Bool),
                        "pow" | "wrapping_add" | "wrapping_sub" | "wrapping_mul" | "wrapping_neg" | "wrapping_abs" | "saturating_abs" | "saturating_neg" | "saturating_add" | "saturating_sub" | "abs" | "min"
                        | "max" | "rem_euclid" | "div_euclid" | "next_multiple_of" | "next_power_of_two" | "rotate_left" | "rotate_right"
                        | "swap_bytes" | "to_be" | "to_le" | "signum" | "wrapping_pow" | "saturating_pow" | "isqrt" | "sqrt" | "floor" | "ceil" | "round" | "trunc"
                        | "powi" | "powf" | "exp" | "ln" | "sin" | "cos" | "tan" | "clamp" => Some(t),
                        _ => None,
                    };
                }
                // (`unsafe { a.get_unchecked(i) }.wrapping_shr(..)`: the element's reference; with a range, the subslice)
                if let Ty::Array(el, _) | Ty::Slice(el) = t {
                    if name == "get_unchecked" || name == "get_unchecked_mut" {
                        let m = name.ends_with("_mut");
                        return Some(match args.first() {
                            Some(Expr::Range(..)) => Ty::Ref(m, intern(Ty::Slice(el))),
                            _ => Ty::Ref(m, el),
                        });
                    }
                }
                let Ty::Struct(sid) = t else { return None };
                let id = self.prog.method(sid, name).or_else(|| self.prog.default_method(sid, name))?;
                let sig = &self.prog.sigs[id as usize];
                let mut sol = vec![None; sig.generics.len()];
                if sig.trait_of.is_some() {
                    if let Some(s0) = sol.first_mut() {
                        *s0 = Some(Ty::Struct(sid));
                    }
                } else if let Some(st) = sig.self_ty {
                    self.prog.match_ty(st, Ty::Struct(sid), &mut sol);
                }
                Some(self.prog.partial_subst(sig.ret, &sol)).filter(|&t| !self.prog.has_param(t))
            }
            Expr::Paren(p) => self.peek(p),
            Expr::Macro(p, _, _) if is_ident(p, "format") => self.prog.struct_index.get("String").map(|&s| Ty::Struct(s)),
            Expr::Unary(op, x, _) if op == "*" => match self.peek(x)? {
                Ty::Ref(_, i) => Some(*i),
                _ => None,
            },
            Expr::Unary(_, x, _) => match self.peek(x)? {
                Ty::Ref(_, i) if i.is_int() || i.is_float() || *i == Ty::Bool => Some(*i),
                t => Some(t),
            },
            // `&v[a..b]`: スライス (文字列なら &str)
            Expr::Ref(m, x) if matches!(strip_paren(x), Expr::Index(_, ix, _) if matches!(&**ix, Expr::Range(..))) => {
                let Expr::Index(b, _, _) = strip_paren(x) else { unreachable!() };
                let mut t = self.peek(b)?;
                while let Ty::Ref(_, i) = t {
                    if t.is_fat() {
                        break;
                    }
                    t = *i;
                }
                let m = *m;
                match t {
                    Ty::Array(e, _) => Some(Ty::Ref(m, intern(Ty::Slice(e)))),
                    Ty::Ref(_, Ty::Slice(e)) => Some(Ty::Ref(m, intern(Ty::Slice(e)))),
                    Ty::Ref(_, Ty::Str) => Some(Ty::Ref(m, intern(Ty::Str))),
                    Ty::Struct(sid) => match self.vec_fields(sid) {
                        Some((_, _, e)) => Some(Ty::Ref(m, intern(Ty::Slice(intern(e))))),
                        None if self.prog.structs[sid as usize].name == "String" => Some(Ty::Ref(m, intern(Ty::Str))),
                        None => None,
                    },
                    _ => None,
                }
            }
            Expr::Ref(m, x) => Some(Ty::Ref(*m, intern(self.peek(x)?))),
            Expr::Field(b, name, _) => {
                let mut t = self.peek(b)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                self.prog.field(sid, name).map(|(t, _)| t)
            }
            Expr::Struct(p, _, _) => {
                let pp = PPath::of(p);
                self.prog.struct_by_p(&pp, self.self_sid).or_else(|| self.prog.variant_by_p(&pp, self.self_sid).map(|v| v.0)).map(Ty::Struct)
            }
            Expr::Binary(op, _, _, _) if is_cmp(op) || op == "&&" || op == "||" => Some(Ty::Bool),
            Expr::If(_, then, _) => match then.stmts.last() {
                Some(Stmt::Expr(e, false)) => self.peek(e),
                _ => None,
            },
            Expr::Block(_, _, b) => match b.stmts.last() {
                Some(Stmt::Expr(e, false)) => self.peek(e),
                _ => None,
            },
            Expr::Binary(op, l, _, _) if op == "<<" || op == ">>" => self.peek(l),
            Expr::Binary(_, l, r, _) => self.peek(l).or_else(|| self.peek(r)),
            _ => None,
        }
    }

    /// `f` / `S::f` / `Self::f` 形式の呼び出し先
    pub fn callee(&self, f: &Expr) -> Option<(u32, &FnSig)> {
        let Expr::Path(q, p) = f else { return None };
        let by_ty = |t: Ty, m: &str| -> Option<u32> {
            match t {
                Ty::Struct(sid) => self.prog.method(sid, m).or_else(|| self.prog.default_method(sid, m)),
                // (else a default fn of a trait t implements: a blanket `impl<T> Tr for T {}`)
                t => self.prog.prim_method(t, m).or_else(|| {
                    if matches!(t, Ty::Param(_)) {
                        return None;
                    }
                    self.prog.traits.iter().find_map(|(tr, ids)| {
                        ids.iter().copied().find(|&d| self.prog.sigs[d as usize].has_body && self.prog.names[d as usize].rsplit("::").next() == Some(m)).filter(|_| self.prog.implements(t, tr))
                    })
                }),
            }
        };
        // `<T as Trait>::m` / `<T>::m`
        if let Some(q) = q {
            let t = self.parse_ty(q).ok()?;
            let m = p.segs.last()?.name.clone();
            // (`<S as Tr>::m`: Tr's impl, also beside an inherent `S::m` of the same name)
            if p.segs.len() >= 2 {
                let tr = &p.segs[p.segs.len() - 2].name;
                if let Some(i) = self.trait_candidate(&format!("{}::{m}", self.ty_key(t)), tr) {
                    return Some((i, &self.prog.sigs[i as usize]));
                }
            }
            let i = by_ty(t, &m)?;
            return Some((i, &self.prog.sigs[i as usize]));
        }
        // `T::f(..)` (T は型引数。インスタンスでは具体的な型)
        if p.segs.len() == 2 {
            let g = &p.segs[0].name;
            if self.env.params.iter().any(|(n, _)| n == g) {
                let ty = crate::parse::Ty::Path(None, path_of(g));
                if let Ok(t) = self.parse_ty(&ty) {
                    if let Some(i) = by_ty(t, &p.segs[1].name) {
                        return Some((i, &self.prog.sigs[i as usize]));
                    }
                    // (still a parameter (`T::identity(t)` in a generic body): the one trait fn of that name, as
                    // `<T as Tr>::m`)
                    if let Ty::Param(_) = t {
                        let m = &p.segs[1].name;
                        let mut found = self.prog.traits.values().flatten().filter(|&&d| self.prog.names[d as usize].rsplit("::").next() == Some(m.as_str()));
                        if let (Some(&d), None) = (found.next(), found.next()) {
                            return Some((d, &self.prog.sigs[d as usize]));
                        }
                    }
                }
            }
        }
        if let Some(id) = ident_of(p) {
            if self.lookup(id).is_some() {
                return None;
            }
        }
        let i = match self.prog.fn_by_p(&PPath::of(p), self.self_sid) {
            Some(i) => i,
            // (`S::f()` where f is a default fn of a trait S implements)
            None => {
                let t = self.path_self_ty(p)?;
                let i = by_ty(t, &p.segs[1].name)?;
                self.prog.sigs[i as usize].trait_of.as_ref()?;
                i
            }
        };
        Some((i, &self.prog.sigs[i as usize]))
    }

    /// The name methods of t are indexed under (`S` for a struct's instances too, `u8`, `#tuple0`)
    pub fn ty_key(&self, t: Ty) -> String {
        match t {
            Ty::Struct(sid) => self.prog.structs[self.prog.base_of(sid) as usize].name.clone(),
            Ty::Unit => "#tuple0".to_string(),
            t => self.prog.tn(t),
        }
    }

    /// The method named `name` (`S::m`, among its overloads) that is an impl of trait tr
    pub fn trait_candidate(&self, name: &str, tr: &str) -> Option<u32> {
        self.trait_candidate_where(name, tr, |_| true)
    }

    /// trait_candidate among those `ok` accepts (`impl PartialEq for &TX<u8>` / `.. for &TX<u16>`: the receiver's)
    pub fn trait_candidate_where(&self, name: &str, tr: &str, ok: impl Fn(u32) -> bool) -> Option<u32> {
        let of_tr = |i: u32| self.prog.sigs[i as usize].impl_trait.as_deref().is_some_and(|t| t.split('<').next().unwrap_or(t).trim() == tr) && ok(i);
        let first = self.prog.index.get(name).copied();
        first.filter(|&i| of_tr(i)).or_else(|| self.prog.overloads.get(name).and_then(|cs| cs.iter().copied().find(|&c| of_tr(c))))
    }

    /// the type `S` of a path `S::f` (two segments, the first a type the program knows), or None
    pub fn path_self_ty(&self, p: &Path) -> Option<Ty> {
        if p.segs.len() != 2 {
            return None;
        }
        let seg = p.segs[0].clone();
        if self.lookup(&seg.name).is_some() {
            return None;
        }
        let ty = crate::parse::Ty::Path(None, Path { global: false, segs: vec![seg], pos: Default::default() });
        self.parse_ty(&ty).ok().filter(|t| !matches!(t, Ty::Param(_)))
    }

    /// 関数 id (インスタンスも) を呼ぶための参照
    pub fn fref(&mut self, id: u32, sig: &FnSig) -> FuncRef {
        if let Some(&f) = self.frefs.get(&id) {
            return f;
        }
        let rets: Vec<Type> = clif_sig(sig).returns.iter().map(|p| p.value_type).collect();
        let f = self.b.import_function(&sym_name('f', id), rets);
        self.frefs.insert(id, f);
        f
    }

    /// 無名データ i のアドレス
    pub fn data_sym(&mut self, i: u32) -> Value {
        let g = self.b.symbol(&sym_name('d', i));
        self.b.ins().symbol_value(PTR, g)
    }

    /// static i のアドレス
    pub fn static_sym(&mut self, i: u32) -> Value {
        // (a static of an extern block: the linked program's symbol)
        if let Some((_, sym)) = crate::parse::extern_static(&self.prog.consts[i as usize].expr) {
            let g = self.b.symbol(sym);
            return self.b.ins().symbol_value(PTR, g);
        }
        let g = self.b.symbol(&sym_name('s', i));
        self.b.ins().symbol_value(PTR, g)
    }

    /// vtable i のアドレス
    pub fn vtable_sym(&mut self, i: u32) -> Value {
        let g = self.b.symbol(&sym_name('v', i));
        self.b.ins().symbol_value(PTR, g)
    }
}

/// The parameters of a fn declared as one of its type parameters (`x: T` / `x: &T`), root being the fn whose generics
/// they name: (name, type parameter)
pub(crate) fn generic_args_of(prog: &Program, params: &[(crate::parse::Pat, crate::parse::Ty)], root: u32) -> Vec<(String, String)> {
    let Some(rs) = prog.sigs.get(root as usize).filter(|rs| !rs.generics.is_empty()) else { return vec![] };
    params
        .iter()
        .filter_map(|(p, t)| {
            let crate::parse::Pat::Ident(_, _, n, None) = p else { return None };
            let t = match t {
                crate::parse::Ty::Ref(_, _, i) => &**i,
                t => t,
            };
            let crate::parse::Ty::Path(None, tp) = t else { return None };
            (tp.segs.len() == 1 && tp.segs[0].args.is_empty() && rs.generics.iter().any(|g| g.0 == tp.segs[0].name)).then(|| (n.clone(), tp.segs[0].name.clone()))
        })
        .collect()
}

/// A pattern of bindings, `_` and tuples of them, which matches any value (no exhaustiveness check needed: a parameter
/// `(_, _, ..)` of thousands of elements). A lowercase name binds; an uppercase one may be a constant or a unit variant
pub(crate) fn plainly_irrefutable(p: &Pat) -> bool {
    match p {
        Pat::Wild | Pat::Rest => true,
        Pat::Ident(_, _, n, sub) => n.starts_with(|c: char| c.is_lowercase() || c == '_') && sub.as_deref().is_none_or(plainly_irrefutable),
        Pat::Tuple(ps) => ps.iter().all(plainly_irrefutable),
        Pat::Ref(_, i) => plainly_irrefutable(i),
        _ => false,
    }
}
