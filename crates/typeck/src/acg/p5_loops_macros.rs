use super::*;
use crate::fxhash::FxSet as HashSet;

use crate::acg::clif::MemFlagsData;
use crate::parse::{Block as PBlock, Expr, GArg, Pat, Path, Stmt, TokKind, tokenize};
use crate::types::{Ty, intern};

/// 無名データ / static / vtable のシンボルの名前空間 (codegen.rs と同じ番号)
pub const NS_DATA: u32 = 1;
pub const NS_STATIC: u32 = 2;
pub const NS_VTABLE: u32 = 3;

// --- マクロのトークンのテキスト (syn の ToTokens の代わり) -----------------------

/// `( .. )` / `[ .. ]` / `{ .. }` の中身
fn strip_delims(g: &str) -> Option<&str> {
    let t = g.trim();
    let first = t.chars().next()?;
    let last = t.chars().last()?;
    if !matches!(first, '(' | '[' | '{') || !matches!(last, ')' | ']' | '}') || t.len() < 2 {
        return None;
    }
    Some(&t[1..t.len() - 1])
}

/// トップレベルの記号 sep で分ける (括弧・文字列の中は分けない)
fn split_top(s: &str, sep: &str) -> Option<Vec<String>> {
    let toks = tokenize(s).ok()?;
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0usize;
    for t in &toks {
        match t.kind {
            TokKind::Open => depth += 1,
            TokKind::Close => depth -= 1,
            TokKind::Punct if depth == 0 && t.text == sep => {
                parts.push(s[start..t.lo as usize].to_string());
                start = t.lo as usize + t.text.len();
            }
            _ => {}
        }
    }
    parts.push(s[start..].to_string());
    Some(parts)
}

/// マクロの引数のテキスト (トップレベルの `,` で分けたもの。空の最後の要素は除く)
fn macro_arg_texts(g: &str) -> Vec<String> {
    let Some(inner) = strip_delims(g) else { return vec![] };
    let mut v = split_top(inner, ",").unwrap_or_default();
    if v.last().is_some_and(|s| s.trim().is_empty()) {
        v.pop();
    }
    v
}

/// ソースのテキストを proc_macro2 の Display と同じ空け方で表示する (parse.rs の tokens_text と同じ)
fn tokens_text(src: &str) -> String {
    let Ok(toks) = tokenize(src) else { return src.trim().to_string() };
    let mut s = String::new();
    for k in 0..toks.len() {
        let t = &toks[k];
        if k > 0 && space_between(&toks, k, src) {
            s.push(' ');
        }
        s.push_str(&t.text);
    }
    s
}

/// `stringify!`: whether a space goes between two tokens. Tokens next to each other in the text stay together; else
/// rustc's printer's rules: none inside a path (`a::b`), before `,` `;` `.`, after `.`, between a name and its `(`
/// (`f(3)`), after `#` before `[`, just inside `(..)` / `[..]`, and between joined symbols (`->`, `>=`)
fn space_between(toks: &[crate::parse::Tok], k: usize, src: &str) -> bool {
    let (a, b) = (&toks[k - 1], &toks[k]);
    // (`::` is two `:` tokens: the one before a joined `:`, and the one after)
    let colon2_starts = |i: usize| toks.get(i).is_some_and(|t| t.text == ":" && t.joint) && toks.get(i + 1).is_some_and(|t| t.text == ":");
    if colon2_starts(k) || (k >= 2 && colon2_starts(k - 2)) {
        return false;
    }
    let end = a.lo as usize + a.text.len();
    if src.get(end..b.lo as usize).is_some_and(|g| g.is_empty()) {
        return false;
    }
    if a.kind == TokKind::Punct && a.joint && b.kind == TokKind::Punct {
        return false;
    }
    let (x, y) = (a.text.as_str(), b.text.as_str());
    if x == "::" || y == "::" || matches!(y, "," | ";" | ".") || x == "." || (x == "#" && y == "[") {
        return false;
    }
    if (a.kind == TokKind::Open && x != "{") || (b.kind == TokKind::Close && y != "}") {
        return false;
    }
    if y == "(" && a.kind == TokKind::Ident && (!crate::parse::reserved_kw(x, false) || matches!(x, "fn" | "Self" | "pub")) {
        return false;
    }
    true
}

/// 文字列リテラルの式ならその中身
fn str_lit(e: &Expr) -> Option<String> {
    match e {
        Expr::Lit(t, _) => match lit::lit(t) {
            Some(Lit::Str(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

fn let_stmt(name: &str, init: Expr) -> Stmt {
    Stmt::Let(Pat::Ident(false, false, name.to_string(), None), None, Some(init), None, Default::default())
}

/// hoist_index_bases の走査 (関数の中に置くと zustc の入れ子のアイテムの持ち上げが impl を扱えないので外に置く)
struct HoistScan {
    /// 現れた添字の鎖: (根の名前, 受け手の式の列 (`x`, `x[i]`, ..), 代入の左辺か)
    chains: Vec<(String, Vec<Expr>, bool)>,
    /// 鎖でない形で現れた名前 (メソッドの受け手・&mut・値として・マクロの中・束縛)
    other: HashSet<String>,
    assigned: HashSet<String>,
    bail: bool,
}
fn hoist_root(e: &Expr) -> Option<String> {
    match e {
        Expr::Path(_, p) => ident_of(p).map(|i| i.to_string()),
        Expr::Index(b, _, _) => hoist_root(b),
        Expr::Paren(p) => hoist_root(p),
        _ => None,
    }
}
fn hoist_chain(e: &Expr, out: &mut Vec<Expr>) -> bool {
    match e {
        Expr::Path(_, p) if ident_of(p).is_some() => {
            out.push(e.clone());
            true
        }
        Expr::Index(b, ix, _) if !matches!(&**ix, Expr::Range(..)) => {
            if !hoist_chain(b, out) {
                return false;
            }
            out.push(e.clone());
            true
        }
        _ => false,
    }
}
impl HoistScan {
    fn visit_expr(&mut self, e: &Expr) {
        match e {
            Expr::Index(_, ix, _) if !matches!(&**ix, Expr::Range(..)) => {
                let mut c = vec![];
                if hoist_chain(e, &mut c) {
                    c.pop(); // (いちばん外の添字は要素そのもの: 受け手は手前まで)
                    self.chains.push((hoist_root(e).unwrap(), c, false));
                    // 添字の式の中も見る
                    let mut x = e;
                    while let Expr::Index(b, ix, _) = x {
                        self.visit_expr(ix);
                        x = b;
                    }
                    return;
                }
                self.walk(e);
            }
            Expr::Assign(l, r) => {
                self.assign_target(l);
                self.visit_expr(r);
            }
            Expr::AssignOp(_, l, r, _) => {
                self.assign_target(l);
                self.visit_expr(r);
            }
            Expr::Path(_, p) => {
                if let Some(i) = ident_of(p) {
                    self.other.insert(i.to_string());
                }
            }
            Expr::Closure(..) => self.bail = true,
            Expr::Macro(_, g, _) => self.visit_macro(g),
            _ => self.walk(e),
        }
    }
    /// 子の式とパターンを見る (syn::visit::visit_expr の代わり)
    fn walk(&mut self, e: &Expr) {
        match e {
            Expr::Let(p, x) => {
                self.visit_pat(p);
                self.visit_expr(x);
            }
            Expr::Match(x, arms) => {
                self.visit_expr(x);
                for a in arms {
                    self.visit_pat(&a.pat);
                    if let Some(g) = &a.guard {
                        self.visit_expr(g);
                    }
                    self.visit_expr(&a.body);
                }
            }
            Expr::For(_, p, x, b) => {
                self.visit_pat(p);
                self.visit_expr(x);
                self.visit_block(b);
            }
            Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => self.visit_block(b),
            Expr::If(c, t, el) => {
                self.visit_expr(c);
                self.visit_block(t);
                if let Some(el) = el {
                    self.visit_expr(el);
                }
            }
            Expr::While(_, c, b) => {
                self.visit_expr(c);
                self.visit_block(b);
            }
            _ => crate::ast_closure::each_child_pub(e, &mut |c| self.visit_expr(c)),
        }
    }
    fn visit_block(&mut self, b: &PBlock) {
        for s in &b.stmts {
            match s {
                Stmt::Let(p, _, init, els, ..) => {
                    self.visit_pat(p);
                    if let Some(e) = init {
                        self.visit_expr(e);
                    }
                    if let Some(b) = els {
                        self.visit_block(b);
                    }
                }
                Stmt::Expr(e, _) => self.visit_expr(e),
                Stmt::Item(_) => {}
            }
        }
    }
    fn visit_pat(&mut self, p: &Pat) {
        match p {
            Pat::Ident(_, _, n, _) => {
                self.other.insert(n.clone());
                self.assigned.insert(n.clone());
            }
            Pat::Lit(e) => self.visit_expr(e),
            Pat::Range(a, b, _) => {
                if let Some(a) = a {
                    self.visit_expr(a);
                }
                if let Some(b) = b {
                    self.visit_expr(b);
                }
            }
            Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Or(ps) | Pat::Slice(ps) => {
                for p in ps {
                    self.visit_pat(p);
                }
            }
            Pat::Struct(_, fs, _) => {
                for f in fs {
                    self.visit_pat(&f.1);
                }
            }
            Pat::Ref(_, x) => self.visit_pat(x),
            Pat::Macro(t) => self.visit_macro(t),
            Pat::Wild | Pat::Rest | Pat::Path(_) => {}
        }
    }
    fn visit_macro(&mut self, g: &str) {
        // (マクロの引数の名前は全部「鎖でない」とみなす)
        if let Ok(toks) = tokenize(g) {
            for t in toks {
                if t.kind == TokKind::Ident {
                    self.other.insert(t.text);
                }
            }
        }
    }
    fn assign_target(&mut self, t: &Expr) {
        let mut c = vec![];
        if matches!(t, Expr::Index(..)) && hoist_chain(t, &mut c) {
            c.pop();
            self.chains.push((hoist_root(t).unwrap(), c, true));
            let mut x = t;
            while let Expr::Index(b, ix, _) = x {
                self.visit_expr(ix);
                x = b;
            }
        } else {
            if let Some(r) = hoist_root(t) {
                self.assigned.insert(r.clone());
                self.other.insert(r);
            }
            self.visit_expr(t);
        }
    }
}

impl Cx<'_> {
    // --- for ループ ------------------------------------------------------

    /// `for pat in &x { .. }` / `for pat in &mut x { .. }`: x is borrowed until the loop ends (a hidden local holds
    /// the loan; it is read when the loop is left)
    pub(crate) fn for_(&mut self, f: &Expr) -> R<V> {
        let mut temp = None;
        if let Expr::For(_, _, fexpr, _) = f {
            if let (true, Expr::Ref(m, inner)) = (self.bk_on(), strip_paren(fexpr)) {
                // (the loan only: the lowering reads x itself)
                let t0 = self.bk.call_temps.len();
                if let Some(ln) = self.bk_loan_only(inner, *m) {
                    self.bk_call_temp(&[borrowck::Src::Loan(ln)], true);
                }
                if self.bk.call_temps.len() > t0 {
                    temp = Some(t0);
                }
            }
        }
        let r = self.for_inner(f);
        if let Some(t0) = temp {
            self.bk_call_temps_end(t0);
        }
        r
    }

    fn for_inner(&mut self, f: &Expr) -> R<V> {
        let Expr::For(label, fpat, fexpr, fbody) = f else { unreachable!() };
        // `for p in opt.iter() { .. }` (Option, with no iterator in the prelude): a loop of at most one turn,
        // `for _ in 0..1 { if let Some(__x) = &opt { let p = __x; .. } }` (`into_iter`: the Option itself)
        // (`for x in Some(1)` / `for x in ok_value`: the Option / Result itself, as `into_iter`)
        let direct: Option<(&Expr, &str)> = match self.peek(fexpr) {
            Some(Ty::Struct(sid)) if matches!(self.prog.structs[self.prog.base_of(sid) as usize].name.as_str(), "Option" | "Result") => {
                Some((&**fexpr, "into_iter"))
            }
            _ => None,
        };
        let call: Option<(&Expr, &str)> = match strip_paren(fexpr) {
            Expr::MethodCall(recv, m, _, margs, _) if margs.is_empty() && matches!(m.as_str(), "iter" | "iter_mut" | "into_iter") => Some((&**recv, m.as_str())),
            _ => direct,
        };
        if let Some((recv, m)) = call {
            {
                if let Some(Ty::Struct(sid)) = self.peek(recv) {
                    let tyname = self.prog.structs[self.prog.base_of(sid) as usize].name.as_str();
                    if matches!(tyname, "Option" | "Result") && self.prog.method(sid, m).is_none() {
                        let ctor = if tyname == "Option" { "Some" } else { "Ok" };
                        let scrut = match m {
                            "iter" => Expr::Ref(false, Box::new(Expr::Paren(Box::new(recv.clone())))),
                            "iter_mut" => Expr::Ref(true, Box::new(Expr::Paren(Box::new(recv.clone())))),
                            _ => Expr::Paren(Box::new(recv.clone())),
                        };
                        let some = Pat::TupleStruct(path_of(ctor), vec![Pat::Ident(false, false, "__exr_ox".into(), None)]);
                        let mut stmts = vec![Stmt::Let((**fpat).clone(), None, Some(path_expr("__exr_ox")), None, Default::default())];
                        stmts.extend(fbody.stmts.iter().cloned());
                        let iflet = Expr::If(Box::new(Expr::Let(Box::new(some), Box::new(scrut))), crate::parse::Block { stmts }, None);
                        // (with no `break` / `continue` in the body, no loop: a value moved in is not moved in a loop)
                        let dbg = format!("{fbody:?}");
                        if label.is_none() && !dbg.contains("Break(") && !dbg.contains("Continue(") {
                            let st = Expr::Block(None, false, crate::parse::Block { stmts: vec![Stmt::Expr(iflet, true)] });
                            let st: &Expr = self.keep_expr(st);
                            return self.expr(st, None);
                        }
                        let range = Expr::Range(Some(Box::new(Expr::Lit("0usize".into(), Default::default()))), Some(Box::new(Expr::Lit("1usize".into(), Default::default()))), false);
                        let lp = Expr::For(label.clone(), Box::new(Pat::Wild), Box::new(range), crate::parse::Block { stmts: vec![Stmt::Expr(iflet, false)] });
                        let lp: &Expr = self.keep_expr(lp);
                        return self.for_inner(lp);
                    }
                }
            }
        }
        // (ラベルは syn の ident と同じく先頭の `'` を外した形で持つ)
        self.pending_label = label.as_ref().map(|l| l.trim_start_matches('\'').to_string());
        // 外側からアダプタ (`.rev()` / `.enumerate()` / `.iter()` ...) を剥がす
        let mut e: &Expr = fexpr;
        let mut adapters = vec![];
        loop {
            match e {
                Expr::MethodCall(recv, name, _, args, _)
                    if args.is_empty() && matches!(name.as_str(), "iter" | "iter_mut" | "into_iter" | "copied" | "cloned" | "enumerate" | "rev") =>
                {
                    adapters.push(name.clone());
                    e = recv;
                }
                Expr::Paren(p) => e = p,
                _ => break,
            }
        }
        adapters.reverse();
        // (`for x in v[1..].iter()`: 範囲の添字は参照にする (メソッド呼び出しの自動参照))
        if let Expr::Index(_, ix, _) = e {
            if matches!(&**ix, Expr::Range(..)) && !adapters.is_empty() {
                e = self.keep_expr(Expr::Ref(false, Box::new(e.clone())));
            }
        }
        let rev = adapters.iter().any(|a| a == "rev");
        let enumerate = adapters.iter().any(|a| a == "enumerate");
        if let Expr::Range(..) = e {
            return self.for_range(f, e, rev, enumerate);
        }
        // 配列・スライス・Vec でなければ Iterator: `while let Some(pat) = it.next() { .. }`
        let base_ty = self.peek(match e {
            Expr::Ref(_, x) => x,
            e => e,
        });
        let mut bt = base_ty;
        while let Some(Ty::Ref(_, i)) = bt {
            if bt.is_some_and(|t| t.is_fat()) {
                break;
            }
            bt = Some(*i);
        }
        // (`for x in c.borrow().iter()` / `for x in g.iter_mut()` with g a guard (`Ref<Vec<T>>`) of a Vec or an array:
        // the elements of what it guards, indexed through its deref; a guard that is a temporary lives to the end
        // of the statement, after the loop)
        let guard_seq = match (bt, adapters.first().map(|a| a.as_str())) {
            (Some(t), Some(a @ ("iter" | "iter_mut"))) => match self.guard_of(t) {
                Some((sid, target, gm)) if matches!(target, Ty::Array(..)) || matches!(target, Ty::Struct(v) if self.vec_fields(v).is_some()) => {
                    if a == "iter_mut" && !gm {
                        let g = self.prog.tn(Ty::Struct(sid));
                        return err(expr_start(e), format!("cannot borrow data in dereference of `{g}` as mutable (E0596)"));
                    }
                    true
                }
                _ => false,
            },
            _ => false,
        };
        // (型が分からないメソッド・関数呼び出し (`s.split(' ')` など) もイテレータとみなす)
        let iterator = match bt {
            Some(Ty::Struct(sid)) => self.vec_fields(sid).is_none() && !guard_seq,
            None => matches!(e, Expr::MethodCall(..) | Expr::Call(..)),
            _ => false,
        };
        if iterator {
            // `for (k, v) in &map`: next がなければ iter() / iter_mut() / into_iter() を通す
            let struct_has_next = |cx: &Self, t: Option<Ty>| match t {
                Some(Ty::Struct(s)) => cx.prog.method(s, "next").is_some(),
                _ => true,
            };
            // (`m.iter_mut()` は剥がした後の m ではなく、式全体の型で見る)
            let has_next = struct_has_next(self, bt) || matches!(strip_paren(fexpr), Expr::MethodCall(..)) && struct_has_next(self, self.peek(fexpr));
            // (ユーザの `impl IntoIterator for &Coll` / `for Coll`)
            let user_into = match bt {
                Some(Ty::Struct(s)) => {
                    let by_ref = matches!(strip_paren(fexpr), Expr::Ref(..));
                    if by_ref && self.prog.method(s, "__ref_into_iter").is_some() {
                        Some(true)
                    } else if !by_ref && self.prog.method(s, "into_iter").is_some() {
                        Some(false)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            let paren = |x: &Expr| Expr::Paren(Box::new(x.clone()));
            let via: Option<Expr> = if has_next {
                None
            } else if let Some(by_ref) = user_into {
                let fe: &Expr = fexpr;
                Some(if by_ref { mcall(paren(fe), "__ref_into_iter", vec![]) } else { mcall(paren(fe), "into_iter", vec![]) })
            } else {
                let fe: &Expr = fexpr;
                Some(match strip_paren(fe) {
                    Expr::Ref(true, inner) => mcall(paren(inner), "iter_mut", vec![]),
                    Expr::Ref(false, inner) => mcall(paren(inner), "iter", vec![]),
                    e => mcall(paren(e), "into_iter", vec![]),
                })
            };
            let src_e: &Expr = match via {
                Some(v) => self.keep_expr(v),
                None => fexpr,
            };
            self.scopes.push(vec![]);
            // (the iterator holds what the iterable expression borrows: `for x in &mut xs { xs.push(1) }`)
            self.bk_frame();
            let r = self.expr(src_e, None);
            let srcs = self.bk_frame_end();
            let (v, t) = r?;
            if t == Ty::Never {
                self.pop_scope();
                return Ok((None, Ty::Never));
            }
            self.bind("__exr_it".into(), t, v);
            self.bk_def_named("__exr_it", &srcs, src_e);
            let call: &Expr = self.keep_expr(mcall(path_expr("__exr_it"), "next", vec![]));
            let pat = Pat::TupleStruct(path_of("Some"), vec![(**fpat).clone()]);
            let r = self.while_let_parts(&pat, call, fbody, "for");
            self.pop_scope();
            r?;
            return Ok((None, Ty::Unit));
        }
        // 配列 / スライスを添字で回す
        let (by_ref, base_e): (Option<bool>, &Expr) = match e {
            Expr::Ref(m, x) => (Some(*m), x),
            _ => (None, e),
        };
        // `&a[lo..hi]` は部分スライス (参照ごと評価する)
        let sub = matches!(base_e, Expr::Index(_, ix, _) if matches!(&**ix, Expr::Range(..)));
        let (ptr, len, elem) = self.indexable(if sub { e } else { base_e })?;
        let mut mode = by_ref;
        let base_is_ref = by_ref.is_none() && matches!(self.peek(base_e), Some(Ty::Ref(..)));
        if base_is_ref {
            mode = Some(matches!(self.peek(base_e), Some(Ty::Ref(true, _))));
        }
        for a in &adapters {
            match a.as_str() {
                "iter" => mode = Some(false),
                "iter_mut" => mode = Some(true),
                "copied" | "cloned" => mode = None,
                _ => {}
            }
        }
        // (`for x in &[S { .. }]`: the array is a temporary the loop borrows; it is dropped at the end of the
        // statement, after the loop)
        if mode.is_some() && !sub && !is_place_expr(base_e) && self.needs_drop(elem) {
            if let Some(at @ Ty::Array(..)) = self.peek(base_e) {
                self.own_temp(ptr, at);
            }
        }
        // 値で回す (`for x in arr`): 要素は x にムーブするので、配列は drop しない。
        // Vec なら長さを 0 にしておき、バッファの解放だけを Vec の drop に任せる
        if mode.is_none() {
            let vec = match self.peek(base_e) {
                Some(Ty::Struct(sid)) => self.vec_fields(sid),
                _ => None,
            };
            if let (Some((_, lo, _)), true) = (vec, is_place_expr(base_e)) {
                if let Place::Mem(a, o, _) = self.place(base_e)? {
                    let z = self.b.ins().iconst(types::I64, 0);
                    self.b.ins().store(mf(), z, a, o + lo as i32);
                }
            } else if self.needs_drop(elem) {
                self.mark_moved(base_e);
            }
        }
        let i = self.b.declare_var(types::I64);
        let start = if rev { len } else { self.b.ins().iconst(types::I64, 0) };
        self.b.def_var(i, start);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let step = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(head);
        let iv = self.b.use_var(i);
        let c = if rev { self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThan, iv, 0) } else { self.b.ins().icmp(IntCC::UnsignedLessThan, iv, len) };
        self.b.ins().brif(c, body, &[], exit, &[]);
        self.b.switch_to_block(body);
        let idx = if rev {
            let v = self.b.ins().iadd_imm_s(iv, -1);
            self.b.def_var(i, v);
            v
        } else {
            iv
        };
        let addr = self.elem_addr(ptr, idx, elem);
        let (val, vt) = match mode {
            Some(m) => (Some(addr), Ty::Ref(m, intern(elem))),
            None => self.read(Place::Mem(addr, 0, elem)),
        };
        // (`for (i, x) in v.iter().enumerate()`: 組を作らずに i と x をそれぞれ束縛する)
        let tuple_pat = matches!(&**fpat, Pat::Tuple(t) if t.len() == 2);
        let (val, vt) = if enumerate && tuple_pat {
            self.pending_pair = Some((idx, val, vt));
            (None, Ty::Unit)
        } else if enumerate {
            self.make_pair(idx, val, vt)?
        } else {
            (val, vt)
        };
        self.for_body(f, val, vt, step, exit)?;
        self.b.switch_to_block(step);
        if !rev {
            let iv = self.b.use_var(i);
            let nx = self.b.ins().iadd_imm_s(iv, 1);
            self.b.def_var(i, nx);
        }
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(exit);
        Ok((None, Ty::Unit))
    }

    /// enumerate 用の (usize, T) を一時領域に作る
    pub(crate) fn make_pair(&mut self, idx: Value, v: Option<Value>, t: Ty) -> R<V> {
        let base = self.prog.tuple_base(2);
        let inst = self.prog.instantiate(base, vec![Ty::Usize, t])?;
        let tt = Ty::Struct(inst);
        let a = self.temp(tt);
        let f = self.prog.structs[inst as usize].fields.clone();
        self.write(Place::Mem(a, f[0].2 as i32, Ty::Usize), Some(idx));
        self.write(Place::Mem(a, f[1].2 as i32, t), v);
        Ok((Some(a), tt))
    }

    /// ループ本体: パターンに値を束縛して本体を実行し、step (continue 先) へ
    /// E0005: the pattern of a `for` must cover the element type (`for Some(x) in xs` does not)
    pub(crate) fn for_pat_check(&self, p: &Pat, t: Ty) -> R<()> {
        if self.user_code && !super::plainly_irrefutable(p) {
            self.lit_pat_type_check(p, t)?;
            if let Some(w) = self.non_exhaustive_pats(&[p], t) {
                return err(SP, format!("refutable pattern in `for` loop binding: `{w}` not covered (E0005)"));
            }
        }
        Ok(())
    }

    /// A pattern coverage does not model (`[Some(x)]`): tested at run time, a trap where it does not match, so nothing
    /// binds from a payload that is not there
    pub(crate) fn runtime_pat_guard(&mut self, p: &Pat, place: Place) -> R<()> {
        if self.user_code && !super::plainly_irrefutable(p) && self.coverage(&[p], place.ty()).is_none() {
            let trap = self.b.create_block();
            self.pat_test(p, place, trap)?;
            let cur = self.b.create_block();
            self.b.ins().jump(cur, &[]);
            self.b.switch_to_block(trap);
            self.b.ins().trap(TrapCode::unwrap_user(2));
            self.b.switch_to_block(cur);
        }
        Ok(())
    }

    pub(crate) fn for_body(&mut self, f: &Expr, val: Option<Value>, vt: Ty, step: Block, exit: Block) -> R<()> {
        let Expr::For(_, fpat, _, fbody) = f else { unreachable!() };
        self.scopes.push(vec![]);
        let to_place = |this: &mut Self, val: Option<Value>, vt: Ty| match val {
            Some(a) if vt.is_agg() => Place::Mem(a, 0, vt),
            Some(v) => {
                let var = this.b.declare_var(clif_ty(vt).unwrap());
                this.b.def_var(var, v);
                Place::Var(var, vt)
            }
            None => Place::Zst(vt),
        };
        if let (Some((idx, v, t)), Pat::Tuple(tp)) = (self.pending_pair.take(), &**fpat) {
            let pi = to_place(self, Some(idx), Ty::Usize);
            self.for_pat_check(&tp[0], Ty::Usize)?;
            self.runtime_pat_guard(&tp[0], pi)?;
            self.pat_bind(&tp[0], pi, BindMode::Move)?;
            let pv = to_place(self, v, t);
            self.for_pat_check(&tp[1], t)?;
            self.runtime_pat_guard(&tp[1], pv)?;
            self.pat_bind(&tp[1], pv, BindMode::Move)?;
        } else {
            let place = to_place(self, val, vt);
            // (`for _ in arr`: `_` binds nothing; the element moved out of arr is dropped at once)
            if let (Pat::Wild, Place::Mem(a, 0, t)) = (&**fpat, place) {
                if !matches!(t, Ty::Ref(..)) && self.needs_drop(t) {
                    self.drop_in_place(a, t);
                }
            }
            self.rg_for_item(f, vt);
            self.for_pat_check(fpat, place.ty())?;
            self.runtime_pat_guard(fpat, place)?;
            self.pat_bind(fpat, place, BindMode::Move)?;
            self.rg.pat = None;
        }
        let bound = self.pending_bound.take();
        let nb = self.loop_bounds.len();
        if let (Some(h), Pat::Ident(by_ref, mutable, name, _)) = (bound, &**fpat) {
            if !*mutable && !*by_ref {
                if let Some((l, _)) = self.lookup(name) {
                    self.loop_bounds.push((l, h));
                }
            }
        }
        self.loops.push(Loop {
            cont: step,
            brk: exit,
            breaks: 0,
            label: self.pending_label.take(),
            val: None,
            is_block: false,
            // パターンの束縛のスコープも break で drop する
            depth: self.scopes.len() - 1,
            temps: self.temps.len(),
            no_value: Some("for"),
        });
        let (_, bt) = self.block(fbody, Some(Ty::Unit))?;
        self.loops.pop();
        self.loop_bounds.truncate(nb);
        self.pop_scope();
        if bt != Ty::Never {
            self.same(SP, bt, Ty::Unit)?;
            self.b.ins().jump(step, &[]);
        } else {
            self.b.ins().trap(TrapCode::unwrap_user(1));
        }
        Ok(())
    }

    /// `for i in lo..hi` / `lo..=hi` / `(lo..hi).rev()`
    pub(crate) fn for_range(&mut self, f: &Expr, rg: &Expr, rev: bool, enumerate: bool) -> R<V> {
        let Expr::For(_, fpat, _, fbody) = f else { unreachable!() };
        let Expr::Range(start, end, closed) = rg else { unreachable!() };
        let Some(lo_e) = start else {
            return err(SP, "for loops need a range with a start");
        };
        // `for i in 0..`: 型の最大値までの閉区間 (事実上終わらない)
        if end.is_none() && rev {
            return err(SP, "cannot reverse an unbounded range");
        }
        let hint = self.peek(lo_e).or_else(|| end.as_ref().and_then(|h| self.peek(h)));
        let (lo, lt) = self.expr(lo_e, hint)?;
        if !lt.is_int() {
            return err(SP, "range bounds must be integers");
        }
        let lo = lo.unwrap();
        let (hi, inclusive) = match end {
            Some(hi_e) => {
                let (hi, ht) = self.expr(hi_e, Some(lt))?;
                self.same(SP, ht, lt)?;
                (hi.unwrap(), *closed)
            }
            None => (self.int_const(lt, crate::program::prim_assoc_const(lt, "MAX").unwrap()), true),
        };
        let s = lt.signed();
        let ct = clif_ty(lt).unwrap();
        let lt_cc = if s { IntCC::SignedLessThan } else { IntCC::UnsignedLessThan };
        let le_cc = if s { IntCC::SignedLessThanOrEqual } else { IntCC::UnsignedLessThanOrEqual };
        // (`for k in 0..3`: 下限が 0 以上・上限が定数なら、k はその上限より小さい)
        self.pending_bound = match (self.iconst_of(lo), self.iconst_of(hi)) {
            (Some(l), Some(h)) if l >= 0 && h >= 0 => Some(h as u64 + inclusive as u64),
            _ => None,
        };
        // -O: 回数が定数で小さい (4 回まで) ループは展開する (`for k in 0..3`: k が定数になり、添字がアドレスの定数に畳まれる)
        if self.prog.opt && !enumerate {
            if let (Some(l), Some(h)) = (self.iconst_of(lo), self.iconst_of(hi)) {
                let h = h + inclusive as i64;
                if l <= h && h - l <= 4 {
                    // (E0005 also for a loop that runs no time: `for 0 in 0..0 {}`)
                    if let Expr::For(_, fp, _, _) = f {
                        self.for_pat_check(fp, lt)?;
                    }
                    let bound = self.pending_bound;
                    let exit = self.b.create_block();
                    let vals: Vec<i64> = if rev { (l..h).rev().collect() } else { (l..h).collect() };
                    for v in vals {
                        let step = self.b.create_block();
                        let iv = self.b.ins().iconst(ct, v);
                        self.pending_bound = bound;
                        self.for_body(f, Some(iv), lt, step, exit)?;
                        self.b.switch_to_block(step);
                    }
                    self.b.ins().jump(exit, &[]);
                    self.b.switch_to_block(exit);
                    return Ok((None, Ty::Unit));
                }
            }
        }
        let i = self.b.declare_var(ct);
        // (enumerate の番号。enumerate でなければ数えない)
        let n = self.b.declare_var(types::I64);
        let z = self.b.ins().iconst(types::I64, 0);
        self.b.def_var(n, z);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let step = self.b.create_block();
        let exit = self.b.create_block();
        // 空かどうか
        let nonempty = self.b.ins().icmp(if inclusive { le_cc } else { lt_cc }, lo, hi);
        // 現在値: 前向きなら lo から、逆向きなら最後の要素から
        let first = if !rev {
            lo
        } else if inclusive {
            hi
        } else {
            self.b.ins().iadd_imm_s(hi, -1)
        };
        self.b.def_var(i, first);
        self.b.ins().brif(nonempty, head, &[], exit, &[]);
        self.b.switch_to_block(head);
        // -O: ループの中で変わらない添字の受け手の先頭と長さを、ループの前で読んでおく
        let nh = self.hoisted.len();
        if self.prog.opt {
            self.hoist_index_bases(fbody, Some(fpat), None)?;
        }
        self.b.ins().jump(body, &[]);
        self.b.switch_to_block(body);
        let iv = self.b.use_var(i);
        let tuple_pat = matches!(&**fpat, Pat::Tuple(t) if t.len() == 2);
        let (val, vt) = if enumerate && tuple_pat {
            let nv = self.b.use_var(n);
            self.pending_pair = Some((nv, Some(iv), lt));
            (None, Ty::Unit)
        } else if enumerate {
            let nv = self.b.use_var(n);
            self.make_pair(nv, Some(iv), lt)?
        } else {
            (Some(iv), lt)
        };
        self.for_body(f, val, vt, step, exit)?;
        self.b.switch_to_block(step);
        let iv = self.b.use_var(i);
        if enumerate {
            let nv = self.b.use_var(n);
            let nn = self.b.ins().iadd_imm_s(nv, 1);
            self.b.def_var(n, nn);
        }
        // 最後の要素を処理したら抜ける (lo..=MAX でも溢れない)
        let last = if !rev {
            if inclusive { hi } else { self.b.ins().iadd_imm_s(hi, -1) }
        } else {
            lo
        };
        let done = self.b.ins().icmp(IntCC::Equal, iv, last);
        let nx = if rev { self.b.ins().iadd_imm_s(iv, -1) } else { self.b.ins().iadd_imm_s(iv, 1) };
        let cont = self.b.create_block();
        self.b.ins().brif(done, exit, &[], cont, &[]);
        self.b.switch_to_block(cont);
        self.b.def_var(i, nx);
        self.b.ins().jump(body, &[]);
        self.b.switch_to_block(exit);
        self.hoisted.truncate(nh);
        Ok((None, Ty::Unit))
    }

    /// ループの本体 body で、添字の受け手 `x` / `x[i]` (x はローカルの Vec、i は本体で変わらない変数) が
    /// ループの中で変わらないなら、その先頭と長さをここ (ループの前) で読み、indexable がそれを使う。
    /// 変わらない: 本体の中で x が `x[..]..` の形 (読むか、要素がスカラーの代入) でしか現れず、
    /// x や i を束縛し直さず、x を含むマクロもクロージャもない。`x[i]` の i の検査は本体の中で (前で比べた値で) 行う
    /// (hoisted の鍵は受け手の式の Debug の文字列 `format!("{e:?}")`)
    pub(crate) fn hoist_index_bases(&mut self, body: &PBlock, pat: Option<&Pat>, cond: Option<&Expr>) -> R<()> {
        let mut sc = HoistScan { chains: vec![], other: Default::default(), assigned: Default::default(), bail: false };
        sc.visit_block(body);
        if let Some(p) = pat {
            sc.visit_pat(p);
        }
        if let Some(c) = cond {
            sc.visit_expr(c);
        }
        if sc.bail {
            return Ok(());
        }
        // 代入の左辺の要素はスカラーでなければならない (行 `c[i] = vec![]` は受け手を変える)
        let mut bad: HashSet<String> = sc.other.clone();
        for (r, c, is_assign) in &sc.chains {
            if *is_assign {
                let full = c.last().map(|b| format!("{b:?}"));
                let _ = full;
                // (左辺の型: 受け手の要素の型)
                // (根の型から添字の数だけ要素の型をたどる)
                let elem_of = |this: &Self, t: Ty| -> Option<Ty> {
                    match this.strip_refs(t) {
                        Ty::Struct(sid) => this.vec_fields(sid).map(|f| f.2),
                        Ty::Array(e, _) | Ty::Slice(e) => Some(*e),
                        _ => None,
                    }
                };
                let mut t = self.lookup(r).map(|x| x.1);
                for _ in 0..c.len() {
                    t = t.and_then(|t| elem_of(self, t));
                }
                let elem_scalar = t.is_some_and(|t| !t.is_agg());
                if !elem_scalar {
                    bad.insert(r.clone());
                }
            }
        }
        let mut done: HashSet<String> = Default::default();
        for (r, c, _) in sc.chains.clone() {
            if bad.contains(&r) {
                continue;
            }
            // 根はループの前からあるローカルの Vec
            // (`&Vec` / `&mut Vec` の引数も: 本体で変えるならメソッド呼び出しか代入として現れる)
            let Some((_, rt)) = self.lookup(&r) else { continue };
            let Ty::Struct(rsid) = self.strip_refs(rt) else { continue };
            if self.vec_fields(rsid).is_none() {
                continue;
            }
            for (k, b) in c.iter().enumerate() {
                let key = format!("{b:?}");
                if done.contains(&key) || self.hoisted.iter().any(|h| h.0 == key) {
                    continue;
                }
                if k == 0 {
                    let (base, len, elem) = self.indexable(b)?;
                    self.hoisted.push((key.clone(), base, len, elem, None));
                    done.insert(key);
                    continue;
                }
                // `x[i]`: i はループの前からある、本体で代入も束縛し直しもされない変数
                let Expr::Index(_, ix, _) = b else { break };
                let Expr::Path(_, ip) = &**ix else { break };
                let Some(iname) = ident_of(ip).map(|i| i.to_string()) else { break };
                if sc.assigned.contains(&iname) || self.lookup(&iname).is_none() {
                    break;
                }
                let pkey = format!("{:?}", c[k - 1]);
                let Some(&(_, pb, pl, pe, _)) = self.hoisted.iter().rev().find(|h| h.0 == pkey) else { break };
                let Ty::Struct(esid) = pe else { break };
                let Some((po, lo, ee)) = self.vec_fields(esid) else { break };
                let (iv, it) = self.expr(ix, Some(Ty::Usize))?;
                let (Some(iv), Ty::Usize) = (iv, it) else { break };
                // (範囲外なら読まない: 読む場所をダミーの領域にして、本体の中の検査で panic する)
                let ok = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, pl);
                let at = self.elem_addr(pb, iv, pe);
                let dummy = self.temp(pe);
                let addr = self.b.ins().select(ok, at, dummy);
                let rb = self.b.ins().load(PTR, mf(), addr, po as i32);
                let rl = self.b.ins().load(types::I64, mf(), addr, lo as i32);
                self.hoisted.push((key.clone(), rb, rl, ee, Some((ok, pl, iv))));
                done.insert(key);
            }
        }
        Ok(())
    }

    // --- panic / マクロ / `?` ---------------------------------------------

    /// cond が立っていれば、算術の検査の panic (kind: 0 足し算 1 引き算 2 掛け算 3 0 除算 4 0 の剰余
    /// 5 (未使用) 6 符号反転 7 左シフト 8 右シフト 9 割り算のあふれ 10 剰余のあふれ)
    pub(crate) fn ovf_panic(&mut self, cond: Value, kind: u8, span: Sp) {
        let _ = (kind, span);
        // std のプログラム: 条件つき trap (main が登録したハンドラが panic として 101 で終わらせる)。
        // 分岐とブロックを増やさないので、検査のコンパイルの重さは命令 1 つ分
        if self.prog.panic_handler.is_none() {
            let code = self.trap_site(kind, span, None);
            self.b.ins().trapnz(cond, code);
            return;
        }
        // (`#[panic_handler]` があればそれを呼ぶ)
        let cont = self.b.create_block();
        let pb = self.b.create_block();
        self.b.set_cold_block(pb);
        self.b.ins().brif(cond, pb, &[], cont, &[]);
        self.b.switch_to_block(pb);
        self.panic_call();
        self.b.switch_to_block(cont);
    }

    /// panic の trap のコードを 1 つ取り、場所 (種類・行・列) とメッセージを覚える (表は .o に置き、
    /// main が登録したハンドラが発生したアドレスで引く)。数が多すぎれば場所なしのコード 4
    /// span の (ファイルの中の行, 1 始まりの列)
    /// (今は Span がない (Sp = 0) ので、行は 0 (+ line_base)・列は 0 (+ 1) として計算する)
    pub(crate) fn loc(&mut self, span: Sp) -> (u32, u32) {
        self.loc_used = true;
        (span.line + self.line_base, span.col + 1)
    }

    pub(crate) fn trap_site(&mut self, kind: u8, span: Sp, msg: Option<String>) -> TrapCode {
        let k = self.traps.len();
        if k >= TRAP_SITE_MAX {
            return TrapCode::unwrap_user(4);
        }
        let info = if self.caller_loc {
            (kind as u32) << 27
        } else {
            // (行の上位ビットのクレートの印は trap の表には入れない: .o を書くときに行をずらす)
            let (l, c) = self.loc(span);
            let l = l & 0xFFFFF;
            ((kind as u32) << 27) | (l.min(0xFFFF) << 11) | c.min(0x7FF)
        };
        let m = match msg {
            Some(m) => {
                let bytes = m.into_bytes();
                match self.datas.iter().position(|d| *d == bytes) {
                    Some(i) => i as u32,
                    None => {
                        self.datas.push(bytes);
                        self.datas.len() as u32 - 1
                    }
                }
            }
            None => u32::MAX,
        };
        self.traps.push((info, m));
        TrapCode::unwrap_user(TRAP_SITE_BASE + k as u8)
    }

    /// 値が整数の定数ならその値 (`x % 4` の 0 除算の検査を省く)
    pub(crate) fn iconst_of(&self, v: Value) -> Option<i64> {
        self.b.iconst_value(v)
    }

    /// panic する: `#[panic_handler]` があればそれを呼び (戻らない)、なければ trap する
    pub(crate) fn panic_call(&mut self) -> V {
        if let Some(h) = self.prog.panic_handler {
            let sig = self.prog.sigs[h as usize].clone();
            let info_ty = match sig.params.first() {
                Some(Ty::Ref(_, t)) => **t,
                _ => Ty::U8,
            };
            let info = self.temp(info_ty);
            let f = self.fref(h, &sig);
            self.b.ins().call(f, &[info]);
        }
        self.b.ins().trap(TrapCode::unwrap_user(3));
        self.dead()
    }

    /// `vec![a, b, c]` / `vec![x; n]`: Vec::with_capacity と push に展開する
    pub(crate) fn vec_macro(&mut self, p: &Path, g: &str, expected: Option<Ty>) -> R<V> {
        let span = SP;
        let mut elem = match expected {
            Some(Ty::Struct(sid)) => self.vec_fields(sid).map(|f| f.2),
            _ => None,
        };
        let repeat = crate::ast_closure::vec_repeat_parts(g, self.mbase);
        if let Some((x, n)) = repeat {
            let x: &Expr = self.keep_expr(x);
            let n: &Expr = self.keep_expr(n);
            let (xv, xt) = self.expr(x, elem)?;
            let (nv, nt) = self.expr(n, Some(Ty::Usize))?;
            self.same(SP, nt, Ty::Usize)?;
            let nv = nv.unwrap();
            // (drop の要る要素は clone して並べる: `Vec::new()` + `resize(n, x)`)
            if self.needs_drop(xt) {
                let Some(xv) = xv else { return Ok((None, Ty::Never)) };
                if !self.prog.index.contains_key("Vec::resize") {
                    return err(SP, "`vec![x; n]` needs a `Copy` element");
                }
                let (v, vt) = self.call_named("Vec::new", vec![xt], &[], span)?;
                let v = v.unwrap();
                let vr = if vt.is_agg() { v } else { unreachable!() };
                self.call_named("Vec::resize", vec![xt], &[(vr, Ty::Ref(true, intern(vt))), (nv, Ty::Usize), (xv, xt)], span)?;
                return Ok((Some(v), vt));
            }
            let (v, vt) = self.call_named("Vec::with_capacity", vec![xt], &[(nv, Ty::Usize)], span)?;
            let v = v.unwrap();
            // 確保した領域に直接書いて、最後に長さを n にする (push の呼び出しと容量の検査をしない)
            if let (Some(xv), Ty::Struct(vsid)) = (xv, vt) {
                if let Some((po, lo, _)) = self.vec_fields(vsid) {
                    let (es, _) = self.prog.size_align(xt);
                    let p = self.b.ins().load(PTR, mf(), v, po as i32);
                    // (1 byte の要素 (`vec![true; n]` / `vec![0u8; n]`) は memset)
                    if es == 1 && clif_ty(xt) == Some(types::I8) && self.prog.index.contains_key("__exr_memset") {
                        let c = self.b.ins().uextend(types::I32, xv);
                        self.call_named("__exr_memset", vec![], &[(p, Ty::Ptr(true, intern(Ty::U8))), (c, Ty::I32), (nv, Ty::Usize)], span)?;
                        self.b.ins().store(mf(), nv, v, lo as i32);
                        return Ok((Some(v), vt));
                    }
                    let i = self.b.declare_var(types::I64);
                    let z = self.b.ins().iconst(types::I64, 0);
                    self.b.def_var(i, z);
                    let head = self.b.create_block();
                    let body = self.b.create_block();
                    let done = self.b.create_block();
                    self.b.ins().jump(head, &[]);
                    self.b.switch_to_block(head);
                    let iv = self.b.use_var(i);
                    let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, nv);
                    self.b.ins().brif(more, body, &[], done, &[]);
                    self.b.switch_to_block(body);
                    let off = self.b.ins().imul_imm_s(iv, es.max(1) as i64);
                    let a = self.b.ins().iadd(p, off);
                    if es > 0 {
                        self.write(Place::Mem(a, 0, xt), Some(xv));
                    }
                    let next = self.b.ins().iadd_imm_s(iv, 1);
                    self.b.def_var(i, next);
                    self.b.ins().jump(head, &[]);
                    self.b.switch_to_block(done);
                    self.b.ins().store(mf(), nv, v, lo as i32);
                    return Ok((Some(v), vt));
                }
            }
            // for i in 0..n { v.push(x) }
            let i = self.b.declare_var(types::I64);
            let z = self.b.ins().iconst(types::I64, 0);
            self.b.def_var(i, z);
            let head = self.b.create_block();
            let body = self.b.create_block();
            let done = self.b.create_block();
            self.b.ins().jump(head, &[]);
            self.b.switch_to_block(head);
            let iv = self.b.use_var(i);
            let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, nv);
            self.b.ins().brif(more, body, &[], done, &[]);
            self.b.switch_to_block(body);
            if let Some(xv) = xv {
                self.call_named("Vec::push", vec![xt], &[(v, Ty::Ref(true, intern(vt))), (xv, xt)], span)?;
            }
            let next = self.b.ins().iadd_imm_s(iv, 1);
            self.b.def_var(i, next);
            self.b.ins().jump(head, &[]);
            self.b.switch_to_block(done);
            return Ok((Some(v), vt));
        }
        let args = self.macro_args(p, g)?;
        let mut vals = vec![];
        for a in args {
            let a: &Expr = self.keep_expr(a);
            // (`vec![&f()]`: a macro's arguments are not extended, the temporary goes away with the statement)
            let d0 = self.bk.direct.len();
            if self.bk_on() && !self.bk.macro_let {
                borrowck::direct_ref(a, &mut self.bk.direct);
            }
            let r = self.expr(a, elem);
            self.bk.direct.truncate(d0);
            let (v, t) = r?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            match elem {
                Some(e) => self.same(SP, t, e)?,
                None => elem = Some(t),
            }
            vals.push(v);
        }
        let Some(e) = elem else {
            return err(span, "type annotations needed for an empty `vec![]`");
        };
        let n = self.b.ins().iconst(types::I64, vals.len() as i64);
        let (v, vt) = self.call_named("Vec::with_capacity", vec![e], &[(n, Ty::Usize)], span)?;
        let v = v.unwrap();
        for x in vals.into_iter().flatten() {
            self.call_named("Vec::push", vec![e], &[(v, Ty::Ref(true, intern(vt))), (x, e)], span)?;
        }
        Ok((Some(v), vt))
    }

    pub(crate) fn macro_args(&self, p: &Path, g: &str) -> R<Vec<Expr>> {
        match crate::ast_closure::macro_args_at(g, self.mbase) {
            Some(v) => Ok(v),
            None => {
                // (the parser's own error for an argument that is no expression: `assert!({one! two})`)
                let inner = g.get(1..g.len().saturating_sub(1)).unwrap_or("");
                for part in split_top(inner, ",").unwrap_or_default() {
                    // (`L:C: expected one of ..` with a token there, `end of input: expected one of ..` at the end)
                    if let Err(e) = crate::parse::parse_expr(&part) {
                        if let Some(i) = e.find("expected one of") {
                            return err(SP, e[i..].trim().to_string());
                        }
                    }
                }
                let name = p.segs.last().map(|s| s.name.as_str()).unwrap_or("");
                err(SP, format!("cannot parse the arguments of `{name}!`"))
            }
        }
    }

    /// 対応しているマクロ: panic! / unreachable! / todo! / unimplemented! / assert! / assert_eq! /
    /// assert_ne! (と debug_ 版) / matches!。メッセージの書式化はしない
    pub(crate) fn macro_(&mut self, p: &Path, g: &str, expected: Option<Ty>) -> R<V> {
        let name = p.segs.last().map(|s| s.name.clone()).unwrap_or_default();
        match name.as_str() {
            "vec" => self.vec_macro(p, g, expected),
            "format" | "print" | "println" | "eprint" | "eprintln" | "write" | "writeln" => self.fmt_macro(p, g, &name),
            // `dbg!(e)`: `[file:line:col] e = {:#?}` を stderr に出して e を返す
            "dbg" => {
                let args = self.macro_args(p, g)?;
                let (sl, sc) = self.loc(self.mstart);
                // (`dbg!()`: only the place; `dbg!(a, b)`: each one, as a tuple)
                if args.len() != 1 {
                    let ex = if args.is_empty() {
                        Expr::Macro(path_of("eprintln"), format!("(\"[src/main.rs:{sl}:{sc}]\")"), crate::parse::Span::default())
                    } else {
                        let texts = macro_arg_texts(g);
                        let sp = crate::parse::Span { start: self.mstart, at: self.mstart };
                        Expr::Tuple(texts.iter().map(|t| Expr::Macro(path_of("dbg"), format!("({t})"), sp)).collect())
                    };
                    let ex: &Expr = self.keep_expr(ex);
                    return self.expr(ex, expected);
                }
                let e = &args[0];
                let text = tokens_text(macro_arg_texts(g).first().map(|s| s.as_str()).unwrap_or(""));
                let head = format!("[src/main.rs:{sl}:{sc}] {} = ", text);
                let ex = Expr::Block(
                    None,
                    false,
                    PBlock {
                        stmts: vec![
                            let_stmt("__exr_dbg", e.clone()),
                            Stmt::Expr(Expr::Macro(path_of("eprintln"), format!("(\"{{}}{{:#?}}\", {head:?}, __exr_dbg)"), crate::parse::Span::default()), true),
                            Stmt::Expr(path_expr("__exr_dbg"), false),
                        ],
                    },
                );
                let ex: &Expr = self.keep_expr(ex);
                self.expr(ex, expected)
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                if self.prog.panic_handler.is_none() {
                    // メッセージ: 文字列だけならそのまま、書式つきなら書式化してから prelude の関数で panic する
                    let args = self.macro_args(p, g).unwrap_or_default();
                    let head = match name.as_str() {
                        "unreachable" => "internal error: entered unreachable code",
                        "todo" => "not yet implemented",
                        "unimplemented" => "not implemented",
                        _ => "",
                    };
                    let lit = match args.first().and_then(str_lit) {
                        Some(s) if args.len() == 1 && !s.contains('{') => Some(s),
                        _ => None,
                    };
                    // (書式化の部品がない (書式化を使わない) プログラムでは、書式の文字列の固定の部分だけを出す)
                    let lit = if lit.is_none() && !self.prog.index.contains_key("__exr_panic_fmt") {
                        match args.first().and_then(str_lit) {
                            Some(v) => {
                                let fixed = v.split('{').next().unwrap_or("").trim_end_matches(": ").to_string();
                                if fixed.is_empty() { None } else { Some(fixed) }
                            }
                            _ => None,
                        }
                    } else {
                        lit
                    };
                    if !args.is_empty() && lit.is_none() {
                        if self.prog.index.contains_key("__exr_panic_fmt") {
                            let e: &Expr = self.keep_expr(Expr::Macro(path_of("format"), g.to_string(), crate::parse::Span::default()));
                            let (s, st) = self.expr(e, None)?;
                            if let Some(s) = s {
                                let pre = if head.is_empty() { String::new() } else { format!("{head}: ") };
                                let pv = self.str_const(&pre);
                                let (sl, sc) = if self.caller_loc {
                                    (0, 0)
                                } else {
                                    let (l, c) = self.loc(self.mstart);
                                    (l as i64, c as i64)
                                };
                                let l = self.b.ins().iconst(types::I32, sl);
                                let c = self.b.ins().iconst(types::I32, sc);
                                let sref = Ty::Ref(false, intern(Ty::Str));
                                self.call_named("__exr_panic_fmt", vec![Ty::Unit], &[(pv, sref), (s, Ty::Ref(false, intern(st))), (l, Ty::U32), (c, Ty::U32)], SP)?;
                            }
                            self.b.ins().trap(TrapCode::unwrap_user(3));
                            return Ok(self.dead());
                        }
                    }
                    let msg = match (lit, head) {
                        (Some(m), "") => m,
                        (Some(m), h) => format!("{h}: {m}"),
                        (None, "") => "explicit panic".to_string(),
                        (None, h) => h.to_string(),
                    };
                    let code = self.trap_site(12, self.mstart, Some(msg));
                    self.b.ins().trap(code);
                    return Ok(self.dead());
                }
                Ok(self.panic_call())
            }
            "assert" | "debug_assert" => {
                let args = self.macro_args(p, g)?;
                let Some(c) = args.first() else {
                    return err(SP, "assert! needs a condition");
                };
                let c: &Expr = self.keep_expr(c.clone());
                let (cv, ct) = self.expr(c, Some(Ty::Bool))?;
                self.same(SP, ct, Ty::Bool)?;
                if let Some(cv) = cv {
                    let text = tokens_text(macro_arg_texts(g).first().map(|s| s.as_str()).unwrap_or(""));
                    let msg = match args.get(1).and_then(str_lit) {
                        Some(s) if args.len() == 2 => s,
                        _ => format!("assertion failed: {text}"),
                    };
                    self.panic_unless_at(cv, 12, self.mstart, Some(msg));
                }
                Ok((None, Ty::Unit))
            }
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne" => {
                let args = self.macro_args(p, g)?;
                if args.len() < 2 {
                    return err(SP, format!("{name}! needs two arguments"));
                }
                // `a == b` / `a != b` として比べる (構造的な比較・手書きの eq・String と &str も)
                let (a, b) = (&args[0], &args[1]);
                let paren = |x: &Expr| Expr::Paren(Box::new(x.clone()));
                // 書式化が使えれば、rustc と同じく left / right の値 ({:?}) も出す (a と b は 1 回だけ評価する)
                if self.prog.panic_handler.is_none() && self.prog.index.contains_key("__exr_panic_fmt") {
                    let op = if name.ends_with("_eq") { "==" } else { "!=" };
                    let (l, c) = if self.caller_loc { (0u32, 0u32) } else { self.loc(self.mstart) };
                    let deref = |n: &str| Expr::Unary("*".into(), Box::new(path_expr(n)), crate::parse::Span::default());
                    let cmp = Expr::Binary(op.to_string(), Box::new(deref("__exr_l")), Box::new(deref("__exr_r")), crate::parse::Span::default());
                    let fail_src = if args.len() > 2 {
                        let fmt = format!("assertion `left {op} right` failed: {{}}\n  left: {{:?}}\n right: {{:?}}");
                        let rest = macro_arg_texts(g)[2..].join(",");
                        format!("{{ let __exr_m = format!({rest}); __exr_panic_fmt::<()>(\"\", &format!({fmt:?}, __exr_m, __exr_l, __exr_r), {l}u32, {c}u32); }}")
                    } else {
                        let fmt = format!("assertion `left {op} right` failed\n  left: {{:?}}\n right: {{:?}}");
                        format!("{{ __exr_panic_fmt::<()>(\"\", &format!({fmt:?}, __exr_l, __exr_r), {l}u32, {c}u32); }}")
                    };
                    let Expr::Block(_, _, fail) = crate::parse::parse_expr(&fail_src)? else {
                        return err(SP, "internal: cannot build the assert_eq! message");
                    };
                    // (左が数のリテラルで右がそうでなければ、右を先に評価して左の型を右から決める (`assert_eq!(2, n)`))
                    let num_lit = |e: &Expr| matches!(e, Expr::Lit(t, _) if matches!(lit::lit(t), Some(Lit::Int(_) | Lit::Float(_))));
                    let lets = if num_lit(a) && !num_lit(b) {
                        vec![let_stmt("__exr_r", Expr::Ref(false, Box::new(paren(b)))), let_stmt("__exr_l", Expr::Ref(false, Box::new(paren(a))))]
                    } else {
                        vec![let_stmt("__exr_l", Expr::Ref(false, Box::new(paren(a)))), let_stmt("__exr_r", Expr::Ref(false, Box::new(paren(b))))]
                    };
                    let mut stmts = lets;
                    stmts.push(Stmt::Expr(Expr::If(Box::new(Expr::Unary("!".into(), Box::new(Expr::Paren(Box::new(cmp))), crate::parse::Span::default())), fail, None), false));
                    let e: &Expr = self.keep_expr(Expr::Block(None, false, PBlock { stmts }));
                    self.expr(e, Some(Ty::Unit))?;
                    return Ok((None, Ty::Unit));
                }
                let op = if name.ends_with("_eq") { "==" } else { "!=" };
                let e: &Expr = self.keep_expr(Expr::Binary(op.to_string(), Box::new(paren(a)), Box::new(paren(b)), crate::parse::Span::default()));
                let (c, ct) = self.expr(e, Some(Ty::Bool))?;
                self.same(SP, ct, Ty::Bool)?;
                if let Some(c) = c {
                    let op = if name.ends_with("_eq") { "==" } else { "!=" };
                    self.panic_unless_at(c, 12, self.mstart, Some(format!("assertion `left {op} right` failed")));
                }
                Ok((None, Ty::Unit))
            }
            "matches" => {
                let (e, pat, guard) = crate::ast_closure::matches_parts(g, self.mbase).ok_or("matches!: cannot parse `(expr, pattern)`")?;
                // (型推論が読んだ同じ式があればそれを使う: 型の記録をアドレスで引ける)
                let e: &Expr = match self.macro_args_of(g) {
                    Some(v) if !v.is_empty() => v[0],
                    _ => self.keep_expr(e),
                };
                let guard: Option<&Expr> = guard.map(|g| self.keep_expr(g));
                // (parse し直した式: 型推論の記録は位置で引く)
                self.in_fmt_args += 1;
                let sc = self.scrutinee(e);
                self.in_fmt_args -= 1;
                let Some(place) = sc? else {
                    return Ok((None, Ty::Never));
                };
                let no = self.b.create_block();
                let merge = self.b.create_block();
                let res = self.b.append_block_param(merge, types::I8);
                self.pat_test(&pat, place, no)?;
                self.scopes.push(vec![]);
                // (束縛は guard でしか使わない: match 対象からムーブしたことにせず、束縛も drop しない (対象の一時値が drop する))
                let n_owned = self.owned.len();
                self.pat_bind(&pat, place, BindMode::Move)?;
                self.owned.truncate(n_owned);
                if let Some(g) = guard {
                    let (c, _) = self.expr(g, Some(Ty::Bool))?;
                    if let Some(c) = c {
                        self.require(c, no);
                    }
                }
                self.pop_scope();
                let one = self.b.ins().iconst(types::I8, 1);
                self.b.ins().jump(merge, &[one]);
                self.b.switch_to_block(no);
                let zero = self.b.ins().iconst(types::I8, 0);
                self.b.ins().jump(merge, &[zero]);
                self.b.switch_to_block(merge);
                let _ = expected;
                Ok((Some(res), Ty::Bool))
            }
            // `offset_of!(Type, field)` / `offset_of!(Type, a.b)`: the field's offset in the layout
            "offset_of" => {
                let a = macro_arg_texts(g);
                let [ty, fields] = a.as_slice() else {
                    return err(SP, "`offset_of!` takes a type and a field");
                };
                let Ok(Expr::Cast(_, pt)) = crate::parse::parse_expr(&format!("0 as {ty}")) else {
                    return err(SP, "`offset_of!`: cannot read the type");
                };
                let mut t = self.parse_ty(&pt)?;
                let mut off = 0u32;
                for f in fields.split('.') {
                    let f = f.trim();
                    let Ty::Struct(sid) = t else {
                        return err(SP, format!("`offset_of!`: `{}` has no fields", self.prog.tn(t)));
                    };
                    let Some((ft, fo)) = self.prog.field(sid, f) else {
                        return err(SP, format!("no field `{f}` on type `{}`", self.prog.tn(t)));
                    };
                    off += fo;
                    t = ft;
                }
                Ok((Some(self.b.ins().iconst(types::I64, off as i64)), Ty::Usize))
            }
            // `addr_of!(p)` / `addr_of_mut!(p)`: `&raw const p` / `&raw mut p`
            "addr_of" | "addr_of_mut" => {
                let a = macro_arg_texts(g);
                let Some(x) = a.first() else {
                    return err(SP, format!("`{name}!` takes a place"));
                };
                // (`&x` as a raw pointer: the same address)
                let src = if name == "addr_of_mut" { format!("&mut ({x})") } else { format!("&({x})") };
                let e = crate::parse::parse_expr_at(&src, self.mbase)?;
                let e: &Expr = self.keep_expr(e);
                let want = expected.filter(|t| matches!(t, Ty::Ptr(..)));
                self.raw_ref += 1;
                let r = self.expr(e, want);
                self.raw_ref -= 1;
                match r? {
                    (v, Ty::Ref(m, i)) => Ok((v, Ty::Ptr(m, i))),
                    r => Ok(r),
                }
            }
            // `try!(e)` (2015): `e?`
            "r#try" | "try" => {
                let args = self.macro_args(p, g)?;
                let Some(x) = args.into_iter().next() else {
                    return err(SP, "`try!` takes an expression");
                };
                let e: &Expr = self.keep_expr(Expr::Try(Box::new(x)));
                self.expr(e, expected)
            }
            // `stringify!(tokens)` / `concat!("a", 1, 'c', true)` / `line!()` / `column!()`: literals
            "stringify" | "concat" | "line" | "column" => {
                let text = match name.as_str() {
                    "stringify" => format!("{:?}", tokens_text(strip_delims(g).unwrap_or(""))),
                    "line" | "column" => {
                        let (l, c) = self.loc(self.mstart);
                        let v = if name == "line" { l } else { c };
                        let e: &Expr = self.keep_expr(Expr::Lit(format!("{v}u32"), crate::parse::Pos::default()));
                        return self.expr(e, expected);
                    }
                    _ => {
                        let mut out = String::new();
                        for a in macro_arg_texts(g) {
                            let a = a.trim();
                            // (only literals: `concat!(x)` with a variable is an error)
                            let lit = matches!(crate::parse::parse_expr(a), Ok(Expr::Lit(..)))
                                || matches!(crate::parse::parse_expr(a), Ok(Expr::Unary(op, x, _)) if op == "-" && matches!(*x, Expr::Lit(..)))
                                || a == "true" || a == "false";
                            if !lit {
                                return err(SP, "expected a literal (only literals, like `\"foo\"`, `-42` and `3.14`, can be passed to `concat!()`)");
                            }
                            match crate::ast_consteval::str_lit(a) {
                                Some((false, b)) => out.push_str(&String::from_utf8_lossy(&b)),
                                _ if a.starts_with('\'') && a.ends_with('\'') && a.len() >= 3 => {
                                    match crate::parse::parse_expr(a) {
                                        Ok(Expr::Lit(t, _)) => match lit::lit(&t) {
                                            Some(Lit::Char(c)) => out.push(c),
                                            _ => out.push_str(&a[1..a.len() - 1]),
                                        },
                                        _ => out.push_str(&a[1..a.len() - 1]),
                                    }
                                }
                                _ => {
                                    // (numbers and bools as written, without a suffix)
                                    let t: String = a.chars().filter(|&c| c != '_').collect();
                                    let t = t.trim_end_matches(|c: char| c.is_ascii_alphabetic() && !t.starts_with("true") && !t.starts_with("false"));
                                    out.push_str(if a == "true" || a == "false" { a } else { t });
                                }
                            }
                        }
                        format!("{out:?}")
                    }
                };
                let e: &Expr = self.keep_expr(Expr::Lit(text, crate::parse::Pos::default()));
                self.expr(e, expected)
            }
            // `assert_matches!(e, pat)`: `assert!(matches!(e, pat))`; `format_args!(..)`: the formatted String
            "assert_matches" | "debug_assert_matches" => {
                let e: &Expr = self.keep_expr(Expr::Macro(path_of("assert"), format!("(matches!{g})"), crate::parse::Span::default()));
                self.expr(e, expected)
            }
            "format_args" => {
                let e: &Expr = self.keep_expr(Expr::Macro(path_of("format"), g.to_string(), crate::parse::Span::default()));
                self.expr(e, None)
            }
            // `compile_error!("msg")`: the error it names, here
            "compile_error" => {
                let args = self.macro_args(p, g)?;
                match args.first().and_then(str_lit) {
                    Some(m) => err(SP, m),
                    None => err(SP, "compile_error! takes a string literal"),
                }
            }
            // `option_env!("X")`: no build-time environment here
            "option_env" => {
                let args = self.macro_args(p, g)?;
                if args.len() != 1 {
                    return err(SP, "option_env! takes 1 argument");
                }
                if str_lit(&args[0]).is_none() {
                    return err(SP, "argument must be a string literal");
                }
                // (always an `Option<&'static str>`)
                let ob = *self.prog.struct_index.get("Option").ok_or("Option is not available")?;
                let t = Ty::Struct(self.prog.instantiate(ob, vec![Ty::Ref(false, intern(Ty::Str))])?);
                let e: &Expr = self.keep_expr(path_expr("None"));
                self.expr(e, Some(t))
            }
            _ => err(SP, format!("macro `{name}!` is not supported")),
        }
    }

    /// 添字の検査: 範囲外なら `index out of bounds: the len is L but the index is I` で panic する。
    /// 値をメッセージに入れるので trap でなく分岐して prelude の関数を呼ぶ (呼ぶブロックは関数で 1 つを共有し、
    /// 長さ・添字・場所はブロック引数で渡す)
    pub(crate) fn index_check(&mut self, ok: Value, len: Value, i: Value, span: Sp) {
        if self.prog.panic_handler.is_some() || !self.prog.index.contains_key("__exr_panic_index") {
            self.panic_unless_at(ok, 12, span, Some("index out of bounds".into()));
            return;
        }
        // (同じブロックの中で、同じ添字と長さの検査はもう通っている)
        let key = match self.iconst_of(len) {
            Some(n) => LenKey::Const(n),
            None => LenKey::Val(len),
        };
        let cur = self.b.current_block();
        if self.checked.0 != cur {
            self.checked = (cur, vec![]);
        }
        if self.checked.1.contains(&(i, key)) {
            return;
        }
        // -O: 場所ごとに冷たいブロックを作り、場所の定数はそこで作る (熱い道で定数をレジスタに置かない)
        if self.prog.opt {
            let pb = self.b.create_block();
            self.b.set_cold_block(pb);
            let cont = self.b.create_block();
            self.b.ins().brif(ok, cont, &[], pb, &[]);
            self.b.switch_to_block(pb);
            let (sl, sc) = self.loc(span);
            let ln = self.b.ins().iconst(types::I32, sl as i64);
            let cl = self.b.ins().iconst(types::I32, sc as i64);
            let _ = self.call_named("__exr_panic_index", vec![Ty::Unit], &[(len, Ty::Usize), (i, Ty::Usize), (ln, Ty::U32), (cl, Ty::U32)], span);
            self.b.ins().trap(TrapCode::unwrap_user(3));
            self.b.switch_to_block(cont);
            let prev = std::mem::take(&mut self.checked.1);
            self.checked = (Some(cont), prev);
            self.checked.1.push((i, key));
            return;
        }
        let pb = match self.index_panic_block {
            Some(b) => b,
            None => {
                let pb = self.b.create_block();
                self.b.set_cold_block(pb);
                let l = self.b.append_block_param(pb, types::I64);
                let x = self.b.append_block_param(pb, types::I64);
                let ln = self.b.append_block_param(pb, types::I32);
                let cl = self.b.append_block_param(pb, types::I32);
                let cur = self.b.current_block().unwrap();
                self.b.switch_to_block(pb);
                let _ = self.call_named("__exr_panic_index", vec![Ty::Unit], &[(l, Ty::Usize), (x, Ty::Usize), (ln, Ty::U32), (cl, Ty::U32)], span);
                self.b.ins().trap(TrapCode::unwrap_user(3));
                self.b.switch_to_block(cur);
                self.index_panic_block = Some(pb);
                pb
            }
        };
        let (sl, sc) = self.loc(span);
        let ln = self.b.ins().iconst(types::I32, sl as i64);
        let cl = self.b.ins().iconst(types::I32, sc as i64);
        let cont = self.b.create_block();
        self.b.ins().brif(ok, cont, &[], pb, &[len, i, ln, cl]);
        self.b.switch_to_block(cont);
        let prev = std::mem::take(&mut self.checked.1);
        self.checked = (Some(cont), prev);
        self.checked.1.push((i, key));
    }

    /// cond が偽なら panic する (kind と msg は std のプログラムでの panic のメッセージ)
    pub(crate) fn panic_unless_at(&mut self, cond: Value, kind: u8, span: Sp, msg: Option<String>) {
        if self.prog.panic_handler.is_none() {
            let code = self.trap_site(kind, span, msg);
            self.b.ins().trapz(cond, code);
            return;
        }
        self.panic_unless(cond);
    }

    /// cond が偽なら panic する
    pub(crate) fn panic_unless(&mut self, cond: Value) {
        let ok = self.b.create_block();
        let bad = self.b.create_block();
        self.b.ins().brif(cond, ok, &[], bad, &[]);
        self.b.switch_to_block(bad);
        self.panic_call();
        self.b.ins().trap(TrapCode::unwrap_user(3));
        self.b.switch_to_block(ok);
    }

    pub(crate) fn store_tag(&mut self, a: Value, inst: u32, vi: usize) {
        let sd = &self.prog.structs[inst as usize];
        let (tag, discr) = (sd.tag, sd.variants[vi].discr);
        let tv = self.b.ins().iconst(clif_ty(tag).unwrap(), discr);
        self.b.ins().store(mf(), tv, a, 0);
    }

    /// `e?`: Option なら None、Result なら Err(e) をそのまま return する
    pub(crate) fn try_(&mut self, t: &Expr) -> R<V> {
        let Expr::Try(texpr) = t else { unreachable!() };
        let Some(place) = self.scrutinee(texpr)? else {
            return Ok((None, Ty::Never));
        };
        // `?` は中身 (Ok / Some の値か、Err の値) をムーブするので、一時値 (や変数) はもう drop しない
        let sflag = self.last_scrut;
        self.release(texpr, sflag)?;
        let (place, _) = self.pat_deref(place, BindMode::Move);
        let Ty::Struct(inst) = place.ty() else {
            return err(SP, format!("the `?` operator cannot be applied to type `{}`", self.prog.tn(place.ty())));
        };
        let family = self.prog.structs[self.prog.base_of(inst) as usize].name.clone();
        let (okn, errn) = match family.as_str() {
            "Option" => ("Some", "None"),
            "Result" => ("Ok", "Err"),
            _ => {
                return err(SP, format!("the `?` operator cannot be applied to type `{}`", self.prog.tn(place.ty())));
            }
        };
        let vpos = |this: &Self, sid: u32, n: &str| this.prog.structs[sid as usize].variants.iter().position(|v| v.name == n).unwrap();
        let ok_vi = vpos(self, inst, okn);
        let err_vi = vpos(self, inst, errn);
        let fail = self.b.create_block();
        self.test_variant(place, inst, ok_vi, SP, fail)?;
        let (_, oty, ooff) = self.prog.structs[inst as usize].variants[ok_vi].fields[0];
        let (okv, _) = self.read(Self::field_place(place, oty, ooff)?);
        let cont = self.b.create_block();
        let param = clif_ty(oty).map(|ct| self.b.append_block_param(cont, ct));
        match okv {
            Some(v) => self.b.ins().jump(cont, &[v]),
            None => self.b.ins().jump(cont, &[]),
        };
        // 失敗側: 関数の戻り値型の None / Err を作って return
        self.b.switch_to_block(fail);
        let ret = self.ret;
        let rinst = match ret {
            Ty::Struct(r) if self.prog.structs[self.prog.base_of(r) as usize].name == family => r,
            _ => {
                return err(SP, format!("the `?` operator can only be used in a function that returns `{family}` (returns `{}`)", self.prog.tn(ret)));
            }
        };
        let a = self.temp(ret);
        let r_err = vpos(self, rinst, errn);
        self.store_tag(a, rinst, r_err);
        if family == "Result" {
            let (_, ety, eoff) = self.prog.structs[inst as usize].variants[err_vi].fields[0];
            let (_, rety, reoff) = self.prog.structs[rinst as usize].variants[r_err].fields[0];
            let (ev, _) = self.read(Self::field_place(place, ety, eoff)?);
            // エラーの型が違えば `From::from` で変換する (`impl From<ParseIntError> for AppError`)
            let ev = if ety != rety {
                let conv = match rety {
                    Ty::Struct(rs) => self.prog.method(rs, "from").filter(|&id| {
                        let sg = &self.prog.sigs[id as usize];
                        !sg.has_self && sg.params.len() == 1 && (sg.params[0] == ety || self.prog.has_param(sg.params[0]))
                    }),
                    _ => None,
                };
                let dyn_err = matches!(rety, Ty::Struct(rs) if self.prog.structs[rs as usize].name == "__DynError");
                match (conv, ev) {
                    (Some(id), Some(v)) => self.call_fn_vals(id, &[(v, ety)], SP)?.0,
                    // `Box<dyn Error>` へは表示の文字列にして入れる
                    (None, Some(v)) if dyn_err => self.call_named("__exr_dyn_error", vec![ety], &[(v, ety)], SP)?.0,
                    _ => {
                        self.same(SP, ety, rety)?;
                        ev
                    }
                }
            } else {
                ev
            };
            self.write(Place::Mem(a, reoff as i32, rety), ev);
        }
        self.drop_for_exit(0, 0);
        self.emit_return(Some(a));
        self.b.switch_to_block(cont);
        Ok((param, oty))
    }

    /// コンパイラが直接扱う関数: `size_of::<T>()` / `align_of::<T>()` / `ptr::null::<T>()` / `null_mut`
    pub(crate) fn intrinsic_call(&mut self, c: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::Call(func, cargs) = c else {
            return Ok(None);
        };
        let Expr::Path(_, p) = &**func else {
            return Ok(None);
        };
        let last = p.segs.last().unwrap();
        let name = last.name.clone();
        // `u32::from_le_bytes([..])` / `from_be_bytes` / `from_ne_bytes`
        if matches!(name.as_str(), "from_le_bytes" | "from_be_bytes" | "from_ne_bytes") && p.segs.len() == 2 && cargs.len() == 1 {
            if let Some(t) = Ty::prim(&p.segs[0].name).filter(|t| t.is_int()) {
                let n = t.bits() / 8;
                let (v, _) = self.expr(&cargs[0], Some(Ty::Array(intern(Ty::U8), n)))?;
                let Some(a) = v else { return Ok(Some((None, Ty::Never))) };
                let mut fl = MemFlagsData::new();
                fl.set_notrap();
                let x = self.b.ins().load(clif_ty(t).unwrap(), fl, a, 0);
                let x = if name == "from_be_bytes" && n > 1 { self.b.ins().bswap(x) } else { x };
                return Ok(Some((Some(x), t)));
            }
        }
        // `u32::from_be(x)` / `from_le` (x86-64 is little-endian)
        if matches!(name.as_str(), "from_be" | "from_le") && p.segs.len() == 2 && cargs.len() == 1 {
            if let Some(t) = Ty::prim(&p.segs[0].name).filter(|t| t.is_int()) {
                let (v, _) = self.expr(&cargs[0], Some(t))?;
                let Some(a) = v else { return Ok(Some((None, Ty::Never))) };
                let x = if name == "from_be" && t.bits() > 8 { self.b.ins().bswap(a) } else { a };
                return Ok(Some((Some(x), t)));
            }
        }
        // `f64::from_bits(u)` / `f32::from_bits(u)`
        if name == "from_bits" && p.segs.len() == 2 && cargs.len() == 1 && matches!(p.segs[0].name.as_str(), "f64" | "f32") {
            let f64_ = p.segs[0].name == "f64";
            let (v, _) = self.expr(&cargs[0], Some(if f64_ { Ty::U64 } else { Ty::U32 }))?;
            let Some(a) = v else { return Ok(Some((None, Ty::Never))) };
            let ft = if f64_ { types::F64 } else { types::F32 };
            return Ok(Some((Some(self.b.ins().bitcast(ft, MemFlagsData::new(), a)), if f64_ { Ty::F64 } else { Ty::F32 })));
        }
        // `core::arch::wasm32`'s SIMD intrinsics (`f32x4_add(a, b)`): one instruction each
        if crate::simd_sig::maybe_simd(&name) {
            if let Some(r) = self.simd_call(p, cargs)? {
                return Ok(Some(r));
            }
        }
        let n = p.segs.len();
        // `Box::leak(b)`: Box の指す先をそのまま `&'static mut T` に (解放しない)
        if name == "leak" && n >= 2 && p.segs[n - 2].name == "Box" && cargs.len() == 1 {
            let (v, t) = self.expr(&cargs[0], None)?;
            let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
            if let Ty::Box(i) = t {
                // (引数の値は一時値として持たれていないので、drop されない)
                return Ok(Some((Some(v), Ty::Ref(true, i))));
            }
            return err(SP, "`Box::leak` needs a Box");
        }
        // `TypeId::of::<T>()` (a hash of T's name) / `type_name::<T>()` (T's name)
        if (name == "of" && n >= 2 && p.segs[n - 2].name == "TypeId" && self.prog.struct_index.contains_key("TypeId")) || name == "type_name" {
            if let (Some(GArg::Ty(t)), true) = (p.segs[n - 1].args.first(), cargs.is_empty()) {
                let t = self.parse_ty(t)?;
                let tn = self.prog.tn(t);
                let e = if name == "type_name" {
                    // (a program's type renamed against the prelude's: its own name, crate::prelude::rename_user_collisions)
                    Expr::Lit(format!("{:?}", tn.replace("__u_", "")), Default::default())
                } else {
                    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                    // (a vector's key is not its display name, which a program's `struct v128` may also have)
                    let key = self.prog.type_key(t);
                    for b in key.bytes() {
                        h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
                    }
                    let seg = |n: &str| crate::parse::Seg { name: n.to_string(), args: vec![], fn_args: None, text: n.to_string() };
                    Expr::Struct(crate::parse::Path { global: false, segs: vec![seg("TypeId")], pos: Default::default() }, vec![("t".into(), Expr::Lit(format!("{h}u64"), Default::default()))], None)
                };
                let e = self.keep_expr(e);
                return self.expr(e, expected).map(Some);
            }
        }
        // `Box::pin(fut)`: futures run at once (`async` is its value, `.await` takes it as is), so the value itself
        // (`Pin<Box<dyn Future<Output = T>>>` is T, see the parser)
        if name == "pin" && n >= 2 && p.segs[n - 2].name == "Box" && cargs.len() == 1 && !self.prog.struct_index.contains_key("Box") {
            return self.expr(&cargs[0], expected).map(Some);
        }
        // `Box::into_raw(b)` / `Box::from_raw(p)`: the same pointer as a raw pointer / as the Box (the Box is not dropped)
        if matches!(name.as_str(), "into_raw" | "from_raw") && n >= 2 && p.segs[n - 2].name == "Box" && cargs.len() == 1 && !self.prog.struct_index.contains_key("Box") {
            // (`Box::from_raw` is unsafe: it takes ownership of any pointer; into_raw is safe)
            if name == "from_raw" {
                self.check_unsafe_op("Box::from_raw", p.pos)?;
            }
            let want = match (name.as_str(), expected) {
                ("from_raw", Some(Ty::Box(i))) => Some(Ty::Ptr(true, i)),
                _ => None,
            };
            let (v, t) = self.expr(&cargs[0], want)?;
            let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
            return match (name.as_str(), t) {
                ("into_raw", Ty::Box(i)) => Ok(Some((Some(v), Ty::Ptr(true, i)))),
                ("from_raw", Ty::Ptr(_, i)) => Ok(Some((Some(v), Ty::Box(i)))),
                _ => err(SP, format!("`Box::{name}` needs a {}", if name == "into_raw" { "Box" } else { "raw pointer" })),
            };
        }
        // `Box::new(x)`: ヒープに置いて、所有する `&mut T` として返す (解放はしない)
        if name == "new" && n >= 2 && p.segs[n - 2].name == "Box" && !self.prog.struct_index.contains_key("Box") {
            if cargs.len() != 1 {
                return err(SP, "`Box::new` takes 1 argument");
            }
            // `Box::new(MyErr(..))` を `Box<dyn Error>` として: 表示の文字列にする
            if let Some(Ty::Struct(sid)) = expected {
                if self.prog.structs[sid as usize].name == "__DynError" {
                    let (v, t) = self.expr(&cargs[0], None)?;
                    let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                    return self.call_named("__exr_dyn_error", vec![t], &[(v, t)], SP).map(Some);
                }
            }
            let inner = match expected {
                Some(Ty::Box(t)) if !matches!(t, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str) => Some(*t),
                _ => None,
            };
            // (`Box::<[bool; 0]>::new([])`: the turbofish names the content)
            let inner = match p.segs[n - 2].args.first() {
                Some(GArg::Ty(t)) if p.segs[n - 2].args.len() == 1 => Some(self.parse_ty(t)?),
                _ => inner,
            };
            let (v, t) = self.expr(&cargs[0], inner)?;
            if t == Ty::Never {
                return Ok(Some((None, Ty::Never)));
            }
            let Some(&alloc) = self.prog.index.get(crate::prelude::BOX_ALLOC) else {
                return err(SP, "internal: the allocator is missing from the prelude");
            };
            let (size, _) = self.prog.size_align(t);
            let asig = self.prog.sigs[alloc as usize].clone();
            let fr = self.fref(alloc, &asig);
            let sz = self.b.ins().iconst(types::I64, size.max(1) as i64);
            let call = self.b.ins().call(fr, &[sz]);
            let ptr = self.b.inst_results(call)[0];
            self.write(Place::Mem(ptr, 0, t), v);
            self.rg_box(c, &cargs[0], t);
            return Ok(Some((Some(ptr), Ty::Box(intern(t)))));
        }
        // prelude の Chars が使う u32 → char (検査しない)
        if name == "__exr_char_from_u32" {
            let (v, t) = self.expr(&cargs[0], Some(Ty::U32))?;
            self.same(SP, t, Ty::U32)?;
            return Ok(Some((v, Ty::Char)));
        }
        // `__exr_tag_of(&e)` (prelude の mem::discriminant が使う): enum の tag (enum でなければ 0)
        if name == "__exr_tag_of" && cargs.len() == 1 {
            let (v, t) = self.expr(&cargs[0], None)?;
            let v = match (v, t) {
                (Some(a), Ty::Ref(_, Ty::Struct(sid))) if self.prog.structs[*sid as usize].is_enum => {
                    let tag = self.prog.structs[*sid as usize].tag;
                    let ct = clif_ty(tag).unwrap();
                    let x = self.b.ins().load(ct, mf(), a, 0);
                    if ct == types::I64 {
                        x
                    } else if tag.signed() {
                        self.b.ins().sextend(types::I64, x)
                    } else {
                        self.b.ins().uextend(types::I64, x)
                    }
                }
                _ => self.b.ins().iconst(types::I64, 0),
            };
            return Ok(Some((Some(v), Ty::U64)));
        }
        // `core::slice::from_raw_parts(p, n)` / `_mut` と `core::str::from_utf8_unchecked(b)`
        if matches!(name.as_str(), "from_raw_parts" | "from_raw_parts_mut" | "from_utf8_unchecked") && !(p.segs.len() == 1 && self.prog.index.contains_key(&name)) {
            self.check_unsafe_op(&name, p.pos)?;
            if name == "from_utf8_unchecked" {
                let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
                let (v, t) = self.expr(&cargs[0], Some(u8s))?;
                self.same(SP, t, u8s)?;
                return Ok(Some((v, Ty::Ref(false, intern(Ty::Str)))));
            }
            if cargs.len() != 2 {
                return err(SP, format!("`{name}` takes 2 arguments"));
            }
            let (pv, pt) = self.expr(&cargs[0], None)?;
            let Ty::Ptr(_, e) = pt else {
                return err(SP, "expected a raw pointer");
            };
            let (n, nt) = self.expr(&cargs[1], Some(Ty::Usize))?;
            self.same(SP, nt, Ty::Usize)?;
            let fat = self.make_fat_v(pv.unwrap(), n.unwrap());
            let m = name == "from_raw_parts_mut";
            return Ok(Some((Some(fat), Ty::Ref(m, intern(Ty::Slice(e))))));
        }
        if !matches!(name.as_str(), "size_of" | "align_of" | "null" | "null_mut" | "needs_drop" | "__exr_int_kind" | "zeroed") {
            return Ok(None);
        }
        // ユーザが同名の関数を定義していればそちら
        if p.segs.len() == 1 && self.prog.index.contains_key(&name) {
            return Ok(None);
        }
        let targ = match last.args.first() {
            Some(GArg::Ty(t)) => Some(self.parse_ty(t)?),
            _ => None,
        };
        Ok(Some(match name.as_str() {
            "size_of" | "align_of" => {
                let Some(t) = targ else {
                    return err(SP, format!("`{name}` needs a type: `{name}::<T>()`"));
                };
                let (sz, al) = self.prog.size_align(t);
                let v = if name == "size_of" { sz } else { al };
                (Some(self.b.ins().iconst(types::I64, v as i64)), Ty::Usize)
            }
            // `__exr_int_kind::<T>()`: 0 = 整数でない、1 = 符号なし、2 = 符号つき (prelude の基数ソートが使う)
            "__exr_int_kind" => {
                let Some(t) = targ else {
                    return err(SP, "`__exr_int_kind` needs a type");
                };
                let v = if t.is_int() && t.bits() <= 64 { 1 + t.signed() as i64 } else { 0 };
                (Some(self.b.ins().iconst(types::I8, v)), Ty::U8)
            }
            // `core::mem::zeroed::<T>()` (型は turbofish か期待型): 0 で埋めた値
            "zeroed" => {
                // (`mem::zeroed` is unsafe: a zeroed `&T` or `Box<T>` is a null one, whatever the module is called
                // (`use std::mem as m; m::zeroed()`). Only `MaybeUninit::zeroed` is safe)
                let qual = (p.segs.len() >= 2).then(|| p.segs[p.segs.len() - 2].name.as_str());
                if !matches!(qual, Some("MaybeUninit")) {
                    self.check_unsafe_op("zeroed", p.pos)?;
                }
                let Some(t) = targ.or(expected) else {
                    return err(SP, "cannot infer the type of `zeroed()`; annotate it");
                };
                if t.is_agg() {
                    let a = self.temp(t);
                    let (size, _) = self.prog.size_align(t);
                    // (a large value: one memset, not a store per word)
                    if size > super::BIG_COPY {
                        let c = self.b.ins().iconst(types::I32, 0);
                        let nv = self.b.ins().iconst(types::I64, size as i64);
                        self.libc_call("memset", &[a, c, nv]);
                        return Ok(Some((Some(a), t)));
                    }
                    let mut off = 0;
                    for (n, ty) in [(8u32, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
                        while off + n <= size {
                            let z = self.b.ins().iconst(ty, 0);
                            self.b.ins().store(mf(), z, a, off as i32);
                            off += n;
                        }
                    }
                    (Some(a), t)
                } else {
                    let v = match clif_ty(t) {
                        None => None,
                        Some(ct) if ct == types::F64 => Some(self.b.ins().f64const(0.0)),
                        Some(ct) if ct == types::F32 => Some(self.b.ins().f32const(0.0)),
                        Some(ct) if ct == types::I128 => Some(self.int_const(Ty::I128, 0)),
                        Some(ct) => Some(self.b.ins().iconst(ct, 0)),
                    };
                    (v, t)
                }
            }
            // `core::mem::needs_drop::<T>()`: drop の glue が要る型か (定数。`Vec<u8>` の truncate で要素ごとの drop を省く)
            "needs_drop" => {
                let Some(t) = targ else {
                    return err(SP, "`needs_drop` needs a type: `needs_drop::<T>()`");
                };
                let v = self.needs_drop(t) as i64;
                (Some(self.b.ins().iconst(types::I8, v)), Ty::Bool)
            }
            _ => {
                let elem = match (targ, expected) {
                    (Some(t), _) => t,
                    (None, Some(Ty::Ptr(_, t))) => *t,
                    _ => Ty::U8,
                };
                (Some(self.b.ins().iconst(PTR, 0)), Ty::Ptr(name == "null_mut", intern(elem)))
            }
        }))
    }

    /// 1 セグメントのパスがローカル変数でなく enum のバリアント (`None` など) か
    pub(crate) fn is_variant_path(&self, p: &Path) -> bool {
        if let Some(id) = ident_of(p) {
            if self.lookup(id).is_some() {
                return false;
            }
        }
        self.prog.variant_by_p(&PPath::of(p), self.self_sid).is_some()
    }

    /// パターンのパスのバリアントを、match 対象の型 t (インスタンス) に合わせて引く
    pub(crate) fn variant_on(&self, p: &Path, t: Ty) -> Option<(u32, usize)> {
        let (sid, vi) = self.prog.variant_by_p(&PPath::of(p), self.self_sid)?;
        match t {
            Ty::Struct(x) if self.prog.base_of(x) == self.prog.base_of(sid) => {
                // (`A::V(_)` with `type A = E<u8>;`: a variant of that instance only, not of an `E<u16>`)
                if p.segs.len() == 2 && !self.prog.struct_index.contains_key(&p.segs[0].name) && self.prog.aliases.contains_key(&p.segs[0].name) {
                    let at = self.parse_ty(&crate::parse::Ty::Path(None, path_of(&p.segs[0].name)));
                    if let Ok(at @ Ty::Struct(_)) = at {
                        if at != t && !self.prog.has_param(at) && !self.prog.has_param(t) {
                            return None;
                        }
                    }
                }
                Some((x, vi))
            }
            _ => Some((sid, vi)),
        }
    }

    /// 変数でもバリアントでもないパス: `u32::MAX` などの関連定数、`core::ptr::null()` 以外のパス
    /// 値として使われた関数のパス (`apply(double, 3)` の `double`、`S::f`) ならその関数
    pub(crate) fn fn_item(&self, e: &Expr) -> Option<u32> {
        let Expr::Path(qself, p) = e else { return None };
        if qself.is_some() {
            return None;
        }
        if let Some(n) = ident_of(p) {
            if self.lookup(n).is_some() || self.prog.const_index.contains_key(n) {
                return None;
            }
        }
        let id = match self.prog.fn_by_p(&PPath::of(p), self.self_sid) {
            Some(id) => id,
            // (`S::f` where f is a default fn of a trait S implements)
            None => match self.path_self_ty(p)? {
                Ty::Struct(sid) => self.prog.default_method(sid, &p.segs[1].name)?,
                t => self.prog.prim_method(t, &p.segs[1].name)?,
            },
        };
        let s = &self.prog.sigs[id as usize];
        (s.has_body || s.is_extern).then_some(id)
    }

    /// 関数アイテムを関数ポインタの値にする。ジェネリックなら turbofish か期待型 (`fn(i64) -> i64`) で型引数を決める
    pub(crate) fn fn_value(&mut self, e: &Expr, id: u32, expected: Option<Ty>) -> R<V> {
        let Expr::Path(_, p) = e else { unreachable!() };
        let sig0 = self.prog.sigs[id as usize].clone();
        let last = p.segs.last().unwrap();
        let tf: Option<&[GArg]> = if last.args.is_empty() { None } else { Some(&last.args[..]) };
        let mut sol = self.turbofish_sol(&sig0, tf)?;
        // (a trait's default fn through a type: `S::f`)
        if sig0.trait_of.is_some() && sol.first() == Some(&None) {
            if let Some(t) = self.path_self_ty(p).filter(|&t| !self.prog.has_param(t)) {
                sol[0] = Some(t);
            }
        }
        if let Some(Ty::Fn(f)) = expected {
            for (&a, &b) in sig0.params.iter().zip(&f.params) {
                self.prog.match_ty(a, b, &mut sol);
            }
            if !self.prog.has_param(f.ret) {
                self.prog.match_ty(sig0.ret, f.ret, &mut sol);
            }
        }
        for (i, g) in sig0.generics.iter().enumerate() {
            if g.0 == "G" && sol[i].is_none() {
                sol[i] = Some(Ty::Unit);
            }
        }
        let (target, sig) = if sig0.is_generic() {
            if let Some(i) = sol.iter().position(|s| s.is_none()) {
                return err(SP, format!("cannot infer type parameter `{}`; use turbofish (`f::<T>`) or annotate the fn pointer type", sig0.generics[i].0));
            }
            let ats: Vec<Ty> = sig0.params.iter().map(|&t| self.prog.partial_subst(t, &sol)).collect();
            self.resolve_call(id, &ats, sol, None, SP)?
        } else {
            (id, sig0)
        };
        // (a fn pointer is called with zustc's own convention, which passes structs by address)
        if sig.is_extern && (sig.ret.is_agg() || sig.params.iter().any(|p| p.is_agg())) {
            return err(SP, "a pointer to an extern fn that takes or returns a struct by value is not supported");
        }
        let f = self.fref(target, &sig);
        let v = self.b.ins().func_addr(PTR, f);
        let t = Ty::Fn(crate::types::intern_fn(crate::types::FnTy { params: sig.params.clone(), ret: sig.ret }));
        Ok((Some(v), t))
    }

    /// `std::u32::MAX`: the primitive type's own path (out of path_value, which every path expression goes through)
    #[cold]
    #[inline(never)]
    fn prim_module_path(&mut self, p: &crate::parse::Path, expected: Option<Ty>) -> R<V> {
        let np = crate::parse::Path { global: false, segs: p.segs[1..].to_vec(), pos: p.pos };
        let ne: &Expr = self.keep_expr(Expr::Path(None, np));
        self.expr(ne, expected)
    }

    /// `f64::MANTISSA_DIGITS` (u32) / `f32::MAX_EXP` (i32) ..
    #[cold]
    #[inline(never)]
    fn float_int_const(&mut self, t: Ty, name: &str) -> Option<V> {
        let f32t = t == Ty::F32;
        let (v, it): (i64, Ty) = match name {
            "RADIX" => (2, Ty::U32),
            "MANTISSA_DIGITS" => (if f32t { 24 } else { 53 }, Ty::U32),
            "DIGITS" => (if f32t { 6 } else { 15 }, Ty::U32),
            "MIN_EXP" => (if f32t { -125 } else { -1021 }, Ty::I32),
            "MAX_EXP" => (if f32t { 128 } else { 1024 }, Ty::I32),
            "MIN_10_EXP" => (if f32t { -37 } else { -307 }, Ty::I32),
            "MAX_10_EXP" => (if f32t { 38 } else { 308 }, Ty::I32),
            _ => return None,
        };
        Some((Some(self.int_const(it, v as i128)), it))
    }

    pub(crate) fn path_value(&mut self, e: &Expr, _expected: Option<Ty>) -> R<V> {
        let Expr::Path(q, p) = e else { unreachable!() };
        // (`g(f1)` after `let f1 = f;`: the fn f, its instance from the expected type)
        if let Some(&(_, target)) = ident_of(p).and_then(|n| self.fn_aliases.iter().rev().find(|a| a.0 == n)) {
            return self.expr(target, _expected);
        }
        if p.segs.iter().skip(1).any(|s| s.name == "crate") {
            return err(SP, "`crate` in paths can only be used in start position (E0433)");
        }
        if p.global && p.segs.first().is_some_and(|s| s.name == "crate") {
            return err(SP, "global paths cannot start with `crate` (E0433)");
        }
        // `<u64 as Integer>::MAX_STR_LEN`: impl の関連 const `u64::MAX_STR_LEN`
        if let (Some(q), Some(last)) = (q, p.segs.last()) {
            if let Ok(t) = self.parse_ty(q) {
                let tn = if t == Ty::Unit { "#tuple0".to_string() } else { self.prog.tn(t) };
                // (`<Foo as Tr>::C` where Foo has an inherent `C` too: the trait impl's, by its qualified name)
                if p.segs.len() >= 2 {
                    let tr = &p.segs[p.segs.len() - 2].name;
                    if let Some(&ci) = self.prog.const_index.get(&format!("{tn} as {tr}::{}", last.name)) {
                        return self.const_value(e, ci);
                    }
                }
                if self.prog.const_index.contains_key(&format!("{tn}::{}", last.name)) {
                    let seg = |n: &str| crate::parse::Seg { name: n.to_string(), args: vec![], fn_args: None, text: n.to_string() };
                    let np = crate::parse::Path { global: false, segs: vec![seg(&tn), seg(&last.name)], pos: p.pos };
                    let ne: &Expr = self.keep_expr(Expr::Path(None, np));
                    return self.expr(ne, _expected);
                }
                // the trait's default (`<i32 as Foo>::ID` with `const ID: i32 = 1;` in Foo, not in the impl)
                if p.segs.len() >= 2 {
                    let tr = &p.segs[p.segs.len() - 2].name;
                    if let Some(&ci) = self.prog.const_index.get(&format!("{tr}::{}", last.name)) {
                        let c = &self.prog.consts[ci as usize];
                        if !c.expr.contains("Self") {
                            return self.const_value(e, ci);
                        }
                        // (`const B: u8 = Self::A;`: Self is the qualified type; a cycle is cut by a depth limit)
                        thread_local!(static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) });
                        if DEPTH.get() < 16 && !self.prog.has_param(c.ty) {
                            let tq = self.prog.tn(t);
                            let src = c.expr.replace("Self ::", "Self::").replace("Self::", &format!("<{tq} as {tr}>::"));
                            let ty = c.ty;
                            if let Ok(ce) = crate::parse::parse_expr(&src) {
                                let ce: &Expr = self.keep_expr(ce);
                                DEPTH.set(DEPTH.get() + 1);
                                let r = self.expr(ce, Some(ty));
                                DEPTH.set(DEPTH.get() - 1);
                                return r;
                            }
                        }
                    }
                }
            }
        }
        if let Some(ci) = self.const_of(e) {
            return self.const_value(e, ci);
        }
        if let Some(id) = self.fn_item(e) {
            return self.fn_value(e, id, _expected);
        }
        // (`std::u32::MAX` / `core::f64::MANTISSA_DIGITS`: the deprecated modules of the primitive types, the type's
        // own constant)
        if q.is_none() && p.segs.len() == 3 && matches!(p.segs[0].name.as_str(), "std" | "core") && Ty::prim(&p.segs[1].name).is_some() {
            return self.prim_module_path(p, _expected);
        }
        let segs: Vec<String> = p.segs.iter().map(|s| s.name.clone()).collect();
        // `f64::MAX` / `f64::consts::PI` / `std::f64::consts::E` など
        let fl = segs.iter().position(|s| s == "f64" || s == "f32");
        if let Some(i) = fl {
            let t = if segs[i] == "f32" { Ty::F32 } else { Ty::F64 };
            let name = segs.last().unwrap().as_str();
            let x: Option<f64> = match name {
                "MAX" => Some(if t == Ty::F32 { f32::MAX as f64 } else { f64::MAX }),
                "MIN" => Some(if t == Ty::F32 { f32::MIN as f64 } else { f64::MIN }),
                "MIN_POSITIVE" => Some(if t == Ty::F32 { f32::MIN_POSITIVE as f64 } else { f64::MIN_POSITIVE }),
                "EPSILON" => Some(if t == Ty::F32 { f32::EPSILON as f64 } else { f64::EPSILON }),
                "INFINITY" => Some(f64::INFINITY),
                "NEG_INFINITY" => Some(f64::NEG_INFINITY),
                "NAN" => Some(f64::NAN),
                "PI" => Some(std::f64::consts::PI),
                "E" => Some(std::f64::consts::E),
                "TAU" => Some(std::f64::consts::TAU),
                "SQRT_2" => Some(std::f64::consts::SQRT_2),
                "LN_2" => Some(std::f64::consts::LN_2),
                "LN_10" => Some(std::f64::consts::LN_10),
                "FRAC_PI_2" => Some(std::f64::consts::FRAC_PI_2),
                "FRAC_PI_4" => Some(std::f64::consts::FRAC_PI_4),
                "FRAC_1_SQRT_2" => Some(std::f64::consts::FRAC_1_SQRT_2),
                "FRAC_PI_3" => Some(std::f64::consts::FRAC_PI_3),
                "FRAC_PI_6" => Some(std::f64::consts::FRAC_PI_6),
                "FRAC_PI_8" => Some(std::f64::consts::FRAC_PI_8),
                "FRAC_1_PI" => Some(std::f64::consts::FRAC_1_PI),
                "FRAC_2_PI" => Some(std::f64::consts::FRAC_2_PI),
                "FRAC_2_SQRT_PI" => Some(std::f64::consts::FRAC_2_SQRT_PI),
                "LOG2_E" => Some(std::f64::consts::LOG2_E),
                "LOG10_E" => Some(std::f64::consts::LOG10_E),
                "LOG2_10" => Some(std::f64::consts::LOG2_10),
                "LOG10_2" => Some(std::f64::consts::LOG10_2),
                _ => None,
            };
            // (the integer constants of a float type: `f64::MANTISSA_DIGITS` is a u32, `f64::MAX_EXP` an i32)
            if x.is_none() {
                if let Some(r) = self.float_int_const(t, name) {
                    return Ok(r);
                }
            }
            if let Some(x) = x {
                let v = if t == Ty::F32 { self.b.ins().f32const(x as f32) } else { self.b.ins().f64const(x) };
                return Ok((Some(v), t));
            }
        }
        if segs.len() == 2 {
            if let Some(t) = Ty::from_suffix(&segs[0]).or(if segs[0] == "char" { Some(Ty::Char) } else { None }) {
                if let Some(v) = crate::program::prim_assoc_const(t, &segs[1]) {
                    let t = if segs[1] == "BITS" { Ty::U32 } else { t };
                    let c = self.int_const(t, v);
                    return Ok((Some(c), t));
                }
            }
        }
        // `m::S` / `crate::m::S` (モジュールの中の static): the static itself, by its index (a local of the same name must not answer)
        if self.static_of(e).is_some() {
            let p = self.place(e)?;
            return Ok(self.read(p));
        }
        // `core::...::NAME` (const / static) は最後のセグメントで
        if segs.len() >= 2 && matches!(segs[0].as_str(), "core" | "crate" | "self") {
            let e: &Expr = self.keep_expr(path_expr(&p.segs.last().unwrap().name));
            return self.expr(e, _expected);
        }
        // (`std::marker::PhantomData` / `m::S`: a unit struct through a module path)
        if segs.len() >= 2 && p.segs.iter().all(|s| s.args.is_empty()) {
            let last = segs.last().unwrap();
            let unit = self.prog.struct_index.get(last.as_str()).is_some_and(|&sid| {
                let sd = &self.prog.structs[sid as usize];
                !sd.is_enum && sd.fields.is_empty()
            });
            // (only when the modules exist: `::m::crate::S` or a misspelled module is an error)
            let std_path = matches!(segs[0].as_str(), "std" | "core" | "alloc") && !segs.iter().skip(1).any(|s| s == "crate");
            let real = std_path || self.strip_mods_path(p).is_some_and(|q| q.segs.len() == 1);
            if unit && real && self.lookup(last).is_none() {
                let ne: &Expr = self.keep_expr(path_expr(last));
                // (the instance inference chose for the written path, for a generic one (`PhantomData<T>`))
                if let Some(t) = self.lets.get(&(e as *const Expr as usize)).copied() {
                    self.lets.insert(ne as *const Expr as usize, t);
                }
                return self.expr(ne, _expected);
            }
        }
        // (`<S as Tr>::m` as a value: the impl's method, as for a call; asked only here, past every other reading)
        if q.is_some() {
            if let Some((id, sg)) = self.callee(e) {
                if sg.has_body || sg.is_extern {
                    return self.fn_value(e, id, _expected);
                }
            }
        }
        // (`Tr::m` as a value of an expected `fn(&S, ..) -> R`: the impl's method of the first parameter's type, as
        // `<S as Tr>::m`)
        if q.is_none() && segs.len() == 2 && p.segs.iter().all(|s| s.args.is_empty()) && self.prog.traits.contains_key(&segs[0]) {
            if let Some(Ty::Fn(f)) = _expected {
                let st = match f.params.first() {
                    Some(Ty::Ref(_, i)) => Some(**i),
                    Some(&t) => Some(t),
                    None => None,
                };
                // (only when `S::m` is Tr's impl: an inherent `m` of S would answer the qualified path)
                let tr_impl = |t: Ty| self.trait_candidate(&format!("{}::{}", self.ty_key(t), segs[1]), &segs[0]).is_some();
                if let Some(st) = st.filter(|&t| !self.prog.has_param(t) && (t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Str) || matches!(t, Ty::Struct(sid) if self.prog.structs[sid as usize].base.is_none())) && tr_impl(t)) {
                    if let Ok(ne) = crate::parse::parse_expr(&format!("<{} as {}>::{}", self.prog.tn(st), segs[0], segs[1])) {
                        let ne: &Expr = self.keep_expr(ne);
                        return self.expr(ne, _expected);
                    }
                }
            }
        }
        err(SP, "cannot find value (unsupported path)")
    }

    /// `m::S` / `crate::m::S` がモジュールの中の static なら、モジュールを外したパス
    pub(crate) fn static_path(&self, e: &Expr) -> Option<Expr> {
        let Expr::Path(qself, p) = e else { return None };
        if qself.is_some() || p.segs.len() < 2 {
            return None;
        }
        let q = self.strip_mods_path(p)?;
        let id = ident_of(&q)?.to_string();
        let ci = *self.prog.const_index.get(&id)?;
        self.prog.consts[ci as usize].stat?;
        Some(Expr::Path(None, q))
    }

    /// static_path's static, by its index in consts
    pub(crate) fn static_of(&self, e: &Expr) -> Option<u32> {
        let Expr::Path(None, q) = self.static_path(e)? else { return None };
        self.prog.const_index.get(ident_of(&q)?).copied()
    }

    /// `strip_mods_p` の結果を自前の構文木のパスに戻す (後ろのセグメントをそのまま使う)
    fn strip_mods_path(&self, p: &Path) -> Option<Path> {
        let q = self.prog.strip_mods_p(&PPath::of(p))?;
        let k = p.segs.len() - q.segs.len();
        Some(Path { global: false, segs: p.segs[k..].to_vec(), pos: Default::default() })
    }

    /// 1 セグメントのパスが (ローカル変数でない) const なら、その index
    /// `N` (const ジェネリクス) の値。ローカル変数が同じ名前なら変数
    pub(crate) fn const_generic(&self, e: &Expr) -> Option<u32> {
        let Expr::Path(_, p) = e else { return None };
        let id = ident_of(p)?;
        if self.lookup(id).is_some() {
            return None;
        }
        crate::program::const_generic_value(&self.env, id)
    }

    /// the declared type of the const generic parameter `e` names, when it is not usize (`const B: bool`)
    pub(crate) fn const_generic_ty(&self, e: &Expr) -> Option<Ty> {
        let Expr::Path(_, p) = e else { return None };
        let id = ident_of(p)?;
        let sig = self.prog.sigs.get(self.root as usize)?;
        let (_, bs) = sig.generics.iter().find(|(n, _)| n == id)?;
        let t = bs.iter().find_map(|b| b.strip_prefix(crate::types::CONST_GENERIC)?.strip_prefix(':'))?;
        Ty::prim(&self.prog.const_decl(t))
    }

    pub(crate) fn const_of(&self, e: &Expr) -> Option<u32> {
        let Expr::Path(qself, p) = e else { return None };
        // (速い判定: 複数セグメントのパスで、最後の名前の const がなければ const ではない)
        if p.segs.len() >= 2 {
            let last = &p.segs.last()?.name;
            if !self.prog.const_by_last.contains_key(last) && !self.prog.const_index.contains_key(last) {
                return None;
            }
        }
        // `geometry::UNIT` / `crate::m::N`: モジュールの部分を外して引く
        if qself.is_none() && p.segs.len() >= 2 {
            if let Some(q) = self.strip_mods_path(p) {
                let qe = Expr::Path(None, q);
                if let Some(ci) = self.const_of_inner(&qe) {
                    return Some(ci);
                }
            }
        }
        self.const_of_inner(e)
    }

    pub(crate) fn const_of_inner(&self, e: &Expr) -> Option<u32> {
        let Expr::Path(qself, p) = e else { return None };
        // 関連定数 `S::N` / `Self::N` / `T::N` (T は型引数) / trait の既定値 `Tr::N`
        if qself.is_none() && p.segs.len() == 2 {
            let (a, n) = (p.segs[0].name.clone(), p.segs[1].name.clone());
            let ty = if a == "Self" {
                self.env.self_ty.or(self.self_sid.map(Ty::Struct))
            } else if self.env.params.iter().any(|(g, _)| *g == a) || self.prog.aliases.contains_key(&a) {
                // (`T::N` of a type parameter, `Alias::N` of a type alias: the type it stands for)
                let ty = crate::parse::Ty::Path(None, path_of(&a));
                self.parse_ty(&ty).ok()
            } else {
                None
            };
            let base = match ty {
                Some(Ty::Struct(sid)) => Some(self.prog.structs[self.prog.base_of(sid) as usize].name.clone()),
                Some(t) => Some(self.prog.tn(t)),
                None => Some(a.clone()),
            }?;
            if let Some(&ci) = self.prog.const_index.get(&format!("{base}::{n}")) {
                return Some(ci);
            }
            // trait の既定値: 型が実装している trait の `Tr::N`
            if let Some(Ty::Struct(sid)) = ty {
                let b = self.prog.base_of(sid);
                for (s2, tr) in &self.prog.impls {
                    if *s2 == b {
                        if let Some(&ci) = self.prog.const_index.get(&format!("{tr}::{n}")) {
                            return Some(ci);
                        }
                    }
                }
            }
            return None;
        }
        let id = ident_of(p)?;
        if self.lookup(id).is_some() {
            return None;
        }
        let &ci = self.prog.const_index.get(id)?;
        self.prog.consts[ci as usize].stat.is_none().then_some(ci)
    }

    /// 無名データ / static のシンボルのアドレス
    pub(crate) fn symbol_addr(&mut self, ns: u32, index: u32) -> Value {
        match ns {
            NS_DATA => self.data_sym(index),
            NS_STATIC => self.static_sym(index),
            NS_VTABLE => self.vtable_sym(index),
            _ => unreachable!("unknown symbol namespace {ns}"),
        }
    }

    /// 無名データを足してそのアドレスを返す (同じ内容は同じデータ)
    pub(crate) fn data_addr(&mut self, bytes: Vec<u8>) -> Value {
        let k = match self.datas.iter().position(|d| *d == bytes) {
            Some(k) => k,
            None => {
                self.datas.push(bytes);
                self.datas.len() - 1
            }
        };
        self.symbol_addr(NS_DATA, k as u32)
    }
}
