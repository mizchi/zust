//! 関数 1 個分の型推論を自前の構文木 (src/parse.rs) の上で行う: infer.rs (syn の構文木を辿る) の移植。
//! 単一化器は src/solver.rs を共有する。外部クレートに依存しないので zustc 自身でもコンパイルできる。
//! tests/ast_infer.rs が全ファイルの関数で、注釈のない let の型が syn 版と同じになることを確かめる。
//! 結果の表のキーはこの構文木のノードのアドレス (let 文は Stmt、値を作る式は Expr)。

use crate::ast_closure;
use crate::parse::{Block, Expr, GArg, Pat, Path, Stmt, Ty as PTy};
use crate::program::{FnSig, PPath, Program, TyEnv};
use crate::solver::{Shape, Solver};
use crate::types::{Ty, TySyn, VariantKind, intern, intern_fn, FnTy};

/// 注釈なし let と、値を作る式の型 (ノードのアドレス → 型)
pub type LetTypes = crate::fxhash::FxMap<usize, Ty>;

/// let 文の型のキー (文のアドレス + 1)。文の中の式 (初期値) が文と同じアドレスにあることがあるので、ずらして区別する
pub fn stmt_key(st: &Stmt) -> usize {
    st as *const Stmt as usize + 1
}


/// 推論結果: (let と式の型, 戻り値型, 引数の型)
pub type Full = (LetTypes, Option<Ty>, Vec<Option<Ty>>);

/// 整数 / &str / 生ポインタの組み込みメソッド名 (infer.rs と同じ)
pub fn is_builtin_scalar_method(n: &str) -> bool {
    matches!(
        n,
        "wrapping_add"
            | "wrapping_sub"
            | "wrapping_mul"
            | "wrapping_neg"
            | "checked_add"
            | "checked_sub"
            | "checked_mul"
            | "checked_div"
            | "wrapping_shl"
            | "wrapping_shr"
            | "checked_shl"
            | "checked_shr"
            | "overflowing_shl"
            | "overflowing_shr"
            | "checked_rem"
            | "checked_pow"
            | "wrapping_pow"
            | "saturating_pow"
            | "overflowing_pow"
            | "overflowing_add"
            | "overflowing_sub"
            | "overflowing_mul"
            | "rem_euclid"
            | "div_euclid"
            | "next_multiple_of"
            | "div_ceil"
            | "wrapping_add_signed"
            | "next_power_of_two"
            | "isqrt"
            | "try_into"
            | "checked_abs"
            | "checked_neg"
            | "wrapping_abs"
            | "saturating_abs"
            | "saturating_neg"
            | "overflowing_neg"
            | "overflowing_abs"
            | "leading_ones"
            | "trailing_ones"
            | "saturating_add"
            | "saturating_sub"
            | "abs"
            | "pow"
            | "count_ones"
            | "count_zeros"
            | "leading_zeros"
            | "trailing_zeros"
            | "is_power_of_two"
            | "rotate_left"
            | "rotate_right"
            | "swap_bytes"
            | "to_be"
            | "to_le"
            | "signum"
            | "is_positive"
            | "is_negative"
            | "abs_diff"
            | "is_ascii_digit"
            | "is_ascii_alphabetic"
            | "is_ascii_uppercase"
            | "is_ascii_lowercase"
            | "to_ascii_uppercase"
            | "to_ascii_lowercase"
            | "is_ascii_whitespace"
            | "is_ascii_alphanumeric"
            | "is_ascii_hexdigit"
            | "is_ascii_punctuation"
            | "is_ascii_graphic"
            | "is_ascii_control"
            | "is_ascii"
            | "to_le_bytes"
            | "to_be_bytes"
            | "to_ne_bytes"
            | "as_bytes"
            | "as_ptr"
            | "is_null"
            | "read_volatile"
            | "write_volatile"
            | "offset"
            | "wrapping_offset"
            | "read_unaligned"
            | "write_unaligned"
    )
}

fn macro_name(p: &Path) -> &str {
    p.segs.last().map(|s| s.name.as_str()).unwrap_or("")
}

fn is_fmt_macro(p: &Path) -> bool {
    matches!(macro_name(p), "print" | "println" | "eprint" | "eprintln" | "write" | "writeln" | "format" | "panic")
}

fn diverging_macro(p: &Path) -> bool {
    matches!(macro_name(p), "panic" | "unreachable" | "todo" | "unimplemented")
}

/// 必ず発散する式 (`return` / `break` / `continue` / `panic!(..)`)
fn diverges(e: &Expr) -> bool {
    match e {
        Expr::Return(..) | Expr::Break(..) | Expr::Continue(..) => true,
        Expr::Macro(p, _, _) => diverging_macro(p),
        _ => false,
    }
}

/// 1 つの識別子だけのパス
fn ident(p: &Path) -> Option<&str> {
    if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].fn_args.is_none() {
        Some(&p.segs[0].name)
    } else {
        None
    }
}

fn path1(name: &str) -> Path {
    Path { global: false, segs: vec![seg(name)], pos: Default::default() }
}

fn seg(name: &str) -> crate::parse::Seg {
    crate::parse::Seg { name: name.to_string(), args: vec![], fn_args: None, text: name.to_string() }
}

/// The receiver of the method call `Tr::m(a0, ..)` is written as, for a trait with an impl for references to a type
/// parameter (`impl<T: Tr> Tr for &T`), whose method m takes self as `kind` (`&self`: Some(false), `&mut self`:
/// Some(true), `self`: None): one whose method probe makes Self what the path call's is, a0's referent for a `&self` /
/// `&mut self` method (`&x` / `&*r` as `&self` of X, also of an `&mut` argument coerced to `&Self`), a0's type for `self`
/// (#144)
pub fn ufcs_ref_recv(a0: &Expr, kind: Option<bool>) -> Expr {
    let star = |e: &Expr| Expr::Unary("*".to_string(), Box::new(Expr::Paren(Box::new(e.clone()))), Default::default());
    match (kind, a0) {
        (Some(false), Expr::Ref(_, e)) => Expr::Ref(false, e.clone()),
        (Some(true), Expr::Ref(true, _)) => Expr::Paren(Box::new(a0.clone())),
        (Some(m), a) => Expr::Ref(m, Box::new(star(a))),
        (None, a) => Expr::Paren(Box::new(a.clone())),
    }
}

fn mcall(recv: Expr, name: &str, args: Vec<Expr>) -> Expr {
    Expr::MethodCall(Box::new(Expr::Paren(Box::new(recv))), name.to_string(), vec![], args, crate::parse::Span::default())
}

/// マクロの引数を、トップレベルの区切り (`,` / `;`) で分けたテキスト
fn split_top(g: &str, sep: &str) -> Vec<String> {
    let inner = g.trim();
    let inner = if inner.len() >= 2 { &inner[1..inner.len() - 1] } else { inner };
    let Ok(toks) = crate::parse::tokenize(inner) else { return vec![] };
    let mut out = vec![];
    let mut cur: Vec<crate::parse::Tok> = vec![];
    let mut d = 0i32;
    for t in toks {
        match t.kind {
            crate::parse::TokKind::Open => d += 1,
            crate::parse::TokKind::Close => d -= 1,
            _ => {}
        }
        if d == 0 && t.kind == crate::parse::TokKind::Punct && t.text == sep {
            out.push(join_toks(&cur));
            cur.clear();
        } else {
            cur.push(t);
        }
    }
    if !cur.is_empty() {
        out.push(join_toks(&cur));
    }
    out
}

/// トークンの列をテキストに戻す (parse し直せればよいので空白で区切る)
fn join_toks(ts: &[crate::parse::Tok]) -> String {
    let mut out = String::new();
    for t in ts {
        out.push_str(&t.text);
        // (`::` / `->` のように続けて書いた記号はつなげる)
        if !(t.joint && t.kind == crate::parse::TokKind::Punct) {
            out.push(' ');
        }
    }
    out
}

/// 整数・浮動小数点のリテラルの (浮動小数点か, 接尾辞)
fn num_suffix(t: &str) -> (bool, String) {
    // (most literals have no `_`: no copy then)
    let s: std::borrow::Cow<str> = if t.contains('_') { t.chars().filter(|&c| c != '_').collect::<String>().into() } else { t.into() };
    let radix_prefix = s.starts_with("0x") || s.starts_with("0b") || s.starts_with("0o");
    if radix_prefix {
        let body = &s[2..];
        let pos = body.find(|c: char| c == 'i' || c == 'u').unwrap_or(body.len());
        return (false, body[pos..].to_string());
    }
    let pos = s.find(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E').unwrap_or(s.len());
    let (num, suf) = s.split_at(pos);
    let float = num.contains('.') || num.contains('e') || num.contains('E') || suf.starts_with('f');
    (float, suf.to_string())
}

struct Inf<'p> {
    s: Solver<'p>,
    pending: Vec<(*const Expr, Vec<u32>, u32)>,
    /// parse し直したマクロの引数など。後回しにしたクロージャ (pending) が指すので推論の終わりまで持っておく
    keep: Vec<Vec<Expr>>,
    /// 次の apply_recv に渡すメソッド呼び出しの turbofish
    turbofish: Option<*const Vec<GArg>>,
    /// (根の関数, 最初の番号, 本体): クロージャに出会ったら clo_ctx を作る
    clo_root: Option<(u32, u32, *const Block)>,
    /// クロージャのキー (構文木の表示) → 前順の番号
    clo_keys: Option<crate::fxhash::FxMap<String, u32>>,
    /// マクロの引数を読んだもの (マクロのトークン列のテキストのアドレス → keep の番号)。コード生成も同じ構文木を使う
    margs: crate::fxhash::FxMap<usize, usize>,
    /// 型を記録した式 (テキストでも引けるようにする)
    exprs: Vec<(*const Expr, u32)>,
    /// 今見ているマクロの引数 (開き括弧) のソース上の位置: 引数を読み直した式の位置の基準
    mbase: crate::parse::Pos,
    /// the targets of the `AsRef<X>` bounds in scope (the function's and its root's): what `x.as_ref()` gives
    asref: Vec<String>,
    /// the same of the `AsMut<X>` bounds
    asmut: Vec<String>,
    /// #144: the generic fn whose body this is (or a closure's in it), whose bindings' types gen_vars has
    gen_root: Option<u32>,
    /// the bindings (their variables) whose types the generic body writes with a type parameter of gen_root's, as it
    /// writes them (a parameter `b: B`, `let r: &mut B`, a closure's capture of the fn's parameter): the code
    /// generator's gen_ty
    gen_vars: Vec<(u32, Ty)>,
    /// closures whose bodies are being inferred within the fn's: their calls are taken as on types of their own (the
    /// closure's body, inferred on its own with its gen_params_of, is what the code generator runs)
    clo_depth: u32,
    /// the receiver ref_impl_method inferred, for the lookup that takes the call over (not inferred twice)
    pre_recv: Option<(usize, u32)>,
    /// (method, trait) of a path call `Tr::m(x)` the method call it becomes keeps to (the code generator's
    /// explicit_traits)
    named_trait: Option<(String, String)>,
}

impl<'p> std::ops::Deref for Inf<'p> {
    type Target = Solver<'p>;
    fn deref(&self) -> &Solver<'p> {
        &self.s
    }
}

impl<'p> std::ops::DerefMut for Inf<'p> {
    fn deref_mut(&mut self) -> &mut Solver<'p> {
        &mut self.s
    }
}

/// sig はこの関数のシグネチャ、env は `Self` と型パラメータ。params は引数のパターン (self を除く)
pub fn infer(prog: &Program, recv: bool, params: &[(Pat, PTy)], body: &Block, sig: &FnSig, env: &TyEnv) -> LetTypes {
    infer_ctx(prog, recv, params, body, sig, env, &[], Some(sig.ret), None).0
}

/// extra は最初からスコープにある変数 (クロージャのキャプチャ)。ret が None なら戻り値型も推論して返す
/// マクロの引数を読んだ構文木 (マクロのトークン列のテキストのアドレス → 引数)。型推論はこの構文木の上で記録する
pub type MacroArgs = crate::fxhash::FxMap<usize, Vec<Expr>>;

/// 式の構文木のハッシュ → 型 (同じ形の式で型が違うものは None)。コード生成が複製した式の型を引くため
pub type TextTypes = crate::fxhash::FxMap<u64, Option<Ty>>;

pub use crate::ast_closure::{expr_hash, expr_hash_memo};

#[allow(clippy::too_many_arguments)]
pub fn infer_ctx(
    prog: &Program,
    recv: bool,
    params: &[(Pat, PTy)],
    body: &Block,
    sig: &FnSig,
    env: &TyEnv,
    extra: &[(String, Ty)],
    ret: Option<Ty>,
    root: Option<(u32, u32)>,
) -> Full {
    infer_ctx_margs(prog, recv, params, body, sig, env, extra, ret, root, &[]).0
}

/// infer_ctx と同じで、読んだマクロの引数も返す
#[allow(clippy::too_many_arguments)]
pub fn infer_ctx_margs(
    prog: &Program,
    recv: bool,
    params: &[(Pat, PTy)],
    body: &Block,
    sig: &FnSig,
    env: &TyEnv,
    extra: &[(String, Ty)],
    ret: Option<Ty>,
    root: Option<(u32, u32)>,
    gen_params: &[(String, Ty)],
) -> (Full, MacroArgs, TextTypes) {

    let clo_root = root.map(|(r, base)| (r, base, body as *const Block));
    // (the targets of the `AsRef<X>` / `AsMut<X>` bounds; a bound is squeezed of white space only when it is one)
    let (mut asref, mut asmut): (Vec<String>, Vec<String>) = (vec![], vec![]);
    let mut as_targets = |g: &[(String, Vec<String>)]| {
        for b in g.iter().flat_map(|(_, bs)| bs.iter()) {
            let b = b.trim_start();
            let into = if b.starts_with("AsRef") {
                &mut asref
            } else if b.starts_with("AsMut") {
                &mut asmut
            } else {
                continue;
            };
            let b: String = b[5..].chars().filter(|c| !c.is_whitespace()).collect();
            if let Some(x) = b.strip_prefix('<').and_then(|x| x.strip_suffix('>')) {
                into.push(x.to_string());
            }
        }
    };
    as_targets(&sig.generics);
    if let Some((r, _)) = root {
        as_targets(&prog.sigs[r as usize].generics);
    }
    // (#144: the bindings whose types the generic body writes with root's type parameters, the code generator's
    // gen_params_of)
    let gen_root = root.filter(|_| !gen_params.is_empty()).map(|r| r.0);
    let mut s = Inf {
        s: Solver::new(prog, env),
        pending: vec![],
        keep: vec![],
        turbofish: None,
        clo_root,
        clo_keys: None,
        margs: Default::default(),
        exprs: vec![],
        mbase: Default::default(),
        asref,
        asmut,
        gen_root,
        gen_vars: vec![],
        clo_depth: 0,
        pre_recv: None,
        named_trait: None,
    };
    s.ret = match ret {
        Some(t) if prog.has_param(t) => s.known_g(t, &mut vec![]),
        Some(t) => s.known(t),
        None => s.var(),
    };
    for (n, t) in extra {
        let v = s.known(*t);
        s.scopes[0].push((n.clone(), v));
    }
    let mut pvars = vec![];
    let mut names: Vec<Option<String>> = vec![];
    if recv {
        names.push(Some("self".to_string()));
    }
    for (p, _) in params {
        names.push(match p {
            Pat::Ident(_, _, n, None) => Some(n.clone()),
            _ => None,
        });
    }
    for (name, &t) in names.into_iter().zip(&sig.params) {
        let Some(name) = name else {
            let v = s.var();
            pvars.push(v);
            continue;
        };
        let v = if prog.has_param(t) { s.var() } else { s.known(t) };
        pvars.push(v);
        s.scopes[0].push((name, v));
    }
    for (n, t) in gen_params {
        if let Some(v) = s.lookup(n) {
            s.gen_vars.push((v, *t));
        }
    }
    let tail = s.block(body);
    let ret = s.ret;
    s.unify(tail, ret);
    s.finish_unsize();
    s.finish_late_bounds();
    s.finish_collects();
    s.finish_assocs();
    let lets = std::mem::take(&mut s.lets);
    let r = s.resolve(ret);
    let ps = pvars.into_iter().map(|v| s.resolve(v)).collect();
    let mut out = LetTypes::default();
    for (k, v) in lets {
        if let Some(t) = s.resolve(v) {
            out.insert(k, t);
        }
    }
    // (同じ式を何度か推論し直すことがある: lets と同じく後の記録を使う)
    let mut recs: Vec<(*const Expr, u32)> = std::mem::take(&mut s.exprs);
    // (アドレスで並べ、同じアドレスは最後の記録だけ残す: 安定ソートなので記録順は保たれる)
    recs.sort_by_key(|r| r.0 as usize);
    let mut dedup: Vec<(*const Expr, u32)> = Vec::with_capacity(recs.len());
    for r in recs {
        match dedup.last_mut() {
            Some(l) if l.0 == r.0 => *l = r,
            _ => dedup.push(r),
        }
    }
    let recs = dedup;
    let mut texts = TextTypes::with_capacity_and_hasher(recs.len(), Default::default());
    let mut memo = crate::fxhash::FxMap::default();
    for (e, v) in recs {
        let Some(t) = s.resolve(v) else { continue };
        let t = Some(t);
        // SAFETY: 記録した式は本体か keep の中にあり、まだ生きている
        let h = expr_hash_memo(unsafe { &*e }, &mut memo);
        match texts.get(&h) {
            Some(&old) if old != t => {
                texts.insert(h, None);
            }
            Some(_) => {}
            None => {
                texts.insert(h, t);
            }
        }
    }
    let mut margs = MacroArgs::default();
    let keep = std::mem::take(&mut s.keep);
    let mut keep: Vec<Option<Vec<Expr>>> = keep.into_iter().map(Some).collect();
    for (&g, &ki) in &s.margs {
        if let Some(v) = keep[ki].take() {
            margs.insert(g, v);
        }
    }
    // (残りも返す: 型の記録はこれらのアドレスも指すので、コード生成の間に同じアドレスが別の式に使われないようにする。
    //  キーは小さい番号 (アドレスにはならない))
    for (i, v) in keep.into_iter().enumerate() {
        if let Some(v) = v {
            margs.insert(i, v);
        }
    }
    ((out, r, ps), margs, texts)
}

impl Inf<'_> {
    /// 書かれた型 (`Self` は env から)
    fn parse_ty(&self, t: &PTy) -> Option<Ty> {
        let ts = crate::ast_types::tysyn(t, Some(&TySyn::Named("Self".into()))).ok()?;
        self.prog.resolve_env(&ts, self.env).ok()
    }

    fn strip_refs_ty(mut t: Ty) -> Ty {
        while let Ty::Ref(_, i) = t {
            t = *i;
        }
        t
    }

    fn variant_by(&self, p: &Path) -> Option<(u32, usize)> {
        self.prog.variant_by_p(&PPath::of(p), self.self_sid)
    }

    fn struct_by(&self, p: &Path) -> Option<u32> {
        self.prog.struct_by_p(&PPath::of(p), self.self_sid)
    }

    /// A pattern's path that writes type arguments (`P::<u16>(x, _)`): the value it matches is of that instance
    fn pat_turbofish(&mut self, v: u32, p: &Path, sid: u32) {
        // (a value of another instance (`S::<u16>(_)` matching `S(1u8)`) is then a mismatch where it is built)
        if p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, GArg::Ty(_)))) {
            if let Some(t) = self.variant_turbofish(p, sid) {
                self.unify_written(v, t);
            }
        }
    }

    /// v is the type t the program writes (a turbofish): t is its representative, whatever v's arguments are
    fn unify_written(&mut self, v: u32, t: Ty) {
        let k = self.known(t);
        self.written += 1;
        self.unify(v, k);
        self.written -= 1;
    }

    /// The struct a path with a turbofish on its last segment names (`S::<u32>`)
    fn struct_by_turbofish(&self, p: &Path) -> Option<u32> {
        let last = p.segs.last()?;
        if last.args.is_empty() || last.fn_args.is_some() {
            return None;
        }
        let mut pp = PPath::of(p);
        pp.segs.last_mut()?.args = false;
        self.prog.struct_by_p(&pp, self.self_sid)
    }

    fn fn_by(&self, p: &Path) -> Option<u32> {
        let id = self.prog.fn_by_p(&PPath::of(p), self.self_sid)?;
        // (of inherent functions in impls for disjoint instances, the one of the self type the path names)
        Some(self.prog.fn_of_path_self(id, p, self.env))
    }

    /// 自前の構文木のノードを keep に移して推論する (アドレスでは記録しない)
    fn expr_kept(&mut self, e: Expr, record: bool) -> u32 {
        let ki = self.keep.len();
        self.keep.push(vec![e]);
        let x: *const Expr = &self.keep[ki][0];
        if !record {
            self.no_record += 1;
        }
        // SAFETY: keep の要素は推論の終わりまで動かさない (外側の Vec が伸びても内側の要素は動かない)
        let v = self.expr(unsafe { &*x });
        if !record {
            self.no_record -= 1;
        }
        v
    }

    /// `None::<u32>` / `Option::<u32>::None`: 型引数を書いたバリアントの型
    fn variant_turbofish(&mut self, p: &Path, sid: u32) -> Option<Ty> {
        let args = p.segs.iter().rev().find(|s| !s.args.is_empty())?;
        let mut ts = vec![];
        for a in &args.args {
            if let GArg::Ty(t) = a {
                ts.push(self.parse_ty(t)?);
            }
        }
        let base = self.prog.base_of(sid);
        self.prog.instantiate(base, ts).ok().map(Ty::Struct)
    }

    /// prelude の helper `f(recv, args..)` の戻り値 (turbofish は型引数の前から)
    fn apply_helper(&mut self, id: u32, tf: &[GArg], args: Vec<u32>) -> u32 {
        let sig = &self.prog.sigs[id as usize];
        let (params, ret, ng) = (sig.params[1.min(sig.params.len())..].to_vec(), sig.ret, sig.generics.len());
        let mut pv: Vec<Option<u32>> = vec![None; ng];
        for (i, a) in tf.iter().enumerate().take(ng) {
            if let GArg::Ty(t) = a {
                if let Some(t) = self.parse_ty(t) {
                    pv[i] = Some(self.known(t));
                }
            }
        }
        for (a, p) in args.into_iter().zip(params) {
            if p == Ty::Ref(false, intern(Ty::Str)) {
                continue;
            }
            let k = self.known_g(p, &mut pv);
            self.unify(a, k);
        }
        self.known_g(ret, &mut pv)
    }

    /// apply_helper for a slice helper of the prelude (`__exr_slice_splitn<T, F>(s: &[T], ..)`): its first type
    /// parameter is the receiver's element el
    fn apply_seq_helper(&mut self, id: u32, el: u32, args: Vec<u32>) -> u32 {
        let sig = &self.prog.sigs[id as usize];
        let (params, ret, ng) = (sig.params[1.min(sig.params.len())..].to_vec(), sig.ret, sig.generics.len());
        let mut pv: Vec<Option<u32>> = vec![None; ng];
        if let Some(p) = pv.first_mut() {
            *p = Some(el);
        }
        for (a, p) in args.into_iter().zip(params) {
            let k = self.known_g(p, &mut pv);
            self.unify(a, k);
        }
        // (a closure's parameter type through the bound, `F: FnMut(&T) -> bool`: `[1, 0, 2].splitn(2, |x: &u8| ..)`
        // makes the elements u8, as apply_recv does)
        let prog = self.prog;
        self.unify_fn_bounds(&prog.sigs[id as usize].generics, &mut pv);
        self.known_g(ret, &mut pv)
    }

    /// 式が (参照越しの) 配列 / スライスなら Some。受け手の型を見るためだけに先に推論する
    fn expr_peek_elem(&mut self, e: &Expr) -> Option<u32> {
        let v = self.expr(e);
        let d = self.deref_expr(v);
        let r = self.find(d);
        if !matches!(self.shape[r as usize], Some(Shape::Arr(..)) | Some(Shape::Slc(_))) {
            return None;
        }
        self.elem_of(v)
    }

    fn apply_recv(&mut self, callee: Option<u32>, args: Vec<u32>, recv: u32) -> u32 {
        let Some(id) = callee else { return self.var() };
        let sig = &self.prog.sigs[id as usize];
        let (params, ret, self_ty, is_trait) =
            (sig.params[1.min(sig.params.len())..].to_vec(), sig.ret, sig.self_ty, sig.trait_of.is_some());
        let mut pv: Vec<Option<u32>> = vec![None; sig.generics.len()];
        if is_trait {
            pv[0] = Some(recv);
        } else if let Some(st) = self_ty {
            let k = self.known_g(st, &mut pv);
            self.unify(k, recv);
        }
        for (a, p) in args.into_iter().zip(params) {
            let k = self.known_g(p, &mut pv);
            self.unify(a, k);
        }
        // turbofish (`collect::<Vec<_>>()`): 後ろの型パラメータ (メソッド自身のもの) から
        if let Some(tf) = self.turbofish.take() {
            // SAFETY: tf は推論の間生きている関数の構文木の中を指す
            let tf = unsafe { &*tf };
            let tys: Vec<&PTy> = tf.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None }).collect();
            let ng = pv.len();
            if tys.len() <= ng {
                for (k, t) in tys.into_iter().enumerate() {
                    let v = self.ty_var(t);
                    let i = (ng + k).wrapping_sub(tf.len());
                    if i < pv.len() {
                        match pv[i] {
                            Some(old) => self.unify(old, v),
                            None => pv[i] = Some(v),
                        }
                    }
                }
            }
        }
        let prog = self.prog;
        self.unify_fn_bounds(&prog.sigs[id as usize].generics, &mut pv);
        self.unify_assoc_bounds(&prog.sigs[id as usize].generics, &mut pv);
        let r = self.known_g(ret, &mut pv);
        self.late_bounds(id, pv);
        r
    }

    /// マクロの引数を読む (同じマクロは 1 回だけ。読んだ構文木はコード生成にも渡す)
    fn macro_args_kept(&mut self, g: &str) -> Option<usize> {
        let k = g.as_ptr() as usize;
        if let Some(&ki) = self.margs.get(&k) {
            return Some(ki);
        }
        let args = ast_closure::macro_args_at(g, self.mbase)?;
        let ki = self.keep.len();
        self.keep.push(args);
        self.margs.insert(k, ki);
        Some(ki)
    }

    /// 書式マクロの引数 (`println!("{}", (1..4).map(..))`) も推論する (読んだ構文木の上で記録する)
    fn fmt_args(&mut self, g: &str) {
        let Some(ki) = self.macro_args_kept(g) else { return };
        let mut xs: Vec<*const Expr> = vec![];
        for a in &self.keep[ki] {
            match a {
                Expr::Lit(t, _) if t.starts_with('"') || t.starts_with("r\"") || t.starts_with("r#") => {}
                Expr::Assign(_, r) => xs.push(&**r as *const Expr),
                a => xs.push(a as *const Expr),
            }
        }
        self.pos_record += 1;
        for x in xs {
            // SAFETY: keep の要素は推論の終わりまで動かさない
            self.expr(unsafe { &*x });
        }
        self.pos_record -= 1;
    }

    /// 書式文字列の幅・精度に名前で使う変数 (`{:w$}` / `{:.p$}`) は usize
    fn fmt_width_vars(&mut self, g: &str) {
        self.fmt_args(g);
        let Ok(toks) = crate::parse::tokenize(g) else { return };
        let Some(lit) = toks.iter().find(|t| t.kind == crate::parse::TokKind::Lit && t.text.starts_with('"')) else { return };
        let Some((_, bytes)) = crate::ast_consteval::str_lit(&lit.text) else { return };
        let s = String::from_utf8_lossy(&bytes).into_owned();
        if !s.contains('$') {
            return;
        }
        let b = s.as_bytes();
        for (i, _) in s.match_indices('$') {
            let mut k = i;
            while k > 0 && (b[k - 1].is_ascii_alphanumeric() || b[k - 1] == b'_') {
                k -= 1;
            }
            let name = &s[k..i];
            if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
                continue;
            }
            if let Some(v) = self.lookup(name) {
                let u = self.known(Ty::Usize);
                self.unify(v, u);
            }
        }
    }

    /// 書かれた型の型変数 (`Vec<_>` の `_` は新しい型変数)
    fn ty_var(&mut self, t: &PTy) -> u32 {
        // (`Box<_>`: Box is not a struct; its argument is a variable too)
        if let PTy::Path(None, p) = t {
            if let Some(last) = p.segs.last() {
                if last.name == "Box" && p.segs.len() == 1 && !self.prog.struct_index.contains_key("Box") {
                    if let [GArg::Ty(a)] = &last.args[..] {
                        let i = self.ty_var(a);
                        return self.with(Shape::Bx(i));
                    }
                }
            }
        }
        if let Some(k) = self.parse_ty(t) {
            return self.known(k);
        }
        match t {
            PTy::Infer => self.var(),
            PTy::Ref(_, m, i) => {
                let i = self.ty_var(i);
                self.with(Shape::Ref(*m, i))
            }
            PTy::Path(None, p) => {
                let Some(last) = p.segs.last() else { return self.var() };
                let base = self.prog.struct_index.get(&last.name).copied();
                match base {
                    Some(b) if !last.args.is_empty() => {
                        let mut args = vec![];
                        for a in &last.args {
                            if let GArg::Ty(t) = a {
                                args.push(self.ty_var(t));
                            }
                        }
                        self.adt(b, args)
                    }
                    _ => self.var(),
                }
            }
            PTy::Tuple(ts) => {
                let mut args = vec![];
                for t in ts {
                    args.push(self.ty_var(t));
                }
                let tb = self.prog.tuple_base(args.len());
                self.adt(tb, args)
            }
            // (`&[_]`: a slice of a new variable, so an array initializer coerces to it)
            PTy::Slice(i) => {
                let i = self.ty_var(i);
                self.with(Shape::Slc(i))
            }
            _ => self.var(),
        }
    }

    fn lit_var(&mut self, t: &str) -> u32 {
        if t == "true" || t == "false" {
            return self.known(Ty::Bool);
        }
        if t.starts_with("b'") {
            return self.known(Ty::U8);
        }
        if t.starts_with('\'') {
            return self.known(Ty::Char);
        }
        if t.starts_with("b\"") || t.starts_with("br") {
            let n = crate::ast_consteval::str_lit(t).map(|x| x.1.len()).unwrap_or(0);
            return self.known(Ty::Ref(false, intern(Ty::Array(intern(Ty::U8), n as u32))));
        }
        if t.starts_with('"') || t.starts_with('r') {
            return self.known(Ty::Ref(false, intern(Ty::Str)));
        }
        if !t.starts_with(|c: char| c.is_ascii_digit()) {
            return self.var();
        }
        let (float, suf) = num_suffix(t);
        if suf == "f32" {
            return self.known(Ty::F32);
        }
        if suf == "f64" {
            return self.known(Ty::F64);
        }
        if float {
            let v = self.var();
            self.float_lit[v as usize] = true;
            return v;
        }
        match Ty::from_suffix(&suf) {
            Some(t) => self.known(t),
            None => {
                let v = self.var();
                self.int_lit[v as usize] = true;
                v
            }
        }
    }

    fn closure_body(&mut self, c: &Expr, vs: &[u32], rv: u32) {
        let Expr::Closure(_, ps, _, body, _) = c else { return };
        self.scopes.push(vec![]);
        for ((p, _), &v) in ps.iter().zip(vs) {
            self.pat(p, v, None);
        }
        let saved = self.ret;
        self.ret = rv;
        self.clo_depth += 1;
        let b = self.expr(body);
        self.clo_depth -= 1;
        self.unify(b, rv);
        self.ret = saved;
        self.scopes.pop();
    }

    fn finish_closures(&mut self, mark: usize) {
        while self.pending.len() > mark {
            let (c, vs, rv) = self.pending.remove(mark);
            let saved = std::mem::take(&mut self.defer);
            // SAFETY: c は推論の間生きている構文木の中を指す
            let c = unsafe { &*c };
            self.closure_body(c, &vs, rv);
            self.defer = saved;
        }
    }

    /// 識別子のパターンが unit バリアント (`None` など) なら、そのバリアント
    fn unit_variant_ident(&self, r: bool, m: bool, n: &str, sub: &Option<Box<Pat>>) -> Option<(u32, usize)> {
        if r || m || sub.is_some() {
            return None;
        }
        let (sid, vi) = self.prog.variant_by_p(&PPath::of(&path1(n)), None)?;
        (self.prog.structs[sid as usize].variants[vi].kind == VariantKind::Unit).then_some((sid, vi))
    }

    /// 識別子のパターンがリテラルの const (`MAX`) なら、そのリテラル
    fn const_lit(&self, r: bool, m: bool, n: &str, sub: &Option<Box<Pat>>) -> Option<String> {
        if r || m || sub.is_some() || !n.starts_with(|c: char| c.is_ascii_uppercase()) {
            return None;
        }
        let &ci = self.prog.const_index.get(n)?;
        let c = &self.prog.consts[ci as usize];
        if c.stat.is_some() {
            return None;
        }
        match crate::parse::parse_expr(&c.expr).ok()? {
            Expr::Lit(t, _) => Some(t),
            _ => None,
        }
    }

    /// パターンを型変数 v に当て、束縛を今のスコープに入れる
    /// The unit struct a pattern's name is (`S` with `struct S;`), not a variant or a const
    fn unit_struct(&self, n: &str) -> Option<u32> {
        let sid = self.struct_by(&path1(n))?;
        let sd = &self.prog.structs[sid as usize];
        (!sd.is_enum && sd.fields.is_empty() && self.variant_by(&path1(n)).is_none() && !self.prog.const_index.contains_key(n)).then_some(sid)
    }

    fn pat(&mut self, pat: &Pat, v: u32, mode: Option<bool>) {
        match pat {
            Pat::Ident(r, m, n, sub) if self.unit_variant_ident(*r, *m, n, sub).is_some() => {
                let (sid, vi) = self.unit_variant_ident(*r, *m, n, sub).unwrap();
                let (v, _) = self.auto_deref(v, mode);
                // (パスでもう一度引く: self_sid つきの検索と同じ結果になる)
                let _ = (sid, vi);
                if let Some((sid, vi)) = self.variant_by(&path1(n)) {
                    self.pat_fields(v, sid, Some(vi));
                }
            }
            Pat::Ident(r, m, n, sub) if self.const_lit(*r, *m, n, sub).is_some() => {
                let t = self.const_lit(*r, *m, n, sub).unwrap();
                let (v, _) = self.auto_deref(v, mode);
                let e = self.lit_var(&t);
                self.unify(v, e);
            }
            // (a const that is not a literal (`const X: u64 = !0 as u16 as u64;`): the scrutinee has its type)
            Pat::Ident(false, false, n, None)
                if n.starts_with(|c: char| c.is_ascii_uppercase())
                    && self.lookup(n).is_none()
                    && self.prog.const_index.get(n.as_str()).is_some_and(|&ci| self.prog.consts[ci as usize].stat.is_none()) =>
            {
                let ci = self.prog.const_index[n.as_str()];
                let t = self.prog.consts[ci as usize].ty;
                let (v, _) = self.auto_deref(v, mode);
                let k = self.known(t);
                self.unify(v, k);
            }
            // (a unit struct (`Some(S)` with `struct S;`): the scrutinee is S, the pattern binds nothing)
            Pat::Ident(false, false, n, None) if n.starts_with(|c: char| c.is_ascii_uppercase()) && self.lookup(n).is_none() && self.unit_struct(n).is_some() => {
                let sid = self.unit_struct(n).unwrap();
                let (v, _) = self.auto_deref(v, mode);
                self.pat_fields(v, sid, None);
            }
            Pat::Ident(r, m, n, sub) => {
                if let Some(sub) = sub {
                    self.pat(sub, v, mode);
                }
                let mode = if *r { Some(*m) } else { mode };
                self.bind_name(n.clone(), v, mode);
            }
            Pat::Ref(m, p) => {
                // (a scrutinee of no type yet is a reference: `match get() { &S => .. }`)
                let r = self.find(v);
                let i = if self.shape[r as usize].is_none() {
                    let i = self.var();
                    let rv = self.with(Shape::Ref(*m, i));
                    self.unify(v, rv);
                    i
                } else {
                    self.deref_var(v)
                };
                self.pat(p, i, None);
            }
            Pat::Or(cs, _) => {
                for c in cs {
                    self.pat(c, v, mode);
                }
            }
            Pat::Lit(e) => {
                let (v, _) = self.auto_deref(v, mode);
                let e = match &**e {
                    Expr::Lit(t, _) => self.lit_var(t),
                    e => self.expr(e),
                };
                self.unify(v, e);
            }
            Pat::Range(a, b, _) => {
                let (v, _) = self.auto_deref(v, mode);
                for e in a.iter().chain(b.iter()) {
                    let e = self.expr(e);
                    self.unify(v, e);
                }
            }
            Pat::Path(p) => {
                let (v, _) = self.auto_deref(v, mode);
                if let Some((sid, vi)) = self.variant_by(p) {
                    self.pat_fields(v, sid, Some(vi));
                } else if let Some(sid) = self.struct_by(p).filter(|&sid| {
                    let sd = &self.prog.structs[sid as usize];
                    !sd.is_enum && sd.fields.is_empty()
                }) {
                    self.pat_fields(v, sid, None);
                }
            }
            Pat::TupleStruct(p, elems) => {
                let (v, mode) = self.auto_deref(v, mode);
                let fts: Vec<u32> = if let Some((sid, vi)) = self.variant_by(p) {
                    self.pat_turbofish(v, p, sid);
                    self.pat_fields(v, sid, Some(vi)).into_iter().map(|f| f.1).collect()
                } else if let Some(sid) = self.struct_by(p).or_else(|| self.struct_by_turbofish(p)) {
                    self.pat_turbofish(v, p, sid);
                    self.pat_fields(v, sid, None).into_iter().map(|f| f.1).collect()
                } else {
                    return;
                };
                self.pat_elems(elems, &fts, mode);
            }
            Pat::Tuple(elems) => {
                let (v, mode) = self.auto_deref(v, mode);
                let base = self.prog.tuple_base(elems.len());
                let fts: Vec<u32> = self.pat_fields(v, base, None).into_iter().map(|f| f.1).collect();
                self.pat_elems(elems, &fts, mode);
            }
            Pat::Struct(p, fps, _) => {
                let (v, mode) = self.auto_deref(v, mode);
                // (a unit variant has no fields: `Skip { iter, .. }` is the struct, even with `use E::*`'s variant `Skip`)
                let variant = self.variant_by(p).filter(|&(sid, vi)| fps.is_empty() || !self.prog.structs[sid as usize].variants[vi].fields.is_empty() || self.struct_by(p).is_none());
                let fields = if let Some((sid, vi)) = variant {
                    self.pat_turbofish(v, p, sid);
                    self.pat_fields(v, sid, Some(vi))
                } else if let Some(sid) = self.struct_by(p).or_else(|| self.struct_by_turbofish(p)) {
                    self.pat_turbofish(v, p, sid);
                    self.pat_fields(v, sid, None)
                } else {
                    return;
                };
                for (name, fp) in fps {
                    if let Some(&(_, ft)) = fields.iter().find(|f| &f.0 == name) {
                        self.pat(fp, ft, mode);
                    }
                }
            }
            // `[a, b, rest @ ..]`: the elements take the element type; a binding of `..` is a slice of them
            Pat::Slice(elems) => {
                let (v, mode) = self.auto_deref(v, mode);
                let mut t = self.resolve(v);
                while let Some(Ty::Ref(_, i)) = t {
                    t = Some(*i);
                }
                let root = self.find(v);
                let elem = match (t, self.shape[root as usize]) {
                    (Some(Ty::Slice(e)) | Some(Ty::Array(e, _)), _) => self.known(*e),
                    (_, Some(Shape::Slc(e)) | Some(Shape::Arr(e, _))) => e,
                    _ => return,
                };
                for p in elems {
                    match p {
                        Pat::Rest => {}
                        Pat::Ident(_, _, _, Some(sub)) if matches!(&**sub, Pat::Rest) => {
                            let sl = self.with(Shape::Slc(elem));
                            self.pat(p, sl, mode);
                        }
                        p => self.pat(p, elem, mode),
                    }
                }
            }
            _ => {}
        }
    }

    fn pat_elems(&mut self, elems: &[Pat], fts: &[u32], mode: Option<bool>) {
        let rest = elems.iter().position(|p| matches!(p, Pat::Rest));
        for (i, p) in elems.iter().enumerate() {
            let fi = match rest {
                Some(r) if i == r => continue,
                Some(r) if i > r => fts.len().wrapping_sub(elems.len() - i),
                _ => i,
            };
            if let Some(&ft) = fts.get(fi) {
                self.pat(p, ft, mode);
            }
        }
    }

    fn block(&mut self, b: &Block) -> u32 {
        self.scopes.push(vec![]);
        let mut res = None;
        let n = b.stmts.len();
        for (i, st) in b.stmts.iter().enumerate() {
            match st {
                Stmt::Let(pat, ann_ty, init, _, ..) => {
                    let ann = ann_ty.as_ref().and_then(|t| self.parse_ty(t));
                    let v = match init {
                        Some(init) => self.expr(init),
                        None => self.var(),
                    };
                    // (注釈が完全な型なら変数はその型: `let s: &[T] = &v;` の強制変換で初期値の型と違ってよい)
                    let v = match ann {
                        Some(t) => {
                            let k = self.known(t);
                            self.unify(v, k);
                            // (`let v: Vec<_> = it.collect();`: what the collect yields is known before the statements
                            // after it, as rustc selects the `FromIterator` impl there. Not from how they use it:
                            // `&v[0]` returned as a `&str` is a `&&str` coerced, not an element of type `str`)
                            let mut call = init.as_ref();
                            while let Some(Expr::Paren(x)) = call {
                                call = Some(x);
                            }
                            if !self.s.collects.is_empty() && matches!(call, Some(Expr::MethodCall(_, m, _, _, _)) if m == "collect") {
                                self.s.ready_collects();
                            }
                            if self.prog.has_param(t) { v } else { k }
                        }
                        None => v,
                    };
                    if self.no_record == 0 {
                        self.lets.push((stmt_key(st), v));
                    }
                    // (#144: the binding's type as the generic body writes it, the code generator's gen_ann / gen_ty)
                    if self.gen_root.is_some() && self.clo_depth == 0 {
                        if let Pat::Ident(false, _, _, None) = pat {
                            let g = match ann_ty {
                                Some(t) => self.gen_ann(t),
                                None => init.as_ref().and_then(|i| self.gen_ty(i, 0)),
                            };
                            if let Some(g) = g {
                                self.gen_vars.push((v, g));
                            }
                        }
                    }
                    match pat {
                        // (`let ref x = e;` binds a reference to e's value: as a pattern)
                        Pat::Ident(false, _, n, None) => self.scopes.last_mut().unwrap().push((n.clone(), v)),
                        p => self.pat(p, v, None),
                    }
                }
                Stmt::Expr(e, semi) => {
                    // (文の位置の `println!(..);` は syn では Stmt::Macro: 書式の変数だけ見る)
                    if let Expr::Macro(p, g, sp) = e {
                        self.mbase = sp.at;
                        if i + 1 == n && diverging_macro(p) {
                            res = Some(self.var());
                            continue;
                        }
                        if *semi || i + 1 < n {
                            if is_fmt_macro(p) {
                                self.fmt_width_vars(g);
                            } else if matches!(macro_name(p), "assert" | "debug_assert" | "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne") {
                                self.macro_expr(p, g);
                            }
                            continue;
                        }
                    }
                    let v = self.expr(e);
                    if i + 1 == n && !*semi {
                        res = Some(v);
                    }
                    if i + 1 == n && *semi && diverges(e) {
                        res = Some(self.var());
                    }
                }
                Stmt::Item(_) => {}
            }
        }
        self.scopes.pop();
        res.unwrap_or_else(|| self.known(Ty::Unit))
    }

    /// 式の型変数。値を作る式 (リテラル・構築・呼び出し) は結果の型も記録する
    fn expr(&mut self, e: &Expr) -> u32 {
        if let Some((a, v)) = self.pre_recv {
            if a == e as *const Expr as usize {
                self.pre_recv = None;
                return v;
            }
        }
        let call = matches!(e, Expr::Call(..) | Expr::MethodCall(..));
        let mark = self.pending.len();
        if call {
            self.defer += 1;
        }
        let frame = match e {
            Expr::Loop(l, _) => Some((l.clone(), false)),
            Expr::While(l, _, _) => Some((l.clone(), false)),
            Expr::For(l, _, _, _) => Some((l.clone(), false)),
            Expr::Block(Some(l), _, _) => Some((Some(l.clone()), true)),
            _ => None,
        };
        let v = match frame {
            None => self.expr_inner(e),
            Some((label, is_block)) => {
                let rv = self.var();
                self.brk.push((label, rv, is_block, false));
                let v = self.expr_inner(e);
                let (_, rv, _, broke) = self.brk.pop().unwrap();
                match e {
                    Expr::Loop(..) if !broke => self.known(Ty::Never),
                    Expr::Loop(..) => rv,
                    Expr::Block(..) if broke => {
                        if self.resolve(v) != Some(Ty::Never) {
                            self.unify(rv, v);
                        }
                        rv
                    }
                    _ => v,
                }
            }
        };
        if call {
            self.defer -= 1;
            self.finish_closures(mark);
        }
        if matches!(
            e,
            Expr::Lit(_, _)
                | Expr::Struct(..)
                | Expr::Call(..)
                | Expr::MethodCall(..)
                | Expr::Path(..)
                | Expr::Macro(..)
                | Expr::Try(_)
                | Expr::Index(..)
                | Expr::Field(..)
                | Expr::Array(_)
                | Expr::Repeat(..)
                | Expr::Tuple(_)
                | Expr::Ref(..)
                | Expr::Unary(..)
                | Expr::Binary(..)
                | Expr::Cast(..)
        ) {
            if self.no_record == 0 || self.pos_record > 0 {
                self.lets.push((e as *const Expr as usize, v));
                self.exprs.push((e as *const Expr, v));
            }
        }
        v
    }

    /// Whether the type of v is surely not Clone (acg's clone_call): a type of the program with no clone and no
    /// derive, or an instance with such an argument, but for the shared pointers
    fn not_clone(&mut self, v: u32, depth: u32) -> bool {
        if depth > 4 {
            return false;
        }
        let f = self.find(v);
        let (b, args) = match self.shape[f as usize] {
            // (a known instance: its arguments are known too)
            Some(Shape::Known(t @ (Ty::Struct(_) | Ty::Ref(true, _)))) => return self.prog.surely_not_clone(t, depth),
            Some(Shape::Ref(true, _)) => return true,
            Some(Shape::Adt(b, st, n)) => (b, Some((st, n))),
            _ => return false,
        };
        let own = self.prog.struct_item.get(b as usize).is_some_and(|&ii| ii < self.prog.prelude_from);
        // (a closure's Clone is its captures', which are not among the arguments here)
        if (own && self.prog.method(b, "clone").is_some()) || self.prog.is_closure(b) {
            return false;
        }
        let derived = self.prog.derived.get(&b).is_some_and(|d| d.iter().any(|x| x == "Clone"));
        if own && !derived && !self.prog.implements(Ty::Struct(b), "Clone") {
            return true;
        }
        let Some((st, n)) = args.filter(|_| self.prog.clone_needs_args(b, own)) else { return false };
        (st..st + n).any(|k| {
            let a = self.adt_args[k as usize];
            self.not_clone(a, depth + 1)
        })
    }

    fn binary(&mut self, op: &str, le: &Expr, re: &Expr) -> u32 {
        let l = self.expr(le);
        let r = self.expr(re);
        let is_adt = |this: &mut Self, v: u32| {
            let mut t = this.resolve(v);
            while let Some(Ty::Ref(_, i)) = t {
                t = Some(*i);
            }
            matches!(t, Some(Ty::Struct(_)))
        };
        let adt = is_adt(self, l) || is_adt(self, r);
        let (l, r) = if adt || matches!(op, "&&" | "||" | "<<" | ">>" | "<<=" | ">>=") {
            (l, r)
        } else {
            (self.num_operand(l), self.num_operand(r))
        };
        match op {
            "&&" | "||" => {
                let k = self.known(Ty::Bool);
                self.unify(l, k);
                self.unify(r, k);
                k
            }
            "==" | "!=" => {
                let (fl, fr) = (self.find(l), self.find(r));
                match (self.shape[fl as usize], self.shape[fr as usize]) {
                    (Some(Shape::Ref(_, x)), Some(s)) if !matches!(s, Shape::Ref(..)) => self.seq_unify(x, r),
                    (Some(s), Some(Shape::Ref(_, y))) if !matches!(s, Shape::Ref(..)) => self.seq_unify(l, y),
                    (Some(Shape::Ref(_, x)), Some(Shape::Ref(_, y))) => {
                        let (fx, fy) = (self.find(x), self.find(y));
                        let seqish =
                            |s: &Option<Shape>| matches!(s, Some(Shape::Slc(_) | Shape::Arr(..) | Shape::Known(Ty::Slice(_) | Ty::Array(..))));
                        if seqish(&self.shape[fx as usize]) && seqish(&self.shape[fy as usize]) {
                            self.seq_unify(x, y)
                        } else {
                            self.unify(l, r)
                        }
                    }
                    _ => self.seq_unify(l, r),
                }
                self.known(Ty::Bool)
            }
            "<" | "<=" | ">" | ">=" => {
                self.unify(l, r);
                self.known(Ty::Bool)
            }
            // (the left operand's type, a number behind a reference dereferenced: `let _: u8 = &0 << 0;`)
            "<<" | ">>" if !adt => self.num_operand(l),
            // (a struct's, or a reference to one's: the impl's `shl` / `shr`, whose Output is the type, `impl Shl<u8> for
            // &S { type Output = u32; .. }`; the operands are not unified)
            "<<" | ">>" => {
                let m = if op == "<<" { "shl" } else { "shr" };
                let found = match self.resolve(l) {
                    Some(Ty::Struct(sid)) => self.prog.method(sid, m),
                    Some(Ty::Ref(_, Ty::Struct(sid))) => self.prog.method(*sid, &format!("__ref_{m}")),
                    _ => None,
                };
                // (of two impls, `Shl<u8>` and `Shl<u16>`, the one the right operand's type picks: while that is not
                // known (`let n = 9; S(1) << n`), neither's signature is taken, as code generation picks by it)
                let group = found.and_then(|id| self.prog.overloads.get(self.prog.names[id as usize].split('#').next().unwrap_or("")));
                let found = match (found, group) {
                    (Some(_), Some(g)) if g.len() > 1 => {
                        let rt = self.resolve(r);
                        let fit: Vec<u32> = g
                            .iter()
                            .copied()
                            .filter(|&c| {
                                let sig = &self.prog.sigs[c as usize];
                                let mut sol = vec![None; sig.generics.len()];
                                rt.is_some_and(|t| sig.params.get(1).is_some_and(|&pt| self.prog.match_ty(pt, t, &mut sol)))
                            })
                            .collect();
                        match fit.as_slice() {
                            [c] => Some(*c),
                            _ => return self.var(),
                        }
                    }
                    _ => found,
                };
                match found {
                    Some(id) => self.apply_recv(Some(id), vec![r], l),
                    None => l,
                }
            }
            "<<=" | ">>=" => self.known(Ty::Unit),
            "+=" | "-=" | "*=" | "/=" | "%=" | "^=" | "&=" | "|=" => {
                self.unify(l, r);
                self.known(Ty::Unit)
            }
            _ => {
                let m = match op {
                    "+" => "add",
                    "-" => "sub",
                    "*" => "mul",
                    "/" => "div",
                    "%" => "rem",
                    "&" => "bitand",
                    "|" => "bitor",
                    "^" => "bitxor",
                    _ => "",
                };
                let lt = self.resolve(l);
                let found = match lt {
                    Some(Ty::Struct(sid)) if !m.is_empty() => self.prog.method(sid, m).map(|id| (id, l)),
                    Some(Ty::Ref(_, Ty::Struct(sid))) if !m.is_empty() => self.prog.method(*sid, &format!("__ref_{m}")).map(|id| (id, l)),
                    None if !m.is_empty() => match self.resolve(r) {
                        Some(Ty::Ref(_, Ty::Struct(sid))) => self.prog.method(*sid, &format!("__ref_{m}")).map(|id| (id, l)),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some((id, recv)) = found {
                    return self.apply_recv(Some(id), vec![r], recv);
                }
                self.unify(l, r);
                l
            }
        }
    }

    /// `i32::pow(2, 10)` / `Trait::m(&x, a)` / `Ord::cmp(&a, &b)` / `HashMap::from([..])` などをメソッド呼び出しなどに
    /// 書き換えたもの (front.rs の prim_method_call と同じ規則)
    fn prim_method_call(&self, f: &Expr, args: &[Expr]) -> Option<Expr> {
        let Expr::Path(None, p) = f else { return None };
        if args.is_empty() {
            return None;
        }
        let pp = PPath::of(p);
        {
            let q = self.prog.strip_mods_p(&pp).unwrap_or(pp.clone());
            if q.segs.len() == 2 && q.segs.iter().all(|s| !s.args) {
                let tr = &q.segs[0].name;
                let ms = &q.segs[1].name;
                if let Some(ds) = self.prog.traits.get(tr) {
                    let selfm = ds
                        .iter()
                        .any(|&d| self.prog.sigs[d as usize].has_self && self.prog.names[d as usize].rsplit("::").next() == Some(ms.as_str()));
                    if selfm && !matches!(tr.as_str(), "Ord" | "PartialOrd" | "PartialEq" | "Eq") {
                        // (a trait with an impl for references to a type parameter: the receiver as ufcs_ref_recv
                        // writes it, whose probe makes Self the path call's (#144))
                        let ref_tr = self.prog.ref_impls_of.contains_key(tr.as_str());
                        let recv = match &args[0] {
                            a if ref_tr => ufcs_ref_recv(a, self.prog.trait_recv_kind(tr, ms).flatten()),
                            Expr::Ref(_, e) => (**e).clone(),
                            a => a.clone(),
                        };
                        return Some(mcall(recv, ms, args[1..].to_vec()));
                    }
                }
            }
        }
        if p.segs.len() != 2 {
            return None;
        }
        let ty = p.segs[0].name.as_str();
        let m = p.segs[1].name.as_str();
        if matches!(ty, "Ord" | "PartialOrd" | "PartialEq" | "Eq") && args.len() == 2 {
            return Some(mcall(args[0].clone(), m, vec![args[1].clone()]));
        }
        let t = Ty::prim(ty)?;
        if t == Ty::Char || t == Ty::Bool {
            return None;
        }
        if m == "from" && args.len() == 1 && self.fn_by(p).is_none() {
            return Some(Expr::Paren(Box::new(Expr::Cast(Box::new(Expr::Paren(Box::new(args[0].clone()))), Box::new(PTy::Path(None, path1(ty)))))));
        }
        if m.starts_with("from") || m.starts_with("try_from") || m == "default" || m == "new" {
            return None;
        }
        if self.fn_by(p).is_some() {
            return None;
        }
        let recv = match &args[0] {
            Expr::Lit(_, _) => Expr::Cast(Box::new(args[0].clone()), Box::new(PTy::Path(None, path1(ty)))),
            a => a.clone(),
        };
        Some(mcall(recv, m, args[1..].to_vec()))
    }

    fn expr_inner(&mut self, e: &Expr) -> u32 {
        match e {
            Expr::Range(a, b, incl) => {
                let name = if *incl {
                    "RangeInclusive"
                } else if b.is_none() {
                    "RangeFrom"
                } else {
                    "Range"
                };
                let el = self.var();
                for x in a.iter().chain(b.iter()) {
                    let xv = self.expr(x);
                    self.unify(el, xv);
                }
                match self.prog.struct_index.get(name) {
                    Some(&b) => self.adt(b, vec![el]),
                    None => self.var(),
                }
            }
            Expr::Macro(p, g, sp) => {
                let saved = std::mem::replace(&mut self.mbase, sp.at);
                let r = self.macro_expr(p, g);
                self.mbase = saved;
                r
            }
            Expr::Lit(t, _) => self.lit_var(t),
            Expr::Paren(x) => self.expr(x),
            Expr::Try(x) => {
                let v = self.expr(x);
                let (v, _) = self.auto_deref(v, None);
                let r = self.find(v);
                let base = match self.shape[r as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(self.prog.base_of(sid)),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                match base {
                    // (戻り値の型がまだ決まっていない (クロージャ): `Ok(a? + b?)` の Result のエラー型は `?` のものとする)
                    Some(b) if self.prog.structs[b as usize].name == "Result" => {
                        let vars = self.adt_view(v, b);
                        let ret0 = self.ret;
                        let r = self.find(ret0);
                        let unknown = match self.shape[r as usize] {
                            None => true,
                            Some(Shape::Adt(rb, st, len)) if rb == b => {
                                let e = self.adt_vars_of(st, len)[1];
                                let fe = self.find(e);
                                self.shape[fe as usize].is_none()
                            }
                            _ => false,
                        };
                        if unknown {
                            let ret = self.ret;
                            let rv = self.adt_view(ret, b);
                            self.unify(rv[1], vars[1]);
                        }
                        vars[0]
                    }
                    Some(b) if self.prog.structs[b as usize].name == "Option" => self.adt_view(v, b)[0],
                    _ => self.var(),
                }
            }
            Expr::Tuple(xs) if xs.is_empty() => self.known(Ty::Unit),
            Expr::Tuple(xs) => {
                let base = self.prog.tuple_base(xs.len());
                let mut args = vec![];
                for (i, x) in xs.iter().enumerate() {
                    let v = self.expr(x);
                    args.push((i.to_string(), v));
                }
                self.adt_ctor(base, None, args)
            }
            Expr::Array(xs) => {
                let el = self.var();
                for x in xs {
                    let v = self.expr(x);
                    self.unify(el, v);
                }
                self.with(Shape::Arr(el, xs.len() as u32))
            }
            Expr::Repeat(x, n) => {
                let el = self.expr(x);
                let n = match &**n {
                    Expr::Lit(t, _) if num_suffix(t).1.is_empty() && !num_suffix(t).0 => t.replace('_', "").parse::<u32>().ok(),
                    Expr::Lit(t, _) => {
                        let digits: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '_').filter(|&c| c != '_').collect();
                        digits.parse::<u32>().ok()
                    }
                    _ => None,
                };
                match n {
                    Some(n) => self.with(Shape::Arr(el, n)),
                    None => self.var(),
                }
            }
            Expr::Index(base, ix, _) => self.index(base, ix),
            Expr::For(_, p, it, body) => self.for_loop(p, it, body),
            Expr::Block(_, _, b) => self.block(b),
            // (`T::BYTES` with T a type parameter: the type the impls' `BYTES` all have, `cur.add(2 * V::BYTES)`)
            Expr::Path(None, p)
                if p.segs.len() == 2 && (self.env.params.iter().any(|(g, _)| *g == p.segs[0].name) || p.segs[0].name == "Self") && self.prog.const_by_last.contains_key(&p.segs[1].name)
                    && self.variant_by(p).is_none() =>
            {
                let cs = &self.prog.const_by_last[&p.segs[1].name];
                let t = self.prog.consts[cs[0] as usize].ty;
                if t.is_int() && cs.iter().all(|&c| self.prog.consts[c as usize].ty == t) {
                    self.known(t)
                } else {
                    self.var()
                }
            }
            Expr::Path(None, p)
                if p.segs.len() >= 2
                    || (p.segs.len() == 1 && self.lookup(&p.segs[0].name).is_none() && self.variant_by(p).is_some()) =>
            {
                match self.variant_by(p) {
                    Some((sid, vi)) => {
                        let v = self.adt_ctor(sid, Some(vi), vec![]);
                        if let Some(t) = self.variant_turbofish(p, sid) {
                            self.unify_written(v, t);
                        }
                        v
                    }
                    None => {
                        let segs: Vec<&str> = p.segs.iter().map(|s| s.name.as_str()).collect();
                        match Ty::from_suffix(segs[0]).or(if segs[0] == "char" { Some(Ty::Char) } else { None }) {
                            Some(_) if segs.len() == 2 && segs[1] == "BITS" => self.known(Ty::U32),
                            Some(t) if segs.len() == 2 => self.known(t),
                            // (`std::marker::PhantomData` / `m::S`: a unit struct through a module path)
                            _ => match self.prog.struct_index.get(crate::types::std_unit_name(&segs)).copied() {
                                Some(sid)
                                    if !self.prog.structs[sid as usize].is_enum
                                        && self.prog.structs[sid as usize].fields.is_empty()
                                        && p.segs.iter().all(|s| s.args.is_empty()) =>
                                {
                                    self.adt_ctor(sid, None, vec![])
                                }
                                _ => self.var(),
                            },
                        }
                    }
                }
            }
            Expr::Match(x, arms) => {
                let sv = self.expr(x);
                let r = self.var();
                for arm in arms {
                    self.scopes.push(vec![]);
                    self.pat(&arm.pat, sv, None);
                    if let Some(g) = &arm.guard {
                        self.expr(g);
                    }
                    let t = self.expr(&arm.body);
                    self.unify(r, t);
                    self.scopes.pop();
                }
                r
            }
            Expr::Let(p, x) => {
                let sv = self.expr(x);
                self.pat(p, sv, None);
                self.known(Ty::Bool)
            }
            Expr::Path(None, p) if ident(p).is_some_and(|id| self.lookup(id).is_none() && self.prog.const_index.contains_key(id)) => {
                let ci = self.prog.const_index[ident(p).unwrap()];
                let t = self.prog.consts[ci as usize].ty;
                self.known(t)
            }
            Expr::Path(None, p) => match ident(p).and_then(|id| self.lookup(id)) {
                Some(v) => v,
                None if ident(p).is_some_and(|id| crate::program::const_generic_value(self.env, id).is_some()) => self.known(Ty::Usize),
                // (a unit struct as a value: `T`)
                None if self.struct_by(p).is_some_and(|sid| {
                    let sd = &self.prog.structs[sid as usize];
                    !sd.is_enum && sd.fields.is_empty() && self.fn_by(p).is_none()
                }) =>
                {
                    let sid = self.struct_by(p).unwrap();
                    self.adt_ctor(sid, None, vec![])
                }
                None => match self.fn_by(p) {
                    Some(id)
                        if !self.prog.sigs[id as usize].is_generic()
                            && !self.prog.const_index.contains_key(&p.segs.last().unwrap().name) =>
                    {
                        let s = &self.prog.sigs[id as usize];
                        let f = intern_fn(FnTy { params: s.params.clone(), ret: s.ret });
                        self.known(Ty::Fn(f))
                    }
                    // (a free generic fn with every type argument written (`id::<&'static u32>`): its instance's
                    // pointer type, so that a call through it knows its arguments' types)
                    Some(id) if self.prog.sigs[id as usize].impl_of.is_none() && self.prog.sigs[id as usize].trait_of.is_none() => {
                        let ng = self.prog.sigs[id as usize].generics.len();
                        let ts: Option<Vec<Ty>> = p.segs.last().map(|sg| sg.args.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None }).map(|t| self.parse_ty(t)).collect()).unwrap_or(None);
                        match ts.filter(|ts| ts.len() == ng && ng > 0 && ts.iter().all(|&t| !self.prog.has_param(t))) {
                            Some(ts) => {
                                let s = &self.prog.sigs[id as usize];
                                let f = intern_fn(FnTy { params: s.params.iter().map(|&t| self.prog.subst(t, &ts)).collect(), ret: self.prog.subst(s.ret, &ts) });
                                self.known(Ty::Fn(f))
                            }
                            None => self.var(),
                        }
                    }
                    _ => self.var(),
                },
            },
            Expr::Unary(op, x, _) => {
                let v = self.expr(x);
                if op == "*" {
                    self.deref_var(v)
                } else {
                    // `-&x` / `!&b`: 数値・bool への参照は中身の型 (impl Neg for &i32)
                    let r = self.find(v);
                    match self.shape[r as usize] {
                        Some(Shape::Ref(_, i)) => {
                            let ri = self.find(i);
                            match self.shape[ri as usize] {
                                Some(Shape::Known(t)) if t.is_int() || t.is_float() || t == Ty::Bool => i,
                                _ => v,
                            }
                        }
                        Some(Shape::Known(Ty::Ref(_, t))) if t.is_int() || t.is_float() || *t == Ty::Bool => self.with(Shape::Known(*t)),
                        _ => v,
                    }
                }
            }
            Expr::Ref(m, x) => {
                let v = self.expr(x);
                self.with(Shape::Ref(*m, v))
            }
            Expr::Field(b, name, _) => {
                let b = self.expr(b);
                self.field(b, name)
            }
            Expr::Struct(p, fs, _) => {
                let mut args = vec![];
                for (n, x) in fs {
                    let v = self.expr(x);
                    args.push((n.clone(), v));
                }
                if let Some((sid, vi)) = self.variant_by(p) {
                    self.adt_ctor(sid, Some(vi), args)
                } else if let Some(sid) = self.struct_by(p) {
                    self.adt_ctor(sid, None, args)
                } else if let Some(sid) = self.struct_by_turbofish(p) {
                    // (`S::<u32> { t: 1 }`: the instance the turbofish writes)
                    let v = self.adt_ctor(sid, None, args);
                    if let Some(t) = self.variant_turbofish(p, sid) {
                        self.unify_written(v, t);
                    }
                    v
                } else {
                    self.var()
                }
            }
            Expr::Binary(op, l, r, _) => self.binary(op, l, r),
            // (構文木の AssignOp の演算子は `=` を除いたもの: `+=` は "+")
            Expr::AssignOp(op, l, r, _) => self.binary(&format!("{op}="), l, r),
            Expr::Assign(l, r) => {
                let l = self.expr(l);
                let r = self.expr(r);
                self.unify(l, r);
                self.known(Ty::Unit)
            }
            Expr::If(c, t, els) => {
                self.scopes.push(vec![]);
                self.expr(c);
                let t = self.block(t);
                self.scopes.pop();
                match els {
                    Some(x) => {
                        let x = self.expr(x);
                        self.unify(t, x);
                        t
                    }
                    None => self.known(Ty::Unit),
                }
            }
            Expr::While(_, c, b) => {
                self.scopes.push(vec![]);
                self.expr(c);
                self.block(b);
                self.scopes.pop();
                self.known(Ty::Unit)
            }
            Expr::Loop(_, b) => {
                self.block(b);
                self.known(Ty::Unit)
            }
            Expr::Return(x, _) => {
                if let Some(x) = x {
                    let v = self.expr(x);
                    let ret = self.ret;
                    self.unify(v, ret);
                }
                self.known(Ty::Never)
            }
            Expr::Break(label, x, _) => {
                let fi = self.brk.iter().rposition(|f| match label {
                    Some(n) => f.0.as_deref() == Some(n.as_str()),
                    None => !f.2,
                });
                let v = match x {
                    Some(x) => Some(self.expr(x)),
                    None => None,
                };
                if let Some(fi) = fi {
                    self.brk[fi].3 = true;
                    let rv = self.brk[fi].1;
                    let v = v.unwrap_or_else(|| self.known(Ty::Unit));
                    self.unify(rv, v);
                }
                self.known(Ty::Never)
            }
            Expr::Continue(..) => self.known(Ty::Never),
            Expr::Call(f, args) => self.call(f, args),
            Expr::Closure(..) => self.closure(e),
            Expr::MethodCall(recv, name, tf, args, _) => self.method_call(recv, name, tf, args),
            Expr::Cast(x, t) => {
                self.expr(x);
                match self.parse_ty(t) {
                    Some(t) => self.known(t),
                    None => self.var(),
                }
            }
            _ => self.var(),
        }
    }

    fn macro_expr(&mut self, p: &Path, g: &str) -> u32 {
        let name = if p.segs.len() == 1 { p.segs[0].name.as_str() } else { "" };
        if name == "format" && self.prog.struct_index.contains_key("String") {
            self.fmt_width_vars(g);
            let ss = self.prog.struct_index["String"];
            return self.known(Ty::Struct(ss));
        }
        if name == "dbg" {
            let Some(xs) = ast_closure::macro_args_at(g, self.mbase) else { return self.var() };
            let Some(x) = xs.into_iter().next() else { return self.var() };
            self.pos_record += 1;
            let v = self.expr_kept(x, false);
            self.pos_record -= 1;
            return v;
        }
        if name == "matches" {
            // (コード生成も同じ構文木を使う: margs に入れて、そのアドレスで型を記録する)
            let k = g.as_ptr() as usize;
            if !self.margs.contains_key(&k) {
                if let Some((x, _, _)) = ast_closure::matches_parts(g, self.mbase) {
                    let ki = self.keep.len();
                    self.keep.push(vec![x]);
                    self.margs.insert(k, ki);
                }
            }
            if let Some(&ki) = self.margs.get(&k) {
                let x: *const Expr = &self.keep[ki][0];
                self.pos_record += 1;
                // SAFETY: keep の要素は推論の終わりまで動かさない
                self.expr(unsafe { &*x });
                self.pos_record -= 1;
            }
            return self.known(Ty::Bool);
        }
        if is_fmt_macro(p) {
            self.fmt_width_vars(g);
            let unit = matches!(macro_name(p), "print" | "println" | "eprint" | "eprintln");
            return if unit { self.known(Ty::Unit) } else { self.var() };
        }
        // (`assert_eq!(6, (0..3).map(|i| x[i]).sum::<u64>())`: both sides are inferred (`i` is a usize from the index),
        // recorded by position for the code generator, which reads the arguments again)
        if matches!(name, "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne") {
            let Some(ki) = self.macro_args_kept(g) else { return self.known(Ty::Unit) };
            if self.keep[ki].len() >= 2 {
                let (a, b): (*const Expr, *const Expr) = (&self.keep[ki][0], &self.keep[ki][1]);
                self.pos_record += 1;
                // SAFETY: keep の要素は推論の終わりまで動かさない
                let va = self.expr(unsafe { &*a });
                let vb = self.expr(unsafe { &*b });
                self.pos_record -= 1;
                // (unified unless both sides are calls: a generic call's result (`foo(&node)` with V from an associated
                // type) would take the other call's integer default (`Some(22)`))
                let simple = |e: &Expr| !matches!(e, Expr::Call(..) | Expr::MethodCall(..));
                // SAFETY: as above
                // (as `==` does: `&[u8]` against `[1, 2, 3]` only makes the elements one type)
                if simple(unsafe { &*a }) || simple(unsafe { &*b }) {
                    self.seq_unify(va, vb);
                }
            }
            return self.known(Ty::Unit);
        }
        // (`assert!(a == e)`: the condition constrains its locals, `let e = 5;` is the type of `a`)
        if name == "assert" || name == "debug_assert" {
            let Some(xs) = ast_closure::macro_args_at(g, self.mbase) else { return self.known(Ty::Unit) };
            let Some(x) = xs.into_iter().next() else { return self.known(Ty::Unit) };
            self.no_record += 1;
            let ki = self.keep.len();
            self.keep.push(vec![x]);
            let x: *const Expr = &self.keep[ki][0];
            // SAFETY: keep の要素は推論の終わりまで動かさない
            let v = self.expr(unsafe { &*x });
            let b = self.known(Ty::Bool);
            self.unify(v, b);
            self.no_record -= 1;
            return self.known(Ty::Unit);
        }
        if name == "vec" && self.prog.struct_index.contains_key("Vec") {
            let el = self.var();
            let parts = split_top(g, ";");
            let xs: Vec<Expr> = if parts.len() == 2 {
                crate::parse::parse_expr(&parts[0]).ok().into_iter().collect()
            } else {
                ast_closure::macro_args_at(g, self.mbase).unwrap_or_default()
            };
            self.no_record += 1;
            let ki = self.keep.len();
            self.keep.push(xs);
            for i in 0..self.keep[ki].len() {
                let x: *const Expr = &self.keep[ki][i];
                let v = self.expr(unsafe { &*x });
                self.unify(el, v);
            }
            self.no_record -= 1;
            let vb = self.prog.struct_index["Vec"];
            return self.adt(vb, vec![el]);
        }
        self.var()
    }

    fn index(&mut self, base: &Expr, ix: &Expr) -> u32 {
        let b = self.expr(base);
        if let Expr::Range(lo, hi, _) = ix {
            for x in lo.iter().chain(hi.iter()) {
                let v = self.expr(x);
                let k = self.known(Ty::Usize);
                self.unify(v, k);
            }
            let d = self.deref_expr(b);
            let r = self.find(d);
            let is_str = match self.shape[r as usize] {
                Some(Shape::Known(Ty::Str)) => true,
                Some(Shape::Known(Ty::Struct(sid))) => {
                    self.prog.structs[sid as usize].base.is_none() && self.prog.structs[sid as usize].name == "String"
                }
                _ => false,
            };
            if is_str {
                return self.known(Ty::Str);
            }
            return match self.elem_of(b) {
                Some(e) => self.with(Shape::Slc(e)),
                None => self.var(),
            };
        }
        let d = self.deref_expr(b);
        let r = self.find(d);
        if let Some(Shape::Known(Ty::Struct(sid))) = self.shape[r as usize] {
            if let Some(id) = self.prog.method(sid, "index") {
                if let Ty::Ref(_, out) = self.prog.sigs[id as usize].ret {
                    if !self.prog.has_param(*out) {
                        let i = self.expr(ix);
                        if let Some(&pt) = self.prog.sigs[id as usize].params.get(1) {
                            if !self.prog.has_param(pt) {
                                let k = self.known(pt);
                                self.unify(i, k);
                            }
                        }
                        return self.known(*out);
                    }
                }
            }
        }
        let i = self.expr(ix);
        let k = self.known(Ty::Usize);
        self.unify(i, k);
        self.elem_of(b).unwrap_or_else(|| self.var())
    }

    fn for_loop(&mut self, p: &Pat, it: &Expr, body: &Block) -> u32 {
        let mut e: &Expr = it;
        let mut adapters: Vec<String> = vec![];
        loop {
            match e {
                Expr::MethodCall(r, n, _, a, _) if a.is_empty() => {
                    adapters.push(n.clone());
                    e = r;
                }
                Expr::Paren(x) => e = x,
                _ => break,
            }
        }
        adapters.reverse();
        let mut iter_item = None;
        let item = if let Expr::Range(lo, hi, _) = e {
            let v = self.var();
            for x in lo.iter().chain(hi.iter()) {
                let xv = self.expr(x);
                self.unify(v, xv);
            }
            v
        } else {
            let (by_ref, base_e) = match e {
                Expr::Ref(m, x) => (Some(*m), &**x),
                _ => (None, e),
            };
            let bv = self.expr(base_e);
            let mut mode = by_ref;
            if mode.is_none() {
                let r = self.find(bv);
                if let Some(Shape::Ref(m, _)) = self.shape[r as usize] {
                    mode = Some(m);
                }
            }
            for a in &adapters {
                match a.as_str() {
                    "iter" => mode = Some(false),
                    "iter_mut" => mode = Some(true),
                    "copied" | "cloned" => mode = None,
                    _ => {}
                }
            }
            match self.elem_of(bv) {
                Some(el) => match mode {
                    Some(m) => self.with(Shape::Ref(m, el)),
                    None => el,
                },
                None => {
                    let mut fv = if adapters.is_empty() { bv } else { self.expr(it) };
                    let user_into = match self.resolve(fv) {
                        Some(Ty::Ref(_, Ty::Struct(sid))) => self.prog.method(*sid, "__ref_into_iter"),
                        Some(Ty::Struct(sid)) if by_ref.is_some() => self
                            .prog
                            .method(sid, "__ref_into_iter")
                            .or_else(|| self.prog.method(sid, if by_ref == Some(true) { "iter_mut" } else { "iter" })),
                        Some(Ty::Struct(sid)) if self.prog.method(sid, "next").is_none() => self.prog.method(sid, "into_iter"),
                        _ => None,
                    };
                    if let Some(id) = user_into {
                        fv = self.apply_recv(Some(id), vec![], fv);
                    }
                    let itv = match self.assoc_var(fv, "Item") {
                        Some(v) => v,
                        None => match self.resolve(fv).and_then(|t| self.prog.assoc_of(t, "Item")) {
                            Some(t) => self.known(t),
                            None => self.var(),
                        },
                    };
                    iter_item = Some(itv);
                    self.var()
                }
            }
        };
        let item = if let Some(itv) = iter_item {
            itv
        } else if adapters.iter().any(|a| a == "enumerate") {
            let base = self.prog.tuple_base(2);
            let u = self.known(Ty::Usize);
            self.adt(base, vec![u, item])
        } else {
            item
        };
        self.scopes.push(vec![]);
        self.pat(p, item, None);
        self.block(body);
        self.scopes.pop();
        self.known(Ty::Unit)
    }

    /// The program defines no fn of p's last name that the path could name (a user's own `size_of`)
    fn fn_by_name_absent(&self, p: &crate::parse::Path) -> bool {
        let Some(n) = p.segs.last().map(|s| s.name.as_str()) else { return true };
        !self.prog.index.contains_key(n) || p.segs.len() >= 2
    }

    fn call(&mut self, f: &Expr, args: &[Expr]) -> u32 {
        // (`u32::max_value()`: the integer type's `MAX`, #265)
        if let Some((t, _)) = crate::parse::int_bound_call(f, args) {
            return self.known(t);
        }
        // (`std::clone::Clone::clone(&x)`, `std::fmt::Write::write_str(&mut s, ..)`: as the call its last two segments
        // make, `Clone::clone(&x)` (#153))
        if let Expr::Path(None, p) = f {
            if let Some(k) = self.prog.std_trait_path_skip(p) {
                let tail = Expr::Path(None, crate::parse::Path { global: false, segs: p.segs[k..].to_vec(), pos: p.pos });
                // (kept to the end of the inference: the call records its callee's address, read after it)
                let ki = self.keep.len();
                self.keep.push(vec![tail]);
                let f: *const Expr = &self.keep[ki][0];
                // SAFETY: keep の要素は推論の終わりまで動かさない (外側の Vec が伸びても内側の要素は動かない)
                return self.call(unsafe { &*f }, args);
            }
        }
        // `HashMap::from([(1, 2), ..])`: 配列を Vec<要素> として `from(Vec)` を呼ぶ (syn 版は `vec![..]` に書き換える)
        if let (Expr::Path(None, p), [Expr::Array(es)]) = (f, args) {
            if p.segs.len() == 2
                && p.segs[1].name == "from"
                && matches!(p.segs[0].name.as_str(), "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque" | "BinaryHeap")
                && self.prog.struct_index.contains_key("Vec")
            {
                let el = self.var();
                self.no_record += 1;
                for x in es {
                    let v = self.expr(x);
                    self.unify(el, v);
                }
                self.no_record -= 1;
                let vb = self.prog.struct_index["Vec"];
                let vv = self.adt(vb, vec![el]);
                let callee = self.fn_by(p);
                return self.apply(callee, vec![vv], 0);
            }
        }
        // (`mem::size_of::<T>()` / `align_of` / `size_of_val(&x)`: usize, the code generator's intrinsics, so a literal
        // beside them (`3 + mem::size_of::<Foo>()`) is a usize too)
        if let Expr::Path(None, p) = f {
            let last = p.segs.last().map(|s| s.name.as_str());
            let mem_path = p.segs.len() == 1 || p.segs.iter().rev().nth(1).is_some_and(|s| s.name == "mem");
            if mem_path && matches!(last, Some("size_of" | "align_of" | "size_of_val" | "align_of_val")) && self.fn_by_name_absent(p) {
                for a in args {
                    self.expr(a);
                }
                return self.known(Ty::Usize);
            }
        }
        if let Some(x) = self.prim_method_call(f, args) {
            // (`A::get(&&s)` of a trait with an impl for references: the method call it becomes is A's, which
            // ref_impl_method keeps to (as the code generator's explicit_traits))
            if !self.prog.ref_dispatch.is_empty() {
                if let Expr::Path(None, p) = f {
                    let pp = PPath::of(p);
                    let q = self.prog.strip_mods_p(&pp).unwrap_or(pp);
                    if q.segs.len() == 2 && self.prog.ref_impls_of.contains_key(q.segs[0].name.as_str()) {
                        self.named_trait = Some((q.segs[1].name.clone(), q.segs[0].name.clone()));
                    }
                }
            }
            self.pos_record += 1;
            let v = self.expr_kept(x, false);
            self.pos_record -= 1;
            return v;
        }
        if let Expr::Path(q, p) = f {
            // `S::default()` / `<S>::default()`: S
            if args.is_empty() && p.segs.last().is_some_and(|s| s.name == "default") {
                let pp = PPath::of(p);
                let k = match self.prog.strip_mods_p(&pp) {
                    Some(t) => p.segs.len() - t.segs.len(),
                    None => 0,
                };
                let rest = &p.segs[k..];
                let ty: Option<PTy> = match q {
                    Some(t) => Some((**t).clone()),
                    None if rest.len() == 2 && rest[0].name != "Default" => {
                        Some(PTy::Path(None, Path { global: false, segs: vec![rest[0].clone()], pos: Default::default() }))
                    }
                    None => None,
                };
                if let Some(t) = ty.and_then(|t| self.parse_ty(&t)).filter(|&t| !self.prog.has_param(t)) {
                    return self.known(t);
                }
            }
            if q.is_none() {
                let segs = &p.segs;
                if segs.len() == 2 && args.len() == 1 && matches!(segs[1].name.as_str(), "from_le_bytes" | "from_be_bytes" | "from_ne_bytes") {
                    if let Some(t) = Ty::prim(&segs[0].name).filter(|t| t.is_int()) {
                        let a = self.expr(&args[0]);
                        let k = self.known(Ty::Array(intern(Ty::U8), t.bits() / 8));
                        self.unify(a, k);
                        return self.known(t);
                    }
                }
                // `u32::from_be(x)` / `from_le`
                if segs.len() == 2 && args.len() == 1 && matches!(segs[1].name.as_str(), "from_be" | "from_le") {
                    if let Some(t) = Ty::prim(&segs[0].name).filter(|t| t.is_int()) {
                        let a = self.expr(&args[0]);
                        let k = self.known(t);
                        self.unify(a, k);
                        return self.known(t);
                    }
                }
                // `core::arch::wasm32`'s SIMD intrinsics (`f32x4_add(a, b)`; acg lowers them in place)
                let last = &segs[segs.len() - 1].name;
                // (not a local of that name (a closure, a fn pointer), nor the program's function)
                let local = ident(p).is_some_and(|id| self.lookup(id).is_some());
                let arch = crate::simd_sig::arch_path(segs[..segs.len() - 1].iter().map(|s| s.name.as_str()), last);
                // (nor a tuple struct or variant of the program's: `struct f32x4(u64, u64, u64, u64)`)
                let ctor = self.variant_by(p).is_some() || self.struct_by(p).is_some();
                // (nor a constant or static of the program's: `const _mm_add_ps: fn(i32, i32) -> i32 = add;`)
                let konst = segs.len() == 1 && self.prog.const_index.contains_key(last.as_str());
                if crate::simd_sig::maybe_simd(last) && arch && !local && !ctor && !konst && self.fn_by(p).is_none() {
                    if let Some((ps, r)) = crate::simd_sig::simd_sig(last).filter(|(ps, _)| ps.len() == args.len()) {
                        for (a, k) in args.iter().zip(ps) {
                            let av = self.expr(a);
                            match k {
                                crate::simd_sig::SArg::V(w) => {
                                    let kv = self.known(Ty::Vec(w));
                                    self.unify(av, kv);
                                }
                                crate::simd_sig::SArg::S(t) => {
                                    let kv = self.known(t);
                                    self.unify(av, kv);
                                }
                                crate::simd_sig::SArg::R(t) => {
                                    let kv = self.known(Ty::Ref(false, intern(t)));
                                    self.unify(av, kv);
                                }
                                // (a raw pointer: acg checks it, with rustc's coercions. A `p.cast()` gets its type from it:
                                // `v128_load(data.cast())`)
                                crate::simd_sig::SArg::P(t, m) => {
                                    let mut inner: &Expr = a;
                                    while let Expr::Paren(x) = inner {
                                        inner = x;
                                    }
                                    if matches!(inner, Expr::MethodCall(_, n, tf, xs, _) if n == "cast" && tf.is_empty() && xs.is_empty()) {
                                        let kv = self.known(Ty::Ptr(m, intern(t)));
                                        self.unify(av, kv);
                                    }
                                }
                            }
                        }
                        return self.known(r);
                    }
                }
                // `f64::from_bits(u)`
                if segs.len() == 2 && args.len() == 1 && segs[1].name == "from_bits" && matches!(segs[0].name.as_str(), "f64" | "f32") {
                    let a = self.expr(&args[0]);
                    let k = self.known(if segs[0].name == "f64" { Ty::U64 } else { Ty::U32 });
                    self.unify(a, k);
                    return self.known(if segs[0].name == "f64" { Ty::F64 } else { Ty::F32 });
                }
                if let Some((sid, vi)) = self.variant_by(p) {
                    let mut xs = vec![];
                    for (i, a) in args.iter().enumerate() {
                        let v = self.expr(a);
                        xs.push((i.to_string(), v));
                    }
                    // (`Cow::Borrowed(s)` / `Cow::Owned(s)` of a `&str` / `String`: prelude's `__CowStr`, as the
                    // code generator reads it; prelude's `Cow<T>` stands for `Cow<[T]>`)
                    if let (Some(&cow), Some(&cs), Some(&(_, x))) = (self.prog.struct_index.get("Cow"), self.prog.struct_index.get("__CowStr"), xs.first()) {
                        if self.prog.base_of(sid) == cow {
                            let string = self.prog.struct_index.get("String").copied();
                            if matches!(self.resolve(x), Some(Ty::Ref(_, Ty::Str))) || self.resolve(x).is_some_and(|t| Some(t) == string.map(Ty::Struct)) {
                                return self.adt_ctor(cs, Some(vi), xs);
                            }
                        }
                    }
                    let v = self.adt_ctor(sid, Some(vi), xs);
                    // (`SomeEnum::SomeVariant::<&u32>(&c)`: the instance the turbofish writes)
                    if p.segs.iter().any(|s| !s.args.is_empty()) {
                        if let Some(t) = self.variant_turbofish(p, sid) {
                            self.unify_written(v, t);
                        }
                    }
                    return v;
                }
                let local = ident(p).is_some_and(|id| self.lookup(id).is_some());
                if !local && self.fn_by(p).is_none() {
                    let plain = self.struct_by(p);
                    if let Some(sid) = plain.or_else(|| self.struct_by_turbofish(p)) {
                        let mut xs = vec![];
                        for (i, a) in args.iter().enumerate() {
                            let v = self.expr(a);
                            xs.push((i.to_string(), v));
                        }
                        let v = self.adt_ctor(sid, None, xs);
                        // (`SomeStruct::<&u32>(&c)`: the instance the turbofish writes)
                        if plain.is_none() {
                            if let Some(t) = self.variant_turbofish(p, sid) {
                                self.unify_written(v, t);
                            }
                        }
                        return v;
                    }
                }
            }
        }
        let callee = match f {
            Expr::Path(None, p) if ident(p).is_some_and(|id| self.lookup(id).is_some()) => None,
            Expr::Path(None, p) => self.fn_by(p),
            _ => None,
        };
        let mut avs = vec![];
        for a in args {
            avs.push(self.expr(a));
        }
        if let Expr::Path(None, p) = f {
            let n = p.segs.len();
            if callee.is_none() && n >= 2 && p.segs[n - 1].name == "new" && p.segs[n - 2].name == "Box" && avs.len() == 1 {
                return self.with(Shape::Bx(avs[0]));
            }
        }
        // (`tr::<u16>()`: the turbofish gives the callee's own type parameters, as the code generator reads them: a free
        // fn's in order, a member's the last ones; an `impl Trait` argument's hidden parameter is not named)
        if let (Some(id), Expr::Path(None, p)) = (callee, f) {
            // (every argument but a lifetime has its slot: a const one (`f::<3, u8>`) is skipped but counted, also a
            // `true` / `false` written as a type)
            let tys: Vec<Option<&PTy>> = p.segs.last().map_or(vec![], |s| {
                s.args
                    .iter()
                    .filter(|a| !matches!(a, GArg::Lifetime(_) | GArg::Bind(..)))
                    .map(|a| match a {
                        GArg::Ty(t) if crate::ast_types::bool_arg(t).is_none() => Some(t),
                        _ => None,
                    })
                    .collect()
            });
            if tys.iter().any(|t| t.is_some()) {
                let sig = &self.prog.sigs[id as usize];
                // (the hidden `impl Trait` parameters are `__impl` and digits, as the lowering's hidden_param has it)
                let named: Vec<usize> = (0..sig.generics.len())
                    .filter(|&k| {
                        let n: &str = &sig.generics[k].0;
                        !(n.starts_with("__impl") && n.len() > 6 && n.as_bytes()[6..].iter().all(|c| c.is_ascii_digit()))
                    })
                    .collect();
                let member = sig.self_ty.is_some() || sig.impl_of.is_some();
                let start = if member { named.len().saturating_sub(tys.len()) } else { sig.trait_of.is_some() as usize };
                let mut pre = vec![];
                for (i, t) in tys.into_iter().enumerate() {
                    let Some(t) = t else { continue };
                    if let Some(&k) = named.get(start + i) {
                        let v = self.ty_var(t);
                        pre.push((k, v));
                    }
                }
                self.pre_pv = pre;
            }
        }
        // (a parameter that is a fn pointer over the callee's type parameters, `f: fn(T) -> T`, and an argument that
        // is a fn item that is not generic: its signature gives them, `spawn(1 + 40, double)` with `fn double(isize)`)
        if let Some(id) = callee {
            let sig = &self.prog.sigs[id as usize];
            let mut sol: Vec<Option<Ty>> = vec![None; sig.generics.len()];
            let mut any = false;
            for (a, &pt) in args.iter().zip(&sig.params) {
                if !matches!(pt, Ty::Fn(_)) || !self.prog.has_param(pt) {
                    continue;
                }
                let Expr::Path(None, ap) = a else { continue };
                if ident(ap).is_some_and(|n| self.lookup(n).is_some()) {
                    continue;
                }
                let Some(fid) = self.fn_by(ap) else { continue };
                let fs = &self.prog.sigs[fid as usize];
                if !fs.generics.is_empty() {
                    continue;
                }
                let ft = Ty::Fn(intern_fn(crate::types::FnTy { params: fs.params.clone(), ret: fs.ret }));
                any |= self.prog.match_ty(pt, ft, &mut sol);
            }
            if any {
                let known: Vec<(usize, Ty)> = sol.iter().enumerate().filter_map(|(k, t)| t.filter(|t| !self.prog.has_param(*t)).map(|t| (k, t))).collect();
                for (k, t) in known {
                    if !self.pre_pv.iter().any(|x| x.0 == k) {
                        let v = self.known(t);
                        self.pre_pv.push((k, v));
                    }
                }
            }
        }
        if callee.is_none() {
            let fv = self.expr(f);
            return match self.callable(fv, avs.len()) {
                Some((ps, r)) => {
                    for (a, p) in avs.into_iter().zip(ps) {
                        self.unify(a, p);
                    }
                    r
                }
                None => self.var(),
            };
        }
        // (`id::<u32>(c)`: the turbofish's arguments are a free fn's generics, in order, as the lowering takes them
        // (turbofish_sol). Its types seed the inference; a const argument (`take::<u8, 1>`, `f::<true>()`) is not a
        // type, and its slot is left to the lowering)
        if let (Some(id), Expr::Path(None, p)) = (callee, f) {
            let sig = &self.prog.sigs[id as usize];
            let free = sig.self_ty.is_none() && sig.impl_of.is_none() && sig.trait_of.is_none();
            let args: Vec<&GArg> = p.segs.last().map(|s| s.args.iter().filter(|a| !matches!(a, GArg::Lifetime(_))).collect()).unwrap_or_default();
            let ng = sig.generics.len();
            if free && !args.is_empty() && args.len() <= ng && args.iter().any(|a| matches!(a, GArg::Ty(t) if crate::ast_types::bool_arg(t).is_none())) {
                let mut pre = vec![None; ng];
                for (k, a) in args.iter().enumerate() {
                    if let GArg::Ty(t) = a {
                        if crate::ast_types::bool_arg(t).is_none() {
                            pre[k] = Some(self.ty_var(t));
                        }
                    }
                }
                return self.apply_with(callee, avs, 0, None, pre);
            }
        }
        self.apply(callee, avs, 0)
    }

    fn closure(&mut self, c: &Expr) -> u32 {
        let Expr::Closure(_, ps, ret, _, _) = c else { return self.var() };
        let st = self.adt_args.len() as u32;
        let n = ps.len() as u32;
        let mut vs = vec![];
        for (_, t) in ps {
            let v = match t {
                Some(t) => match self.parse_ty(t) {
                    Some(t) => self.known(t),
                    None => self.var(),
                },
                None => self.var(),
            };
            vs.push(v);
        }
        let rv = match ret {
            Some(t) => match self.parse_ty(t) {
                Some(t) => self.known(t),
                None => self.var(),
            },
            None => self.var(),
        };
        let all: Vec<u32> = vs.iter().copied().chain([rv]).collect();
        self.adt_args.extend(all);
        // (クロージャの struct を後で解けるように、キャプチャの型変数を覚えておく)
        if self.clo_ctx.is_none() {
            if let Some((r, base, b)) = self.clo_root.take() {
                let mut m = crate::fxhash::FxMap::default();
                let mut keys = crate::fxhash::FxMap::default();
                // SAFETY: b は推論の間生きている関数の本体を指す
                // (本体に書かれたクロージャはアドレスで、マクロの引数の中のものは式の文字列で引く)
                for (i, (a, t)) in ast_closure::closure_slots(unsafe { &*b }).into_iter().enumerate() {
                    if let Some(a) = a {
                        keys.insert(format!("@{a}"), base + i as u32);
                    }
                    keys.entry(t).or_insert(base + i as u32);
                }
                let _ = &mut m;
                self.clo_keys = Some(keys);
                self.clo_ctx = Some((r, m));
            }
        }
        let k = self.clo_keys.as_ref().and_then(|ks| ks.get(&format!("@{}", c as *const Expr as usize)).or_else(|| ks.get(&ast_closure::closure_key(c))).copied());
        if let (Some((root, _)), Some(k)) = (&self.clo_ctx, k) {
            if let Some(&fid) = self.prog.closure_by.get(&(*root, k)) {
                if let Some(cd) = self.prog.closures.get(&fid) {
                    let caps: Option<Vec<u32>> = cd.captures.iter().map(|n| self.lookup(n)).collect();
                    let free: Vec<usize> = ps.iter().enumerate().filter(|(_, p)| p.1.is_none()).map(|(i, _)| i).collect();
                    if let Some(caps) = caps {
                        self.clo_info.insert(st, (fid, caps, free));
                    }
                }
            }
        }
        if self.no_record == 0 || self.pos_record > 0 {
            for ((p, _), &v) in ps.iter().zip(&vs) {
                self.lets.push((p as *const Pat as usize, v));
            }
            self.lets.push((c as *const Expr as usize, rv));
        }
        if self.defer > 0 {
            self.pending.push((c as *const Expr, vs, rv));
        } else {
            self.closure_body(c, &vs, rv);
        }
        self.with(Shape::Clo(st, n))
    }

    /// Of the inherent methods with `self` overloading `id` (Program's overloads: impls for other instances of the
    /// type), the one whose self type the receiver `rv` fits; `id` when it has none, None when the receiver does not
    /// tell them apart yet
    fn pick_inherent_overload(&mut self, id: u32, rv: u32) -> Option<u32> {
        let p = self.prog;
        // (by the group's name: self_method may give an overload, `W::f#1`)
        let name = &p.names[id as usize];
        let Some(group) = p.overloads.get(name.split('#').next().unwrap_or(name)) else { return Some(id) };
        let inh: Vec<u32> = group.iter().copied().filter(|&c| p.sigs[c as usize].has_self && p.sigs[c as usize].impl_trait.is_none() && p.sigs[c as usize].self_ty.is_some()).collect();
        if inh.len() < 2 || !inh.contains(&id) {
            // (impls of one trait for instances of the type (the prelude's `impl Read for Cursor<Vec<u8>>` and `.. for
            // Cursor<&[u8]>`): the one the receiver is, once it is known; else the first, as before)
            let of_trait: Vec<u32> = group.iter().copied().filter(|&c| p.sigs[c as usize].has_self && p.sigs[c as usize].impl_trait.is_some() && p.sigs[c as usize].self_ty.is_some()).collect();
            if of_trait.len() >= 2 && of_trait.contains(&id) {
                if let Some(t) = self.resolve(rv) {
                    let fit: Vec<u32> = of_trait
                        .iter()
                        .copied()
                        .filter(|&c| {
                            let sig = &p.sigs[c as usize];
                            let mut sol = vec![None; sig.generics.len()];
                            sig.self_ty.is_some_and(|st| p.match_ty(st, t, &mut sol) && p.arg_refs_agree(st, t))
                        })
                        .collect();
                    if let [c] = fit.as_slice() {
                        return Some(*c);
                    }
                }
            }
            return Some(id);
        }
        let Some(t) = self.resolve(rv) else { return None };
        let fit: Vec<u32> = inh
            .iter()
            .copied()
            .filter(|&c| {
                let sig = &p.sigs[c as usize];
                let mut sol = vec![None; sig.generics.len()];
                sig.self_ty.is_some_and(|st| p.match_ty(st, t, &mut sol) && p.arg_refs_agree(st, t))
            })
            .collect();
        match fit.as_slice() {
            [c] => Some(*c),
            _ => None,
        }
    }

    /// The declaration of method n that the bound of placeholder ps declares, with the type arguments of its
    /// signature's generics the bound gives: `Self` the placeholder, the trait's parameters the bound's arguments
    /// (when it has one bound of the trait). None where no bound's trait declares it
    fn placeholder_decl(&self, ps: u32, n: &str) -> Option<(u32, Vec<Option<Ty>>)> {
        let bs = self.prog.structs[ps as usize].placeholder.clone().unwrap_or_default();
        for b in &bs {
            let tr = b.split('<').next().unwrap_or(b).trim();
            let tail = format!("::{tr}");
            let Some(ds) = self.prog.traits.get(tr).or_else(|| self.prog.traits.iter().find(|(k, _)| k.ends_with(&tail)).map(|(_, v)| v)) else { continue };
            let Some(d) = ds.iter().copied().find(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(n) && self.prog.sigs[d as usize].has_self) else { continue };
            let mut sol: Vec<Option<Ty>> = vec![None; self.prog.sigs[d as usize].generics.len()];
            if let Some(s0) = sol.first_mut() {
                *s0 = Some(Ty::Struct(ps));
            }
            // (the bound's arguments, where it is the only bound of its trait)
            let same = bs.iter().filter(|x| x.split('<').next().unwrap_or(x).trim() == tr).count();
            if same == 1 && b.contains('<') {
                if let Ok(f) = crate::parse::parse_file(&format!("type __B = dyn {b};")) {
                    if let Some(crate::parse::ItemKind::Type(_, _, _, Some(PTy::Dyn(dbs)))) = f.items.first().map(|i| &i.kind) {
                        if let Some(crate::parse::Bound::Trait(_, p)) = dbs.first() {
                            let args: Vec<&PTy> = p.segs.last().map(|s| s.args.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None }).collect()).unwrap_or_default();
                            for (i, t) in args.into_iter().enumerate() {
                                if let (Some(slot), Some(t)) = (sol.get_mut(1 + i), self.parse_ty(t)) {
                                    *slot = Some(t);
                                }
                            }
                        }
                    }
                }
            }
            return Some((d, sol));
        }
        None
    }

    /// A method n with `self` of an impl for `Box<dyn tr>` (`impl MyTrait for Box<dyn O>`), where the receiver r is
    /// such a Box (also behind references)
    fn boxed_dyn_method(&mut self, r: u32, tr: &'static String, n: &str) -> Option<u32> {
        let mut x = self.find(r);
        for _ in 0..8 {
            match self.shape[x as usize] {
                Some(Shape::Ref(_, i)) => x = self.find(i),
                Some(Shape::Bx(_)) => {
                    let t = Ty::Box(crate::types::intern(Ty::Dyn(tr)));
                    return self.prog.prim_method(t, n).filter(|&d| self.prog.sigs[d as usize].has_self);
                }
                _ => return None,
            }
        }
        None
    }

    fn method_call(&mut self, recv: &Expr, n: &str, tf: &Vec<GArg>, args: &[Expr]) -> u32 {
        // (this call's, not one in its receiver or arguments)
        let named = self.named_trait.take().filter(|(m, _)| m == n).map(|(_, t)| t);
        // (a method of an impl for references to a type parameter that rustc's probe takes: that impl's fn, or the
        // trait's default with Self the reference, as the code generator's ref_impl_pick (#144))
        if !self.prog.ref_dispatch.is_empty() && self.prog.ref_dispatch.contains_key(n) {
            if let Some(r) = self.ref_impl_method(recv, n, tf, args, named.as_deref()) {
                return r;
            }
            let r = self.method_call_rest(recv, n, tf, args);
            self.pre_recv = None;
            return r;
        }
        self.method_call_rest(recv, n, tf, args)
    }

    /// method_call's for a method in Program::ref_dispatch: the probe's walk (Program::ref_impl_walk) of the receiver's
    /// type, of a type parameter as the generic body writes it (gen_ty) or of a type of its own. None where it takes
    /// another method, method_call_rest's (the receiver inferred is kept for it, pre_recv)
    #[cold]
    #[inline(never)]
    fn ref_impl_method(&mut self, recv: &Expr, n: &str, tf: &Vec<GArg>, args: &[Expr], named: Option<&str>) -> Option<u32> {
        let prog = self.prog;
        let all = prog.ref_dispatch.get(n)?;
        let r = self.expr(recv);
        self.pre_recv = Some((recv as *const Expr as usize, r));
        let t = self.resolve(r).filter(|t| !prog.has_param(*t))?;
        let g = if self.clo_depth == 0 { self.gen_ty(recv, 0) } else { None };
        let (mut param, mut k) = (None, 0usize);
        if let Some(mut g) = g {
            while let Ty::Ref(_, i) = g {
                g = *i;
                k += 1;
            }
            if let Ty::Param(i) = g {
                param = Some(i as usize);
            }
        }
        let gens = self.gen_root.and_then(|r| prog.sigs.get(r as usize)).map(|s| s.generics.as_slice()).unwrap_or(&[]);
        let (f, derefs, autoref) = prog.ref_impl_walk(all, t, n, param, k, gens, named, &|x| prog.ref_own_method(x, n))?;
        // (Self: the receiver through its derefs and the autoref, or what that points to for a `&self` / `&mut self`)
        let mut st = t;
        for _ in 0..derefs {
            st = match st {
                Ty::Ref(_, i) => *i,
                _ => return None,
            };
        }
        if let Some(m) = autoref {
            st = Ty::Ref(m, intern(st));
        }
        if matches!(prog.recv_code(f), 1 | 2) {
            st = match st {
                Ty::Ref(_, i) => *i,
                _ => return None,
            };
        }
        self.pre_recv = None;
        let avs: Vec<u32> = args.iter().map(|a| self.expr(a)).collect();
        let sv = self.known(st);
        self.turbofish = if tf.is_empty() { None } else { Some(tf as *const Vec<GArg>) };
        let r = self.apply_recv(Some(f), avs, sv);
        self.turbofish = None;
        Some(r)
    }

    /// The type of e as the generic body writes it (the code generator's gen_ty): a binding gen_vars has, through `&`,
    /// `*` and the fields of such a value
    fn gen_ty(&self, e: &Expr, depth: u32) -> Option<Ty> {
        if depth > 8 {
            return None;
        }
        match e {
            Expr::Paren(i) => self.gen_ty(i, depth + 1),
            Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => {
                let v = self.lookup(&p.segs[0].name)?;
                self.gen_vars.iter().rev().find(|g| g.0 == v).map(|g| g.1)
            }
            Expr::Unary(op, i, _) if op == "*" => match self.gen_ty(i, depth + 1)? {
                Ty::Ref(_, t) => Some(*t),
                _ => None,
            },
            Expr::Ref(m, i) => self.gen_ty(i, depth + 1).map(|t| Ty::Ref(*m, intern(t))),
            Expr::Field(b, f, _) => {
                let mut t = self.gen_ty(b, depth + 1)?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return None };
                let sd = &self.prog.structs[sid as usize];
                if let Some(x) = sd.fields.iter().find(|x| x.0 == *f) {
                    return Some(x.1);
                }
                // (an instance whose fields are not laid out: the definition's field, a type parameter of the
                // definition under references, as the code generator's)
                let base = &self.prog.structs[sd.base? as usize];
                let ft = base.fields.iter().find(|x| x.0 == *f)?.1;
                let mut refs = vec![];
                let mut b = ft;
                while let Ty::Ref(m, i) = b {
                    refs.push(m);
                    b = *i;
                }
                let Ty::Param(k) = b else { return None };
                let mut out = *sd.args.get(k as usize)?;
                for m in refs.into_iter().rev() {
                    out = Ty::Ref(m, intern(out));
                }
                Some(out)
            }
            _ => None,
        }
    }

    /// A `let`'s annotation as the generic body writes it (the code generator's gen_ann): a type parameter of
    /// gen_root's under references
    fn gen_ann(&self, t: &PTy) -> Option<Ty> {
        let gens = &self.prog.sigs.get(self.gen_root? as usize)?.generics;
        match t {
            PTy::Ref(_, m, i) => Some(Ty::Ref(*m, intern(self.gen_ann(i)?))),
            PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => gens.iter().position(|g| g.0 == p.segs[0].name).map(|i| Ty::Param(i as u8)),
            _ => None,
        }
    }

    fn method_call_rest(&mut self, recv: &Expr, n: &str, tf: &Vec<GArg>, args: &[Expr]) -> u32 {
        // (a Vec's `as_ref()` / `as_mut()`: `&[T]` / `&mut [T]` where the bounds in scope ask for slices alone, as the
        // code generator's (a Vec is AsRef<Vec<T>> / AsMut<Vec<T>> too))
        if matches!(n, "as_ref" | "as_mut") && args.is_empty() {
            let bs = if n == "as_ref" { &self.asref } else { &self.asmut };
            if !bs.is_empty() && bs.iter().all(|b| b.starts_with('[')) {
                let r = self.expr(recv);
                let rv = self.deref_expr(r);
                let root = self.find(rv);
                let vec = match self.shape[root as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => self.is_vec(self.prog.base_of(sid)),
                    Some(Shape::Adt(b, _, _)) => self.is_vec(b),
                    _ => false,
                };
                if let Some(el) = self.elem_of(r).filter(|_| vec) {
                    let sl = self.with(Shape::Slc(el));
                    return self.with(Shape::Ref(n == "as_mut", sl));
                }
            }
        }
        // (`s.as_ref()` on a `&str` / `String` (also through references, `&&str` of an `AsRef<str>` instance): `&str`,
        // or `&[u8]` where the bounds in scope ask for `AsRef<[u8]>` alone, as the code generator's as_ref gives)
        if n == "as_ref" && args.is_empty() {
            let r = self.expr(recv);
            let rv = self.deref_expr(r);
            let string = self.prog.struct_index.get("String").copied();
            let text = match self.resolve(rv) {
                Some(Ty::Str) => true,
                Some(Ty::Ref(_, t)) => matches!(Self::strip_refs_ty(*t), Ty::Str),
                Some(Ty::Struct(sid)) => Some(sid) == string,
                _ => false,
            };
            if text {
                let bytes = self.asref.iter().any(|t| t == "[u8]") && !self.asref.iter().any(|t| t == "str");
                let t = if bytes { Ty::Slice(intern(Ty::U8)) } else { Ty::Str };
                return self.known(Ty::Ref(false, intern(t)));
            }
        }
        // 値としての `[a, b].into_iter()`
        if n == "into_iter" && args.is_empty() && self.prog.struct_index.contains_key("IntoIter") {
            if let Expr::Array(xs) = recv {
                let el = self.var();
                for x in xs {
                    let v = self.expr(x);
                    self.unify(el, v);
                }
                let ib = self.prog.struct_index["IntoIter"];
                return self.adt(ib, vec![el]);
            }
        }
        // `v.get(a..b)`: Option<&[T]>
        if (n == "get" || n == "get_mut") && matches!(args.first(), Some(Expr::Range(..))) && {
            let r = self.expr(recv);
            self.elem_of(r).is_some()
        } {
            let r = self.expr(recv);
            let el = self.elem_of(r).unwrap();
            if let Some(Expr::Range(lo, hi, _)) = args.first() {
                for x in lo.iter().chain(hi.iter()) {
                    let v = self.expr(x);
                    let k = self.known(Ty::Usize);
                    self.unify(v, k);
                }
            }
            let sl = self.with(Shape::Slc(el));
            let rr = self.with(Shape::Ref(n == "get_mut", sl));
            return self.option_of(rr);
        }
        if self.expr_peek_elem(recv).is_some() {
            return self.seq_method(recv, n, args);
        }
        if is_builtin_scalar_method(n) {
            let r = self.expr(recv);
            let rv = self.deref_expr(r);
            let mut avs = vec![];
            for a in args {
                avs.push(self.expr(a));
            }
            let root = self.find(rv);
            let sid = match self.shape[root as usize] {
                Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                Some(Shape::Adt(b, _, _)) => Some(b),
                _ => None,
            };
            if let Some(id) = sid.and_then(|s| self.prog.self_method(s, n)) {
                // (a method of the program's own type that has a scalar method's name (`checked_add`): its overloads too,
                // as below)
                let id = self.pick_inherent_overload(id, rv);
                return self.apply_recv(id, avs, rv);
            }
            return self.scalar_method(n, tf, args, r, rv, avs);
        }
        let r = self.expr(recv);
        let rv = self.deref_expr(r);
        let root = self.find(rv);
        let sid = match self.shape[root as usize] {
            Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
            Some(Shape::Adt(b, _, _)) => Some(b),
            _ => None,
        };
        // (a method with `self`: an associated function of the name is none (self_method), by the reference the receiver
        // reaches its struct through last, as the lowering's lookup (self_method_at))
        let mut wrap = None;
        let mut v = r;
        while sid.is_some() {
            let x = self.find(v);
            match self.shape[x as usize] {
                Some(Shape::Ref(m, i)) => {
                    wrap = Some(m);
                    v = i;
                }
                Some(Shape::Bx(i)) => {
                    wrap = None;
                    v = i;
                }
                _ => break,
            }
        }
        let mut callee = sid.and_then(|sid| self.prog.self_method_at(sid, n, wrap).or_else(|| self.prog.self_default_method(sid, n)));
        let mut rv = rv;
        let mut sid = sid;
        let mut hops = 0;
        while callee.is_none() && hops < 4 {
            let Some(s0) = sid else { break };
            let Some(did) = self.prog.method(s0, "deref") else { break };
            // (a guard (RefCell's Ref / RefMut, MutexGuard) has none of these itself: `g.clone()` is the value's)
            let guard = matches!(self.prog.structs[self.prog.base_of(s0) as usize].name.as_str(), "Ref" | "RefMut" | "MutexGuard" | "PeekMut");
            if guard || !matches!(n, "clone" | "to_string" | "cmp" | "partial_cmp" | "eq") {
                let r = self.apply_recv(Some(did), vec![], rv);
                rv = self.deref_expr(r);
                let root = self.find(rv);
                sid = match self.shape[root as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                callee = sid.and_then(|sid| self.prog.self_method(sid, n).or_else(|| self.prog.self_default_method(sid, n)));
            } else {
                break;
            }
            hops += 1;
        }
        // (inherent methods of one name in impls for two instances of the type (`impl W<i32> { fn f(&self) }`, `impl
        // W<u8> ..`, either's `Either<&L, &R>` / `Either<&mut L, &mut R>`): the one whose self type the receiver is.
        // While the receiver is not known that far, none: its type is not taken from a candidate's)
        if let Some(id) = callee {
            callee = self.pick_inherent_overload(id, rv);
        }
        // (`b.clone()` of a Box or an array: the receiver's own type, its variables kept (`Box::new(22).clone()`
        // where a `Box<isize>` is wanted: the literal is not settled to i32 here))
        let rroot = self.find(rv);
        {
            // (the receiver before auto-deref: for a Box, deref_expr gave its contents)
            // (only when what the Box holds is a numeric literal not settled yet: sharing the variable otherwise would let a
            // coercion's target (`let c: Box<dyn A> = v.clone()`) flow back into the receiver)
            let r0 = self.find(r);
            let lit = match self.shape[r0 as usize] {
                Some(Shape::Bx(i) | Shape::Arr(i, _)) => {
                    let ir = self.find(i);
                    self.shape[ir as usize].is_none() && (self.int_lit[ir as usize] || self.float_lit[ir as usize])
                }
                _ => false,
            };
            if callee.is_none() && n == "clone" && args.is_empty() && lit {
                return r;
            }
        }
        // (`x.clone()`: struct で clone を持たない (derive の Copy など) か、整数・浮動小数点・bool・char・生ポインタ)
        let prim = matches!(self.shape[rroot as usize], Some(Shape::Known(t)) if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Ptr(..)));
        if callee.is_none() && n == "clone" && args.is_empty() && (sid.is_some() || prim) {
            // (`r.clone()` with `r: &S` and S a type of the program that is not Clone: the reference's clone, `&S`)
            let fr = self.find(r);
            let refd = matches!(self.shape[fr as usize], Some(Shape::Ref(..)));
            // (and an instance with such an argument, `Option<NoClone>`, but for the shared pointers: as the code does)
            if refd && self.not_clone(rv, 0) {
                return r;
            }
            return rv;
        }
        let mut avs = vec![];
        for a in args {
            avs.push(self.expr(a));
        }
        // `it.zip(&v)` / `it.chain([x])`: Vec / スライス / 配列の引数はそのイテレータの型
        if matches!(n, "zip" | "chain") && avs.len() == 1 {
            let mut base = self.resolve(avs[0]);
            let is_ref = matches!(base, Some(Ty::Ref(..)));
            // (`it.zip(dst)` with dst: &mut [T] / `zip(&mut v)`: its iter_mut, as IntoIterator for &mut gives)
            let is_mut = matches!(base, Some(Ty::Ref(true, _)));
            while let Some(Ty::Ref(_, i)) = base {
                if matches!(i, Ty::Slice(_)) {
                    break;
                }
                base = Some(*i);
            }
            let vec_id = self.prog.struct_index.get("Vec").copied();
            let opt_id = self.prog.struct_index.get("Option").copied();
            let seq = match base {
                Some(Ty::Ref(_, Ty::Slice(_))) | Some(Ty::Array(..)) => true,
                Some(Ty::Struct(s)) => Some(self.prog.base_of(s)) == vec_id || Some(self.prog.base_of(s)) == opt_id,
                _ => false,
            };
            let syntactic_array = matches!(&args[0], Expr::Array(_) | Expr::Repeat(..));
            if seq || syntactic_array {
                let a = &args[0];
                let it = if is_mut { "iter_mut" } else { "iter" };
                let x = match (is_ref, a) {
                    (true, Expr::Ref(_, inner)) => mcall((**inner).clone(), it, vec![]),
                    (true, _) => mcall(a.clone(), it, vec![]),
                    _ => mcall(a.clone(), "into_iter", vec![]),
                };
                avs[0] = self.expr_kept(x, false);
            }
        }
        let mut dt = self.resolve(rv);
        while let Some(Ty::Ref(_, i)) = dt {
            dt = Some(*i);
        }
        if let Some(Ty::Dyn(tr)) = dt {
            // (the trait's method, else one of `impl dyn Tr { .. }` / `impl X for dyn Tr`: `__dyn_Tr::m`)
            let decl = self
                .prog
                .traits
                .get(crate::types::dyn_autos(tr).0)
                .and_then(|ds| ds.iter().copied().find(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(n)))
                .or_else(|| self.prog.prim_method(Ty::Dyn(tr), n).filter(|&d| self.prog.sigs[d as usize].has_self))
                // (or a default method of a trait the object implements, `impl MyTrait for dyn Tr {}`)
                .or_else(|| {
                    self.prog.dyn_impl_traits(tr).into_iter().filter(|t2| self.prog.implements(Ty::Dyn(tr), t2)).find_map(|t2| {
                        self.prog.traits.get(t2)?.iter().copied().find(|&d| {
                            let s = &self.prog.sigs[d as usize];
                            s.has_body && s.has_self && self.prog.names[d as usize].rsplit("::").next() == Some(n)
                        })
                    })
                });
            if let Some(d) = decl {
                let sig = &self.prog.sigs[d as usize];
                // (an impl's parameters solved from the receiver's object: `impl<T> Get<T> for dyn O<T>` through `dyn
                // O<u8>`, T u8)
                let mut sol = vec![None; sig.generics.len()];
                if let (false, Some(&p0 @ Ty::Ref(m, _))) = (sol.is_empty(), sig.params.first()) {
                    self.prog.match_ty(p0, Ty::Ref(m, crate::types::intern(Ty::Dyn(tr))), &mut sol);
                }
                // (a default's trait parameters from the one impl for the object: `impl<T> Get<T> for dyn Obj<T>`
                // through `dyn Obj<u8>`, T u8)
                if let (Some(trn), true) = (sig.trait_of.as_deref(), sol.iter().skip(1).any(|x| x.is_none())) {
                    let prog = self.prog;
                    let fits: Vec<Vec<Ty>> = prog
                        .impl_targs
                        .get(trn)
                        .into_iter()
                        .flatten()
                        .filter_map(|(ist, args, ng)| {
                            let mut isol = vec![None; *ng];
                            if !prog.match_ty(*ist, Ty::Dyn(tr), &mut isol) {
                                return None;
                            }
                            let a: Vec<Ty> = args.iter().map(|&a| prog.partial_subst(a, &isol)).collect();
                            a.iter().all(|&t| !prog.has_param(t)).then_some(a)
                        })
                        .collect();
                    if let [a] = fits.as_slice() {
                        for (i, &t) in a.iter().enumerate() {
                            if let Some(x @ None) = sol.get_mut(1 + i) {
                                *x = Some(t);
                            }
                        }
                    }
                }
                let (ps, ret) = (sig.params[1..].to_vec(), sig.ret);
                for (a, p) in avs.into_iter().zip(ps) {
                    let p = self.prog.partial_subst(p, &sol);
                    if !self.prog.has_param(p) {
                        let k = self.known(p);
                        self.unify(a, k);
                    }
                }
                let ret = self.prog.partial_subst(ret, &sol);
                if !self.prog.has_param(ret) {
                    return self.known(ret);
                }
                // (the method's own parameters from its turbofish, the last ones: `x.make::<u8>()`)
                let tys: Vec<&PTy> = tf.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None }).collect();
                let ng = sol.len();
                if !tys.is_empty() && tys.len() <= ng {
                    let mut pv: Vec<Option<u32>> = vec![None; ng];
                    for (i, t) in sol.into_iter().enumerate() {
                        pv[i] = t.map(|t| self.known(t));
                    }
                    for (k, t) in tys.into_iter().enumerate() {
                        let v = self.ty_var(t);
                        let i = (ng + k).wrapping_sub(tf.len());
                        if i < ng && pv[i].is_none() {
                            pv[i] = Some(v);
                        }
                    }
                    return self.known_g(ret, &mut pv);
                }
            } else if let Some(d) = self.boxed_dyn_method(r, tr, n) {
                // (none of the object's: a method of an impl for the Box itself, `impl MyTrait for Box<dyn O>` through
                // `val: &Box<dyn O>`)
                let sig = &self.prog.sigs[d as usize];
                let (ps, ret) = (sig.params[1..].to_vec(), sig.ret);
                for (a, p) in avs.into_iter().zip(ps) {
                    if !self.prog.has_param(p) {
                        let k = self.known(p);
                        self.unify(a, k);
                    }
                }
                if !self.prog.has_param(ret) {
                    return self.known(ret);
                }
            }
            return self.var();
        }
        // (a type parameter's placeholder (zustc check of a generic fn nothing calls): the method its bound's trait
        // declares, its parameters' and result's types as the declaration writes them, the trait's parameters the
        // bound's arguments (`F: Get<&'z u32>` with `fn get(&self) -> A`: `&u32`), as for `dyn`. So an unsuffixed
        // literal that meets them takes their type (`let a = 1; *f.get() == a`))
        if let (None, Some(Ty::Struct(ps))) = (callee, dt) {
            if self.prog.is_placeholder(ps) {
                if let Some((d, sol)) = self.placeholder_decl(ps, n) {
                    let sig = &self.prog.sigs[d as usize];
                    let (ps, ret) = (sig.params[1..].to_vec(), sig.ret);
                    for (a, p) in avs.into_iter().zip(ps) {
                        let p = self.prog.partial_subst(p, &sol);
                        if !self.prog.has_param(p) {
                            let k = self.known(p);
                            self.unify(a, k);
                        }
                    }
                    let ret = self.prog.partial_subst(ret, &sol);
                    if !self.prog.has_param(ret) {
                        return self.known(ret);
                    }
                    return self.var();
                }
            }
        }
        if callee.is_none() && n == "to_string" && avs.is_empty() {
            if let Some(&ss) = self.prog.struct_index.get("String") {
                return self.known(Ty::Struct(ss));
            }
        }
        if callee.is_none() && matches!(n, "join" | "concat") && avs.len() <= 1 {
            let mut t = self.resolve(rv);
            while let Some(Ty::Ref(_, i)) = t {
                t = Some(*i);
            }
            let elem = match t {
                Some(Ty::Array(e, _)) | Some(Ty::Slice(e)) => Some(*e),
                Some(Ty::Struct(s)) if self.prog.structs[s as usize].name.starts_with("Vec") => self.prog.structs[s as usize].args.first().copied(),
                _ => None,
            };
            if let Some(e @ Ty::Struct(es)) = elem {
                if n == "concat" && self.prog.base_of(es) == self.prog.struct_index.get("Vec").copied().unwrap_or(u32::MAX) {
                    return self.known(e);
                }
            }
            if elem.is_some() || t.is_none() {
                if let Some(&ss) = self.prog.struct_index.get("String") {
                    return self.known(Ty::Struct(ss));
                }
            }
        }
        if callee.is_none() && matches!(n, "cmp" | "partial_cmp") && avs.len() == 1 {
            if let Some(&os) = self.prog.struct_index.get("Ordering") {
                let o = self.known(Ty::Struct(os));
                return if n == "cmp" { o } else { self.option_of(o) };
            }
        }
        if callee.is_none() && sid.is_none() {
            let mut t = self.resolve(rv);
            while let Some(Ty::Ref(_, i)) = t {
                t = Some(*i);
            }
            let root = self.find(rv);
            let elem = match (t, self.shape[root as usize]) {
                (Some(Ty::Slice(e)) | Some(Ty::Array(e, _)), _) => Some(self.known(*e)),
                (_, Some(Shape::Slc(e)) | Some(Shape::Arr(e, _))) => Some(e),
                _ => None,
            };
            if let (Some(e), Some(&vb)) = (elem, self.prog.struct_index.get("Vec")) {
                if let Some(id) = self.prog.self_method(vb, n) {
                    if !matches!(n, "len" | "is_empty" | "iter" | "iter_mut" | "into_iter") {
                        let vv = self.adt(vb, vec![e]);
                        return self.apply_recv(Some(id), avs.clone(), vv);
                    }
                }
            }
            if t.is_none() && matches!(n, "swap" | "split_at" | "split_at_mut" | "truncate" | "rotate_left" | "rotate_right" | "split_off") {
                for &a in &avs {
                    let u = self.known(Ty::Usize);
                    self.unify(a, u);
                }
            }
        }
        if callee.is_none() && sid.is_none() {
            // (`3.double()` / `let x = 3; x.double()` with `impl Double for usize` only: the one integer type whose
            // impl has the method fixes the literal's type, as rustc's probe of `{integer}`)
            let root = self.find(rv);
            if self.shape[root as usize].is_none() && self.int_lit[root as usize] && !is_builtin_scalar_method(n) {
                let mut hit = None;
                // (or a default method of a trait the integer type implements: `3.hi()`)
                let dtraits: Vec<&String> = self
                    .prog
                    .traits
                    .iter()
                    .filter(|(_, ds)| ds.iter().any(|&d| self.prog.sigs[d as usize].has_body && self.prog.names[d as usize].rsplit("::").next() == Some(n)))
                    .map(|(tr, _)| tr)
                    .collect();
                for t in [Ty::I8, Ty::I16, Ty::I32, Ty::I64, Ty::I128, Ty::Isize, Ty::U8, Ty::U16, Ty::U32, Ty::U64, Ty::U128, Ty::Usize] {
                    if self.prog.prim_method(t, n).is_some() || dtraits.iter().any(|tr| self.prog.implements(t, tr)) {
                        hit = if hit.is_none() { Some(t) } else { Some(Ty::Never) };
                    }
                }
                if let Some(t) = hit.filter(|t| *t != Ty::Never) {
                    let k = self.known(t);
                    self.unify(rv, k);
                }
            }
            if let Some(t) = self.resolve(rv) {
                if let Some(id) = self.prog.prim_method(t, n) {
                    return self.apply_recv(Some(id), avs, rv);
                }
            }
            if self.resolve(rv).is_none() && !is_builtin_scalar_method(n) && !matches!(n, "min" | "max" | "len" | "is_empty") {
                return self.var();
            }
            return self.scalar_method(n, tf, args, r, rv, avs);
        }
        if n == "extend" && avs.len() == 1 {
            if let Some(b) = sid {
                let bn = self.prog.structs[self.prog.base_of(b) as usize].name.clone();
                if matches!(bn.as_str(), "Vec" | "BinaryHeap") {
                    let root = self.find(rv);
                    let elem = match self.shape[root as usize] {
                        Some(Shape::Adt(_, st, l)) => self.adt_vars_of(st, l).first().copied(),
                        Some(Shape::Known(Ty::Struct(s))) => {
                            let a = self.prog.structs[s as usize].args.first().copied();
                            a.map(|t| self.known(t))
                        }
                        _ => None,
                    };
                    let known_elem = elem.is_some_and(|ev| self.resolve(ev).is_some());
                    if let (Some(ev), Some(at), false) = (elem, self.resolve(avs[0]), known_elem) {
                        if let Some(it) = self.extend_item(at) {
                            let it = match it {
                                Ty::Ref(_, i) if !matches!(i, Ty::Str | Ty::Slice(_) | Ty::Dyn(_)) => *i,
                                t => t,
                            };
                            let k = self.known(it);
                            self.unify(ev, k);
                        }
                    }
                    return self.known(Ty::Unit);
                }
            }
        }
        if callee.is_none() {
            if let Some(b) = sid {
                if self.prog.structs[self.prog.base_of(b) as usize].name == "String" {
                    let h = if n == "split" && !matches!(args.first(), Some(Expr::Closure(..))) && avs.first().is_some_and(|&a| self.resolve(a) != Some(Ty::Char)) {
                        "split_str".to_string()
                    } else if n == "split_whitespace" {
                        "split_ws".to_string()
                    } else if matches!(args.first(), Some(Expr::Closure(..))) && self.prog.index.contains_key(&format!("__exr_str_{n}_fn")) {
                        format!("{n}_fn")
                    } else {
                        n.to_string()
                    };
                    if let Some(&id) = self.prog.index.get(&format!("__exr_str_{h}")) {
                        return self.apply_helper(id, tf, avs);
                    }
                }
            }
        }
        // (an inherent method of one name in impls for disjoint instances, `W<u8>::get` / `W<u16>::get`, `W<&T>::copied`
        // / `W<&mut T>::copied`: the receiver's, once its type tells them apart; until then nothing is learned from it)
        if let Some(id) = callee {
            let prog = self.prog;
            if let Some(cands) = prog.overloads.get(&prog.names[id as usize]) {
                let inherent = |c: &u32| prog.sigs[*c as usize].impl_trait.is_none() && prog.sigs[*c as usize].has_self;
                if cands.iter().all(inherent) {
                    let fits = |c: u32, t: Ty| {
                        let sig = &prog.sigs[c as usize];
                        let mut sol = vec![None; sig.generics.len()];
                        sig.self_ty.is_some_and(|st| prog.match_ty_exact(st, t, &mut sol))
                    };
                    match self.resolve(rv).and_then(|t| cands.iter().copied().find(|&c| fits(c, t))) {
                        Some(c) => callee = Some(c),
                        None => return self.var(),
                    }
                }
            }
        }
        self.turbofish = if tf.is_empty() { None } else { Some(tf as *const Vec<GArg>) };
        let r = self.apply_recv(callee, avs, rv);
        self.turbofish = None;
        if n == "collect" {
            self.collects.push((rv, r));
        }
        r
    }

    /// 配列 / スライスの受け手の組み込みメソッド
    fn seq_method(&mut self, recv: &Expr, n: &str, args: &[Expr]) -> u32 {
        let r = self.expr(recv);
        let el = self.elem_of(r).unwrap();
        let mut avs = vec![];
        for a in args {
            avs.push(self.expr(a));
        }
        match n {
            "len" => self.known(Ty::Usize),
            "is_empty" | "contains" => self.known(Ty::Bool),
            // (the prelude's `__exr_slice_<n>(s, ..)`, its T the element: `Option<(&T, &[T])>`, `SliceSplitN<T, F>`)
            "split_first" | "split_last" | "splitn" if self.prog.index.contains_key(&format!("__exr_slice_{n}")) => {
                let id = self.prog.index[&format!("__exr_slice_{n}")];
                self.apply_seq_helper(id, el, avs)
            }
            // (a slice's to_owned is its to_vec; an array's, a clone of it, is left to the use)
            "to_owned" if self.prog.struct_index.contains_key("Vec") && {
                let d = self.deref_expr(r);
                let root = self.find(d);
                matches!(self.shape[root as usize], Some(Shape::Slc(_)))
            } => {
                let vb = self.prog.struct_index["Vec"];
                self.adt(vb, vec![el])
            }
            "to_vec" if self.prog.struct_index.contains_key("Vec") => {
                let vb = self.prog.struct_index["Vec"];
                self.adt(vb, vec![el])
            }
            "copy_from_slice" | "clone_from_slice" if avs.len() == 1 => {
                if let Some(ae) = self.elem_of(avs[0]) {
                    self.unify(ae, el);
                }
                self.known(Ty::Unit)
            }
            "try_into" if self.prog.struct_index.contains_key("TryFromSliceError") => {
                let rb = self.prog.struct_index["Result"];
                let eb = self.prog.struct_index["TryFromSliceError"];
                let t = self.var();
                let e = self.known(Ty::Struct(eb));
                self.adt(rb, vec![t, e])
            }
            "get" | "get_mut" if matches!(args.first(), Some(Expr::Range(..))) => {
                let sl = self.with(Shape::Slc(el));
                let rr = self.with(Shape::Ref(n == "get_mut", sl));
                self.option_of(rr)
            }
            "get" | "get_mut" | "first" | "last" => {
                for a in avs {
                    let k = self.known(Ty::Usize);
                    self.unify(a, k);
                }
                let rr = self.with(Shape::Ref(n == "get_mut", el));
                self.option_of(rr)
            }
            "swap" | "truncate" | "rotate_left" | "rotate_right" => {
                for a in avs {
                    let k = self.known(Ty::Usize);
                    self.unify(a, k);
                }
                self.known(Ty::Unit)
            }
            "split_at" | "split_at_mut" => {
                for a in avs {
                    let k = self.known(Ty::Usize);
                    self.unify(a, k);
                }
                self.var()
            }
            "iter" | "iter_mut" => {
                let name = if n == "iter" { "Iter" } else { "IterMut" };
                match self.prog.struct_index.get(name) {
                    Some(&b) => self.adt(b, vec![el]),
                    None => self.var(),
                }
            }
            "as_ptr" | "as_mut_ptr" => {
                let k = self.resolve(el).map(|t| Ty::Ptr(n == "as_mut_ptr", intern(t)));
                match k {
                    Some(t) => self.known(t),
                    None => self.var(),
                }
            }
            "as_slice" | "as_mut_slice" => {
                let sl = self.with(Shape::Slc(el));
                self.with(Shape::Ref(n == "as_mut_slice", sl))
            }
            // (an array's or a slice's as_ref / as_mut: its one AsRef / AsMut is `AsRef<[T]>` / `AsMut<[T]>` (bindgen's
            // `self.storage.as_ref()[i]` of a `[u8; N]`); a Vec's have two, left to the use)
            "as_ref" | "as_mut" if args.is_empty() && {
                let mut t = self.resolve(r);
                while let Some(Ty::Ref(_, i)) = t {
                    t = Some(*i);
                }
                matches!(t, Some(Ty::Array(..) | Ty::Slice(_)))
            } => {
                let sl = self.with(Shape::Slc(el));
                self.with(Shape::Ref(n == "as_mut", sl))
            }
            _ => self.var(),
        }
    }

    /// 整数 / &str / 生ポインタの組み込みメソッドの型
    fn scalar_method(&mut self, name: &str, tf: &[GArg], args: &[Expr], r: u32, rv: u32, avs: Vec<u32>) -> u32 {
        let root = self.find(rv);
        let recv_ty = match self.shape[root as usize] {
            Some(Shape::Known(t)) => Some(t),
            _ => None,
        };
        let pre = match self.shape[root as usize] {
            Some(Shape::Known(Ty::Str)) => Some("__exr_str_"),
            Some(Shape::Known(Ty::Char)) => Some("__exr_char_"),
            _ => None,
        };
        if let Some(pre) = pre {
            let h = if name == "split" && pre == "__exr_str_" && !matches!(args.first(), Some(Expr::Closure(..))) && avs.first().is_some_and(|&a| self.resolve(a) != Some(Ty::Char)) {
                "split_str".to_string()
            } else if name == "split_whitespace" {
                "split_ws".to_string()
            } else if matches!(args.first(), Some(Expr::Closure(..))) && self.prog.index.contains_key(&format!("{pre}{name}_fn")) {
                format!("{name}_fn")
            } else {
                name.to_string()
            };
            if let Some(&id) = self.prog.index.get(&format!("{pre}{h}")) {
                let builtin = matches!(name, "len" | "is_empty" | "as_bytes" | "as_ptr");
                if !builtin {
                    return self.apply_helper(id, tf, avs);
                }
            }
        }
        if let Some(t) = recv_ty.filter(|t| t.is_int()) {
            if matches!(name, "to_le_bytes" | "to_be_bytes" | "to_ne_bytes") {
                return self.known(Ty::Array(intern(Ty::U8), t.bits() / 8));
            }
        }
        if let Some(t) = recv_ty.filter(|t| t.is_float()) {
            // (the methods whose arguments are Self: a literal or generic argument (`x.min(black_box(9.0))`) is that type)
            if matches!(name, "min" | "max" | "powf" | "atan2" | "hypot" | "copysign" | "mul_add" | "clamp" | "rem_euclid" | "div_euclid" | "abs_sub" | "maximum" | "minimum" | "midpoint") {
                for &a in &avs {
                    self.unify(a, rv);
                }
            }
            return match name {
                "is_nan" | "is_finite" | "is_infinite" | "is_sign_negative" | "is_sign_positive" | "is_normal" | "is_subnormal" => self.known(Ty::Bool),
                "to_bits" => self.known(if t == Ty::F32 { Ty::U32 } else { Ty::U64 }),
                "total_cmp" => match self.prog.struct_index.get("Ordering") {
                    Some(&o) => self.known(Ty::Struct(o)),
                    None => self.var(),
                },
                "classify" => match self.prog.struct_index.get("FpCategory") {
                    Some(&o) => self.known(Ty::Struct(o)),
                    None => self.var(),
                },
                _ => self.known(t),
            };
        }
        if let Some(Shape::Known(Ty::Str)) = self.shape[root as usize] {
            return match name {
                "len" => self.known(Ty::Usize),
                "is_empty" => self.known(Ty::Bool),
                "as_bytes" => self.known(Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))))),
                "as_ptr" => self.known(Ty::Ptr(false, intern(Ty::U8))),
                "trim" | "trim_start" | "trim_end" => self.known(Ty::Ref(false, intern(Ty::Str))),
                "starts_with" | "ends_with" | "contains" => self.known(Ty::Bool),
                "to_string" | "to_owned" => match self.prog.struct_index.get("String") {
                    Some(&s) => self.known(Ty::Struct(s)),
                    None => self.var(),
                },
                "find" => {
                    let u = self.known(Ty::Usize);
                    self.option_of(u)
                }
                _ => self.var(),
            };
        }
        if let Some(Ty::Ptr(m, e)) = self.resolve(r) {
            return match name {
                // (the count is a usize (an isize for the offsets): `p.add(2 * V::BYTES)`'s literal is one)
                "add" | "sub" | "offset" | "wrapping_add" | "wrapping_sub" | "wrapping_offset" => {
                    let k = self.known(if name.contains("offset") { Ty::Isize } else { Ty::Usize });
                    for a in avs {
                        self.unify(a, k);
                    }
                    r
                }
                // (`x.cast::<u32>()`: a pointer to the turbofish's type, of the same mutability)
                "cast" => match tf.first() {
                    Some(GArg::Ty(t)) => match self.parse_ty(t) {
                        Some(u) => self.known(Ty::Ptr(m, intern(u))),
                        None => self.var(),
                    },
                    // (no turbofish: a pointer of the same mutability to what the use wants, `v128_load(p.cast())`)
                    _ => {
                        let i = self.var();
                        self.with(crate::solver::Shape::Ptr(m, i))
                    }
                },
                "is_null" => self.known(Ty::Bool),
                // (the argument may be a `*mut` of a `*const` receiver, or the other way: not unified with it)
                "offset_from" => self.known(Ty::Isize),
                "read" | "read_volatile" | "read_unaligned" => self.known(*e),
                _ => self.known(Ty::Unit),
            };
        }
        match name {
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" | "min" | "max" | "saturating_add" | "saturating_sub" => {
                for a in avs {
                    self.unify(a, rv);
                }
                rv
            }
            "rem_euclid" | "div_euclid" | "next_multiple_of" | "div_ceil" => {
                for a in avs {
                    self.unify(a, rv);
                }
                rv
            }
            // (the argument is the signed type of the receiver's width)
            "wrapping_add_signed" => {
                let st = match self.resolve(rv) {
                    Some(Ty::U8) => Some(Ty::I8),
                    Some(Ty::U16) => Some(Ty::I16),
                    Some(Ty::U32) => Some(Ty::I32),
                    Some(Ty::U64) => Some(Ty::I64),
                    Some(Ty::U128) => Some(Ty::I128),
                    Some(Ty::Usize) => Some(Ty::Isize),
                    _ => None,
                };
                for a in avs.into_iter().filter(|_| st.is_some()) {
                    let k = self.known(st.unwrap_or(Ty::Isize));
                    self.unify(a, k);
                }
                rv
            }
            "overflowing_add" | "overflowing_sub" | "overflowing_mul" | "overflowing_pow" => {
                for a in avs {
                    let k = if name == "overflowing_pow" { self.known(Ty::U32) } else { rv };
                    self.unify(a, k);
                }
                let b = self.known(Ty::Bool);
                let tb = self.prog.tuple_base(2);
                self.adt(tb, vec![rv, b])
            }
            "checked_pow" => {
                for a in avs {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                self.option_of(rv)
            }
            "leading_ones" | "trailing_ones" => self.known(Ty::U32),
            "checked_abs" | "checked_neg" => self.option_of(rv),
            "wrapping_abs" | "saturating_abs" | "saturating_neg" => rv,
            "overflowing_neg" | "overflowing_abs" => {
                let b = self.known(Ty::Bool);
                let tb = self.prog.tuple_base(2);
                self.adt(tb, vec![rv, b])
            }
            "try_into" => match (self.prog.struct_index.get("Result").copied(), self.prog.struct_index.get("TryFromIntError").copied()) {
                (Some(rb), Some(eb)) => {
                    let t = self.var();
                    let e = self.known(Ty::Struct(eb));
                    self.adt(rb, vec![t, e])
                }
                _ => self.var(),
            },
            "wrapping_neg" | "abs" | "signum" | "swap_bytes" | "to_be" | "to_le" | "to_ascii_uppercase" | "to_ascii_lowercase" | "next_power_of_two" | "isqrt" => rv,
            "reverse_bits" => rv,
            // (the shift amount is a u32: `x.wrapping_shl(1)` is x's type, not the default bool)
            "wrapping_shl" | "wrapping_shr" => {
                for a in avs {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                rv
            }
            "checked_shl" | "checked_shr" => {
                for a in avs {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                self.option_of(rv)
            }
            "overflowing_shl" | "overflowing_shr" => {
                for a in avs {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                self.var()
            }
            "pow" | "rotate_left" | "rotate_right" | "wrapping_pow" | "saturating_pow" => {
                for a in avs {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                rv
            }
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" | "checked_rem" => {
                for a in avs {
                    self.unify(a, rv);
                }
                self.option_of(rv)
            }
            "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" => self.known(Ty::U32),
            "abs_diff" => self.var(),
            // (受け手の型がまだ分からない `c.len()` (chunks の要素など) も usize)
            "len" => self.known(Ty::Usize),
            _ => self.known(Ty::Bool),
        }
    }
}

/// 関数本体の let 文を前順に (クロージャの本体の中も)
pub fn lets_in_order(b: &Block, out: &mut Vec<*const Stmt>) {
    for st in &b.stmts {
        match st {
            Stmt::Let(_, _, init, els, ..) => {
                out.push(st as *const Stmt);
                if let Some(e) = init {
                    lets_in_expr(e, out);
                }
                if let Some(b) = els {
                    lets_in_order(b, out);
                }
            }
            Stmt::Expr(e, _) => lets_in_expr(e, out),
            Stmt::Item(_) => {}
        }
    }
}

fn lets_in_expr(e: &Expr, out: &mut Vec<*const Stmt>) {
    match e {
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => lets_in_order(b, out),
        Expr::If(c, t, els) => {
            lets_in_expr(c, out);
            lets_in_order(t, out);
            if let Some(x) = els {
                lets_in_expr(x, out);
            }
        }
        Expr::While(_, c, b) => {
            lets_in_expr(c, out);
            lets_in_order(b, out);
        }
        Expr::For(_, _, it, b) => {
            lets_in_expr(it, out);
            lets_in_order(b, out);
        }
        _ => ast_closure::each_child_pub(e, &mut |c| lets_in_expr(c, out)),
    }
}
