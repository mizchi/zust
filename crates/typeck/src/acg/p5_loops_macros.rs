use super::*;
use crate::fxhash::FxSet as HashSet;

use crate::acg::clif::MemFlagsData;
use crate::parse::{Block as PBlock, Expr, GArg, Pat, Path, Stmt, TokKind, tokenize};
use crate::types::{Ty, intern};

/// Symbol namespaces of anonymous data / statics / vtables (the same numbers as codegen.rs)
pub const NS_DATA: u32 = 1;
pub const NS_STATIC: u32 = 2;
pub const NS_VTABLE: u32 = 3;

// --- Text of macro tokens (in place of syn's ToTokens) -----------------------

/// The contents of `( .. )` / `[ .. ]` / `{ .. }`
fn strip_delims(g: &str) -> Option<&str> {
    let t = g.trim();
    let first = t.chars().next()?;
    let last = t.chars().last()?;
    if !matches!(first, '(' | '[' | '{') || !matches!(last, ')' | ']' | '}') || t.len() < 2 {
        return None;
    }
    Some(&t[1..t.len() - 1])
}

/// Splits on the top-level symbol sep (not inside brackets or strings)
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

/// The texts of a macro's arguments (split on top-level `,`; an empty last element is dropped)
/// The text of `stringify!(tokens)` / `concat!("a", 1, 'c', true, stringify!(x))`, unquoted
pub(crate) fn lit_macro_text(name: &str, g: &str) -> Result<String, String> {
    if name == "stringify" {
        return Ok(tokens_text(strip_delims(g).unwrap_or("")));
    }
    let mut out = String::new();
    for a in macro_arg_texts(g) {
        let a = a.trim();
    // (`concat!(stringify!($Rng), " {{ .. }}")`: a nested concat! / stringify! is its text, as rustc expands them first)
    if let Ok(Expr::Macro(mp, mg, _)) = crate::parse::parse_expr(a) {
        if mp.segs.len() == 1 && matches!(mp.segs[0].name.as_str(), "concat" | "stringify") {
            out.push_str(&lit_macro_text(&mp.segs[0].name, &mg)?);
            continue;
        }
    }
        // (only literals: `concat!(x)` with a variable is an error)
        let lit = matches!(crate::parse::parse_expr(a), Ok(Expr::Lit(..)))
            || matches!(crate::parse::parse_expr(a), Ok(Expr::Unary(op, x, _)) if op == "-" && matches!(*x, Expr::Lit(..)))
            || a == "true" || a == "false";
        if !lit {
            return Err("expected a literal (only literals, like `\"foo\"`, `-42` and `3.14`, can be passed to `concat!()`)".into());
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
    Ok(out)
}

fn macro_arg_texts(g: &str) -> Vec<String> {
    let Some(inner) = strip_delims(g) else { return vec![] };
    let mut v = split_top(inner, ",").unwrap_or_default();
    if v.last().is_some_and(|s| s.trim().is_empty()) {
        v.pop();
    }
    v
}

/// Prints source text with the same spacing as proc_macro2's Display (the same as parse.rs's tokens_text)
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

/// The contents, if the expression is a string literal
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

/// A value path no reading answers, named as written, as rustc's E0425 / E0433 name theirs: ``cannot find value
/// `core::char::REPLACEMENT_CHARACTER` (unsupported path)`` (#265). `<T as Tr>::C` keeps its qualified self (the
/// trait's path is the segments before the last). Made only here, on the error, so the readings above pay nothing
#[cold]
#[inline(never)]
fn unsupported_path<T>(q: Option<&crate::parse::Ty>, p: &Path) -> R<T> {
    let path = match (q, p.segs.split_last()) {
        (Some(q), Some((last, []))) => format!("<{}>::{}", crate::parse::ty_text(q), last.name),
        (Some(q), Some((last, tr))) => {
            let tr = Path { global: p.global, segs: tr.to_vec(), pos: p.pos };
            format!("<{} as {}>::{}", crate::parse::ty_text(q), crate::parse::path_text(&tr), last.name)
        }
        _ => crate::parse::path_text(p),
    };
    err(SP, format!("cannot find value `{path}` (unsupported path)"))
}

/// The walk of hoist_index_bases (outside the function: zustc's hoisting of nested items cannot handle an impl)
struct HoistScan {
    /// Index chains seen: (root name, receiver expressions (`x`, `x[i]`, ..), whether on an assignment's left side)
    chains: Vec<(String, Vec<Expr>, bool)>,
    /// Names seen in a form other than a chain (method receiver, &mut, as a value, inside a macro, binding)
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
                    c.pop(); // (the outermost index is the element itself: the receivers stop before it)
                    self.chains.push((hoist_root(e).unwrap(), c, false));
                    // Look inside the index expressions too
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
    /// Visits the child expressions and patterns (in place of syn::visit::visit_expr)
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
            Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Or(ps, _) | Pat::Slice(ps) => {
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
        // (every name in a macro's arguments counts as "not a chain")
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
    // --- for loops ------------------------------------------------------

    /// `for pat in &x { .. }` / `for pat in &mut x { .. }`: x is borrowed until the loop ends (a hidden local holds
    /// the loan; it is read when the loop is left)
    /// `for x in mk().iter().cloned()` (a Vec or an array a call makes, under adapters): the value goes to a local of
    /// the loop's own block first, `{ let t = mk(); for x in t.iter().cloned() { .. } }`, so it is dropped when the
    /// loop is done (its elements, which a borrowed loop leaves in it; its buffer, which a loop by value empties)
    fn for_temp_base(&self, e: &Expr) -> Option<(Expr, Expr)> {
        match strip_paren(e) {
            Expr::MethodCall(r, name, tf, args, sp) if args.is_empty() && matches!(name.as_str(), "iter" | "iter_mut" | "into_iter" | "copied" | "cloned" | "enumerate" | "rev") => {
                let (base, inner) = self.for_temp_base(r)?;
                Some((base, Expr::MethodCall(Box::new(inner), name.clone(), tf.clone(), vec![], *sp)))
            }
            x @ (Expr::Call(..) | Expr::MethodCall(..) | Expr::Macro(..) | Expr::Array(..)) => {
                let t = self.peek(x)?;
                let seq = match t {
                    Ty::Array(..) => true,
                    Ty::Struct(sid) => self.vec_fields(sid).is_some(),
                    _ => false,
                };
                // (the local is where the value was written: a borrow of it (`for ref x in [s]`) is reported there; an
                // array literal's first element's)
                let at = match x {
                    Expr::Array(xs) if !xs.is_empty() => expr_start(&xs[0]),
                    _ => expr_start(x),
                };
                let mut pe = path_expr("for#tmp");
                if let Expr::Path(_, p) = &mut pe {
                    p.pos = at;
                }
                seq.then(|| (x.clone(), pe))
            }
            _ => None,
        }
    }

    pub(crate) fn for_(&mut self, f: &Expr) -> R<V> {
        if let Expr::For(label, fpat, fexpr, fbody) = f {
            if let Some((base, rebuilt)) = self.for_temp_base(fexpr) {
                let at = expr_start(fexpr);
                let blk = Expr::Block(
                    None,
                    false,
                    crate::parse::Block {
                        stmts: vec![
                            Stmt::Let(Pat::Ident(false, true, "for#tmp".into(), None), None, Some(base), None, at),
                            Stmt::Expr(Expr::For(label.clone(), fpat.clone(), Box::new(rebuilt), fbody.clone()), false),
                        ],
                        spos: Default::default(),
                    },
                );
                let blk = self.keep_expr(blk);
                return self.expr(blk, None);
            }
        }
        let mut temp = None;
        let mut items = None;
        if let Expr::For(_, _, fexpr, _) = f {
            // (`for x in v.iter()` / `v.iter_mut()`, through `copied` / `cloned` / `enumerate` / `rev`: a loan of v for
            // the loop, as `&v` / `&mut v`)
            let mut base = strip_paren(fexpr);
            let mut by_iter = None;
            // (through `copied` / `cloned`: the items are copies, which hold what v holds, not the loan of v)
            let mut copies = false;
            while let Expr::MethodCall(r, name, _, args, _) = base {
                if !args.is_empty() {
                    break;
                }
                match name.as_str() {
                    "copied" | "cloned" | "enumerate" | "rev" => {
                        copies |= matches!(name.as_str(), "copied" | "cloned");
                        base = strip_paren(r);
                    }
                    "iter" | "iter_mut" => {
                        by_iter = Some((strip_paren(r), name == "iter_mut"));
                        break;
                    }
                    _ => break,
                }
            }
            if let (true, Some((inner, m))) = (self.bk_on(), by_iter.filter(|x| is_place_expr(x.0))) {
                let t0 = self.bk.call_temps.len();
                self.bk.recv_deref = true;
                let lo = self.bk_loan_only(inner, m);
                self.bk.recv_deref = false;
                if let Some(ln) = lo {
                    self.bk_call_temp(&[borrowck::Src::Loan(ln)], true);
                    let mut it = self.bk_for_items(inner, ln);
                    if copies {
                        it.retain(|s| !matches!(s, borrowck::Src::Loan(_)));
                    }
                    items = Some(it);
                }
                if self.bk.call_temps.len() > t0 {
                    temp = Some(t0);
                }
            }
            if let (true, Expr::Ref(m, inner)) = (self.bk_on(), strip_paren(fexpr)) {
                // (the loan only: the lowering reads x itself)
                let t0 = self.bk.call_temps.len();
                if let Some(ln) = self.bk_loan_only(inner, *m) {
                    self.bk_call_temp(&[borrowck::Src::Loan(ln)], true);
                    items = Some(self.bk_for_items(inner, ln));
                }
                if self.bk.call_temps.len() > t0 {
                    temp = Some(t0);
                }
            }
        }
        // (an entry for every loop, empty for one over a value: its items hold nothing of an enclosing loop's)
        self.bk.for_items.push(items.unwrap_or_default());
        let st0 = self.bk.stmt_temps.len();
        let r = self.for_inner(f);
        // (the temporaries of the iterator's expression go away with the loop, a terminating scope: after it, before
        // the locals of a block whose tail it is)
        self.bk_temps_end(st0);
        self.bk.for_items.pop();
        if let Some(t0) = temp {
            self.bk_call_temps_end(t0);
        }
        r
    }

    /// What an item of a loop over `&v` / `v.iter()` holds: the loop's loan of v, and what v holds (its own
    /// references, `for s in &names` with `names: Vec<&str>`)
    fn bk_for_items(&mut self, v: &Expr, ln: u32) -> Vec<borrowck::Src> {
        let mut out = vec![borrowck::Src::Loan(ln)];
        if let Some((p, _)) = self.bk_place(v) {
            if self.bk.locals[p.local as usize].refs {
                out.push(borrowck::Src::Local(p.local));
            }
        }
        out
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
        // (`for x in Ok::<_, ()>(1)`: a constructor of the prelude's Option / Result, whose type is not peeked)
        let ctor_of = |cx: &Self, e: &Expr| -> Option<&'static str> {
            let Expr::Call(f, a) = strip_paren(e) else { return None };
            let Expr::Path(None, p) = &**f else { return None };
            let last = &p.segs.last()?.name;
            let qualified = p.segs.len() == 1 || (p.segs.len() == 2 && matches!(p.segs[0].name.as_str(), "Option" | "Result"));
            (a.len() == 1 && qualified && cx.lookup(last).is_none()).then_some(())?;
            match last.as_str() {
                "Some" => Some("Option"),
                "Ok" | "Err" => Some("Result"),
                _ => None,
            }
        };
        let call = call.or_else(|| ctor_of(self, fexpr).map(|_| (&**fexpr, "into_iter")));
        if let Some((recv, m)) = call {
            {
                let peeked = match self.peek(recv) {
                    Some(Ty::Struct(sid)) => Some((self.prog.structs[self.prog.base_of(sid) as usize].name.as_str(), self.prog.self_method(sid, m).is_none())),
                    _ => ctor_of(self, recv).filter(|_| m == "into_iter").map(|n| (n, true)),
                };
                if let Some((tyname, true)) = peeked {
                    if matches!(tyname, "Option" | "Result") {
                        let ctor = if tyname == "Option" { "Some" } else { "Ok" };
                        let scrut = match m {
                            "iter" => Expr::Ref(false, Box::new(Expr::Paren(Box::new(recv.clone())))),
                            "iter_mut" => Expr::Ref(true, Box::new(Expr::Paren(Box::new(recv.clone())))),
                            _ => Expr::Paren(Box::new(recv.clone())),
                        };
                        let some = Pat::TupleStruct(path_of(ctor), vec![Pat::Ident(false, false, "__exr_ox".into(), None)]);
                        // (the loop's pattern binds the item where the iterable is written, as rustc reports a move out
                        // of it: `for &a in x.iter()` with `x: Option<Box<_>>` is E0507 at `x.iter()`)
                        let at = expr_start(fexpr);
                        let mut item = path_expr("__exr_ox");
                        if let Expr::Path(_, p) = &mut item {
                            p.pos = at;
                        }
                        let mut stmts = vec![Stmt::Let((**fpat).clone(), None, Some(item), None, at)];
                        stmts.extend(fbody.stmts.iter().cloned());
                        let iflet = Expr::If(Box::new(Expr::Let(Box::new(some), Box::new(scrut))), crate::parse::Block { stmts, spos: Default::default() }, None);
                        // (with no `break` / `continue` in the body, no loop: a value moved in is not moved in a loop)
                        let dbg = format!("{fbody:?}");
                        if label.is_none() && !dbg.contains("Break(") && !dbg.contains("Continue(") {
                            let st = Expr::Block(None, false, crate::parse::Block { stmts: vec![Stmt::Expr(iflet, true)], spos: Default::default() });
                            let st: &Expr = self.keep_expr(st);
                            return self.expr(st, None);
                        }
                        let range = Expr::Range(Some(Box::new(Expr::Lit("0usize".into(), Default::default()))), Some(Box::new(Expr::Lit("1usize".into(), Default::default()))), false);
                        let lp = Expr::For(label.clone(), Box::new(Pat::Wild), Box::new(range), crate::parse::Block { stmts: vec![Stmt::Expr(iflet, false)], spos: Default::default() });
                        let lp: &Expr = self.keep_expr(lp);
                        return self.for_inner(lp);
                    }
                }
            }
        }
        // (the label is kept without its leading `'`, as syn's ident is)
        self.pending_label = label.as_ref().map(|l| l.trim_start_matches('\'').to_string());
        // Peel the adapters (`.rev()` / `.enumerate()` / `.iter()` ...) from the outside
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
        // (`for x in v[1..].iter()`: a range index becomes a reference (the autoref of a method call))
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
        // Not an array, slice or Vec: an Iterator, `while let Some(pat) = it.next() { .. }`
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
        // (a method or function call of unknown type (`s.split(' ')` and so on) counts as an iterator too)
        let iterator = match bt {
            Some(Ty::Struct(sid)) => self.vec_fields(sid).is_none() && !guard_seq,
            None => matches!(e, Expr::MethodCall(..) | Expr::Call(..)),
            _ => false,
        };
        if iterator {
            // `for (k, v) in &map`: without a next, go through iter() / iter_mut() / into_iter()
            let struct_has_next = |cx: &Self, t: Option<Ty>| match t {
                Some(Ty::Struct(s)) => cx.prog.method(s, "next").is_some(),
                _ => true,
            };
            // (`m.iter_mut()` is judged by the type of the whole expression, not by the peeled m)
            let has_next = struct_has_next(self, bt) || matches!(strip_paren(fexpr), Expr::MethodCall(..)) && struct_has_next(self, self.peek(fexpr));
            // (the user's `impl IntoIterator for &Coll` / `for Coll`)
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
            // (a struct of the program's with a `next` of its own, but no Iterator impl nor `into_iter`: `for x in bogus`
            // is rustc's E0277 at the expression)
            if let Some(Ty::Struct(s)) = bt {
                let user = self.prog.struct_item.get(s as usize).is_some_and(|&i| i < self.prog.prelude_from);
                if user && has_next && user_into.is_none() && !matches!(strip_paren(fexpr), Expr::MethodCall(..)) && !self.prog.implements(Ty::Struct(s), "Iterator") {
                    let n = self.prog.tn(Ty::Struct(s)).replace('#', "");
                    return Err(self.at(expr_start(fexpr), format!("`{n}` is not an iterator (E0277)")));
                }
            }
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
        // Loop over an array / slice by index
        let (by_ref, base_e): (Option<bool>, &Expr) = match e {
            Expr::Ref(m, x) => (Some(*m), x),
            _ => (None, e),
        };
        // `&a[lo..hi]` is a subslice (evaluated with its reference)
        let sub = matches!(base_e, Expr::Index(_, ix, _) if matches!(&**ix, Expr::Range(..)));
        // (a number, `bool` or `char` is no iterator: `for _ in *x` of `x: &mut i32`, E0277 at the expression)
        if let Some(t) = self.peek(e).filter(|t| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char)) {
            let n = self.prog.tn(t).replace('#', "");
            return Err(self.at(expr_start(e), format!("`{n}` is not an iterator (E0277)")));
        }
        let (ptr, len, elem) = self.indexable(if sub { e } else { base_e })?;
        let mut mode = by_ref;
        let base_is_ref = by_ref.is_none() && matches!(self.peek(base_e), Some(Ty::Ref(..)));
        if base_is_ref {
            mode = Some(matches!(self.peek(base_e), Some(Ty::Ref(true, _))));
            // (`for n in v` with v: &mut Vec<T>: `IntoIterator::into_iter(v)` takes a type parameter, so v is moved,
            // not reborrowed: used after, E0382)
            if adapters.is_empty() && is_place_expr(base_e) && matches!(self.peek(base_e), Some(Ty::Ref(true, _))) {
                self.bk_acc(base_e, crate::acg::borrowck::Acc::Move);
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
        // (`for x in &[S { .. }]`: the array is a temporary the loop borrows; it is dropped at the end of the
        // statement, after the loop)
        if mode.is_some() && !sub && !is_place_expr(base_e) && self.needs_drop(elem) {
            if let Some(at @ Ty::Array(..)) = self.peek(base_e) {
                self.own_temp(ptr, at);
            }
        }
        // By value (`for x in arr`): the elements move into x, so the array is not dropped.
        // A Vec gets length 0 here, so its drop only frees the buffer
        // (`v.iter().copied()` / `.cloned()`: the elements are copied out of a borrow, v stays as it is)
        let borrowed = adapters.iter().any(|a| a == "iter" || a == "iter_mut");
        if mode.is_none() && !borrowed {
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
            // (the borrow check: `into_iter` takes the Vec / array, which is not used after (E0382). The elements are
            // read by the loop itself, not through it)
            if is_place_expr(base_e) && self.peek(base_e).is_some_and(|t| !matches!(t, Ty::Ref(..)) && !self.bk_ty(t).0) {
                self.bk_acc(base_e, crate::acg::borrowck::Acc::Move);
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
        // (`v.iter().cloned()`: each element is cloned (`Clone::clone` of a reference to it, a user's `Clone` too), not
        // copied bit for bit; a primitive's clone is its copy)
        let prim = matches!(elem, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::I128 | Ty::Isize | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::U128 | Ty::Usize | Ty::F32 | Ty::F64 | Ty::Bool | Ty::Char | Ty::Unit | Ty::Ref(false, _));
        let clone_elems = mode.is_none() && borrowed && adapters.iter().any(|a| a == "cloned") && !prim;
        let (val, vt) = match mode {
            Some(m) => (Some(addr), Ty::Ref(m, intern(elem))),
            None if clone_elems => {
                self.bind("__exr_cl".into(), Ty::Ref(false, intern(elem)), Some(addr));
                let ce = self.keep_expr(mcall(path_expr("__exr_cl"), "clone", vec![]));
                self.expr(ce, Some(elem))?
            }
            None => self.read(Place::Mem(addr, 0, elem)),
        };
        // (`for (i, x) in v.iter().enumerate()`: binds i and x separately, without building a pair)
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

    /// Builds the (usize, T) for enumerate in a temporary
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

    /// Loop body: binds the value to the pattern, runs the body, and goes to step (the continue target)
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
            let pv = to_place(self, v, t);
            self.for_pat_check(&tp[1], t)?;
            self.runtime_pat_guard(&tp[1], pv)?;
            // (`for (ref i, ref x) in v.iter().enumerate()`: the pair is a temporary of the iteration too)
            let srcs0 = self.bk_for_item_srcs(t);
            let saved = self.bk_for_item_begin(f, t);
            let r = self.pat_bind(&tp[0], pi, BindMode::Move).and_then(|_| self.pat_bind(&tp[1], pv, BindMode::Move));
            self.bk_for_item_end(saved);
            self.bk.bind_srcs = srcs0;
            r?;
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
            let srcs0 = self.bk_for_item_srcs(place.ty());
            let saved = self.bk_for_item_begin(f, place.ty());
            let r = self.pat_bind(fpat, place, BindMode::Move);
            self.bk_for_item_end(saved);
            self.bk.bind_srcs = srcs0;
            r?;
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
            // break drops the scope of the pattern's bindings too
            depth: self.scopes.len() - 1,
            temps: self.temps.len(),
            no_value: Some("for"),
        });
        let (_, bt) = self.block(fbody, Some(Ty::Unit))?;
        self.loops.pop();
        self.loop_bounds.truncate(nb);
        self.pop_scope();
        if bt != Ty::Never {
            // (a body with a value: at its tail, as rustc points)
            self.same(block_end_pos(fbody), bt, Ty::Unit)?;
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
            // (`..10` / `..=10` is no iterator, E0277)
            let r = if *closed { "RangeToInclusive" } else { "RangeTo" };
            let el = match end.as_deref() {
                Some(e) if super::p2_call::is_int_lit(e) => "{integer}".to_string(),
                Some(e) => self.peek(e).map_or("_".to_string(), |t| self.prog.tn(t).replace('#', "")),
                None => "_".to_string(),
            };
            return Err(self.at(expr_start(rg), format!("`{r}<{el}>` is not an iterator (E0277)")));
        };
        // `for i in 0..`: an inclusive range up to the type's maximum (it never ends in practice)
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
        // (`for k in 0..3`: with a lower bound >= 0 and a constant upper bound, k is below that bound)
        self.pending_bound = match (self.iconst_of(lo), self.iconst_of(hi)) {
            (Some(l), Some(h)) if l >= 0 && h >= 0 => Some(h as u64 + inclusive as u64),
            _ => None,
        };
        // -O: unrolls a loop of a small constant count (up to 4) (`for k in 0..3`: k is constant, indexes fold)
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
        // (the enumerate counter; not counted without enumerate)
        let n = self.b.declare_var(types::I64);
        let z = self.b.ins().iconst(types::I64, 0);
        self.b.def_var(n, z);
        let head = self.b.create_block();
        let body = self.b.create_block();
        let step = self.b.create_block();
        let exit = self.b.create_block();
        // Whether it is empty
        let nonempty = self.b.ins().icmp(if inclusive { le_cc } else { lt_cc }, lo, hi);
        // Current value: from lo going forward, from the last element going backward
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
        // -O: reads the start and length of index receivers the loop does not change, before the loop
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
        // Leave after the last element is processed (no overflow even for lo..=MAX)
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

    /// In the loop's body, an index receiver `x` / `x[i]` (x a local Vec, i a variable the body does not change)
    /// that does not change in the loop has its start and length read here (before the loop), for indexable to use.
    /// Not changing: in the body, x appears only as `x[..]..` (a read, or a scalar element assignment), x and i
    /// are not rebound, no macro or closure holds x. The body checks i of `x[i]` (with the comparison made before)
    /// (the key in hoisted is the Debug string of the receiver expression, `format!("{e:?}")`)
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
        // An assigned element must be a scalar (assigning a row, `c[i] = vec![]`, changes the receiver)
        let mut bad: HashSet<String> = sc.other.clone();
        for (r, c, is_assign) in &sc.chains {
            if *is_assign {
                let full = c.last().map(|b| format!("{b:?}"));
                let _ = full;
                // (the left side's type: the receiver's element type)
                // (follows element types from the root's type, once per index)
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
            // The root is a local Vec that exists before the loop
            // (a `&Vec` / `&mut Vec` argument too: if the body changes it, that shows as a method call or an assignment)
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
                // `x[i]`: i is a variable from before the loop, which the body neither assigns nor rebinds
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
                // (out of range: no read; the read goes to a dummy area, and the check in the body panics)
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

    // --- panic / macros / `?` ---------------------------------------------

    /// Panics if cond is set, for an arithmetic check (kind: 0 add, 1 sub, 2 mul, 3 div by zero, 4 rem by zero,
    /// 5 (unused), 6 negation, 7 shift left, 8 shift right, 9 division overflow, 10 remainder overflow)
    pub(crate) fn ovf_panic(&mut self, cond: Value, kind: u8, span: Sp) {
        self.cond_panic(cond, kind, span, None);
    }

    /// A panic with std's message for a check that is not an overflow (`(-4i32).isqrt()`), at span when cond holds
    pub(crate) fn msg_panic(&mut self, cond: Value, msg: &str, span: Sp) {
        self.cond_panic(cond, 12, span, Some(msg.to_string()));
    }

    fn cond_panic(&mut self, cond: Value, kind: u8, span: Sp, msg: Option<String>) {
        // A std program: a conditional trap (the handler main registers ends it as a panic, with 101).
        // It adds no branch or block, so a check costs one instruction to compile
        if self.prog.panic_handler.is_none() {
            let code = self.trap_site(kind, span, msg);
            self.b.ins().trapnz(cond, code);
            return;
        }
        // (with a `#[panic_handler]`, call it)
        let cont = self.b.create_block();
        let pb = self.b.create_block();
        self.b.set_cold_block(pb);
        self.b.ins().brif(cond, pb, &[], cont, &[]);
        self.b.switch_to_block(pb);
        self.panic_call();
        self.b.switch_to_block(cont);
    }

    /// Takes one panic trap code and records the place (kind, line, column) and message (the table goes in the .o,
    /// and the handler main registers looks it up by the faulting address). With too many, code 4, with no place
    /// (line in the file, 1-based column) of a span
    /// (there is no Span now (Sp = 0), so the line is computed as 0 (+ line_base) and the column as 0 (+ 1))
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
            // (the crate mark in the line's high bits is not in the trap table: lines are shifted when the .o is written)
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

    /// The value, if it is an integer constant (to skip the division-by-zero check of `x % 4`)
    pub(crate) fn iconst_of(&self, v: Value) -> Option<i64> {
        self.b.iconst_value(v)
    }

    /// Panics: calls the `#[panic_handler]` if there is one (it does not return), and traps otherwise
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

    /// `vec![a, b, c]` / `vec![x; n]`: expands to Vec::with_capacity and push
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
            // (elements that need drop are cloned into place: `Vec::new()` + `resize(n, x)`)
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
            // Writes straight into the allocation and sets the length to n at the end (no push calls, no capacity checks)
            if let (Some(xv), Ty::Struct(vsid)) = (xv, vt) {
                if let Some((po, lo, _)) = self.vec_fields(vsid) {
                    let (es, _) = self.prog.size_align(xt);
                    let p = self.b.ins().load(PTR, mf(), v, po as i32);
                    // (1-byte elements (`vec![true; n]` / `vec![0u8; n]`) use memset)
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
        let kept: Vec<&Expr> = args.into_iter().map(|a| &*self.keep_expr(a)).collect();
        // (elements that are distinct closures with no captures and one signature: their fn pointer, as for an array)
        let is_clo = |t: Option<Ty>| matches!(t, Some(Ty::Struct(s)) if self.prog.closure_of_struct.contains_key(&self.prog.base_of(s)));
        let mut lubbed = false;
        // (also over the first element's `&String`, which inference may have given)
        let string_ref = |t: Option<Ty>| matches!(t, Some(Ty::Ref(_, Ty::Struct(s))) if self.prog.struct_index.get("String") == Some(&self.prog.base_of(*s)));
        if elem.is_none() || string_ref(elem) {
            if let Some(t) = self.str_refs_lub(&kept) {
                elem = Some(t);
            }
        }
        if elem.is_none() || is_clo(elem) {
            if let Some(f) = self.closures_lub(&kept) {
                elem = Some(f);
                lubbed = true;
                self.lub_fnptr += 1;
            }
        }
        let mut vals = vec![];
        for a in kept {
            // (`vec![&f()]`: a macro's arguments are not extended, the temporary goes away with the statement)
            let d0 = self.bk.direct.len();
            if self.bk_on() && !self.bk.macro_let {
                borrowck::direct_ref(self.prog, a, &mut self.bk.direct);
            }
            let r = self.expr(a, elem);
            self.bk.direct.truncate(d0);
            let r = r.and_then(|(v, t)| if t == Ty::Never { Ok((v, t)) } else { match elem { Some(e) => self.same(SP, t, e).map(|_| (v, t)), None => Ok((v, t)) } });
            let (v, t) = match r {
                Ok(x) => x,
                Err(er) => {
                    if lubbed {
                        self.lub_fnptr -= 1;
                    }
                    return Err(er);
                }
            };
            if t == Ty::Never {
                if lubbed {
                    self.lub_fnptr -= 1;
                }
                return Ok((None, Ty::Never));
            }
            if elem.is_none() {
                elem = Some(t);
            }
            vals.push(v);
        }
        if lubbed {
            self.lub_fnptr -= 1;
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

    /// Supported macros: panic! / unreachable! / todo! / unimplemented! / assert! / assert_eq! /
    /// assert_ne! (and the debug_ forms) / matches!. Messages are not formatted
    pub(crate) fn macro_(&mut self, p: &Path, g: &str, expected: Option<Ty>) -> R<V> {
        let name = p.segs.last().map(|s| s.name.clone()).unwrap_or_default();
        match name.as_str() {
            "vec" => self.vec_macro(p, g, expected),
            "format" | "print" | "println" | "eprint" | "eprintln" | "write" | "writeln" => self.fmt_macro(p, g, &name),
            // `dbg!(e)`: prints `[file:line:col] e = {:#?}` to stderr and returns e
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
                        spos: Default::default(),
                    },
                );
                let ex: &Expr = self.keep_expr(ex);
                self.expr(ex, expected)
            }
            "panic" | "unreachable" | "todo" | "unimplemented" => {
                if self.prog.panic_handler.is_none() {
                    // Message: a plain string as it is; with a format, formatted first; then panics through a prelude function
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
                    // (a program without the formatting parts (using no formatting) prints only the format string's fixed part)
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
                // Compare as `a == b` / `a != b` (structural comparison, a hand-written eq, String and &str too)
                let (a, b) = (&args[0], &args[1]);
                // (a range index, `mockdata[..n]`, as it is: `&(mockdata[..n])` would take the unsized slice by value; `&mockdata[..n]`
                // borrows it, as std's `match (&$left, &$right)` does)
                let paren = |x: &Expr| match x {
                    Expr::Index(_, ix, _) if matches!(**ix, Expr::Range(..)) => x.clone(),
                    _ => Expr::Paren(Box::new(x.clone())),
                };
                // With formatting available, also prints the left / right values ({:?}) as rustc does (a, b evaluated once)
                if self.prog.panic_handler.is_none() && self.prog.index.contains_key("__exr_panic_fmt") {
                    let op = if name.ends_with("_eq") { "==" } else { "!=" };
                    let (l, c) = if self.caller_loc { (0u32, 0u32) } else { self.loc(self.mstart) };
                    // (the references compared where one side is a range index, an unsized slice: `&[u8; 16] == &[u8]`, as
                    // std's `*left_val == *right_val` compares the unsized places)
                    let range_ix = |x: &Expr| matches!(x, Expr::Index(_, ix, _) if matches!(**ix, Expr::Range(..)));
                    let by_ref = range_ix(a) || range_ix(b);
                    let deref = |n: &str| if by_ref { path_expr(n) } else { Expr::Unary("*".into(), Box::new(path_expr(n)), crate::parse::Span::default()) };
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
                    // (a number literal only on the left: the right goes first and gives the left its type (`assert_eq!(2, n)`))
                    let num_lit = |e: &Expr| matches!(e, Expr::Lit(t, _) if matches!(lit::lit(t), Some(Lit::Int(_) | Lit::Float(_))));
                    let lets = if num_lit(a) && !num_lit(b) {
                        vec![let_stmt("__exr_r", Expr::Ref(false, Box::new(paren(b)))), let_stmt("__exr_l", Expr::Ref(false, Box::new(paren(a))))]
                    } else {
                        vec![let_stmt("__exr_l", Expr::Ref(false, Box::new(paren(a)))), let_stmt("__exr_r", Expr::Ref(false, Box::new(paren(b))))]
                    };
                    let mut stmts = lets;
                    stmts.push(Stmt::Expr(Expr::If(Box::new(Expr::Unary("!".into(), Box::new(Expr::Paren(Box::new(cmp))), crate::parse::Span::default())), fail, None), false));
                    let e: &Expr = self.keep_expr(Expr::Block(None, false, PBlock { stmts, spos: Default::default() }));
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
                // (use the same expression type inference read, if there is one: its type records are looked up by address)
                let e: &Expr = match self.macro_args_of(g) {
                    Some(v) if !v.is_empty() => v[0],
                    _ => self.keep_expr(e),
                };
                let guard: Option<&Expr> = guard.map(|g| self.keep_expr(g));
                // (a reparsed expression: type inference records are looked up by position)
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
                // (the bindings only serve the guard: no move out of the scrutinee and no drop (its temporary drops))
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
                    "stringify" | "concat" => match lit_macro_text(name.as_str(), g) {
                        Ok(t) => format!("{t:?}"),
                        Err(e) => return err(SP, e),
                    },
                    _ => {
                        let (l, c) = self.loc(self.mstart);
                        let v = if name == "line" { l } else { c };
                        let e: &Expr = self.keep_expr(Expr::Lit(format!("{v}u32"), crate::parse::Pos::default()));
                        return self.expr(e, expected);
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
            // `cfg!(..)` left by the cfg pass: `version(..)` (unstable), or a cfg! the pass did not reach
            "cfg" if g.to_string().contains("version") => err(SP, "`cfg(version)` is experimental and subject to change (E0658)"),
            // `compile_error!("msg")`: the error, as written
            "compile_error" => {
                let args = self.macro_args(p, g)?;
                match args.first().and_then(|a| str_lit(a)) {
                    Some(m) => err(SP, m),
                    None => err(SP, "compile_error! takes 1 argument"),
                }
            }
            // `env!("X")` left by the preprocessor (macros::expand_env): X is not set
            "env" => {
                let args = self.macro_args(p, g)?;
                // (`env!("X", "message")`: the message is the error)
                match (args.first().and_then(|a| str_lit(a)), args.get(1).and_then(|a| str_lit(a))) {
                    (Some(_), Some(m)) => err(SP, m),
                    (Some(n), None) => err(SP, format!("environment variable `{n}` not defined at compile time")),
                    (None, _) => err(SP, "expected string literal"),
                }
            }
            // `is_x86_feature_detected!("avx2")`: no detection at run time, so only what every x86-64 has (sse, sse2):
            // a caller takes its fallback (memchr's sse2 routines instead of its avx2 ones)
            "is_x86_feature_detected" => {
                let args = self.macro_args(p, g)?;
                let Some(f) = args.first().and_then(|a| str_lit(a)) else {
                    return err(SP, "is_x86_feature_detected! takes a string literal");
                };
                let v = self.b.ins().iconst(types::I8, matches!(f.as_str(), "sse" | "sse2" | "fxsr") as i64);
                Ok((Some(v), Ty::Bool))
            }
            _ => err(SP, format!("macro `{name}!` is not supported")),
        }
    }

    /// Index check: out of range, panics with `index out of bounds: the len is L but the index is I`.
    /// The values go in the message, so it branches to a prelude function, no trap (one calling block per function
    /// is shared, and takes the length, index and place as block arguments)
    pub(crate) fn index_check(&mut self, ok: Value, len: Value, i: Value, span: Sp) {
        if self.prog.panic_handler.is_some() || !self.prog.index.contains_key("__exr_panic_index") {
            self.panic_unless_at(ok, 12, span, Some("index out of bounds".into()));
            return;
        }
        // (the same index and length check has already passed in the same block)
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
        // -O: a cold block per place, which makes the place constants (the hot path keeps no constants in registers)
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

    /// Panics if cond is false (kind and msg are the panic message in a std program)
    pub(crate) fn panic_unless_at(&mut self, cond: Value, kind: u8, span: Sp, msg: Option<String>) {
        if self.prog.panic_handler.is_none() {
            let code = self.trap_site(kind, span, msg);
            self.b.ins().trapz(cond, code);
            return;
        }
        self.panic_unless(cond);
    }

    /// Panics if cond is false
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

    /// `e?`: returns None for an Option, Err(e) for a Result, as it is
    pub(crate) fn try_(&mut self, t: &Expr) -> R<V> {
        let Expr::Try(texpr) = t else { unreachable!() };
        let Some(place) = self.scrutinee(texpr)? else {
            return Ok((None, Ty::Never));
        };
        // `?` moves the contents out (Ok / Some value or Err value), so the temporary (or variable) is no longer dropped
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
        let fp = self.field_place(place, oty, ooff)?;
        let (okv, _) = self.read(fp);
        let cont = self.b.create_block();
        let param = clif_ty(oty).map(|ct| self.b.append_block_param(cont, ct));
        match okv {
            Some(v) => self.b.ins().jump(cont, &[v]),
            None => self.b.ins().jump(cont, &[]),
        };
        // Failure side: build the None / Err of the function's return type and return
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
            let fp = self.field_place(place, ety, eoff)?;
            let (ev, _) = self.read(fp);
            // A different error type is converted with `From::from` (`impl From<ParseIntError> for AppError`)
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
                    // Into a `Box<dyn Error>`: as the display string
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
        self.bk_cap_escape();
        self.drop_for_exit(0, 0);
        self.emit_return(Some(a));
        self.b.switch_to_block(cont);
        Ok((param, oty))
    }

    /// Functions the compiler handles itself: `size_of::<T>()` / `align_of::<T>()` / `ptr::null::<T>()` / `null_mut`
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
        // `Box::leak(b)`: the Box's pointee as it is, as a `&'static mut T` (not freed)
        if name == "leak" && n >= 2 && p.segs[n - 2].name == "Box" && cargs.len() == 1 {
            let (v, t) = self.expr(&cargs[0], None)?;
            let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
            if let Ty::Box(i) = t {
                // (the argument's value is not held as a temporary, so it is not dropped)
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
        // `Box::from(x)` of a value (an array, a number ..): `Box::new(x)` (`Box::from([!0; 256])`). Of a `&str` / `&[T]` /
        // String / Vec it is not: those stay unsupported here
        if name == "from" && n >= 2 && p.segs[n - 2].name == "Box" && cargs.len() == 1 && !self.prog.struct_index.contains_key("Box") {
            let owned = |t: Ty| !matches!(t, Ty::Ref(..) | Ty::Str | Ty::Slice(_) | Ty::Dyn(_)) && !matches!(t, Ty::Struct(sid) if matches!(self.prog.structs[self.prog.base_of(sid) as usize].name.as_str(), "String" | "Vec"));
            let arg = self.peek(&cargs[0]).or(match expected { Some(Ty::Box(i)) => Some(*i), _ => None });
            // (`Box::from(b)` of a Box of that type: `From<T> for T`, the Box itself)
            if let (Some(a @ Ty::Box(_)), Some(e)) = (arg, expected) {
                if a == e {
                    return self.expr(&cargs[0], expected).map(Some);
                }
            }
            // (not when the program has its own `impl From<S> for Box<T>` (S a struct or a Box): that impl is the conversion)
            let user_from = self.prog.names.iter().enumerate().any(|(i, n)| n.starts_with("Box::from") && self.prog.sigs[i].impl_trait.as_deref().is_some_and(|t| t.starts_with("From")));
            // (`Box::from(s)` of a `&[T]` into a `Box<[T]>`: a copy, `s.to_vec().into_boxed_slice()`)
            let to_slice = matches!(expected, Some(Ty::Box(Ty::Slice(_))));
            if !user_from && (to_slice || matches!(arg, Some(Ty::Ref(_, Ty::Slice(_))))) && !matches!(arg, Some(Ty::Box(_) | Ty::Struct(_))) {
                let sp = crate::parse::Span { start: p.pos, at: p.pos };
                let v = Expr::MethodCall(Box::new(cargs[0].clone()), "to_vec".into(), vec![], vec![], sp);
                let ne: &Expr = self.keep_expr(Expr::MethodCall(Box::new(v), "into_boxed_slice".into(), vec![], vec![], sp));
                return self.expr(ne, expected).map(Some);
            }
            if arg.is_some_and(owned) && !(user_from && matches!(arg, Some(Ty::Struct(_) | Ty::Box(_)))) {
                // (the `Box` segment as written, with its arguments: `Box::<u64>::from(1)`)
                let seg = |n: &str| crate::parse::Seg { name: n.to_string(), args: vec![], fn_args: None, text: n.to_string() };
                let bn = crate::parse::Path { global: false, segs: vec![p.segs[n - 2].clone(), seg("new")], pos: p.pos };
                let ne: &Expr = self.keep_expr(Expr::Call(Box::new(Expr::Path(None, bn)), cargs.to_vec()));
                return self.expr(ne, expected).map(Some);
            }
        }
        // `Box::new(x)`: puts x on the heap and returns it as an owning `&mut T` (never freed)
        if name == "new" && n >= 2 && p.segs[n - 2].name == "Box" && !self.prog.struct_index.contains_key("Box") {
            if cargs.len() != 1 {
                return err(SP, "`Box::new` takes 1 argument");
            }
            // `Box::new(MyErr(..))` as a `Box<dyn Error>`: becomes the display string
            if let Some(Ty::Struct(sid)) = expected {
                if self.prog.structs[sid as usize].name == "__DynError" {
                    let (v, t) = self.expr(&cargs[0], None)?;
                    let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                    return self.call_named("__exr_dyn_error", vec![t], &[(v, t)], SP).map(Some);
                }
            }
            let inner = match expected {
                // (not an unsized struct (`Box<Wrap<[u8]>>`): the value is built at its sized type and the box coerced)
                Some(Ty::Box(t)) if !t.is_unsized() => Some(*t),
                // (a closure boxed where a `Box<dyn Fn(..)>` is expected: the trait object is its signature, as for
                // `&|x| ..` where a `&dyn Fn(..)` is (its parameters' types, the bound its body is checked with))
                Some(Ty::Box(t @ Ty::Dyn(_))) if matches!(crate::acg::strip_paren(&cargs[0]), Expr::Closure(..)) => Some(*t),
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
            // (a closure boxed where a `Box<dyn Fn()>` is expected: the coercion wants it `Fn`)
            if let (Some(Ty::Box(Ty::Dyn(want))), true) = (expected, self.bk_on()) {
                self.fn_kind_move_check(&cargs[0], want)?;
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
        // u32 → char for the prelude's Chars (unchecked)
        if name == "__exr_char_from_u32" {
            let (v, t) = self.expr(&cargs[0], Some(Ty::U32))?;
            self.same(SP, t, Ty::U32)?;
            return Ok(Some((v, Ty::Char)));
        }
        // `__exr_tag_of(&e)` (used by the prelude's mem::discriminant): the enum's tag (0 if not an enum)
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
        // `core::slice::from_raw_parts(p, n)` / `_mut` and `core::str::from_utf8_unchecked(b)`
        // (not `Vec::from_raw_parts(ptr, len, cap)` / `String::from_raw_parts`: the prelude's)
        let owner = p.segs.len() >= 2 && matches!(p.segs[p.segs.len() - 2].name.as_str(), "Vec" | "String");
        if matches!(name.as_str(), "from_raw_parts" | "from_raw_parts_mut" | "from_utf8_unchecked") && !owner && !(p.segs.len() == 1 && self.prog.index.contains_key(&name)) {
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
        // If the user defined a function of the same name, that one
        if p.segs.len() == 1 && self.prog.index.contains_key(&name) {
            return Ok(None);
        }
        // (nor a trait's or a type's fn of the name, `<JsValue as NullableIntoWasmAbi>::null()` / `S::null()`)
        if matches!(&**func, Expr::Path(Some(_), _)) || (p.segs.len() >= 2 && self.prog.fn_by_p(&PPath::of(p), self.self_sid).is_some()) {
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
                // (E0277: `size_of::<str>()`, `size_of::<T>()` of a `T: ?Sized`; the prelude's own uses are of its
                // instances, `size_of_val`'s T among them)
                if self.user_code && self.prog.maybe_unsized(t) {
                    let n = crate::traits::display_ty(self.prog, t);
                    return err(SP, format!("the size for values of type `{n}` cannot be known at compilation time {}", crate::traits::SOLVED_NO));
                }
                let (sz, al) = self.prog.size_align(t);
                let v = if name == "size_of" { sz } else { al };
                (Some(self.b.ins().iconst(types::I64, v as i64)), Ty::Usize)
            }
            // `__exr_int_kind::<T>()`: 0 = not an integer, 1 = unsigned, 2 = signed (used by the prelude's radix sort)
            "__exr_int_kind" => {
                let Some(t) = targ else {
                    return err(SP, "`__exr_int_kind` needs a type");
                };
                let v = if t.is_int() && t.bits() <= 64 { 1 + t.signed() as i64 } else { 0 };
                (Some(self.b.ins().iconst(types::I8, v)), Ty::U8)
            }
            // `core::mem::zeroed::<T>()` (the type from the turbofish or the expected type): a value filled with 0
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
            // `core::mem::needs_drop::<T>()`: whether T needs drop glue (a constant; `Vec<u8>`'s truncate skips drops)
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

    /// Whether a one-segment path is an enum variant (`None` and so on), not a local variable
    pub(crate) fn is_variant_path(&self, p: &Path) -> bool {
        if let Some(id) = ident_of(p) {
            if self.lookup(id).is_some() {
                return false;
            }
        }
        self.prog.variant_by_p(&PPath::of(p), self.self_sid).is_some()
    }

    /// Looks up the variant of a pattern's path, matched to the scrutinee's type t (an instance)
    pub(crate) fn variant_on(&self, p: &Path, t: Ty) -> Option<(u32, usize)> {
        let (sid, vi) = self.prog.variant_by_p(&PPath::of(p), self.self_sid)?;
        // (`A::V(_)` with `type A = E<u8>;`: a variant of that instance only, not of an `E<u16>`)
        let other_alias = || {
            if p.segs.len() == 2 && !self.prog.struct_index.contains_key(&p.segs[0].name) && self.prog.aliases.contains_key(&p.segs[0].name) {
                if let Ok(at @ Ty::Struct(_)) = self.parse_ty(&crate::parse::Ty::Path(None, path_of(&p.segs[0].name))) {
                    return at != t && !self.prog.has_param(at) && !self.prog.has_param(t);
                }
            }
            false
        };
        // (`Cow::Borrowed(s)` on a `Cow<str>`: prelude's `__CowStr`, whose variants are in the same order; not through an
        // alias of another Cow, `type Bytes = Cow<'static, [u8]>;`)
        if let (Ty::Struct(x), Some(&cow), Some(&cs)) = (t, self.prog.struct_index.get("Cow"), self.prog.struct_index.get("__CowStr")) {
            if x == cs && self.prog.base_of(sid) == cow {
                return if other_alias() { None } else { Some((cs, vi)) };
            }
        }
        match t {
            Ty::Struct(x) if self.prog.base_of(x) == self.prog.base_of(sid) => {
                if other_alias() {
                    return None;
                }
                Some((x, vi))
            }
            _ => Some((sid, vi)),
        }
    }

    /// Non-variable, non-variant paths: an associated const like `u32::MAX`, a path other than `core::ptr::null()`
    /// If it is the path of a function used as a value (`double` of `apply(double, 3)`, `S::f`), that function
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
            Some(id) => self.prog.fn_of_path_self(id, p, &self.env),
            // (`S::f` where f is a default fn of a trait S implements)
            None => match self.path_self_ty(p)? {
                Ty::Struct(sid) => self.prog.default_method(sid, &p.segs[1].name)?,
                t => self.prog.prim_method(t, &p.segs[1].name)?,
            },
        };
        let s = &self.prog.sigs[id as usize];
        (s.has_body || s.is_extern).then_some(id)
    }

    /// Fn item to fn pointer value. Generic: type arguments from the turbofish or expected type (`fn(i64) -> i64`)
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
        // `<u64 as Integer>::MAX_STR_LEN`: the impl's associated const `u64::MAX_STR_LEN`
        if let (Some(q), Some(last)) = (q, p.segs.last()) {
            if let Ok(t) = self.parse_ty(q) {
                let mut tn = if t == Ty::Unit { "#tuple0".to_string() } else { self.prog.tn(t) };
                // (a vector type's consts are under the name its impl wrote: `impl Vector for __m128i` has `__m128i::BYTES`)
                // (the trait's own first, among all the names: `<__m128 as B>::C` is B's even if `__m128i` has an A::C)
                let names = crate::program::vector_names(t).iter().filter(|n| !self.prog.struct_index.contains_key(**n));
                let qualified = (p.segs.len() >= 2).then(|| p.segs[p.segs.len() - 2].name.clone());
                let by_trait = qualified.as_ref().and_then(|tr| names.clone().find(|n| self.prog.const_index.contains_key(&format!("{n} as {tr}::{}", last.name))));
                // (a qualified path whose impl has no `C` takes the trait's default: not another trait's bare `C`)
                let trait_default = qualified.as_ref().is_some_and(|tr| self.prog.const_index.contains_key(&format!("{tr}::{}", last.name)));
                let bare = || if qualified.is_some() && trait_default { None } else { names.clone().find(|n| self.prog.const_index.contains_key(&format!("{n}::{}", last.name))) };
                if let Some(n) = by_trait.or_else(bare) {
                    tn = n.to_string();
                }
                // (an impl for a reference, `impl Tr for &S { const C }` (der's `&OctetStringRef`), or for a generic
                // type's instance (`<W<u8>>::C`): under the name of the type, S or W, as the impl's own; pick_const
                // chooses by t among those of more impls)
                if matches!(t, Ty::Ref(..) | Ty::Struct(_)) && !self.prog.const_index.contains_key(&format!("{tn}::{}", last.name)) {
                    let mut b = t;
                    while let Ty::Ref(_, i) = b {
                        b = *i;
                    }
                    let bn = self.ty_key(b);
                    if self.prog.const_index.contains_key(&format!("{bn}::{}", last.name)) {
                        tn = bn;
                    }
                }
                // (`<Foo as Tr>::C` where Foo has an inherent `C` too: the trait impl's, by its qualified name)
                if p.segs.len() >= 2 {
                    let tr = &p.segs[p.segs.len() - 2].name;
                    if let Some(&ci) = self.prog.const_index.get(&format!("{tn} as {tr}::{}", last.name)) {
                        let ci = self.pick_const_qualified(ci, t, p, _expected);
                        return self.const_value(e, ci);
                    }
                }
                // (`<W<u8>>::C` / `<&S>::C` of a const of more impls: the one of t's)
                if let Some(&ci) = self.prog.const_index.get(&format!("{tn}::{}", last.name)).filter(|ci| self.prog.const_group.contains_key(ci)) {
                    let ci = self.pick_const_qualified(ci, t, p, _expected);
                    return self.const_value(e, ci);
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
            let ci = self.pick_const_expr(e, ci, _expected);
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
        // `f64::MAX` / `f64::consts::PI` / `std::f64::consts::E` and so on
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
            // (through an alias of a primitive too: `type Mask = u16;` and `Mask::BITS`)
            // (a type alias is not hidden by a value binding of the name: another namespace)
            // (an alias of a primitive, or one of core::ffi's C types: `c_int::MAX` in libc 0.2's types.rs)
            let alias = || {
                let prim = self.prog.prim_of_name(&segs[0]);
                // (the integer ones: `c_float::MAX` / `c_double::MAX` are not integer bounds)
                Ty::from_suffix(&prim).or_else(|| crate::ast_types::ffi_alias(&prim).filter(|t| t.is_int() && !self.prog.struct_index.contains_key(prim.as_str())))
            };
            if let Some(t) = Ty::from_suffix(&segs[0]).or(if segs[0] == "char" { Some(Ty::Char) } else { None }).or_else(alias) {
                if let Some(v) = crate::program::prim_assoc_const(t, &segs[1]) {
                    let t = if segs[1] == "BITS" { Ty::U32 } else { t };
                    let c = self.int_const(t, v);
                    return Ok((Some(c), t));
                }
            }
        }
        // `m::S` / `crate::m::S` (a module's static): the static itself, by its index (a local of the same name must not answer)
        if self.static_of(e).is_some() {
            let p = self.place(e)?;
            return Ok(self.read(p));
        }
        // `core::...::NAME` (const / static) by its last segment
        if segs.len() >= 2 && matches!(segs[0].as_str(), "core" | "crate" | "self") {
            let e: &Expr = self.keep_expr(path_expr(&p.segs.last().unwrap().name));
            return self.expr(e, _expected);
        }
        // (`std::marker::PhantomData` / `m::S`: a unit struct through a module path)
        if segs.len() >= 2 && p.segs.iter().all(|s| s.args.is_empty()) {
            let names: Vec<&str> = segs.iter().map(|s| s.as_str()).collect();
            let last = &crate::types::std_unit_name(&names).to_string();
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
        // (`V::Mask::first_offset` as a value, V a type parameter: the method of the associated type's trait impl, as
        // `<V::Mask as MoveMask>::first_offset`)
        if q.is_none() && segs.len() == 3 && p.segs.iter().all(|s| s.args.is_empty()) {
            let syn = crate::parse::Ty::Path(None, crate::parse::Path { global: false, segs: p.segs[..2].to_vec(), pos: p.pos });
            if let Some(t) = self.parse_ty(&syn).ok().filter(|&t| !self.prog.has_param(t)) {
                let key = format!("{}::{}", self.ty_key(t), segs[2]);
                let mut trs: Vec<&String> = self.prog.traits.keys().filter(|tr| self.trait_candidate(&key, tr).is_some()).collect();
                trs.sort();
                if let [tr] = trs[..] {
                    if let Ok(ne) = crate::parse::parse_expr(&format!("<{} as {}>::{}", self.prog.tn(t), tr, segs[2])) {
                        let ne: &Expr = self.keep_expr(ne);
                        return self.expr(ne, _expected);
                    }
                }
            }
        }
        unsupported_path(q.as_deref(), p)
    }

    /// If `m::S` / `crate::m::S` is a static in a module, the path without the modules
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

    /// Turns the result of `strip_mods_p` back into a path of our own syntax tree (the trailing segments as they are)
    fn strip_mods_path(&self, p: &Path) -> Option<Path> {
        let q = self.prog.strip_mods_p(&PPath::of(p))?;
        let k = p.segs.len() - q.segs.len();
        Some(Path { global: false, segs: p.segs[k..].to_vec(), pos: Default::default() })
    }

    /// If a one-segment path is a const (not a local variable), its index
    /// The value of `N` (const generics). A local variable of the same name is the variable
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
        // (fast check: a multi-segment path whose last name has no const is not a const)
        if p.segs.len() >= 2 {
            let last = &p.segs.last()?.name;
            if !self.prog.const_by_last.contains_key(last) && !self.prog.const_index.contains_key(last) {
                return None;
            }
        }
        // `geometry::UNIT` / `crate::m::N`: look up without the module part
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
        // (`<&S>::C` / `<W<u8>>::C` / `<S as Tr>::C` as a place (`&<&S>::C`), of an impl for a reference (der's `impl
        // FixedTag for &OctetStringRef`) or a const of more impls (const_groups): under the name of the type its impls
        // give it, a reference's under its referent's, as path_value reads them; the trait impl's by its qualified
        // name. Other qualified paths are read as before)
        if let Some(q) = qself.as_ref().filter(|_| p.segs.len() <= 2 && p.segs.last().is_some_and(|s| self.prog.const_by_last.contains_key(&s.name))) {
            let n = &p.segs.last()?.name;
            if let Ok(t0) = self.parse_ty(q) {
                let mut t = t0;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                let base = self.ty_key(t);
                let key = match p.segs.len() {
                    2 => format!("{base} as {}::{n}", p.segs[0].name),
                    _ => format!("{base}::{n}"),
                };
                let found = self.prog.const_index.get(&key).copied().filter(|&ci| self.prog.consts[ci as usize].stat.is_none());
                if let Some(ci) = found.filter(|ci| matches!(t0, Ty::Ref(..)) || self.prog.const_group.contains_key(ci)) {
                    return Some(ci);
                }
            }
        }
        // Associated constants `S::N` / `Self::N` / `T::N` (T a type parameter) / a trait's default `Tr::N`
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
            // (`T::C` with T a reference, of `impl Tr for &S { const C }` (der's `impl FixedTag for &OctetStringRef`):
            // under S's name, as the impl's; path_value chooses among more impls by T)
            if let Some(Ty::Ref(..)) = ty {
                let mut b = ty.unwrap();
                while let Ty::Ref(_, i) = b {
                    b = *i;
                }
                if let Some(&ci) = self.prog.const_index.get(&format!("{}::{n}", self.ty_key(b))) {
                    return Some(ci);
                }
            }
            // (`V::BYTES` with V a vector type: its impl's const, under the name the impl wrote (`__m128i::BYTES`))
            if let Some(&ci) = ty.and_then(|t| crate::program::vector_names(t).iter().filter(|v| !self.prog.struct_index.contains_key(**v)).find_map(|v| self.prog.const_index.get(&format!("{v}::{n}")))) {
                return Some(ci);
            }
            // Trait default: `Tr::N` of a trait the type implements
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

    /// Of consts of one name in more impls (Program::const_groups), the one of the impl the path e names (ci as
    /// const_of found it): pick_const_qualified / pick_const_of_path. ci when it has no group
    pub(crate) fn pick_const_expr(&mut self, e: &Expr, ci: u32, expected: Option<Ty>) -> u32 {
        if !self.prog.const_group.contains_key(&ci) {
            return ci;
        }
        match e {
            Expr::Path(Some(q), p) => match self.parse_ty(q) {
                Ok(t) => self.pick_const_qualified(ci, t, p, expected),
                Err(_) => ci,
            },
            Expr::Path(None, p) => self.pick_const_of_path(p, ci, expected),
            _ => ci,
        }
    }

    /// Program::pick_const for the const ci of a group that a path `T::C` / `Self::C` / `S::C` / `Alias::C` names: by the
    /// self type the path names, and for a type parameter by the arguments of its bound on the const's trait in this
    /// instance (time-core's `T::VALUE` with `T: MultipleOf<U, Output>`), else by the type expected
    #[cold]
    #[inline(never)]
    pub(crate) fn pick_const_of_path(&mut self, p: &Path, ci: u32, expected: Option<Ty>) -> u32 {
        let n = p.segs.len();
        let t = match n {
            2 if p.segs[0].name == "Self" => self.env.self_ty.or(self.self_sid.map(Ty::Struct)),
            _ => self.prog.path_self(p, &self.env),
        };
        let (mut tr, mut args) = (None, None);
        if n == 2 && self.env.params.iter().any(|(g, _)| *g == p.segs[0].name) {
            // (the fn's own bounds: `root`, also for a body inlined into another)
            let g = &p.segs[0].name;
            let bounds = self.prog.sigs.get(self.root as usize).and_then(|s| s.generics.iter().find(|x| x.0 == *g)).map(|x| x.1.clone()).unwrap_or_default();
            let group = &self.prog.const_groups[self.prog.const_group[&ci] as usize];
            let of_group = |b: &str| {
                let b = b.split('<').next().unwrap_or(b).trim();
                let b = b.rsplit("::").next().unwrap_or(b);
                group.iter().any(|c| self.prog.const_impls.get(c).is_some_and(|im| im.trait_name.as_deref() == Some(b)))
            };
            if let Some(b) = bounds.iter().find(|b| of_group(b)) {
                let (trn, a) = self.dyn_trait_args(b);
                tr = Some(trn);
                args = Some(a);
            }
        }
        self.prog.pick_const(ci, t, tr.as_deref(), args.as_deref(), expected)
    }

    /// pick_const for the const ci a qualified path `<T as Tr<A>>::C` / `<T>::C` names: by T, and Tr and its arguments
    #[cold]
    #[inline(never)]
    pub(crate) fn pick_const_qualified(&mut self, ci: u32, t: Ty, p: &Path, expected: Option<Ty>) -> u32 {
        if !self.prog.const_group.contains_key(&ci) {
            return ci;
        }
        let trait_seg = p.segs.len().checked_sub(2).map(|k| &p.segs[k]);
        let args: Option<Vec<Ty>> = trait_seg.filter(|s| !s.args.is_empty()).and_then(|s| {
            s.args
                .iter()
                .filter(|a| !matches!(a, crate::parse::GArg::Lifetime(_) | crate::parse::GArg::Bind(..)))
                .map(|a| match a {
                    crate::parse::GArg::Ty(x) => self.parse_ty(x).ok(),
                    _ => None,
                })
                .collect()
        });
        let tr = trait_seg.map(|s| s.name.clone());
        self.prog.pick_const(ci, Some(t), tr.as_deref(), args.as_deref(), expected)
    }

    /// The address of an anonymous data / static symbol
    pub(crate) fn symbol_addr(&mut self, ns: u32, index: u32) -> Value {
        match ns {
            NS_DATA => self.data_sym(index),
            NS_STATIC => self.static_sym(index),
            NS_VTABLE => self.vtable_sym(index),
            _ => unreachable!("unknown symbol namespace {ns}"),
        }
    }

    /// Adds anonymous data and returns its address (the same contents give the same data)
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
