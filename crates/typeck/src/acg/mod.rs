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
/// A vtable entry that is a number, not a function (the concrete type's size and alignment, after the drop entry):
/// `VT_LIT | n` with n at most VT_LIT_MAX (function ids stay far below VT_LIT; `u32::MAX`, the null entry, is above
/// every literal)
pub const VT_LIT: u32 = 0xC000_0000;
pub const VT_LIT_MAX: u32 = 0x3FFF_FFFE;
/// The second word of the pair of a struct with an unsized tail made by a coercion: the tail's length, or the
/// vtable of the tail's type (`S<T>` to `S<dyn Tr>`)
#[derive(Clone, Copy)]
pub(crate) enum TailMeta {
    Len(i64),
    Dyn(Ty, &'static String),
}

pub fn vt_lit(f: u32) -> Option<u64> {
    (f != u32::MAX && f >= VT_LIT).then_some((f & !VT_LIT) as u64)
}

/// エラーの場所 (今は使わない)
/// ソース上の位置 (アイテムの中の行・桁)。分からないところは SP (0 行目)
pub type Sp = crate::parse::Pos;
pub const SP: Sp = crate::parse::Pos { line: 0, col: 0 };

/// Whether an environment variable is set, read once for each flag: getenv scans the whole environment, and these are
/// asked for each function, closure or temporary
fn env_once(cell: &'static std::sync::OnceLock<bool>, name: &str) -> bool {
    *cell.get_or_init(|| std::env::var_os(name).is_some())
}

/// ZUSTC_NO_POLONIUS=1: the borrow check's NLL rules instead of Polonius Alpha's where they differ (issue #1): a
/// reborrow returned lives to the function's end, and an assignment to a part of a local kills no loans
pub(crate) fn no_polonius() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    env_once(&C, "ZUSTC_NO_POLONIUS")
}

/// ACG_PANIC_ERR=1: an error panics (to find where it is made from the backtrace)
/// An error of the borrow or region check's kind (lowering itself returns some: `m[k] = v` of a HashMap is E0594)
pub fn borrow_error(e: &str) -> bool {
    ["cannot assign", "cannot borrow", "cannot move out", "use of moved", "borrow of moved", "does not live long enough", "may not live long enough", "(E0594)", "(E0596)", "(E0384)"]
        .iter()
        .any(|k| e.contains(k))
}

/// borrow_error, and the borrow check's other errors: a use of what is not initialized (E0381), a temporary freed while
/// borrowed (E0716), a match guard's change (E0510), a returned borrow (E0515, E0713), a closure that outlives what it
/// borrows (E0373), a use of what is mutably borrowed (E0503) or partly moved (E0382), an assignment to a part of
/// what was moved (E0382), a closure's borrow that escapes it (E0521)
/// A placeholder instance's error that holds whatever its type parameters are: an operator its bounds do not give, or
/// mismatched types of two types written
/// out (no projection `T::Item` / `<T as Tr>::X`, no `impl` / `dyn` type, no hole, no lifetime), as `x` of
/// `impl Debug` returned as a String (a placeholder is `T#`)
pub fn placeholder_mismatch(e: &str, bound_eqs: &[&str], free: &[String]) -> bool {
    // (a local where a constant goes (E0435), a binding of a variant's name (E0170): hold whatever the parameters are)
    if e.ends_with("attempt to use a non-constant value in a constant (E0435)")
        || (e.ends_with("(E0170)") && e.contains("pattern binding `"))
        || e.ends_with("use of unstable library feature `fn_traits` (E0658)")
    {
        return true;
    }
    // (an operator a type parameter's bounds do not give, p3_ops: holds whatever the parameter is)
    if (e.ends_with("(E0369)") || e.ends_with("(E0368)"))
        && ["cannot add `", "cannot subtract `", "cannot multiply `", "cannot divide `", "cannot calculate the remainder of `", "binary assignment operation `"].iter().any(|v| e.contains(v))
    {
        // (on a reference to a parameter, `&a + a`: only when no where clause may give it (`for<'x> &'x T: Add<T>`),
        // as for a free parameter)
        let on_ref = ["to `&", "from `&", "of `&"].iter().find_map(|k| e.find(k).map(|i| &e[i + k.len()..])).or_else(|| {
            ["multiply `&", "divide `&"].iter().find_map(|k| e.find(k).map(|i| &e[i + k.len()..]))
        });
        return match on_ref {
            Some(r) => {
                let n: String = r.trim_start_matches("mut ").chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                free.contains(&n)
            }
            None => true,
        };
    }
    // (`[x; 3]` of a free parameter without Copy among its bounds: holds whatever it is)
    if let Some(n) = e.split("the trait bound `").nth(1).and_then(|r| r.strip_suffix(": Copy` is not satisfied (E0277)")) {
        return n.strip_suffix('#').is_some_and(|n| free.iter().any(|f| f == n));
    }
    let Some(m) = e.find("mismatched types: expected `") else { return false };
    let tys: Vec<&str> = e[m..].split('`').skip(1).step_by(2).take(2).collect();
    // (the placeholders named: `T#`)
    let names = |t: &str| -> Vec<String> {
        t.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '#')).filter_map(|w| w.strip_suffix('#')).map(str::to_string).collect()
    };
    if tys.len() == 2 {
        let (a, b) = (names(tys[0]), names(tys[1]));
        // (two placeholders: a closure or a bound may relate them; one equated by a bound `Tr<X = T>`)
        let equated = a.iter().chain(&b).any(|n| bound_eqs.iter().any(|q| q.split('=').skip(1).any(|r| r.trim_start().split(|c: char| !(c.is_alphanumeric() || c == '_')).next() == Some(n.as_str()))));
        // (a parameter with an equation in its bounds, `=& T`: its mismatches behind a reference (`&T#`), where a deref
        // coercion through the projection may hold)
        let by_ref = |n: &str| tys.iter().any(|t| t.contains(&format!("&{n}#")) || t.contains(&format!("&mut {n}#")));
        let equated = equated || a.iter().chain(&b).any(|n| bound_eqs.iter().any(|q| q.strip_prefix("=&").is_some_and(|r| r.trim() == n.as_str())) && by_ref(n));
        // (unless every one of them is free: nothing relates those)
        let all_free = a.iter().chain(&b).all(|n| free.contains(n));
        if (!a.is_empty() && !b.is_empty() && !all_free) || equated {
            return false;
        }
    }
    // (a hole `_`, or a type parameter left unsubstituted (`T0`): what inference did not reach)
    let unknown = |t: &str| {
        t.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '#')).any(|w| {
            w == "_" || (w.starts_with(|c: char| c.is_ascii_uppercase()) && w[1..].chars().all(|c| c.is_ascii_digit()))
        })
    };
    tys.len() == 2 && tys.iter().all(|t| !t.contains("::") && !t.contains(" as ") && !t.contains("impl ") && !t.contains("dyn ") && !t.contains('\'') && !unknown(t))
}

pub fn borrow_check_error(e: &str) -> bool {
    borrow_error(e)
        || [
            "isn't initialized",
            "temporary value",
            "in match guard",
            "destructor runs",
            "cannot return",
            "may outlive the current function",
            "because it was mutably borrowed",
            "of partially moved",
            "to part of moved",
            "escapes outside of closure",
            "(E0505)",
            "(E0499)",
            "(E0502)",
            "(E0506)",
            "(E0503)",
        ]
            .iter()
            .any(|k| e.contains(k))
}

pub(crate) fn panic_err() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    env_once(&C, "ACG_PANIC_ERR")
}

pub(crate) fn no_fresh() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    env_once(&C, "ZUSTC_NO_FRESH")
}

pub(crate) fn no_ret_dst() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    env_once(&C, "ZUSTC_NO_RET_DST")
}

pub(crate) fn debug_at() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    env_once(&C, "ACG_DEBUG_AT")
}

pub(crate) fn debug_clo() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    env_once(&C, "ACG_DEBUG_CLO")
}

pub fn err<T>(_: Sp, msg: impl std::fmt::Display) -> R<T> {
    // (ACG_PANIC_ERR=1: エラーの場所をバックトレースで調べる)
    if panic_err() {
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
    /// What the function's lowering told the closures it makes (Insts::fx_log): given back to Insts with the Blob
    pub clo_facts: Vec<crate::program::CloFact>,
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
    // (an impl's method whose regions make it less general than its trait's declaration)
    if regionck::enabled() {
        let b = insts.lock().unwrap().body_of(id);
        let user = prog.item_of.get(b as usize).copied().flatten().is_some_and(|i| i < prog.prelude_from);
        if user && prog.sigs[b as usize].impl_trait.is_some() {
            if let Some(d) = regionck::trait_decl_of(prog, b) {
                if regionck::impl_sig_mismatch(prog, d, b) {
                    return Err(format!("{}:1: `impl` item signature doesn't match `trait` item signature (E0308)", line_base + 1));
                }
            }
        }
    }
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
    let (sig, env, body_id, fnptr, inst_args, clo_rg, lenient, (recheck, recheck_static), fnmut_ret, where_bound) = {
        let g = insts.lock().unwrap();
        let b = g.body_of(id);
        (g.sig(prog, id).clone(), g.env(prog, id), b, g.get(id).is_some_and(|i| i.fnptr), g.get(id).map(|i| i.args.clone()), g.closure_rg.get(&b).cloned(), g.lenient, (g.recheck.contains(&id), g.recheck_static.contains(&id)), g.kind_free.contains(&b), g.where_bound.contains(&id))
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
        bk: borrowck::Bk { on: bk_level > 0, level: bk_level, line_base, fnmut_ret, ..Default::default() },
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
        int_as_float: 0,
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
        fake_reads: prog.closures.get(&body_id).map(|cd| cd.fake_reads.clone()).unwrap_or_default(),
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
        lub_fnptr: 0,
        hr_lost: vec![],
        clo_ann: vec![],
        clo_ret_param: vec![],
        clo_ret_from: vec![],
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
        at_binds: vec![],
        at_pat_at: None,
        dead_stmt: false,
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
        deref_depth: 0,
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
    // (a fn whose instances give `Copy` types for its parameters, again with placeholders: what moving a T does is
    // reported, nothing else (the instances checked the rest). One whose body names a struct or an enum that
    // bounds a type parameter by 'static: what T: 'static its types need (E0310))
    if recheck || recheck_static {
        let kept = |e: &String| (recheck && (e.contains("moved value") || e.contains("cannot move out"))) || (recheck_static && e.contains("must outlive `'static`") && e.ends_with("(E0310)"));
        let ok = r.is_ok();
        let mut e = match r {
            Err(e) => Some(e),
            Ok(_) => cx.bk_check().err(),
        };
        // (the region check, where the T: 'static of the types is required; run again forced as compile's other
        // instances are)
        if recheck_static && ok && e.as_ref().is_none_or(|e| !kept(e)) {
            if cx.rg.rerun && !force_rg {
                *rerun_out = true;
            } else if let Err(re) = cx.rg_check() {
                e = Some(re);
            }
        }
        if let Some(e) = e.filter(kept) {
            return Err(e.replace("#`", "`"));
        }
        let b = cx.b;
        return Ok((Compiled { rec: b.m, late: vec![], datas: vec![], vtables: vec![], infos: vec![], loc_used: false, line_base }, vec![]));
    }
    // (a placeholder's mismatch holds, unless a bound equates its parameter with another type (`Tr<X = T>`))
    let mut bound_eqs: Vec<&str> = vec![];
    let mut bound_params: Vec<String> = vec![];
    if placeholder && r.is_err() {
        bound_eqs.extend(sig.generics.iter().flat_map(|(_, bs)| bs.iter().map(|b| b.as_str())).filter(|b| b.contains('=')));
        // (a parameter with an equation in its bounds (`T: Deref<Target = i32>`): its mismatches may be the projection's)
        // (behind a reference only: `&T` to `&i32` through `T: Deref<Target = i32>`, not `T` to `i32`)
        bound_params.extend(sig.generics.iter().filter(|(_, bs)| bs.iter().any(|b| b.contains('='))).map(|(n, _)| format!("=& {n}")));
    }
    // (and the fn's own where clauses, on any type: `where Back: Backend<Layout = DSL>`)
    for (subj, bs) in f.sig.generics.preds.iter().filter(|_| placeholder && r.is_err()) {
        for b in bs {
            if let crate::parse::Bound::Trait(_, p) = b {
                for seg in &p.segs {
                    for a in &seg.args {
                        if let crate::parse::GArg::Bind(_, bt) = a {
                            if let crate::parse::Ty::Path(None, tp) = bt {
                                if let Some(s) = tp.segs.last() {
                                    bound_params.push(format!("= {}", s.name));
                                }
                            }
                            // (`where T: Deref<Target = i32>`: T's mismatches behind a reference may be the projection's)
                            if let crate::parse::Ty::Path(None, sp) = subj {
                                if let [s] = sp.segs.as_slice() {
                                    bound_params.push(format!("=& {}", s.name));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    bound_eqs.extend(bound_params.iter().map(|s| s.as_str()));
    // (the parameters nothing can relate to another: bounds without an equation or a fn signature, no where clause, not
    // in a closure's body. A mismatch of two of them holds: `fn f<X, Y>(y: Y) -> X { y }`)
    let free: Vec<String> = match Some(cx.rg.inst_args.as_slice()) {
        Some(args) if placeholder && r.is_err() && f.sig.generics.preds.is_empty() && !prog.closures.contains_key(&body_id) => args
            .iter()
            .filter_map(|&t| {
                let Ty::Struct(s) = t else { return None };
                let sd = &prog.structs[s as usize];
                let bs = sd.placeholder.as_ref()?;
                // (nor one through a supertrait: `T: View<U>` of `trait View<U>: Deref<Target = U>`)
                bs.iter().all(|b| !b.contains('=') && !b.contains('(') && !prog.bound_has_eq(b, 0)).then(|| sd.name.trim_end_matches('#').to_string())
            })
            .collect(),
        _ => vec![],
    };
    // (a name nothing of the program's declares, `foo == 2` / `this.a`: holds whatever the parameters are. Not one an
    // item has (a generic fn used as a value, which zustc may not lower), nor what zustc cannot read (`cannot find
    // value (unsupported path)`))
    let undeclared = |e: &str| {
        let Some(r) = e.split("cannot find value `").nth(1) else { return false };
        let n = r.split('`').next().unwrap_or("");
        let item = prog.index.contains_key(n) || prog.struct_index.contains_key(n) || prog.consts.iter().any(|c| c.name == n || c.name.ends_with(&format!("::{n}")))
            || prog.names.iter().any(|x| x.rsplit("::").next() == Some(n))
            || prog.use_names.contains(n);
        !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_') && !e.contains("(unsupported path)") && !item
    };
    // (E0599: a method of a parameter with no bounds but markers (`fn f<T>(t: T) { t.clone() }`), and no where clause:
    // no trait gives it one, unless a blanket impl does (`impl<T> Tr for T`, std's `Into`), whatever the parameter is)
    let unbounded_method = |e: &str| -> Option<String> {
        if !f.sig.generics.preds.is_empty() || where_bound {
            return None;
        }
        let i = e.find("no method named `")?;
        let r = &e[i + 17..];
        let m = r.split('`').next()?;
        let rest = r[m.len() + 1..].strip_prefix(" found for ")?;
        let rest = rest.strip_prefix("struct ").unwrap_or(rest);
        let t = rest.strip_prefix('`')?.split('`').next()?;
        let n = t.strip_suffix('#')?;
        let marker = |b: &String| b.starts_with('\'') || matches!(b.as_str(), "Sized" | "?Sized" | "Send" | "Sync" | "Unpin");
        let unbounded = cx.rg.inst_args.iter().any(|&a| matches!(a, Ty::Struct(s) if prog.structs[s as usize].name == t && prog.structs[s as usize].placeholder.as_ref().is_some_and(|bs| bs.iter().all(marker))));
        let blanket = prog.blanket.contains_key(m) || matches!(m, "into" | "try_into" | "borrow" | "borrow_mut" | "type_id");
        (unbounded && !blanket).then(|| format!("{}no method named `{m}` found for type parameter `{n}` in the current scope (E0599)", &e[..i]))
    };
    let r = match r {
        Err(e) if placeholder => Err(unbounded_method(&e).unwrap_or(e)),
        r => r,
    };
    let held = |e: &str| e.ends_with("in the current scope (E0599)") && e.contains("found for type parameter `");
    // (nor what the trait solver proved of a type with no placeholder in it: crate::traits::SOLVED_NO)
    let solved = |e: &str| e.contains(crate::traits::SOLVED_NO) || e.contains(&crate::traits::SOLVED_NO.replace("E0277", "E0271"));
    if placeholder && r.as_ref().is_err_and(|e| !borrow_error(e) && !placeholder_mismatch(e, &bound_eqs, &free) && !undeclared(e) && !held(e) && !solved(e)) {
        if regionck::debug() {
            eprintln!("regionck: placeholder instance not checked: {}", r.as_ref().err().map(|e| e.as_str()).unwrap_or(""));
        }
        let b = cx.b;
        return Ok((Compiled { rec: b.m, late: vec![], datas: vec![], vtables: vec![], infos: vec![], loc_used: false, line_base }, vec![]));
    }
    // (a placeholder's type by its parameter's name: `T`, not `T#`)
    let r = if placeholder { r.map_err(|e| e.replace("#`", "`").replace("#>", ">").replace("#,", ",").replace("#:", ":")) } else { r };
    let r = r.map_err(|e| if solved(&e) { e.replace(crate::traits::SOLVED_NO, "(E0277)").replace(&crate::traits::SOLVED_NO.replace("E0277", "E0271"), "(E0271)") } else { e });
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
        Blob { align: 16, code: vec![], relocs: vec![], datas: vec![], vtables: vec![], traps: vec![], line_base: None, code_ref: None, clo_facts: vec![] }
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
        Blob { align: 16, code, relocs: out, datas, vtables, traps, line_base: loc_used.then_some(line_base), code_ref: None, clo_facts: vec![] }
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
                        // (a formatting macro's `&x` is read for its value; another's takes x's address (`vec![&mut x]`))
                        let fmt = matches!(name, "println" | "print" | "eprintln" | "eprint" | "format" | "format_args" | "write" | "writeln" | "panic" | "assert" | "debug_assert" | "unreachable" | "todo" | "unimplemented");
                        for e in &es {
                            match e {
                                Expr::Ref(_, x) if fmt => visit(x, out, mm),
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
        // (one the lowering made has no position: its receiver's)
        Expr::MethodCall(r, _, _, _, s) | Expr::Field(r, _, s) if s.at.line == 0 => expr_start(r),
        Expr::MethodCall(_, _, _, _, s) | Expr::Field(_, _, s) => s.at,
        e => expr_start(e),
    }
}

/// Where rustc points when a branch's value does not have the type of the branches before it (`if`/`else`, `match`
/// arms): a block's tail, the last statement of a block without one (whose `;` made the block `()`), or the
/// expression itself. SP when that has no position (an empty block): the caller's own position then
pub fn branch_pos(e: &Expr) -> Sp {
    match e {
        Expr::Block(_, _, b) => block_end_pos(b),
        e => expr_start(e),
    }
}

/// Where rustc points when a value has not the type its place wants (a `let`'s annotation, a parameter): a block's
/// tail expression, or the block itself when it has none (SP: the caller's position, its statement), or the expression
pub fn value_pos(e: &Expr) -> Sp {
    match e {
        Expr::Block(_, false, b) => match b.stmts.last() {
            Some(Stmt::Expr(t, false)) => value_pos(t),
            _ => SP,
        },
        // (a `loop`'s value is its `break`'s, `let _: i32 = loop { break };`)
        Expr::Loop(..) => loop_break_pos(e),
        e => expr_start(e),
    }
}

/// Where the value of an expression comes from, as rustc blames a mismatch: a `match`'s first arm that does not
/// diverge (`None => ()` beside `Some(v) => { return v; }`), a block's tail, the expression itself; SP if unknown
pub fn value_origin(e: &Expr) -> Sp {
    match e {
        Expr::Match(_, arms) if !arms.is_empty() => match arms.iter().find(|a| !diverges(&a.body)) {
            Some(a) => match value_origin(&a.body) {
                p if p.line != 0 => p,
                _ => a.pos,
            },
            None => SP,
        },
        Expr::Block(_, false, b) => match b.stmts.last() {
            Some(Stmt::Expr(t, false)) => value_origin(t),
            _ => SP,
        },
        e => expr_start(e),
    }
}

/// Where rustc points when a block-like statement without `;` is not `()`: the tail of the first branch that is a
/// block with a value (rustc checks a branch's block against the `()` its statement must be), SP for the statement
/// itself (`match 3 { 4 => 1, _ => 2 }`)
pub fn unit_stmt_pos(e: &Expr) -> Sp {
    fn tail(b: &crate::parse::Block) -> Sp {
        match b.stmts.last() {
            Some(Stmt::Expr(t, false)) if !matches!(t, Expr::Tuple(xs) if xs.is_empty()) && !diverges(t) => match unit_stmt_pos(t) {
                p if p.line != 0 => p,
                _ => expr_start(t),
            },
            _ => SP,
        }
    }
    match e {
        Expr::Block(_, _, b) => tail(b),
        Expr::If(_, b, els) => match tail(b) {
            p if p.line != 0 => p,
            _ => els.as_deref().map_or(SP, unit_stmt_pos),
        },
        Expr::Match(_, arms) => arms.iter().find_map(|a| match &a.body {
            Expr::Block(_, _, b) => Some(tail(b)).filter(|p| p.line != 0),
            _ => None,
        }).unwrap_or(SP),
        _ => SP,
    }
}

/// A `return` / `break` / `continue` / `panic!`-like macro, or a block whose last statement is one
fn diverges(e: &Expr) -> bool {
    match e {
        Expr::Return(..) | Expr::Break(..) | Expr::Continue(..) => true,
        Expr::Macro(p, _, _) => p.segs.last().is_some_and(|s| matches!(s.name.as_str(), "panic" | "unreachable" | "todo" | "unimplemented")),
        Expr::Block(_, _, b) => match b.stmts.last() {
            Some(Stmt::Expr(x, _)) => diverges(x),
            _ => false,
        },
        _ => false,
    }
}

/// A `loop`'s value is its `break`s': where the first one without a value is (not one of a loop or closure inside it;
/// of a labeled loop, also a `break 'a` to its label)
pub fn loop_break_pos(e: &Expr) -> Sp {
    fn in_block(b: &crate::parse::Block, l: Option<&str>) -> Sp {
        b.stmts.iter().map(|s| match s {
            Stmt::Expr(x, _) => find(x, l),
            _ => SP,
        }).find(|p| p.line != 0).unwrap_or(SP)
    }
    fn find(e: &Expr, l: Option<&str>) -> Sp {
        match e {
            Expr::Break(None, None, p) => *p,
            Expr::Break(Some(b), None, p) if Some(b.as_str()) == l => *p,
            Expr::Block(_, _, b) => in_block(b, l),
            Expr::If(_, b, els) => match in_block(b, l) {
                p if p.line != 0 => p,
                _ => els.as_deref().map_or(SP, |x| find(x, l)),
            },
            Expr::Match(_, arms) => arms.iter().map(|a| find(&a.body, l)).find(|p| p.line != 0).unwrap_or(SP),
            _ => SP,
        }
    }
    match e {
        Expr::Loop(l, b) => in_block(b, l.as_deref()),
        _ => SP,
    }
}

/// [`branch_pos`] of a block
pub fn block_end_pos(b: &crate::parse::Block) -> Sp {
    match b.stmts.last() {
        Some(Stmt::Expr(e, false)) => branch_pos(e),
        Some(Stmt::Expr(e, true)) => expr_start(e),
        Some(Stmt::Let(.., p)) => *p,
        _ => SP,
    }
}

/// 式の先頭のソース上の位置 (syn の `e.span()` の始まり)。位置を持つ式か、その一番左の子から。分からなければ SP
pub fn expr_start(e: &Expr) -> Sp {
    match e {
        // (one the lowering made has no position (`&v[..]` for `&*v`): that of its first operand)
        Expr::Unary(_, x, s) | Expr::Binary(_, x, _, s) | Expr::AssignOp(_, x, _, s) | Expr::MethodCall(x, _, _, _, s) | Expr::Index(x, _, s) if s.start.line == 0 => expr_start(x),
        Expr::Unary(_, _, s) | Expr::Binary(_, _, _, s) | Expr::AssignOp(_, _, _, s) | Expr::MethodCall(_, _, _, _, s) | Expr::Index(_, _, s) | Expr::Macro(_, _, s) => s.start,
        Expr::Field(_, _, s) => s.start,
        Expr::Call(x, _) | Expr::Cast(x, _) | Expr::Try(x) | Expr::Paren(x) | Expr::Assign(x, _) => expr_start(x),
        Expr::Range(Some(x), _, _) => expr_start(x),
        Expr::Path(_, p) | Expr::Struct(p, _, _) => p.pos,
        Expr::Lit(_, p) => *p,
        Expr::Closure(.., p) => *p,
        // (no position of their own: that of their first part, usually on the same line)
        Expr::Ref(_, x) | Expr::RawRef(_, x) | Expr::Repeat(x, _) | Expr::Let(_, x) | Expr::Match(x, _) | Expr::If(x, _, _) => expr_start(x),
        Expr::While(_, x, _) | Expr::For(_, _, x, _) | Expr::Return(Some(x), _) | Expr::Break(_, Some(x), _) => expr_start(x),
        // (`break` / `continue` / `break 'a`: the keyword, or the label when there is one)
        Expr::Break(_, None, p) | Expr::Continue(_, p) => *p,
        Expr::Array(es) | Expr::Tuple(es) => es.first().map_or(SP, expr_start),
        _ => SP,
    }
}

/// A message of what zustc does not support (not an error rustc reports)
pub fn unsupported(m: &str) -> bool {
    m.contains("not supported") || m.contains("unsupported") || m.starts_with("array length must be a literal or a const")
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

/// A `&mut T` wanted of a reference to a reference with a shared one on the way (`&mut &T`, `&&mut T`): the deref
/// coercion would borrow mutably through a `&`, rustc's E0596 where the value is coerced
pub(crate) fn mut_through_shared(t: Ty, want: Ty) -> bool {
    let Ty::Ref(true, w) = want else { return false };
    let (mut cur, mut shared) = (t, false);
    for depth in 0..8 {
        let Ty::Ref(m, inner) = cur else { return false };
        shared |= !m;
        if depth >= 1 && *inner == *w {
            return shared;
        }
        cur = *inner;
    }
    false
}

pub(crate) const MUT_THROUGH_SHARED: &str = "cannot borrow data in a `&` reference as mutable (E0596)";

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

/// How a binding of a pattern binds, for the check of `x @ sub` (at_conflict)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AtBind {
    /// by value, of a type that is not `Copy`
    Move,
    /// by value, of a `Copy` type
    Copy,
    /// by reference (`ref`, `ref mut`, or the default binding mode): whether `mut`
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
    /// The text of the last integer literal reported where a float was expected (its address: the operator around
    /// it reports rustc's E0277 instead)
    pub int_as_float: usize,
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
    /// A closure's body: the variables it reads only by `let _ = place;`, which it does not capture
    pub fake_reads: Vec<String>,
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
    /// >0 while lowering match arms / array or vec! elements whose fn-pointer type came from closures_lub: a closure
    /// local there is coerced to its fn pointer (nowhere else: calling a closure local must not become a fn pointer)
    pub lub_fnptr: u32,
    /// Closures made where nothing was expected (their kind, signature: from the body), by their definition: the
    /// parameters with no type written that are references (one region each, not higher-ranked)
    pub hr_lost: Vec<(u32, Vec<usize>)>,
    /// The closures made with a parameter annotated with a lifetime (`|_: &'static u32| {}`), by struct: their
    /// expressions, for a cast of a local holding one to a fn pointer type (rg_closure_fnptr)
    pub clo_ann: Vec<(u32, Expr)>,
    /// Of those, the ones whose body is one of their parameters (`|y| y`): that parameter's type argument
    pub clo_ret_param: Vec<(u32, usize)>,
    /// A closure's parameters its result is made of, as its body is written (borrowck::closure_ret_params): a call's
    /// result holds what those arguments borrow
    pub clo_ret_from: Vec<(u32, Vec<usize>)>,
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
    /// The bindings the subpattern of each `x @ sub` being bound makes, innermost last: by move (not `Copy`), by copy,
    /// or by reference (whether `mut`), with their names (pat_bind, at_conflict)
    pub at_binds: Vec<Vec<(AtBind, String)>>,
    /// Where the pattern a `let` binds starts, while it binds (at_pat_pos)
    pub at_pat_at: Option<Sp>,
    /// The statement being lowered comes after one that diverges, in its block or one around it (`return;`): the
    /// borrow check's errors are not reported there, so the moves of an `x @ sub` are not checked (at_conflict)
    pub dead_stmt: bool,
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
    /// The overloaded derefs a field access is lowered through so far (`(*s.deref()).f`, then again): at most rustc's
    /// default recursion limit (128), then E0055
    pub deref_depth: u32,
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
        // (a tuple struct's constructor, not a fn of the same name (`fn AddFlags(..) -> AddFlags` with a braced
        // `struct AddFlags`), which is called when it runs, as the call's lowering decides)
        Expr::Call(f, args) => {
            matches!(&**f, Expr::Path(None, p)
                if cx.prog.variant_by_p(&PPath::of(p), cx.self_sid).is_some()
                    || (cx.prog.struct_by_p(&PPath::of(p), cx.self_sid).is_some()
                        && cx.prog.fn_by_p(&PPath::of(p), cx.self_sid).is_none()))
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
        P::Fn(ps, r) => ps.iter().any(names_std_path) || names_std_path(r),
        _ => false,
    }
}

pub fn has_hole(prog: &Program, t: Ty) -> bool {
    match t {
        Ty::Param(i) => i == crate::program::HOLE,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => has_hole(prog, *i),
        // (`_` は型パラメータなので、型パラメータの残らないインスタンス (symbolic でない) には無い: 辿らない)
        Ty::Struct(sid) => prog.structs[sid as usize].symbolic && prog.structs[sid as usize].args.iter().any(|&a| has_hole(prog, a)),
        // (`let f: fn(_) -> _ = foo;`: its parameters' and result's)
        Ty::Fn(f) => f.params.iter().any(|&p| has_hole(prog, p)) || has_hole(prog, f.ret),
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
        let pp = PPath::of(&path);
        // (or a unit struct: `let Unit = u;` binds nothing, nor moves u)
        self.prog.variant_by_p(&pp, self.self_sid).is_some() || self.prog.struct_by_p(&pp, self.self_sid).is_some_and(|s| self.unit_struct_pat(n, Ty::Struct(s)))
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
        Pat::Or(cs, _) => cs.iter().any(has_bindings),
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
            if panic_err() {
                panic!("mismatched types: expected `{}`, found `{}`", self.prog.tn(want), self.prog.tn(got));
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
        let r = self.prog.resolve_env(&ts, &self.env).map_err(|e| self.local_len_err(t, e))?;
        // (E0277: a type written in the program's code that puts a maybe-unsized type where only a sized one may be:
        // `Foo<T>` of a `T: ?Sized` with `struct Foo<T>`, `Vec<str>`, `[[u8]; 2]`)
        if self.user_code && self.inline_depth == 0 {
            if let Some(bad) = self.prog.sized_wf(r, 0) {
                let n = crate::traits::display_ty(self.prog, bad);
                return Err(format!("the size for values of type `{n}` cannot be known at compilation time {}", crate::traits::SOLVED_NO));
            }
        }
        Ok(r)
    }

    /// E0435 of an array type whose length names a local, `let n = 3; let a: [u8; n];`, at the name (the program's
    /// "cannot find constant `n`" otherwise)
    #[cold]
    #[inline(never)]
    fn local_len_err(&self, t: &crate::parse::Ty, e: String) -> String {
        use crate::parse::Ty as T;
        let Some(n) = e.strip_prefix("cannot find constant `").and_then(|r| r.strip_suffix('`')) else { return e };
        if !self.user_code || self.lookup(n).is_none() {
            return e;
        }
        fn find(t: &T, n: &str) -> Option<Sp> {
            match t {
                T::Array(el, len) => match strip_paren(len) {
                    Expr::Path(None, p) if ident_of(p) == Some(n) => Some(p.pos),
                    _ => find(el, n),
                },
                T::Ref(_, _, x) | T::Ptr(_, x) | T::Slice(x) => find(x, n),
                T::Tuple(ts) => ts.iter().find_map(|x| find(x, n)),
                _ => None,
            }
        }
        match find(t, n) {
            Some(sp) => self.at(sp, "attempt to use a non-constant value in a constant (E0435)".to_string()),
            None => e,
        }
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
        if !no_fresh() {
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
        // (plain loops: a `flat_map` over the scopes costs more per name, and most lookups of a function's names miss)
        for s in self.scopes.iter().rev() {
            for (n, l, t) in s.iter().rev() {
                if n.len() == name.len() && n.as_bytes() == name.as_bytes() {
                    return Some((*l, *t));
                }
            }
        }
        None
    }

    fn body(&mut self, f: &ItemFn, sig: &FnSig) -> R<()> {
        // (E0107: a struct or enum of the signature written with another number of lifetime arguments than it has:
        // `Foo<'_, '_, 'a>` for `struct Foo<'c, 'd>`)
        if self.user_code && self.inline_depth == 0 {
            let tps: Vec<&str> = f.sig.generics.params.iter().filter_map(|p| if let crate::parse::GParam::Ty(n, ..) = p { Some(n.as_str()) } else { None }).collect();
            for t in f.sig.params.iter().map(|p| &p.1).chain(f.sig.ret.iter()) {
                self.sig_lt_arity(t, &tps, 0)?;
            }
        }
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
        // (`mut x` parameters, for the borrow checker; `self` is `mut self` / `mut self: T` (not `&mut self`, whose
        // binding is not mutable))
        let mut muts: Vec<Option<bool>> = vec![];
        if let Some(r) = &f.sig.recv {
            muts.push(Some(r.starts_with("mut ")));
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
        // (or, before edition 2024, one whose bounds name no lifetime (`-> impl Iterator<Item = u32>`, `+ use<>`): it
        // captures none, so its hidden type's regions are 'static too (E0700 in rustc). Not with type parameters,
        // which it captures)
        self.bk.lint_mut = self.prog.deny_unused_mut && self.user_code && self.bk_on() && self.bk.level >= 2;
        self.bk.ret_impl = matches!(&f.sig.ret, Some(crate::parse::Ty::Impl(_)));
        self.bk.ret_no_capture = self.bk_on()
            && self.user_code
            && self.prog.opaque.contains(&self.rg.base)
            && self.prog.edition < 2024
            && !sig.is_generic()
            && self.rg.inst_args.is_empty()
            && matches!(&f.sig.ret, Some(crate::parse::Ty::Impl(bs)) if !bs.iter().any(|b| matches!(b, crate::parse::Bound::Lifetime(l) if l == "'static")) && crate::regions::captures_no_lifetime(bs));
        if self.bk.ret_no_capture {
            // (a parameter whose regions are all 'static: its slots write `'static`, or lifetimes bounded by it)
            if let Some(r) = sig.regions.as_deref() {
                // (through the outlives bounds: `'a: 'b, 'b: 'static`)
                let st = |c: u8| {
                    let mut seen = vec![c];
                    let mut k = 0;
                    while k < seen.len() && seen.len() < 64 {
                        let x = seen[k];
                        if x == crate::types::R_STATIC {
                            return true;
                        }
                        for &(a, b) in &r.outlives {
                            if a == x && !seen.contains(&b) {
                                seen.push(b);
                            }
                        }
                        k += 1;
                    }
                    false
                };
                let n = r.slots.len().saturating_sub(1);
                for (j, s) in r.slots[..n].iter().enumerate() {
                    // (a path written with no lifetime is a mark that stands for no region when its type has none
                    // (`&'static String`): the regions are the codes but those marks, as many as the type has)
                    let codes: Vec<u8> = s.iter().copied().filter(|&c| c != crate::types::R_ELIDED_ADT).collect();
                    let pt = sig.params.get(j).copied();
                    let fits = pt.is_some_and(|t| regionck::count(self.prog, t, 0) == Some(codes.len()));
                    if !codes.is_empty() && fits && codes.iter().all(|&c| st(c)) {
                        if let Some(&l) = self.bk.params.get(j) {
                            self.bk.static_params.push(l);
                        }
                    }
                    // (a region under the parameter's own reference (`&Foo<'a>`'s 'a; all of a value's), or a mark
                    // that may stand for one)
                    let inner = if matches!(pt, Some(Ty::Ref(..))) { s.get(1..).unwrap_or(&[]) } else { &s[..] };
                    if inner.iter().any(|&c| if c == crate::types::R_ELIDED_ADT { !fits } else { !st(c) }) {
                        if let Some(&l) = self.bk.params.get(j) {
                            self.bk.inner_params.push(l);
                        }
                    }
                }
            }
        }
        self.rg.static_opaque = self.rg.on
            && self.rg.closure.is_none()
            && self.prog.opaque.contains(&self.rg.base)
            && match &f.sig.ret {
                Some(crate::parse::Ty::Impl(bs)) => {
                    bs.iter().any(|b| matches!(b, crate::parse::Bound::Lifetime(l) if l == "'static"))
                        || (self.prog.edition < 2024 && !sig.is_generic() && self.rg.inst_args.is_empty() && crate::regions::captures_no_lifetime(bs))
                }
                _ => false,
            };
        if self.rg.on && sig.regions.is_none() && self.rg.closure.is_none() && !self.rg.force && !self.rg.static_opaque {
            let n: usize = sig.params.iter().map(|&t| regionck::count(self.prog, t, 0).unwrap_or(0)).sum();
            // (a placeholder's region is universal, whether a parameter's type has it or not (`f::<T>(x)`))
            let n = n + self.rg.inst_args.iter().filter(|&&t| matches!(t, Ty::Struct(s) if self.prog.is_placeholder(s))).count();
            // (an `impl Trait` result of a fn with no region in its signature: its hidden type captures none, its
            // regions are 'static)
            let opaque = n == 0 && !sig.is_generic() && self.prog.opaque.contains(&self.rg.base) && regionck::count(self.prog, sig.ret, 0).is_some_and(|c| c > 0);
            // (a placeholder instance (zustc check of a generic fn nothing calls) where a struct or an enum of the program
            // bounds a type parameter by 'static: its types' well-formedness may need T: 'static (`None::<Static<T>>`
            // with `struct Static<T: 'static>(T)`, E0310))
            let static_bounds = n == 1 && regionck::placeholder_static_bounds(self.prog, &self.rg.inst_args);
            if n < 2 && !(n == 1 && regionck::has_dyn(self.prog, sig.ret, 0)) && !opaque && !static_bounds {
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
                // (a parameter whose type is a projection the summary does not follow (`_wf: <&'a u8 as Tr>::Ty`): its
                // impl's associated type's value, with the regions its self type writes; what it implies is known
                // (implied-bounds/assoc-ty-wf-used-to-get-assoc-ty.rs))
                // (the where clauses on a type that is no type parameter (`&'max G: Get`), with their trees as written:
                // a call whose bound only such a clause meets takes its regions)
                if self.rg.on {
                    for (subj, bs) in &f.sig.generics.preds {
                        if matches!(subj, crate::parse::Ty::Path(None, p) if p.segs.len() == 1) {
                            continue;
                        }
                        let trs: Vec<String> = bs.iter().filter_map(|b| if let crate::parse::Bound::Trait(false, p) = b { p.segs.last().map(|s| s.name.clone()) } else { None }).collect();
                        if trs.is_empty() || !crate::regions::names_lt(subj) {
                            continue;
                        }
                        let Ok(t) = self.parse_ty(subj) else { continue };
                        let mut sl = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                        let mut codes = vec![];
                        sl.slots(subj, false, &mut codes);
                        let lts = self.rg.lts.clone();
                        let prog = self.prog;
                        if let Some(tr) = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var()) {
                            self.rg.where_trees.push((t, tr, trs));
                        }
                    }
                }
                if self.rg.on && !self.prog.assoc_regions.is_empty() {
                    // (and a projection a parameter's type or the result's is requires what its impl's header declares
                    // of the impl's lifetimes, reported at the type)
                    for pty in f.sig.params.iter().map(|x| &x.1).chain(f.sig.ret.as_ref()) {
                        if let Some(sp) = crate::wf::pos_of(pty) {
                            self.rg_proj_wf(pty, sp);
                        }
                    }
                    for (k, (p, pty)) in f.sig.params.iter().enumerate() {
                        let (crate::parse::Ty::Path(Some(_), _), Some(&(loc, t))) = (pty, ps.get(k + f.sig.recv.is_some() as usize)) else { continue };
                        let _ = p;
                        if self.rg.locals.contains_key(&loc) {
                            continue;
                        }
                        if let Some(tr) = self.rg_proj_ann(pty, t) {
                            let prog = self.prog;
                            self.rg.implied(prog, t, &tr, 0);
                            self.rg.locals.insert(loc, tr);
                        }
                    }
                }
            } else {
                self.rg.on = false;
            }
        }
        // クロージャ: キャプチャを `__env` のフィールド (by-ref なら指す先) として見せる
        // (the borrow checker's locals of the captures held by value)
        let mut cap_vals: Vec<u32> = vec![];
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
                    let id = self.bk.locals.len() as u32;
                    self.bk_local(loc, &n, t);
                    if self.bk.locals.len() as u32 > id {
                        cap_vals.push(id);
                    }
                } else if self.bk_on() {
                    // (one held by reference is the place the environment points to: `*x`, which the body borrows
                    // and assigns as the closure's own (`let p = &this.x; &mut this.x;` conflicts), and which is no
                    // local of the body's (a borrow of it may be returned))
                    let id = self.bk.locals.len();
                    self.bk_local(loc, &n, t);
                    if let Some(l) = self.bk.locals.get_mut(id) {
                        l.via_ref = true;
                    }
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
            self.bk_ret_leaves(e);
        }
        self.bk.cap_vals = cap_vals;
        let (v, t) = self.block(&f.block, Some(self.ret))?;
        self.bk_cap_escape();
        // (the temporaries of the body's tail go away after its locals (edition 2021))
        self.bk_temps_end(0);
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        // (エラーの場所は末尾の式 (なければ関数の先頭)): its value's position (a block's tail, a `loop`'s `break`), else
        // where its statement starts (`()`, `unsafe { .. }`)
        let tail = || {
            let n = f.block.stmts.len();
            match f.block.stmts.last() {
                // (a reference has no position of its own: where its statement starts, at the `&`)
                Some(Stmt::Expr(Expr::Ref(..) | Expr::RawRef(..), false)) if f.block.spos.get(n - 1).is_some() => f.block.spos.get(n - 1).unwrap_or(SP),
                Some(Stmt::Expr(e, false)) => match if matches!(e, Expr::Match(..)) { value_origin(e) } else { err_pos(e) } {
                    p if p.line != 0 => p,
                    _ => match loop_break_pos(e) {
                        p if p.line != 0 => p,
                        _ => f.block.spos.get(n - 1).unwrap_or(SP),
                    },
                },
                _ => SP,
            }
        };
        // (`fn f(x: &&T) -> &T { x }`: the deref coercion)
        let (v, t) = if t != self.ret { self.deref_coerce(v, t, self.ret) } else { (v, t) };
        if let Err(m) = self.same(SP, t, self.ret) {
            return Err(self.at(tail(), m));
        }
        self.drop_for_exit(0, 0);
        self.emit_return(v);
        Ok(())
    }

    /// e が呼び出しなら、戻り値を sret の場所に直接書かせる印を付ける
    pub fn mark_ret_call(&mut self, e: &Expr) {
        let e = strip_paren(e);
        if self.sret.is_some() && self.inline_ret.is_none() && matches!(e, Expr::Call(..) | Expr::MethodCall(..) | Expr::Struct(..)) && !no_ret_dst() {
            self.ret_call = Some(e as *const Expr as usize);
        }
    }

    /// A closure's captures held by value outlive the body's call, in its environment: at an exit of the body, what
    /// they borrow of the body's own (another capture held by value, a local) escapes it (`move || { b = Some(&a); }`,
    /// rustc's E0521). Not while inlining (the borrow check is off): a `return` there leaves the inlined body only
    pub(crate) fn bk_cap_escape(&mut self) {
        if !self.bk_on() {
            return;
        }
        for i in 0..self.bk.cap_vals.len() {
            let id = self.bk.cap_vals[i];
            if self.bk.locals.get(id as usize).is_some_and(|l| l.refs) {
                self.bk_ev(borrowck::Ev::Escape(vec![borrowck::Src::Local(id)]));
            }
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
        // (`*mut T` where a `*const T` is wanted: the same pointer, thin or fat)
        if let (Ty::Ptr(false, w), Ty::Ptr(true, a)) = (want, t) {
            if w == a {
                return (v, want);
            }
        }
        while let (Ty::Ref(wm, _), Ty::Ref(om, inner @ Ty::Ref(..)), Some(val)) = (want, t, v) {
            // (a `&mut` is not reached through a shared reference: `&&mut u8` is no `&mut u8`)
            if t == want || t.is_fat() || (wm && !om) {
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

    /// A block's value that is a `Copy` aggregate read out of a place that what the block drops may own (`v[0]` of a
    /// Vec local, `b.0` of a Box): copied out first, as rustc evaluates the tail before the drops. So is the deref
    /// coercion to the type expected of it (`let c = &v[0]; c` returned as a `&str`): what it reads is read before
    fn detach_tail(&mut self, r: V, t0: usize, want: Option<Ty>) -> V {
        let depth = self.scopes.len() - 1;
        if r.0.is_none() || !(self.temps.len() > t0 || self.owned.iter().any(|o| o.depth >= depth)) {
            return r;
        }
        let (v, t) = match want {
            Some(w @ Ty::Ref(..)) if r.1 != w && matches!(r.1, Ty::Ref(_, Ty::Ref(..))) => self.deref_coerce(r.0, r.1, w),
            _ => r,
        };
        // (not a stack slot's address (a local's, a match's result): no drop frees it)
        if let Some(v) = v {
            if t.is_agg() && !self.fresh.contains_key(&v.0) && !self.b.is_stack_addr(v) {
                let a = self.temp_fresh(t);
                self.copy(a, v, t);
                return (Some(a), t);
            }
        }
        (v, t)
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
                // (`[];`, `match () {}`, a block's tail `()`: where the parser saw the statement start. Not what zustc
                // does not support, which is no error of the program's at any line of it)
                let sp = match blk.spos.get(self.stmt_i) {
                    Some(p) if sp.line == 0 && !unsupported(&m) => p,
                    _ => sp,
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
            let nb: &'static crate::parse::Block = Box::leak(Box::new(crate::parse::Block { stmts, spos: Default::default() }));
            return self.block_stmts(nb, expected);
        }
        self.scopes.push(vec![]);
        let mut diverged = false;
        let mut result: V = (None, Ty::Unit);
        // (`let Foo { value } = Foo::<&'static u32> { value: &x };`: a destructuring let of a literal whose type
        // arguments name lifetimes, as `let mut #slI: Foo<&'static u32> = ..; let Foo { value } = #slI;`, whose
        // first let the region check reads. The block's statements are kept by reference, with their indices)
        let split = if blk.stmts.iter().any(|st| matches!(st, Stmt::Let(pat, None, Some(Expr::Struct(p, ..)), None, _) if !is_simple_let(pat) && p.segs.last().is_some_and(|s| !s.args.is_empty()))) {
            self.lifetime_lit_split(blk)
        } else {
            Vec::new()
        };
        let n = if split.is_empty() { blk.stmts.len() } else { split.len() };
        // (the borrowed temporaries of each statement end with it; the tail expression's, with the enclosing one)
        let bk_mark = self.bk.stmt_temps.len();
        let mut tail_srcs: Vec<borrowck::Src> = vec![];
        let dead0 = self.dead_stmt;
        for k in 0..n {
            self.dead_stmt = dead0 || diverged;
            let (i, st) = if split.is_empty() { (k, &blk.stmts[k]) } else { split[k] };
            // (`let x = S::<&'static u32> { t: &c };` / `S::<&'a u32> { t: &c };`: a `let` of the type the path writes,
            // whose annotation the region check reads; a let's temporaries are extended as the literal's would be. Only
            // that statement is replaced: the others keep their identity, which inference's records and the tail's
            // borrow check go by)
            let st: &Stmt = match st {
                Stmt::Let(_, None, Some(Expr::Struct(p, ..)), None, _) | Stmt::Expr(Expr::Struct(p, ..), true) if p.segs.last().is_some_and(|s| !s.args.is_empty()) => self.lifetime_lit(st).unwrap_or(st),
                st => st,
            };
            self.bk_temps_end(bk_mark);
            let last = k + 1 == n;
            let t0 = self.temps.len();
            self.stmt_i = i;
            match st {
                // (`let None = x;`: a unit variant or a constant, a pattern to test (E0005), not a new name)
                Stmt::Let(pat, ann_ty, init, els, let_at) if !is_simple_let(pat) || els.is_some() || self.names_variant_or_const(pat) => {
                    // 分解する let (`let S { x, y } = s;`): 反駁不能パターンとして束縛だけ行う
                    let Some(init) = init else {
                        // (a pattern that binds no name declares nothing: `let S { field: _, .. };`, libc 0.2's
                        //  `offset_of!`, whose check that the field exists rustc makes and zustc does not yet)
                        if matches!(pat, Pat::Struct(..)) && !pat_binds_any(pat) {
                            continue;
                        }
                        return err(SP, "`let` without initializer is not supported");
                    };
                    // (a Sized error of the annotation stands: `let (a, b): (Vec<str>, u8) = loop {};`)
                    let ann = match ann_ty.as_ref().map(|t| self.parse_ty(t)) {
                        Some(Ok(a)) => Some(a),
                        Some(Err(e)) if e.contains(crate::traits::SOLVED_NO) => return Err(e),
                        _ => None,
                    };
                    // (`let Some(n): &mut Option<i32> = &mut &Some(5) else { .. }`: E0596, as for a plain `let`)
                    if let (Some(a), true) = (ann, self.bk_on()) {
                        if self.peek(init).is_some_and(|t| mut_through_shared(t, a)) {
                            return Err(self.at(expr_start(init), MUT_THROUGH_SHARED.to_string()));
                        }
                    }
                    // (an annotation that writes `'static`: checked from here on, as a `let x: &'static T`, from
                    // its initializer, whose calls are lowered with the check on)
                    if let Some(a) = ann_ty {
                        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && crate::regions::mentions_static(a) {
                            self.rg_late_on();
                        }
                    }
                    // (destructuring reads the parts the pattern binds, not the whole value)
                    // (a `let`'s temporary bound by `ref` lives as long as the block: not one of the statement)
                    self.bk.let_scrut = true;
                    // (`let &Foo(ref x) = &f();`: the temporary lives as long as the block)
                    let ext1 = self.bk.extended.len();
                    if self.bk_on() {
                        borrowck::extended_refs(self.prog, init, &mut self.bk.extended);
                    }
                    let sc = self.bk_scrutinee(init, ann, &[pat]);
                    self.bk.extended.truncate(ext1);
                    self.bk.let_scrut = false;
                    let Some(place) = sc? else {
                        diverged = true;
                        continue;
                    };
                    // (`let (y, _): (&'a u32, u32) = ..;`: the pattern binds from a value of the annotation's type)
                    if let Some(a) = ann_ty {
                        if let Some((t, tr)) = self.rg_let_ann(a, init, Some(place.ty())) {
                            // (a `ref mut` binding of a place: the place is of the annotation's type, invariantly
                            // (`let Foo(ref mut y): Foo<fn(&'static str)> = x;` with x: `Foo<fn(&'a str)>` needs
                            // 'a: 'static))
                            if pat_binds_ref_mut(pat) {
                                self.rg_place_is_ann(init, t, &tr);
                            }
                            if t == place.ty() {
                                self.rg.scrut = Some((t, tr));
                            }
                        }
                    }
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
                    let pm = self.bk.pat_moves;
                    let pre = if is_place_expr(init) { self.moved_flag(init) } else { None };
                    self.bk_bind_begin();
                    // (the pattern's position, after the `let`, where an error of a `x @ sub` in it is reported)
                    let ap0 = std::mem::replace(&mut self.at_pat_at, Some(Sp { line: let_at.line, col: let_at.col + 4 }).filter(|s| s.line != 0));
                    let r = self.pat_bind(pat, place, BindMode::Move);
                    self.at_pat_at = ap0;
                    self.bk_bind_end();
                    r?;
                    if self.owned_binds > ob || self.bk.pat_moves > pm {
                        self.deref_move_check(init)?;
                    }
                    if self.owned_binds > ob {
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
                    if extends_temps(init) || self.bk_ty(place.ty()).1 || format!("{pat:?}").contains("Ident(true") {
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
                    // (`let _ = s;` in a closure that does not capture s for it: it reads nothing)
                    if ann_ty.is_none() && crate::ast_closure::wild_root(init).is_some_and(|n| self.fake_reads.iter().any(|x| x == n) && self.lookup(n).is_none()) {
                        continue;
                    }
                    let let_sp0 = std::mem::replace(&mut self.rg.let_sp, *let_at);
                    // (or a projection whose associated type's value names lifetimes: `let _: <() as Foo>::Out = &a;`
                    // with `type Out = &'static u32`)
                    let proj = |a: &crate::parse::Ty| matches!(a, crate::parse::Ty::Path(Some(_), _)) && !self.prog.assoc_regions.is_empty();
                    if !self.rg.on && self.rg.eligible && ann_ty.as_ref().is_some_and(|a| crate::regions::mentions_static(a) || proj(a)) {
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
                        // (and a Sized error, whatever the initializer: `let _: Vec<str> = loop {};`)
                        Some((_, Err(e))) if e.contains(crate::traits::SOLVED_NO) => return Err(e),
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
                        self.bk_fake_read(init);
                    } else {
                        // (`let _: [(); 0] = [];`: an array literal takes its element type from it, holes and all)
                        let exp = match ann {
                            Some(a) if matches!(strip_paren(init), Expr::Array(_) | Expr::Repeat(..)) => Some(a).filter(|&a| !has_hole(self.prog, a)),
                            _ => full,
                        };
                        let (mut v, mut t) = self.expr(init, exp)?;
                        if t == Ty::Never {
                            diverged = true;
                            // (the annotation's type is well formed all the same: `let _: &'a S<'b> = loop {};`)
                            if let Some(at) = ann_ty.as_ref() {
                                self.rg_let_uninit(None, at, *let_at);
                            }
                            continue;
                        }
                        if let Some(a) = full.filter(|&a| a != t) {
                            if self.bk_on() && mut_through_shared(t, a) {
                                return Err(self.at(expr_start(init), MUT_THROUGH_SHARED.to_string()));
                            }
                            let (v2, t2) = self.deref_coerce(v, t, a);
                            if t2 == a {
                                v = v2;
                                t = t2;
                            }
                            // (`*mut T` where `*const T` is written: the same pointer. Types apart in their lifetimes only
                            // (`dyn Foo<Bar = &'a u32>`, which keeps the text of its bounds) are the region check's)
                            let ptr = matches!((t, a), (Ty::Ptr(true, x), Ty::Ptr(false, y)) if x == y);
                            if !ptr && !self.apart_in_lifetimes(t, a) {
                                self.same(SP, t, a).map_err(|m| self.at(value_pos(init), m))?;
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
                                // (a reference's mutability as written: inference does not unify `&mut T` with the `&T` of
                                // its initializer (`let y: &mut [_] = &x[..];` is a mismatch))
                                Some(inf) if has_hole(self.prog, a) && !has_hole(self.prog, inf) && !self.prog.has_param(inf) && std::mem::discriminant(&inf) == std::mem::discriminant(&a)
                                    && !matches!((inf, a), (Ty::Struct(x), Ty::Struct(y)) if self.prog.base_of(x) != self.prog.base_of(y)) => Some(written_muts(inf, a)),
                                _ => Some(a),
                            }
                        }
                        // (型推論は強制変換の行き先 `[T]` などを入れてしまうことがあるので、サイズ不定の型は使わない)
                        None => self
                            .lets
                            .get(&crate::ast_infer::stmt_key(st))
                            .copied()
                            .filter(|t| !t.is_unsized()),
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
                        // (the declaration, where unused_mut is reported)
                        let let_sp0 = std::mem::replace(&mut self.rg.let_sp, *let_at);
                        self.bind(name.clone(), ty, v);
                        self.rg.let_sp = let_sp0;
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
                    // (`let b = a;`: what the type inference found is not an annotation; not of a macro's `let`)
                    self.bk.move_mut_hint = ann_ty.is_none() && !name.starts_with("__exr_") && matches!(strip_paren(init), Expr::Path(None, _));
                    let ext0 = self.bk.direct.len();
                    let ext1 = self.bk.extended.len();
                    if self.bk_on() {
                        borrowck::direct_temps_let(self.prog, init, &mut self.bk.direct);
                        borrowck::direct_ref_recv(self.prog, init, &mut self.bk.direct);
                        // (`let x = &f();`: the temporary lives as long as x's block; also within the initializer's
                        // extending expressions, `let x = Some(&f());`)
                        borrowck::extended_refs(self.prog, init, &mut self.bk.extended);
                    }
                    // (a function the gate left out, with `let x: &'static T = ..`: from here on, a borrow of what it
                    // owns must not have to be 'static. What came before has no trees and adds no constraint)
                    let static_cast = || matches!(strip_paren(init), Expr::Cast(_, t) if crate::regions::mentions_static(t));
                    let ctor_lts = || self.prog.path_lts && self.inline_depth == 0 && crate::regions::writes_path_lts(init);
                    let proj = |a: &crate::parse::Ty| matches!(a, crate::parse::Ty::Path(Some(_), _)) && !self.prog.assoc_regions.is_empty();
                    if !self.rg.on && self.rg.eligible && (static_cast() || ann_ty.as_ref().is_some_and(|a| crate::regions::mentions_static(a) || proj(a)) || ctor_lts()) {
                        self.rg_late_on();
                    }
                    let let_sp0 = std::mem::replace(&mut self.rg.let_sp, *let_at);
                    let ml0 = std::mem::replace(&mut self.bk.macro_let, name.starts_with("__exr_"));
                    let plain = if ann_ty.is_none() && self.bk_on() { borrowck::value_tail(init) as *const Expr as usize } else { 0 };
                    let plain0 = std::mem::replace(&mut self.bk.plain_init, plain);
                    let r = self.expr(init, ann.or(hint));
                    self.bk.plain_init = plain0;
                    // (`let b: &mut T = a;` / `let b: &T = s.f;` with a place of a `&mut T`: a coercion site, b reborrows
                    // what it points to (rustc's `&mut *a` / `&*s.f`), not a copy or a move of it: a use of `*a` while b
                    // lives conflicts)
                    if let (true, Some(w @ Ty::Ref(..))) = (r.is_ok() && ann_ty.is_some(), ann) {
                        self.bk_reborrow_let(init, w);
                    }
                    // (`let v: &T = x.borrow().0;` with the field a `&mut T`: a coercion site, v reborrows `*x.borrow().0`
                    // through the temporary guard, which goes away at the end of the statement (E0716 when v is used
                    // after): rustc's `&*`, which the `let` does not extend, as it is not written)
                    if let (true, Some(Ty::Ref(m, _)), Some(Ty::Ref(true, _))) = (r.is_ok() && ann_ty.is_some() && self.bk_on(), ann, self.owned_field_ty(init)) {
                        if let Some(t) = self.temp_root(init).and_then(|x| self.peek(x)) {
                            if self.bk_place(init).is_none() {
                                self.bk.direct.push(init as *const Expr as usize);
                                self.bk_temp_borrow(init, m, t);
                            }
                        }
                    }
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
                                self.bind(name.clone(), a, v);
                                // (and the annotation's type is well formed, as with any initializer:
                                // `let _: &'a S<'b> = loop {};` needs 'b: 'a)
                                if let Some(at) = ann_ty.as_ref() {
                                    self.rg_let_uninit(Some(&name), at, *let_at);
                                }
                            }
                            None => {
                                // (the annotation's type is well formed here too, a type that needs Drop
                                // (`let x: Box<&'a S<'b>> = loop {};`): its tree is not the binding's, which has none)
                                if let Some(at) = ann_ty.as_ref() {
                                    self.rg_let_uninit(None, at, *let_at);
                                }
                                self.bind(name, Ty::Never, None)
                            }
                        }
                        self.rg.let_sp = let_sp0;
                        continue;
                    }
                    // (an annotation with holes, of another struct than the value's: `let x: Foo<_> = Bar::<usize>(..)`. No
                    // coercion makes one struct another)
                    if let (Some(Ty::Struct(a)), Ty::Struct(b), true) = (ann, t, ann_ty.is_some()) {
                        let p = self.prog;
                        if has_hole(p, Ty::Struct(a)) && p.base_of(a) != p.base_of(b) && !p.is_tuple(a) && !p.is_tuple(b) && !p.has_param(t) && p.structs[b as usize].placeholder.is_none() && !p.closure_of_struct.contains_key(&p.base_of(b)) {
                            let m = format!("mismatched types: expected `{}`, found `{}`", p.tn(Ty::Struct(a)), p.tn(t));
                            return Err(self.at(expr_start(init), m));
                        }
                    }
                    let mut ann = ann.filter(|&a| !has_hole(self.prog, a));
                    // (型推論の結果 (注釈なし) と違う: `&&str` → `&str` は deref の強制変換、それでも違えば実際の型)
                    if let Some(a) = ann.filter(|&a| a != t) {
                        if self.bk_on() && mut_through_shared(t, a) {
                            return Err(self.at(expr_start(init), MUT_THROUGH_SHARED.to_string()));
                        }
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
                    // (a block's value is its tail's: rustc points there, `let _: ! = { 'a: while break 'a {} };`)
                    self.same(SP, t, ty).map_err(|m| self.at(value_pos(init), m))?;
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
                    if extends_temps(init) || self.bk_ty(ty).1 {
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
                        result = self.detach_tail(r, t0, None);
                        self.drop_tail_temps(t0, result);
                    } else {
                        self.drop_temps(t0);
                    }
                }
                Stmt::Expr(e, semi) => {
                    let exp = if last && !*semi { expected } else { None };
                    // (a path that writes lifetimes, after a borrow in the same statement: from its start)
                    if !self.rg.on && self.rg.eligible && self.prog.path_lts && self.inline_depth == 0 && crate::regions::writes_path_lts(e) {
                        self.rg_late_on();
                    }
                    // (the value of the function body: what it borrows must outlive the function)
                    let tail = last && !*semi && self.bk.tail == e as *const Expr as usize && self.bk_on();
                    if tail {
                        self.bk_frame();
                    }
                    self.bk.move_mut_next = last && !*semi;
                    // (a statement's temporaries borrowed by its outermost call, or by an assignment (`x = &f();`: no
                    // `let` extends them): they go away at its end)
                    let ext0 = self.bk.direct.len();
                    // (edition 2024: a block's tail is a statement of its own for its temporaries, which go away at
                    // the end of the block (`{ String::new().as_str() }.len()` is E0716). Not the function's body)
                    if !*semi && last && self.prog.edition >= 2024 && self.bk_on() && self.bk.tail != e as *const Expr as usize {
                        if let e @ (Expr::MethodCall(..) | Expr::Tuple(..) | Expr::Array(..) | Expr::Struct(..)) = strip_paren(e) {
                            borrowck::direct_temps(self.prog, e, &mut self.bk.direct);
                        }
                    }
                    if *semi && self.bk_on() {
                        match strip_paren(e) {
                            Expr::Assign(_, v) => borrowck::direct_ref(self.prog, v, &mut self.bk.direct),
                            // (a method of a local that may hold references: `v.push(&f());`)
                            e @ Expr::MethodCall(r, ..) => {
                                borrowck::direct_temps(self.prog, e, &mut self.bk.direct);
                                let holds = || matches!(strip_paren(r), Expr::Path(None, p) if p.segs.len() == 1 && self.lookup(&p.segs[0].name).is_some_and(|(_, t)| borrowck::has_refs(self.prog, t, 0)));
                                if self.bk.direct.len() > ext0 && !holds() {
                                    self.bk.direct.truncate(ext0);
                                }
                            }
                            _ => {}
                        }
                    }
                    let watch = if last && !*semi { self.bk_watch_tail(e) } else { (vec![], self.bk.watch_at.len()) };
                    // (edition 2024: what the block's value holds, for its temporaries' end below)
                    let tail24 = !*semi && last && self.prog.edition >= 2024 && self.bk_on() && !tail;
                    if tail24 {
                        self.bk_frame();
                    }
                    // (a block's value that a statement discards (`{ let t = D(1); d.next(&t) };`) is a temporary of
                    // the statement: made at the block's tail, while its locals live, and dropped at the statement's
                    // end, after them (E0597 when its `Drop` may see a borrow of one of them). In every edition: 2024
                    // changes when the tail's temporaries go, not the block's value)
                    let saved_discard = self.bk.discard;
                    let saved_stmt_block = self.bk.stmt_block;
                    if let (true, Expr::Block(None, _, b)) = (*semi, strip_paren(e)) {
                        self.bk.stmt_block = b as *const crate::parse::Block as usize;
                    }
                    let mark_discard = self.bk.discard_temps.len();
                    let stmt_block = *semi && self.bk_on() && self.bk.level >= 2 && self.user_code;
                    if let (true, Expr::Block(None, _, b)) = (stmt_block, strip_paren(e)) {
                        self.bk.discard = b as *const crate::parse::Block as usize;
                    }
                    // (the tail of such a block: an inner block's value is the same value; another expression makes it)
                    let made_here = last && !*semi && self.bk.discard != 0 && self.bk.discard == blk as *const crate::parse::Block as usize;
                    if made_here {
                        match strip_paren(e) {
                            Expr::Block(None, _, b) => self.bk.discard = b as *const crate::parse::Block as usize,
                            _ => self.bk_frame(),
                        }
                    }
                    let made_value = made_here && !matches!(strip_paren(e), Expr::Block(None, ..));
                    // (a statement's value is discarded: what it holds is no part of the enclosing value
                    // (`{ *v += 1; f(&format!("{v}")); (x, y) }`), but what a `break` in it gives the loop)
                    fn breaks_value(e: &Expr, depth: u32) -> bool {
                        if depth > 64 || matches!(e, Expr::Closure(..)) {
                            return false;
                        }
                        if matches!(e, Expr::Break(_, Some(_), _)) {
                            return true;
                        }
                        let mut found = false;
                        crate::ast_closure::each_child_pub(e, &mut |c| found = found || breaks_value(c, depth + 1));
                        found
                    }
                    let stmt_mark = if *semi && self.bk_on() && !breaks_value(e, 0) { self.bk.frames.last().map(|f| (self.bk.frames.len(), f.len())) } else { None };
                    let r = self.expr(e, exp);
                    if let Some((n, k)) = stmt_mark {
                        if self.bk.frames.len() == n {
                            if let Some(f) = self.bk.frames.last_mut() {
                                f.truncate(k);
                            }
                        }
                    }
                    if tail24 {
                        tail_srcs = self.bk_frame_end();
                    }
                    if made_value {
                        let srcs = self.bk_frame_end();
                        if let Ok((_, t)) = &r {
                            if !srcs.is_empty() && self.dropck(*t, 0) {
                                self.bk_hold_value(srcs, *t);
                            }
                        }
                    }
                    if made_here || stmt_block {
                        self.bk.discard = saved_discard;
                    }
                    self.bk.stmt_block = saved_stmt_block;
                    if stmt_block && self.bk.discard_temps.len() > mark_discard {
                        let ts: Vec<u32> = self.bk.discard_temps.drain(mark_discard..).collect();
                        self.bk_drop_held(&ts, borrowck::pos(e));
                    }
                    self.bk.direct.truncate(ext0);
                    if last && !*semi && self.bk.tail == e as *const Expr as usize && r.is_ok() {
                        self.rg_ret(e);
                    }
                    // (a block's value: its tree, while the block's locals are in scope)
                    if last && !*semi && r.is_ok() {
                        self.rg_keep(e);
                        self.bk_tail_drop_temps(e);
                        if !tail {
                            if let Ok((_, t)) = &r {
                                self.bk_block_value(blk, e, *t);
                            }
                        }
                    }
                    self.bk_unwatch(&watch);
                    if tail {
                        let srcs = self.bk_frame_end();
                        if self.bk_ty(self.ret).1 {
                            { let _ = &srcs; self.bk_ret(e); }
                        }
                    }
                    let r = r?;
                    if r.1 == Ty::Never {
                        diverged = true;
                    }
                    if last && !*semi {
                        result = self.detach_tail(r, t0, exp);
                        self.drop_tail_temps(t0, result);
                    } else {
                        // (a block-like statement, `if .. {}` / `const { 2 }` without `;`, has the type `()`: rustc's
                        // E0308)
                        if !*semi && !matches!(r.1, Ty::Unit | Ty::Never) {
                            let m = format!("mismatched types: expected `()`, found `{}`", self.prog.tn(r.1).replace('#', ""));
                            // (where the statement starts, `match`, when no branch's block is to blame)
                            let at = match unit_stmt_pos(e) {
                                p if p.line != 0 => p,
                                _ if matches!(e, Expr::Match(..)) => blk.spos.get(i).unwrap_or(SP),
                                _ => SP,
                            };
                            return Err(self.at(at, m));
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
        // (a last statement that is no tail (`match heap.peek_mut() { .. };`, a `let`): its temporaries end with it,
        // before the block's locals, as any statement's)
        if !matches!(blk.stmts.last(), Some(Stmt::Expr(_, false))) {
            self.bk_temps_end(bk_mark);
        }
        // (edition 2024: the tail expression's temporaries go away at the end of the block, before its locals)
        if self.prog.edition >= 2024 {
            // (a reference the block's value is, or holds, into one of them, is used after:
            // `{ String::new().as_str() }.len()`, `{ (String::new().as_str(),) }.0.len()` are E0716)
            let held = match (blk.stmts.last(), result.1) {
                (Some(Stmt::Expr(t, false)), ty) if self.bk_on() && self.bk.stmt_temps.len() > bk_mark && (matches!(ty, Ty::Ref(..)) || borrowck::has_refs(self.prog, ty, 0)) => {
                    let dying = self.bk.stmt_temps[bk_mark..].to_vec();
                    let mut srcs = std::mem::take(&mut tail_srcs);
                    self.bk_direct_pub(t, &mut srcs);
                    srcs.retain(|s| matches!(s, borrowck::Src::Loan(ln) if dying.contains(&self.bk.loans[*ln as usize].place.local)));
                    srcs.sort_unstable_by_key(|s| match s { borrowck::Src::Loan(l) => *l, _ => 0 });
                    srcs.dedup();
                    (!srcs.is_empty()).then(|| (srcs, expr_start(t)))
                }
                _ => None,
            };
            self.bk_temps_end(bk_mark);
            if let Some((srcs, sp)) = held {
                self.bk_hold_after(srcs, sp);
            }
        }
        self.pop_scope();
        self.dead_stmt = dead0;
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
            // (an array's or a slice's as_mut: `&mut [T]`, its one AsMut)
            if m == "as_mut" && a.is_empty() {
                if let Some(Ty::Array(el, _) | Ty::Slice(el)) = self.peek(r).map(|t| self.strip_refs(t)) {
                    return Some(Ty::Ref(true, intern(Ty::Slice(el))));
                }
                // (a Vec's, where it is its AsMut<[T]> or its AsMut<Vec<T>>)
                match self.as_mut_of_vec(r, None) {
                    Some((true, el)) => return Some(Ty::Ref(true, intern(Ty::Slice(intern(el))))),
                    Some((false, v)) => return Some(Ty::Ref(true, intern(v))),
                    None => {}
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
                        // (a type's own `Index`: what its `index` returns a reference to, so a field read through its
                        // `Deref` is found (`v[0].f` with `Output = MyPtr<Foo>`))
                        // (of the program's own types: the prelude's (`Vec`) are read where they are lowered)
                        Ty::Struct(sid) if self.prog.struct_item.get(self.prog.base_of(sid) as usize).is_some_and(|&i| i < self.prog.prelude_from) => {
                            let id = self.prog.method(sid, "index")?;
                            // (not where it has several (`Index<usize>` and `Index<&str>`): which one the index takes is
                            // not known here)
                            let gname = &self.prog.names[id as usize];
                            if self.prog.overloads.get(gname.split('#').next().unwrap_or(gname)).is_some_and(|c| c.len() > 1) {
                                return None;
                            }
                            let sig = &self.prog.sigs[id as usize];
                            let mut sol = vec![None; sig.generics.len()];
                            if sig.trait_of.is_some() {
                                if let Some(s0) = sol.first_mut() {
                                    *s0 = Some(Ty::Struct(sid));
                                }
                            } else if let Some(st) = sig.self_ty {
                                self.prog.match_ty(st, Ty::Struct(sid), &mut sol);
                            }
                            return match self.prog.partial_subst(sig.ret, &sol) {
                                Ty::Ref(_, i) if !self.prog.has_param(*i) => Some(*i),
                                _ => None,
                            };
                        }
                        _ => return None,
                    }
                }
            }
            Expr::Array(a) => {
                let e = a.iter().find_map(|e| self.peek(e))?;
                Some(Ty::Array(intern(e), a.len() as u32))
            }
            Expr::MethodCall(recv, name, tf, args, _) => {
                let mut t = self.peek(recv)?;
                // (a raw pointer's `cast::<U>()` / `add(n)`: `x.cast::<u32>().read_unaligned()`)
                if let Ty::Ptr(m, _) = t {
                    match (name.as_str(), tf.first()) {
                        ("cast", Some(crate::parse::GArg::Ty(u))) => return self.parse_ty(u).ok().map(|u| Ty::Ptr(m, intern(u))),
                        ("add" | "sub" | "offset" | "wrapping_add" | "wrapping_sub" | "wrapping_offset", _) => return Some(t),
                        _ => {}
                    }
                }
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
                        | "max" | "rem_euclid" | "div_euclid" | "next_multiple_of" | "div_ceil" | "wrapping_add_signed" | "next_power_of_two" | "rotate_left" | "rotate_right"
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
                let id = self.prog.self_method(sid, name).or_else(|| self.prog.self_default_method(sid, name))?;
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
    /// The fn m of an impl for a reference to a type parameter, `impl<T: Tr + ?Sized> Tr for &T { fn m() }` (named
    /// `T::__ref_m`), whose self type t is (`&u8`, `&&str`); of trait tr when one is named
    #[cold]
    #[inline(never)]
    pub(crate) fn ref_blanket_fn(&self, t: Ty, m: &str, tr: Option<&str>) -> Option<u32> {
        if !matches!(t, Ty::Ref(..)) {
            return None;
        }
        // (also one for the reference to a type the prelude names otherwise, `impl Tr for &Cow<'_, str>` (`Cow::__ref_m`,
        // whose self type is the prelude's `&__CowStr`): by its self type, an impl for the type itself first)
        let suffix = format!("::__ref_{m}");
        let fits = |i: u32, blanket: bool| {
            let n = self.prog.names[i as usize].as_str();
            let sig = &self.prog.sigs[i as usize];
            n.split('#').next().is_some_and(|b| b.ends_with(&suffix))
                && matches!(sig.self_ty, Some(Ty::Ref(_, Ty::Param(_)))) == blanket
                && tr.is_none_or(|tr| sig.impl_trait.as_deref().is_some_and(|it| it.split('<').next().unwrap_or(it).trim() == tr))
                && sig.self_ty.is_some_and(|st| self.prog.match_ty_exact(st, t, &mut vec![None; sig.generics.len()]))
        };
        let n = self.prog.names.len() as u32;
        (0..n).find(|&i| fits(i, false)).or_else(|| (0..n).find(|&i| fits(i, true)))
    }

    pub fn callee(&self, f: &Expr) -> Option<(u32, &FnSig)> {
        let Expr::Path(q, p) = f else { return None };
        let by_ty = |t: Ty, m: &str| -> Option<u32> {
            match t {
                // (a reference's: an impl for it (`impl Tr for &u8`'s `u8::__ref_m`), then one for a reference to any
                // type, `impl<T> Tr for &T`)
                Ty::Ref(..) => {
                    let (mut pre, mut b) = (String::new(), t);
                    while let Ty::Ref(_, i) = b {
                        pre.push_str("__ref_");
                        b = *i;
                    }
                    let base = match b {
                        Ty::Slice(_) => Some("__slice".to_string()),
                        Ty::Array(..) => Some("__array".to_string()),
                        Ty::Box(_) => Some("Box".to_string()),
                        Ty::Struct(sid) => Some(self.prog.structs[self.prog.base_of(sid) as usize].name.clone()),
                        b if b.is_int() || b.is_float() || matches!(b, Ty::Bool | Ty::Char | Ty::Str) => Some(self.prog.tn(b)),
                        _ => None,
                    };
                    base.and_then(|base| self.prog.index.get(&format!("{base}::{pre}{m}")).copied())
                        .or_else(|| self.prog.prim_method(t, m))
                        .or_else(|| self.ref_blanket_fn(t, m, None))
                }
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
            // (`<S as Tr>::m`: Tr's impl, also beside an inherent `S::m` of the same name; or Tr's default, not another
            // trait's of the same name)
            if p.segs.len() >= 2 {
                let tr = &p.segs[p.segs.len() - 2].name;
                // (among impls of the trait for instances of one type, `<Box<[f32]> as FromWasmAbi>::from_abi` beside
                // Box<[u8]>'s: the one for the type itself)
                let key = format!("{}::{m}", self.ty_key(t));
                let multi = self.prog.overloads.contains_key(&key);
                // (and among impls of the trait with other arguments, `<Z0 as ToInt<i16>>::to_int` beside `ToInt<i8>`'s:
                // the one whose trait arguments the path writes)
                let targs: Option<Vec<Ty>> = (multi && !self.prog.fn_targs.is_empty() && !p.segs[p.segs.len() - 2].args.is_empty())
                    .then(|| p.segs[p.segs.len() - 2].args.iter().map(|a| match a {
                        crate::parse::GArg::Ty(x) => self.parse_ty(x).ok(),
                        _ => None,
                    }).collect())
                    .flatten();
                let own = |c: u32| {
                    let sig = &self.prog.sigs[c as usize];
                    let mut sol = vec![None; sig.generics.len()];
                    sig.self_ty.is_some_and(|st| self.prog.match_ty_exact(st, t, &mut sol))
                        && match (&targs, self.prog.fn_targs.get(&c)) {
                            (Some(a), Some(ta)) => a.len() == ta.len() && ta.iter().zip(a).all(|(&x, &y)| self.prog.match_ty_exact(x, y, &mut sol)),
                            _ => true,
                        }
                };
                if let Some(i) = multi.then(|| self.trait_candidate_where(&key, tr, own)).flatten().or_else(|| self.trait_candidate(&key, tr)).or_else(|| self.trait_default(tr, &m, t)) {
                    return Some((i, &self.prog.sigs[i as usize]));
                }
                // (`<&J as Tr>::m`: the impl for the reference, whose functions are `J::__ref_m`)
                if let Ty::Ref(..) = t {
                    let (mut pre, mut b) = (String::new(), t);
                    while let Ty::Ref(_, i) = b {
                        pre.push_str("__ref_");
                        b = *i;
                    }
                    // (an impl for `&[T]` / `&[T; N]` has its functions under `__slice` / `__array`)
                    let base = match b {
                        Ty::Slice(_) => "__slice".to_string(),
                        Ty::Array(..) => "__array".to_string(),
                        Ty::Box(_) => "Box".to_string(),
                        b => self.ty_key(b),
                    };
                    let key = format!("{base}::{pre}{m}");
                    if let Some(i) = self.trait_candidate_where(&key, tr, own).or_else(|| self.trait_candidate(&key, tr)).or_else(|| self.ref_blanket_fn(t, &m, Some(tr))) {
                        return Some((i, &self.prog.sigs[i as usize]));
                    }
                }
            }
            // (`<&Box<str> as MaybeIntoWasmAbi>::none()`: the trait's blanket impl, `impl<T: OptionIntoWasmAbi>
            // MaybeIntoWasmAbi for T`, when its bounds hold of the type)
            if p.segs.len() >= 2 {
                let tr = &p.segs[p.segs.len() - 2].name;
                // (the blanket impl's fn of that trait: Program::blanket keeps one of each name, of whichever trait
                // came first)
                let suffix = format!("::{m}");
                let of_tr = |i: usize| {
                    let bs = &self.prog.sigs[i];
                    matches!(bs.self_ty, Some(Ty::Param(_)))
                        && bs.impl_trait.as_deref().is_some_and(|it| it.split('<').next().unwrap_or(it).trim() == tr.as_str())
                        && self.prog.names[i].split('#').next().is_some_and(|n| n.ends_with(&suffix))
                };
                let holds = || self.prog.blanket_bounds.get(tr.as_str()).is_none_or(|b| super::acg::p2_call::blanket_impls_hold(self.prog, t, b, &mut vec![], &mut vec![]));
                if !self.prog.has_param(t) && self.prog.blanket_traits.contains(tr.as_str()) {
                    if let Some(bid) = (0..self.prog.sigs.len()).find(|&i| of_tr(i)).filter(|_| holds()) {
                        return Some((bid as u32, &self.prog.sigs[bid]));
                    }
                }
            }
            // (`<T as Tr>::A::m`: a fn of the associated type A, its instance's)
            if p.segs.len() >= 3 {
                if let Some(i) = self.assoc_ty_callee(Some(q), p, &by_ty) {
                    return Some((i, &self.prog.sigs[i as usize]));
                }
            }
            let mut i = by_ty(t, &m)?;
            // (`<Box<[u8]>>::from_abi(js)`: among impls for instances of one type, the one for the type itself)
            let group = self.prog.names[i as usize].split('#').next().unwrap_or("");
            if let Some(cs) = self.prog.overloads.get(group) {
                let own = |c: u32| self.prog.sigs[c as usize].self_ty.is_some_and(|st| self.prog.match_ty_exact(st, t, &mut vec![None; self.prog.sigs[c as usize].generics.len()]));
                if !own(i) {
                    if let Some(&c) = cs.iter().find(|&&c| own(c)) {
                        i = c;
                    }
                }
            }
            return Some((i, &self.prog.sigs[i as usize]));
        }
        // (`T::A::m(..)`: a fn of the associated type A of the type parameter T)
        if p.segs.len() == 3 && self.env.params.iter().any(|(n, _)| *n == p.segs[0].name) {
            if let Some(i) = self.assoc_ty_callee(None, p, &by_ty) {
                return Some((i, &self.prog.sigs[i as usize]));
            }
        }
        // `T::f(..)` (T は型引数。インスタンスでは具体的な型)
        if p.segs.len() == 2 {
            let g = &p.segs[0].name;
            if self.env.params.iter().any(|(n, _)| n == g) {
                let ty = crate::parse::Ty::Path(None, path_of(g));
                if let Ok(t) = self.parse_ty(&ty) {
                    // (a fn of the name in two traits: the one of T's bounds, `T::none()` with `T: MaybeIntoWasmAbi` beside
                    // OptionIntoWasmAbi's `none`)
                    let m = &p.segs[1].name;
                    if !self.prog.has_param(t) && self.prog.multi_trait_methods.contains(m.as_str()) {
                        // (the fn's own bounds: `root`, also for a body inlined into another)
                        let bounds = self.prog.sigs.get(self.root as usize).and_then(|s| s.generics.iter().find(|x| x.0 == *g)).map(|x| x.1.clone()).unwrap_or_default();
                        for b in &bounds {
                            let tr = b.split('<').next().unwrap_or(b).trim();
                            if !self.prog.traits.get(tr).is_some_and(|ds| ds.iter().any(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(m.as_str()))) {
                                continue;
                            }
                            if let Ok(Expr::Path(q2, p2)) = crate::parse::parse_expr(&format!("<{} as {tr}>::{m}", self.prog.tn(t))) {
                                if let Some(r) = self.callee(&Expr::Path(q2, p2)) {
                                    return Some(r);
                                }
                            }
                        }
                    }
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
            // (of inherent functions in impls for disjoint instances, the one of the self type the path names)
            Some(i) => self.prog.fn_of_path_self(i, p, &self.env),
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

    /// The fn m of `<T as Tr>::A::m` / `T::A::m`: the path without its last segment is a type (an associated one), whose
    /// fn m it is, by by_ty
    #[cold]
    #[inline(never)]
    fn assoc_ty_callee(&self, q: Option<&Box<crate::parse::Ty>>, p: &Path, by_ty: &dyn Fn(Ty, &str) -> Option<u32>) -> Option<u32> {
        let (m, pre) = p.segs.split_last()?;
        let ty = crate::parse::Ty::Path(q.cloned(), Path { global: p.global, segs: pre.to_vec(), pos: p.pos });
        let t = self.parse_ty(&ty).ok().filter(|t| !matches!(t, Ty::Param(_)))?;
        by_ty(t, &m.name)
    }

    /// blk's statements (index, statement), with each destructuring `let pat = S::<&'a u32> {..};` (type arguments
    /// that name lifetimes) as `let mut #slI: S<&'a u32> = S::<&'a u32> {..};` and `let pat = #slI;`
    #[cold]
    #[inline(never)]
    fn lifetime_lit_split<'b>(&self, blk: &'b crate::parse::Block) -> Vec<(usize, &'b Stmt)> {
        let mut out = Vec::with_capacity(blk.stmts.len() + 1);
        for (i, st) in blk.stmts.iter().enumerate() {
            if let Stmt::Let(pat, None, Some(init @ Expr::Struct(path, ..)), None, lp) = st {
                if !is_simple_let(pat) && path.segs.iter().any(|s| s.args.iter().any(|a| format!("{a:?}").contains('\''))) {
                    let mut bare = path.clone();
                    for s in &mut bare.segs {
                        s.args.clear();
                    }
                    if self.prog.struct_by_p(&PPath::of(&bare), self.self_sid).is_some() {
                        // (a name source cannot write, so no binding of the user's is shadowed; `mut`, as the literal it
                        // holds was a temporary that `ref mut` in the pattern may borrow)
                        let tmp = format!("#sl{i}");
                        let first: &'static Stmt = Box::leak(Box::new(Stmt::Let(Pat::Ident(false, true, tmp.clone(), None), Some(crate::parse::Ty::Path(None, path.clone())), Some(init.clone()), None, *lp)));
                        let second: &'static Stmt = Box::leak(Box::new(Stmt::Let(pat.clone(), None, Some(path_expr(&tmp)), None, *lp)));
                        out.push((i, first));
                        out.push((i, second));
                        continue;
                    }
                }
            }
            out.push((i, st));
        }
        out
    }

    /// The statement `let x = S::<&'a u32> {..};` / `S::<&'a u32> {..};` (type arguments that name lifetimes) as
    /// `let x: S<&'a u32> = S::<&'a u32> {..};` / `let _: S<&'a u32> = S::<&'a u32> {..};`; None for any other
    #[cold]
    #[inline(never)]
    fn lifetime_lit(&self, st: &Stmt) -> Option<&'static Stmt> {
        let (pat, init, lp) = match st {
            Stmt::Let(pat, None, Some(init @ Expr::Struct(..)), None, lp) if is_simple_let(pat) => (pat.clone(), init, *lp),
            Stmt::Expr(init @ Expr::Struct(..), true) => (Pat::Wild, init, Default::default()),
            _ => return None,
        };
        let Expr::Struct(path, ..) = init else { return None };
        if !path.segs.iter().any(|s| s.args.iter().any(|a| format!("{a:?}").contains('\''))) {
            return None;
        }
        let mut bare = path.clone();
        for s in &mut bare.segs {
            s.args.clear();
        }
        self.prog.struct_by_p(&PPath::of(&bare), self.self_sid)?;
        // (the literal keeps its arguments too: struct_lit_inner checks them against the fields and an update's base,
        // also when the annotation, with a `_`, gives no type)
        Some(Box::leak(Box::new(Stmt::Let(pat, Some(crate::parse::Ty::Path(None, path.clone())), Some(init.clone()), None, lp))))
    }

    /// Trait tr's default fn m (one with a body), when t implements tr
    pub fn trait_default(&self, tr: &str, m: &str, t: Ty) -> Option<u32> {
        let d = self.prog.traits.get(tr)?.iter().copied().find(|&d| self.prog.sigs[d as usize].has_body && self.prog.names[d as usize].rsplit("::").next() == Some(m))?;
        self.prog.implements(t, tr).then_some(d)
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

/// p binds a name by `ref mut` somewhere
/// Whether a pattern binds a name (an identifier pattern anywhere in it)
pub(crate) fn pat_binds_any(p: &Pat) -> bool {
    match p {
        Pat::Ident(..) => true,
        Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Slice(ps) | Pat::Or(ps, _) => ps.iter().any(pat_binds_any),
        Pat::Struct(_, fs, _) => fs.iter().any(|(_, q)| pat_binds_any(q)),
        Pat::Ref(_, q) => pat_binds_any(q),
        _ => false,
    }
}

pub(crate) fn pat_binds_ref_mut(p: &Pat) -> bool {
    match p {
        Pat::Ident(r, m, _, sub) => (*r && *m) || sub.as_deref().is_some_and(pat_binds_ref_mut),
        Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Or(ps, _) | Pat::Slice(ps) => ps.iter().any(pat_binds_ref_mut),
        Pat::Struct(_, fs, _) => fs.iter().any(|f| pat_binds_ref_mut(&f.1)),
        Pat::Ref(_, x) => pat_binds_ref_mut(x),
        _ => false,
    }
}

/// inf with the mutabilities of the references a writes where inf has references too (`&[i32]` for `&mut [_]`:
/// `&mut [i32]`)
fn written_muts(inf: Ty, a: Ty) -> Ty {
    match (inf, a) {
        (Ty::Ref(_, i), Ty::Ref(m, j)) => Ty::Ref(m, intern(written_muts(*i, *j))),
        (Ty::Ptr(_, i), Ty::Ptr(m, j)) => Ty::Ptr(m, intern(written_muts(*i, *j))),
        _ => inf,
    }
}
