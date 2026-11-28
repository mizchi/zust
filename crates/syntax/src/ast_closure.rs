//! Closure analysis (what src/closure.rs does), done on our own syntax tree (src/parse.rs).
//! The first step of the port off syn: closures in a body (preorder, in macro arguments too), free variables,
//! and whether a capture is used by value. tests/ast_closure.rs checks that the results match the syn version.
//! It depends on no external crate, so zustc itself can compile it.

use crate::parse::{Block, Expr, Pat, Path, Pos, Stmt, TokKind, Ty, parse_expr, parse_expr_at, pos_at, tokenize};

/// Reads a macro's arguments (the text `( a , b )`) as `,`-separated expressions. None if they do not parse
pub fn macro_args(g: &str) -> Option<Vec<Expr>> {
    macro_args_at(g, Pos::default())
}

/// As macro_args; base is the source position of g (its opening bracket), so expressions get source positions
pub fn macro_args_at(g: &str, base: Pos) -> Option<Vec<Expr>> {
    let inner = strip_delims(g)?;
    // (inner starts after g's first character (the opening bracket))
    let ioff = g.find(inner).unwrap_or(1);
    let mut out = Vec::new();
    if inner.trim().is_empty() {
        return Some(out);
    }
    // (split at `,`, then parse as expressions: a `,` inside brackets does not split)
    let parts = split_top_commas_at(inner)?;
    let mut i = 0;
    while i < parts.len() {
        let (off, part) = &parts[i];
        i += 1;
        if part.trim().is_empty() {
            continue;
        }
        match parse_expr_at(part, pos_at(g, ioff + off, base)) {
            Ok(e) => out.push(e),
            // (a `,` inside `<..>` (`f::<A, B>()`): join the parts that follow until they read as one expression)
            Err(_) => {
                let mut joined = part.clone();
                let mut got = None;
                while i < parts.len() {
                    joined.push(',');
                    joined.push_str(&parts[i].1);
                    i += 1;
                    if let Ok(e) = parse_expr_at(&joined, pos_at(g, ioff + off, base)) {
                        got = Some(e);
                        break;
                    }
                }
                out.push(got?);
            }
        }
    }
    Some(out)
}

fn strip_delims(g: &str) -> Option<&str> {
    let t = g.trim();
    let first = t.chars().next()?;
    let last = t.chars().last()?;
    if !matches!(first, '(' | '[' | '{') || !matches!(last, ')' | ']' | '}') || t.len() < 2 {
        return None;
    }
    Some(&t[1..t.len() - 1])
}

/// As split_top_commas, also returning each part's byte offset in s
fn split_top_commas_at(s: &str) -> Option<Vec<(usize, String)>> {
    let toks = tokenize(s).ok()?;
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0usize;
    for t in &toks {
        match t.kind {
            TokKind::Open => depth += 1,
            TokKind::Close => depth -= 1,
            TokKind::Punct if depth == 0 && t.text == "," => {
                parts.push((start, s[start..t.lo as usize].to_string()));
                start = t.lo as usize + 1;
            }
            _ => {}
        }
    }
    parts.push((start, s[start..].to_string()));
    Some(parts)
}

/// Splits at top-level `,` (by tokens: a `,` inside a string or char does not count)
fn split_top_commas(s: &str) -> Option<Vec<String>> {
    let toks = tokenize(s).ok()?;
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0usize;
    for t in &toks {
        match t.kind {
            TokKind::Open => depth += 1,
            TokKind::Close => depth -= 1,
            TokKind::Punct if depth == 0 && t.text == "," => {
                parts.push(s[start..t.lo as usize].to_string());
                start = t.lo as usize + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].to_string());
    Some(parts)
}

/// The macro arguments that contain closures (only when a `|` token is present). `vec![x; n]` too
pub fn macro_exprs(g: &str) -> Vec<Expr> {
    macro_exprs_at(g, Pos::default())
}

/// As macro_exprs. base is g's position in the source
pub fn macro_exprs_at(g: &str, base: Pos) -> Vec<Expr> {
    let Ok(toks) = tokenize(g) else { return Vec::new() };
    if !toks.iter().any(|t| t.kind == TokKind::Punct && t.text == "|") {
        return Vec::new();
    }
    if let Some(es) = macro_args_at(g, base) {
        return es;
    }
    let Some(inner) = strip_delims(g) else { return Vec::new() };
    // (`matches!(e, PAT if guard)`: the closures in the expression and the guard)
    if let Some((e, _, guard)) = matches_parts(g, base) {
        let mut out = vec![e];
        out.extend(guard);
        return out;
    }
    // (`vec![x; n]`: parsed as an array expression, at the original position)
    match parse_expr_at(&format!("[{inner}]"), base) {
        Ok(e) => vec![e],
        Err(_) => Vec::new(),
    }
}

/// (e, PAT, guard) of `matches!(e, PAT if guard)`: one reading for type inference and codegen (same positions)
pub fn matches_parts(g: &str, base: Pos) -> Option<(Expr, Pat, Option<Expr>)> {
    let inner = strip_delims(g)?;
    let ioff = g.find(inner).unwrap_or(1);
    let parts = split_top_commas_at(inner)?;
    if parts.len() < 2 {
        return None;
    }
    let e = parse_expr_at(&parts[0].1, pos_at(g, ioff + parts[0].0, base)).ok()?;
    let roff = parts[1].0;
    let (p, guard) = crate::parse::parse_pat_guard_at(&inner[roff..], pos_at(g, ioff + roff, base))?;
    Some((e, p, guard))
}

/// The x and n of `vec![x; n]` (read as macro_exprs_at reads them: the positions match too)
pub fn vec_repeat_parts(g: &str, base: Pos) -> Option<(Expr, Expr)> {
    let inner = strip_delims(g)?;
    match parse_expr_at(&format!("[{inner}]"), base).ok()? {
        Expr::Repeat(x, n) => Some((*x, *n)),
        _ => None,
    }
}

/// Passes the child expressions to f in order (inside blocks and statements too, but not into items)
fn each_child(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match e {
        Expr::Lit(_, _) | Expr::Path(..) | Expr::Continue(..) => {}
        Expr::Unary(_, x, _) | Expr::Try(x) | Expr::Paren(x) | Expr::Ref(_, x) | Expr::RawRef(_, x) | Expr::Field(x, _, _) | Expr::Cast(x, _) => f(x),
        Expr::Binary(_, l, r, _) | Expr::Assign(l, r) | Expr::AssignOp(_, l, r, _) | Expr::Index(l, r, _) | Expr::Repeat(l, r) => {
            f(l);
            f(r);
        }
        Expr::Call(x, a) => {
            f(x);
            for y in a {
                f(y);
            }
        }
        Expr::MethodCall(x, _, _, a, _) => {
            f(x);
            for y in a {
                f(y);
            }
        }
        Expr::Range(a, b, _) => {
            if let Some(a) = a {
                f(a);
            }
            if let Some(b) = b {
                f(b);
            }
        }
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => block_exprs(b, f),
        Expr::If(c, t, e) => {
            f(c);
            block_exprs(t, f);
            if let Some(e) = e {
                f(e);
            }
        }
        Expr::Let(_, x) => f(x),
        Expr::Match(x, arms) => {
            f(x);
            for a in arms {
                if let Some(g) = &a.guard {
                    f(g);
                }
                f(&a.body);
            }
        }
        Expr::While(_, c, b) => {
            f(c);
            block_exprs(b, f);
        }
        Expr::For(_, _, x, b) => {
            f(x);
            block_exprs(b, f);
        }
        Expr::Break(_, v, _) | Expr::Return(v, _) => {
            if let Some(v) = v {
                f(v);
            }
        }
        Expr::Closure(_, _, _, b, _) => f(b),
        Expr::Tuple(v) | Expr::Array(v) => {
            for y in v {
                f(y);
            }
        }
        Expr::Struct(_, fs, base) => {
            for x in fs {
                f(&x.1);
            }
            if let Some(b) = base {
                f(b);
            }
        }
        Expr::Macro(..) => {}
    }
}

fn block_exprs(b: &Block, f: &mut dyn FnMut(&Expr)) {
    for s in &b.stmts {
        match s {
            Stmt::Let(_, _, init, els, ..) => {
                if let Some(e) = init {
                    f(e);
                }
                if let Some(b) = els {
                    block_exprs(b, f);
                }
            }
            Stmt::Expr(e, _) => f(e),
            Stmt::Item(_) => {}
        }
    }
}

/// The closure expressions in a body (in preorder, macro arguments included)
pub fn closures_of(b: &Block) -> Vec<Expr> {
    let mut out = Vec::new();
    block_exprs(b, &mut |e| collect_closures(e, &mut out));
    out
}

fn collect_closures(e: &Expr, out: &mut Vec<Expr>) {
    if let Expr::Closure(..) = e {
        out.push(e.clone());
    }
    if let Expr::Macro(_, g, sp) = e {
        for x in macro_exprs_at(g, sp.at) {
            collect_closures(&x, out);
        }
        return;
    }
    each_child(e, &mut |c| collect_closures(c, out));
}

/// In closures_of's order, whether each closure is written inside `unsafe { }` (its body is an unsafe context:
/// the others are checked for E0133)
pub fn closures_in_unsafe(b: &Block) -> Vec<bool> {
    let mut out = Vec::new();
    block_exprs(b, &mut |e| collect_in_unsafe(e, false, &mut out));
    out
}

fn collect_in_unsafe(e: &Expr, u: bool, out: &mut Vec<bool>) {
    if let Expr::Closure(..) = e {
        out.push(u);
    }
    if let Expr::Macro(_, g, sp) = e {
        for x in macro_exprs_at(g, sp.at) {
            collect_in_unsafe(&x, u, out);
        }
        return;
    }
    let u = u || matches!(e, Expr::Block(_, true, _));
    each_child(e, &mut |c| collect_in_unsafe(c, u, out));
}

/// The key a closure is looked up by through its expression's shape (the shape hash)
pub fn closure_key(e: &Expr) -> String {
    format!("#{:x}", expr_hash(e))
}

/// Each closure's place, in closures_of's order: one in the body has its address (Some); one in macro arguments
/// (reparsed, so with no fixed address) has None and the expression's Debug string
pub fn closure_slots(b: &Block) -> Vec<(Option<usize>, String)> {
    let mut out = Vec::new();
    block_exprs(b, &mut |e| collect_slots(e, &mut out));
    out
}

fn collect_slots(e: &Expr, out: &mut Vec<(Option<usize>, String)>) {
    if let Expr::Closure(..) = e {
        out.push((Some(e as *const Expr as usize), closure_key(e)));
    }
    if let Expr::Macro(_, g, sp) = e {
        let mut v = Vec::new();
        for x in macro_exprs_at(g, sp.at) {
            collect_closures(&x, &mut v);
        }
        for c in v {
            out.push((None, closure_key(&c)));
        }
        return;
    }
    each_child(e, &mut |c| collect_slots(c, out));
}

/// The names a pattern binds (a bare name starting with an uppercase letter is a path pattern)
pub fn pat_idents(p: &Pat, out: &mut Vec<String>) {
    match p {
        Pat::Ident(r, m, n, sub) => {
            let path_like = n.starts_with(|c: char| c.is_ascii_uppercase()) && sub.is_none() && !*r && !*m;
            if !path_like {
                out.push(n.clone());
            }
            if let Some(s) = sub {
                pat_idents(s, out);
            }
        }
        Pat::Tuple(v) | Pat::TupleStruct(_, v) | Pat::Or(v, _) | Pat::Slice(v) => {
            for x in v {
                pat_idents(x, out);
            }
        }
        Pat::Struct(_, fs, _) => {
            for f in fs {
                pat_idents(&f.1, out);
            }
        }
        Pat::Ref(_, x) => pat_idents(x, out),
        _ => {}
    }
}

/// The names written directly in a format string's `{name}` / `{name:?}`
pub fn fmt_inline_names(f: &str) -> Vec<String> {
    let b = f.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            if b.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            let st = i + 1;
            let mut j = st;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j > st && !b[st].is_ascii_digit() && matches!(b.get(j), Some(b'}') | Some(b':')) {
                out.push(f[st..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// The contents of a string literal (`"..."` / `r#"..."#`; escapes left as they are)
fn str_lit_body(t: &str) -> Option<&str> {
    let t = t.strip_prefix('r').unwrap_or(t);
    let t = t.trim_matches('#');
    t.strip_prefix('"')?.strip_suffix('"')
}

struct Free {
    scopes: Vec<Vec<String>>,
    out: Vec<String>,
    /// Where each name of `out` is first used
    at: Vec<Pos>,
    /// Count `let _ = s;` as a use of s (fake_reads)
    keep_wild: bool,
}

impl Free {
    fn bound(&self, n: &str) -> bool {
        self.scopes.iter().any(|s| s.iter().any(|x| x == n))
    }
    fn bind(&mut self, p: &Pat) {
        let mut v = vec![];
        pat_idents(p, &mut v);
        self.scopes.last_mut().unwrap().extend(v);
    }
    fn name(&mut self, n: &str, at: Pos) {
        // (`_` as an expression is an inference hole (syn's Expr::Infer), not a variable)
        if n != "_" && !self.bound(n) && !self.out.iter().any(|x| x == n) {
            self.out.push(n.to_string());
            self.at.push(at);
        }
    }
    fn block(&mut self, b: &Block) {
        self.scopes.push(vec![]);
        for s in &b.stmts {
            match s {
                // (`let _ = s;` / `let _ = s.f;` reads no value: a closure does not capture s for it (edition 2021's
                // captures), so a `move` closure leaves s usable after it)
                Stmt::Let(Pat::Wild, None, Some(e), None, ..) if !self.keep_wild && wild_fake_reads() && wild_read_only(e) => {}
                Stmt::Let(p, _, init, els, ..) => {
                    if let Some(e) = init {
                        self.expr(e);
                    }
                    if let Some(b) = els {
                        self.block(b);
                    }
                    self.bind(p);
                }
                Stmt::Expr(e, _) => self.expr(e),
                Stmt::Item(_) => {}
            }
        }
        self.scopes.pop();
    }
    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Path(None, p) => {
                if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() {
                    let n = p.segs[0].name.clone();
                    self.name(&n, p.pos);
                }
            }
            Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => self.block(b),
            Expr::Closure(_, ps, _, body, _) => {
                self.scopes.push(vec![]);
                for (p, _) in ps {
                    self.bind(p);
                }
                self.expr(body);
                self.scopes.pop();
            }
            Expr::For(_, p, x, b) => {
                self.expr(x);
                self.scopes.push(vec![]);
                self.bind(p);
                self.block(b);
                self.scopes.pop();
            }
            Expr::Match(x, arms) => {
                self.expr(x);
                for a in arms {
                    self.scopes.push(vec![]);
                    self.bind(&a.pat);
                    if let Some(g) = &a.guard {
                        self.expr(g);
                    }
                    self.expr(&a.body);
                    self.scopes.pop();
                }
            }
            Expr::Let(p, x) => {
                self.expr(x);
                self.bind(p);
            }
            Expr::If(c, t, els) => {
                self.scopes.push(vec![]);
                self.expr(c);
                self.block(t);
                self.scopes.pop();
                if let Some(e) = els {
                    self.expr(e);
                }
            }
            Expr::While(_, c, b) => {
                self.scopes.push(vec![]);
                self.expr(c);
                self.block(b);
                self.scopes.pop();
            }
            Expr::Macro(mp, g, sp) if mp.segs.len() == 1 && mp.segs[0].name == "matches" => {
                // `matches!(e, pat if guard)`: walk e and guard (the pattern binds only within the guard), with positions
                // from the macro arguments' position (matches_parts: read as type inference reads it)
                let Some((e, pat, guard)) = matches_parts(g, sp.at) else { return };
                self.expr(&e);
                if let Some(gd) = guard {
                    self.scopes.push(vec![]);
                    self.bind(&pat);
                    self.expr(&gd);
                    self.scopes.pop();
                }
            }
            // (`stringify!(self)`: its tokens are text, no use of a variable)
            Expr::Macro(mp, ..) if mp.segs.last().is_some_and(|s| s.name == "stringify") => {}
            Expr::Macro(_, g, sp) => {
                if let Some(es) = macro_args_at(g, sp.at) {
                    for x in &es {
                        self.expr(x);
                        if let Expr::Lit(t, at) = x {
                            if let Some(body) = str_lit_body(t) {
                                for n in fmt_inline_names(body) {
                                    self.name(&n, *at);
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                let mut kids: Vec<Expr> = Vec::new();
                each_child(e, &mut |c| kids.push(c.clone()));
                for k in &kids {
                    self.expr(k);
                }
            }
        }
    }
}

thread_local! {
    /// Edition 2021 and later: `let _ = place;` in a closure captures nothing (the driver sets it for the program it
    /// analyzes; edition 2015 / 2018 capture the place's variable). Of the thread: an analysis runs on one thread,
    /// and two at once on two threads (tests) do not read each other's
    pub static WILD_FAKE_READS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

fn wild_fake_reads() -> bool {
    WILD_FAKE_READS.with(|c| c.get())
}

/// A place made of a variable and its fields (`s`, `s.f.g`, `(s)`): `let _ = place;` reads nothing of it
fn wild_read_only(e: &Expr) -> bool {
    match e {
        Expr::Paren(x) => wild_read_only(x),
        Expr::Field(b, ..) => wild_read_only(b),
        Expr::Path(None, p) => !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty(),
        _ => false,
    }
}

/// A closure's free variables (in order of use)
pub fn free_vars(c: &Expr) -> Vec<String> {
    let mut v = Free { scopes: vec![vec![]], out: Vec::new(), at: Vec::new(), keep_wild: false };
    v.expr(c);
    v.out
}

/// free_vars with `let _ = place;` a use of the place's variable (the unused-variable lint: rustc counts it as one)
pub fn used_vars(c: &Expr) -> Vec<String> {
    let mut v = Free { scopes: vec![vec![]], out: Vec::new(), at: Vec::new(), keep_wild: true };
    v.expr(c);
    v.out
}

/// used_vars, each with where it is first used free (the path that names it; for a name only a format string's
/// `{x}` names, the string)
pub fn used_vars_at(c: &Expr) -> Vec<(String, Pos)> {
    let mut v = Free { scopes: vec![vec![]], out: Vec::new(), at: Vec::new(), keep_wild: true };
    v.expr(c);
    v.out.into_iter().zip(v.at).collect()
}

/// The variables closure c reads only by `let _ = place;`, which it does not capture: its body skips those `let`s
pub fn fake_reads(c: &Expr) -> Vec<String> {
    // (none before edition 2021, where such a `let` captures; and none without such a `let`, found without the two
    // walks of the body below: most closures have none)
    if !wild_fake_reads() || !has_wild_read(c) {
        return vec![];
    }
    let mut v = Free { scopes: vec![vec![]], out: Vec::new(), at: Vec::new(), keep_wild: true };
    v.expr(c);
    let caps = free_vars(c);
    v.out.into_iter().filter(|n| !caps.contains(n)).collect()
}

/// The places of the `let _ = place;`s in e (in its `let .. else` blocks and the closures in it too)
pub fn wild_reads(e: &Expr, out: &mut Vec<Expr>) {
    fn block(b: &Block, out: &mut Vec<Expr>) {
        for s in &b.stmts {
            match s {
                Stmt::Let(Pat::Wild, None, Some(x), None, ..) if wild_read_only(x) => out.push(x.clone()),
                Stmt::Let(_, _, _, Some(els), ..) => {
                    block(els, out);
                    wild_reads(&Expr::Block(None, false, els.clone()), out);
                }
                _ => {}
            }
        }
    }
    if let Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) | Expr::If(_, b, _) | Expr::While(_, _, b) | Expr::For(_, _, _, b) = e {
        block(b, out);
    }
    each_child(e, &mut |x| wild_reads(x, out));
}

/// A `let _ = place;` somewhere in e (in a closure in it too)
fn has_wild_read(e: &Expr) -> bool {
    // (a `let .. else`'s block too, which the walk of the children does not enter)
    fn block(b: &Block) -> bool {
        b.stmts.iter().any(|s| match s {
            Stmt::Let(Pat::Wild, None, Some(x), None, ..) if wild_read_only(x) => true,
            Stmt::Let(_, _, _, Some(els), ..) => block(els) || has_wild_read(&Expr::Block(None, false, els.clone())),
            _ => false,
        })
    }
    let here = match e {
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) | Expr::If(_, b, _) | Expr::While(_, _, b) | Expr::For(_, _, _, b) => block(b),
        _ => false,
    };
    if here {
        return true;
    }
    let mut found = false;
    each_child(e, &mut |x| found = found || has_wild_read(x));
    found
}

/// The variable `let _ = place;` names, when the place is one of it and its fields (wild_read_only)
pub fn wild_root(e: &Expr) -> Option<&str> {
    match e {
        Expr::Paren(x) | Expr::Field(x, ..) => wild_root(x),
        Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => Some(p.segs[0].name.as_str()),
        _ => None,
    }
}

/// The capture e names (a path that is one of caps)
/// e without the parentheses around it (`(b)`)
fn unparen(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(x) => unparen(x),
        _ => e,
    }
}

fn cap_name<'c>(e: &Expr, caps: &'c [String]) -> Option<&'c str> {
    match e {
        Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => caps.iter().find(|c| **c == p.segs[0].name).map(|c| c.as_str()),
        _ => None,
    }
}

fn is_cap(e: &Expr, caps: &[String]) -> bool {
    match e {
        Expr::Path(None, p) => !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && caps.iter().any(|c| *c == p.segs[0].name),
        _ => false,
    }
}

/// Whether `let p = cap;` (cap named by the str) moves cap, or a part of it, out: by default (binds_by_value) when p
/// binds a part by value; the type check knows which parts are Copy (`let (n, _) = pair;` with n an integer copies)
pub type MovesBy<'a> = &'a dyn Fn(&Pat, &str) -> bool;

fn by_value(p: &Pat, _: &str) -> bool {
    binds_by_value(p)
}

fn moves_in(e: &Expr, caps: &[String], by: MovesBy) -> bool {
    if caps.is_empty() {
        return false;
    }
    match e {
        // (a nested closure that owns the capture: `|| { let c = move || t.0 = ..; }` moves `t` into the outer
        // closure too)
        Expr::Closure(mv, ..) => {
            return if *mv { free_vars(e).iter().any(|n| caps.contains(n)) } else { moves_capture_by(e, caps, by) };
        }
        Expr::Call(_, a) if a.iter().any(|x| is_cap(x, caps)) => return true,
        Expr::Return(Some(v), _) if is_cap(v, caps) => return true,
        _ => {}
    }
    // (`let C(ref mut r, _) = c;` binds by reference: not a move)
    scoped_children(e, caps, &mut |c, cs, let_pat| let_pat.is_some_and(|p| cap_name(unparen(c), cs).is_some_and(|n| by(p, n))) || moves_in(c, cs, by))
}

/// Whether the body of closure c moves one of caps by a `let` that binds it by value (`let bar = b;`). Not by passing
/// it: an argument of type `&mut T` is reborrowed, which the syntax does not tell. Not in a nested closure
pub fn let_moves_capture(c: &Expr, caps: &[String]) -> bool {
    let Expr::Closure(_, ps, _, body, _) = c else { return false };
    let mut caps = caps.to_vec();
    for (p, _) in ps {
        caps = unbound(&caps, p);
    }
    fn walk(e: &Expr, caps: &[String]) -> bool {
        if caps.is_empty() || matches!(e, Expr::Closure(..)) {
            return false;
        }
        scoped_children(e, caps, &mut |c, cs, let_pat| let_pat.is_some_and(|p| cap_name(unparen(c), cs).is_some() && binds_by_value(p)) || walk(c, cs))
    }
    walk(body, &caps)
}

/// The names of caps that p does not bind
fn unbound(caps: &[String], p: &Pat) -> Vec<String> {
    let mut v = vec![];
    pat_idents(p, &mut v);
    caps.iter().filter(|c| !v.contains(c)).cloned().collect()
}

/// f on each child of e (until it returns true), with the captures a name in scope there still means: not one a
/// `let` before it in a block, or a pattern of the arm, loop, `if let` or closure it is in, binds again
/// (`let val = String::new(); drop(val);` moves the new val). A `let`'s initializer comes with its pattern
fn scoped_children(e: &Expr, caps: &[String], f: &mut dyn FnMut(&Expr, &[String], Option<&Pat>) -> bool) -> bool {
    match e {
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => {
            let mut cur = caps.to_vec();
            for st in &b.stmts {
                match st {
                    Stmt::Let(p, _, init, els, ..) => {
                        if init.as_ref().is_some_and(|x| f(x, &cur, Some(p))) {
                            return true;
                        }
                        if els.as_ref().is_some_and(|eb| f(&Expr::Block(None, false, eb.clone()), &cur, None)) {
                            return true;
                        }
                        cur = unbound(&cur, p);
                    }
                    // (`a;`: the value is moved into the statement's temporary, as `let x = a;` binds it)
                    Stmt::Expr(x, true) if matches!(unparen(x), Expr::Path(None, p) if p.segs.len() == 1) => {
                        let tmp = Pat::Ident(false, false, "__stmt".to_string(), None);
                        if f(x, &cur, Some(&tmp)) {
                            return true;
                        }
                    }
                    Stmt::Expr(x, _) => {
                        if f(x, &cur, None) {
                            return true;
                        }
                    }
                    Stmt::Item(_) => {}
                }
            }
            false
        }
        Expr::Match(x, arms) => {
            // (the scrutinee comes with each arm's pattern, as a `let`'s initializer with its: `match cap { E::V(_, s)
            // => .. }` moves cap.1)
            if f(x, caps, None) || arms.iter().any(|a| f(x, caps, Some(&a.pat))) {
                return true;
            }
            arms.iter().any(|a| {
                let cs = unbound(caps, &a.pat);
                a.guard.as_ref().is_some_and(|g| f(g, &cs, None)) || f(&a.body, &cs, None)
            })
        }
        Expr::For(_, p, x, b) => f(x, caps, None) || f(&Expr::Block(None, false, b.clone()), &unbound(caps, p), None),
        // (`if let P = x { .. }` / `while let`: P's names are bound in the block)
        Expr::If(c, t, _) | Expr::While(_, c, t) if matches!(&**c, Expr::Let(..)) => {
            let Expr::Let(p, x) = &**c else { return false };
            let els = if let Expr::If(_, _, els) = e { els.as_deref() } else { None };
            f(x, caps, Some(p)) || f(&Expr::Block(None, false, t.clone()), &unbound(caps, p), None) || els.is_some_and(|x| f(x, caps, None))
        }
        Expr::Closure(_, ps, _, body, _) => {
            let mut cs = caps.to_vec();
            for (p, _) in ps {
                cs = unbound(&cs, p);
            }
            f(body, &cs, None)
        }
        _ => {
            let mut found = false;
            each_child_stmts(e, &mut |c| {
                if !found && f(c, caps, None) {
                    found = true;
                }
            });
            found
        }
    }
}

/// The captures the tail of a closure's body still means: not one a `let` of the body binds again
fn tail_caps(body: &Expr, caps: &[String]) -> Vec<String> {
    let mut cur = caps.to_vec();
    if let Expr::Block(_, _, b) = body {
        for st in &b.stmts {
            if let Stmt::Let(p, ..) = st {
                cur = unbound(&cur, p);
            }
        }
    }
    cur
}

/// As each_child, but also walks child blocks, to judge the right-hand sides of a block's lets (in moves_in)
fn each_child_stmts(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match e {
        Expr::If(c, t, els) => {
            f(c);
            f(&Expr::Block(None, false, t.clone()));
            if let Some(x) = els {
                f(x);
            }
        }
        Expr::While(_, c, b) => {
            f(c);
            f(&Expr::Block(None, false, b.clone()));
        }
        Expr::For(_, _, x, b) => {
            f(x);
            f(&Expr::Block(None, false, b.clone()));
        }
        _ => each_child(e, f),
    }
}

/// Whether the closure body uses a capture by value (call argument, body value, in `let y = x;`, `return x`)
pub fn moves_capture(c: &Expr, caps: &[String]) -> bool {
    moves_capture_by(c, caps, &by_value)
}

/// moves_capture, with what a `let` that destructures a capture moves decided by `by`
pub fn moves_capture_by(c: &Expr, caps: &[String], by: MovesBy) -> bool {
    let Expr::Closure(_, ps, _, body, _) = c else { return false };
    // (not what the closure's parameters name)
    let mut caps = caps.to_vec();
    for (p, _) in ps {
        caps = unbound(&caps, p);
    }
    let tail = match &**body {
        Expr::Block(_, _, b) => b.stmts.last().and_then(|s| match s {
            Stmt::Expr(e, false) => Some(e),
            _ => None,
        }),
        e => Some(e),
    };
    if tail.is_some_and(|e| is_cap(e, &tail_caps(body, &caps))) {
        return true;
    }
    moves_in(body, &caps, by)
}

/// Where the body of closure c first moves one of caps out (what moves_capture finds), with that capture's name. Not
/// in a nested closure (its move is the nested closure's capture)
pub fn move_site(c: &Expr, caps: &[String]) -> Option<(String, Pos)> {
    move_site_by(c, caps, &by_value)
}

/// move_site, with what a `let` that destructures a capture moves decided by `by` (see MovesBy)
pub fn move_site_by(c: &Expr, caps: &[String], by: MovesBy) -> Option<(String, Pos)> {
    let Expr::Closure(_, ps, _, body, _) = c else { return None };
    let mut caps = caps.to_vec();
    for (p, _) in ps {
        caps = unbound(&caps, p);
    }
    let tail = match &**body {
        Expr::Block(_, _, b) => b.stmts.last().and_then(|s| match s {
            Stmt::Expr(e, false) => Some(e),
            _ => None,
        }),
        e => Some(e),
    };
    tail.and_then(|e| cap_at(e, &tail_caps(body, &caps))).or_else(|| move_site_in(body, &caps, by))
}

fn cap_at(e: &Expr, caps: &[String]) -> Option<(String, Pos)> {
    match e {
        Expr::Path(None, p) if is_cap(e, caps) => Some((p.segs[0].name.clone(), p.pos)),
        _ => None,
    }
}

fn move_site_in(e: &Expr, caps: &[String], by: MovesBy) -> Option<(String, Pos)> {
    if caps.is_empty() {
        return None;
    }
    match e {
        // (a nested closure that takes one by value, a `move` one that captures it or one whose body moves it out
        // (`|| drop(v)`): it is moved into that closure where the closure is made. One that only borrows it does
        // not move it)
        Expr::Closure(true, _, _, _, at) => return free_vars(e).into_iter().find(|n| caps.contains(n)).map(|n| (n, *at)),
        Expr::Closure(false, _, _, _, at) => {
            return caps.iter().find(|n| moves_capture_by(e, std::slice::from_ref(*n), by)).map(|n| (n.clone(), *at));
        }
        Expr::Call(_, a) => {
            if let Some(x) = a.iter().find_map(|x| cap_at(x, caps)) {
                return Some(x);
            }
        }
        Expr::Return(Some(v), _) => {
            if let Some(x) = cap_at(v, caps) {
                return Some(x);
            }
        }
        _ => {}
    }
    let mut found = None;
    scoped_children(e, caps, &mut |c, cs, let_pat| {
        found = let_pat.and_then(|p| cap_at(unparen(c), cs).filter(|(n, _)| by(p, n))).or_else(|| move_site_in(c, cs, by));
        found.is_some()
    });
    found
}

/// Passes the child expressions to f in order (the version for outside use)
/// Every use of the capture n in the closure c is through a deref of it: a method call's receiver (`n.m(..)`), a
/// field's or an index's base (`n.f`, `n[i]`), `*n`, a call's argument or callee (`f(n)`, `n()`). With n a shared
/// reference, edition 2021 captures `*n` by reference there (a reborrow of the referent, not a borrow of n), which is
/// n's value: copying n is the same. rustc reports E0373 for a closure that escapes only where it uses n itself
/// (`|| n`, `let x = n`, `(n, 1)`, `for _ in n`). Not where the closure binds a name n of its own
pub fn deref_uses_only(c: &Expr, n: &str) -> bool {
    let Expr::Closure(_, ps, _, body, _) = c else { return false };
    if ps.iter().any(|(p, _)| pat_binds_name(p, n)) || binds_name(body, n, 0) {
        return false;
    }
    let (mut ok, mut seen) = (true, false);
    deref_walk(body, n, &mut ok, &mut seen, 0);
    ok && seen
}

fn is_name(e: &Expr, n: &str) -> bool {
    match e {
        Expr::Paren(x) => is_name(x, n),
        Expr::Path(None, p) => !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].name == n,
        _ => false,
    }
}

fn deref_walk(e: &Expr, n: &str, ok: &mut bool, seen: &mut bool, d: u32) {
    if !*ok {
        return;
    }
    if d > 64 {
        *ok = false;
        return;
    }
    let mut slot = |x: &Expr, ok: &mut bool, seen: &mut bool| {
        if is_name(x, n) {
            *seen = true;
        } else {
            deref_walk(x, n, ok, seen, d + 1);
        }
    };
    match e {
        Expr::MethodCall(r, _, _, args, _) => {
            slot(r, ok, seen);
            for a in args {
                slot(a, ok, seen);
            }
        }
        Expr::Call(f, args) => {
            slot(f, ok, seen);
            for a in args {
                slot(a, ok, seen);
            }
        }
        Expr::Field(b, _, _) => slot(b, ok, seen),
        Expr::Index(b, i, _) => {
            slot(b, ok, seen);
            deref_walk(i, n, ok, seen, d + 1);
        }
        Expr::Unary(op, x, _) if op == "*" => slot(x, ok, seen),
        // (a macro's arguments: the name in them, as a use of its own (`println!("{}", n)` borrows n))
        Expr::Macro(_, g, _) => {
            if g.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')).any(|w| w == n) {
                *ok = false;
            }
        }
        x if is_name(x, n) => *ok = false,
        _ => each_child(e, &mut |c| deref_walk(c, n, ok, seen, d + 1)),
    }
}

pub fn pat_binds_name_pub(p: &Pat, n: &str) -> bool {
    pat_binds_name(p, n)
}

fn pat_binds_name(p: &Pat, n: &str) -> bool {
    let mut v = vec![];
    pat_idents(p, &mut v);
    v.iter().any(|x| x == n)
}

/// A pattern in e (a `let`, a match arm, a `for`, a closure's parameter) binds the name n
pub fn binds_name(e: &Expr, n: &str, d: u32) -> bool {
    if d > 64 {
        return true;
    }
    // (a `let .. else`'s block is a block too: its lets bind names, as any block's)
    fn block_binds(b: &crate::parse::Block, n: &str, d: u32) -> bool {
        if d > 64 {
            return true;
        }
        b.stmts.iter().any(|s| match s {
            Stmt::Let(p, _, init, els, ..) => pat_binds_name(p, n) || init.as_ref().is_some_and(|x| binds_name(x, n, d + 1)) || els.as_ref().is_some_and(|b| block_binds(b, n, d + 1)),
            Stmt::Expr(x, _) => binds_name(x, n, d + 1),
            _ => false,
        })
    }
    let block = |b: &crate::parse::Block| block_binds(b, n, d);
    match e {
        Expr::Closure(_, ps, _, b, _) => ps.iter().any(|(p, _)| pat_binds_name(p, n)) || binds_name(b, n, d + 1),
        Expr::Let(p, x) => pat_binds_name(p, n) || binds_name(x, n, d + 1),
        Expr::Match(x, arms) => {
            binds_name(x, n, d + 1) || arms.iter().any(|a| pat_binds_name(&a.pat, n) || a.guard.as_ref().is_some_and(|g| binds_name(g, n, d + 1)) || binds_name(&a.body, n, d + 1))
        }
        Expr::For(_, p, it, b) => pat_binds_name(p, n) || binds_name(it, n, d + 1) || block(b),
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => block(b),
        Expr::If(c, t, el) => binds_name(c, n, d + 1) || block(t) || el.as_ref().is_some_and(|x| binds_name(x, n, d + 1)),
        Expr::While(_, c, b) => binds_name(c, n, d + 1) || block(b),
        _ => {
            let mut found = false;
            each_child(e, &mut |c| found = found || binds_name(c, n, d + 1));
            found
        }
    }
}

pub fn each_child_pub(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    each_child(e, f)
}

fn assign_base(e: &Expr) -> Option<String> {
    match e {
        Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => Some(p.segs[0].name.clone()),
        Expr::Field(x, _, _) | Expr::Index(x, _, _) | Expr::Paren(x) | Expr::Unary(_, x, _) => assign_base(x),
        _ => None,
    }
}

fn assigns_in(e: &Expr, caps: &[String]) -> bool {
    match e {
        Expr::Closure(..) => return false,
        Expr::Assign(l, _) | Expr::AssignOp(_, l, _, _) if assign_base(l).is_some_and(|n| caps.iter().any(|c| *c == n)) => return true,
        _ => {}
    }
    let mut found = false;
    each_child(e, &mut |c| {
        if !found && assigns_in(c, caps) {
            found = true;
        }
    });
    found
}

// (an assignment to a place rooted at one of caps, or a `&mut` / `&raw mut` of one)
fn mutates_in(e: &Expr, caps: &[String]) -> bool {
    match e {
        Expr::Closure(..) => return false,
        Expr::Ref(true, x) | Expr::RawRef(true, x) if assign_base(x).is_some_and(|n| caps.iter().any(|c| *c == n)) => return true,
        _ => {}
    }
    if assigns_in(e, caps) {
        return true;
    }
    let mut found = false;
    each_child(e, &mut |c| {
        if !found && mutates_in(c, caps) {
            found = true;
        }
    });
    found
}

/// Whether the closure body assigns to the captured variable n or borrows it mutably, at its top level (`n = ..`,
/// `n.f += 1`, `*n = ..`, `&mut n`; not in a nested closure): a closure that holds n by reference holds a `&mut` of
/// it (or a unique borrow). A call of a `&mut self` method on it is not seen (it needs the type). Never where the body
/// binds the name again anywhere (`let mut n = ..; n = ..` is another variable's): the answer is then false, which
/// the auto traits take as "either kind", the rule that holds of both
pub fn mutates_capture(c: &Expr, n: &str) -> bool {
    let Expr::Closure(_, _, _, body, _) = c else { return false };
    !binds_name(body, n, 0) && mutates_in(body, &[n.to_string()])
}

/// Whether the closure body assigns to a captured variable (`r = ..` / `n += 1` / `v.x = ..`)
pub fn assigns_capture(c: &Expr, caps: &[String]) -> bool {
    let Expr::Closure(_, _, _, body, _) = c else { return false };
    assigns_in(body, caps)
}

/// A hash of an expression's shape (the tree's shape and positions; the same whenever the Debug output is)
pub fn expr_hash(e: &Expr) -> u64 {
    let mut h = EHash { h: 0xcbf2_9ce4_8422_2325, memo: false };
    let mut m = crate::fxhash::FxMap::default();
    h.expr_in(e, &mut m);
    h.h
}

/// The same hash, memoizing subexpression hashes by address (use only while the expression is alive)
pub fn expr_hash_memo(e: &Expr, memo: &mut crate::fxhash::FxMap<usize, u64>) -> u64 {
    let mut h = EHash { h: 0xcbf2_9ce4_8422_2325, memo: true };
    h.expr_in(e, memo);
    h.h
}

/// Hash mixed over a tree walk (subexpressions by their own hash, non-expression leaves by their Debug string)
/// (FNV-like: written without traits so zustc can compile it too)
struct EHash {
    h: u64,
    /// Whether to memoize subexpression hashes by address
    memo: bool,
}

impl EHash {
    fn n(&mut self, x: u64) {
        self.h = (self.h ^ x).wrapping_mul(0x0100_0000_01b3);
        self.h ^= self.h >> 29;
    }
    fn s(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.h = (self.h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
        self.n(0xfe);
    }
    fn pos(&mut self, p: &crate::parse::Pos) {
        self.n(((p.line as u64) << 32) | p.col as u64);
    }
    fn span(&mut self, sp: &crate::parse::Span) {
        self.pos(&sp.start);
        self.pos(&sp.at);
    }
    fn ty(&mut self, t: &Ty) {
        let d = format!("{t:?}");
        self.s(&d);
    }
    fn pat(&mut self, p: &Pat) {
        let d = format!("{p:?}");
        self.s(&d);
    }
    fn path(&mut self, p: &Path) {
        self.n(p.global as u64);
        self.pos(&p.pos);
        self.n(p.segs.len() as u64);
        for sg in &p.segs {
            self.s(&sg.name);
            if !sg.args.is_empty() || sg.fn_args.is_some() {
                let d = format!("{:?}{:?}", sg.args, sg.fn_args);
                self.s(&d);
            }
        }
    }
    fn opt_s(&mut self, s: &Option<String>) {
        match s {
            Some(s) => self.s(s),
            None => self.n(0xfd),
        }
    }
    fn sub(&mut self, e: &Expr, memo: &mut crate::fxhash::FxMap<usize, u64>) {
        let k = e as *const Expr as usize;
        if self.memo {
            if let Some(&v) = memo.get(&k) {
                self.n(v);
                return;
            }
        }
        let mut c = EHash { h: 0xcbf2_9ce4_8422_2325, memo: self.memo };
        c.expr_in(e, memo);
        if self.memo {
            memo.insert(k, c.h);
        }
        self.n(c.h);
    }
    fn opt_e(&mut self, e: &Option<Box<Expr>>, memo: &mut crate::fxhash::FxMap<usize, u64>) {
        match e {
            Some(e) => self.sub(e, memo),
            None => self.n(0xfd),
        }
    }
    fn block(&mut self, b: &Block, memo: &mut crate::fxhash::FxMap<usize, u64>) {
        self.n(0xb0 + b.stmts.len() as u64);
        for st in &b.stmts {
            match st {
                Stmt::Let(p, t, e, els, ..) => {
                    self.n(1);
                    self.pat(p);
                    match t {
                        Some(t) => self.ty(t),
                        None => self.n(0xfd),
                    }
                    match e {
                        Some(e) => self.sub(e, memo),
                        None => self.n(0xfd),
                    }
                    match els {
                        Some(b) => self.block(b, memo),
                        None => self.n(0xfd),
                    }
                }
                Stmt::Expr(e, semi) => {
                    self.n(2 + *semi as u64);
                    self.sub(e, memo);
                }
                Stmt::Item(it) => {
                    let d = format!("{it:?}");
                    self.s(&d);
                }
            }
        }
    }
    fn expr_in(&mut self, e: &Expr, memo: &mut crate::fxhash::FxMap<usize, u64>) {
        match e {
            Expr::Lit(l, p) => {
                self.n(1);
                self.s(l);
                self.pos(p);
            }
            Expr::Path(q, p) => {
                self.n(2);
                match q {
                    Some(t) => self.ty(t),
                    None => self.n(0xfd),
                }
                self.path(p);
            }
            Expr::Unary(o, x, sp) => {
                self.n(3);
                self.s(o);
                self.sub(x, memo);
                self.span(sp);
            }
            Expr::Binary(o, l, r, sp) => {
                self.n(4);
                self.s(o);
                self.sub(l, memo);
                self.sub(r, memo);
                self.span(sp);
            }
            Expr::Assign(l, r) => {
                self.n(5);
                self.sub(l, memo);
                self.sub(r, memo);
            }
            Expr::AssignOp(o, l, r, sp) => {
                self.n(6);
                self.s(o);
                self.sub(l, memo);
                self.sub(r, memo);
                self.span(sp);
            }
            Expr::Call(f, xs) => {
                self.n(7);
                self.sub(f, memo);
                self.n(xs.len() as u64);
                for x in xs {
                    self.sub(x, memo);
                }
            }
            Expr::MethodCall(r, m, ga, xs, sp) => {
                self.n(8);
                self.sub(r, memo);
                self.s(m);
                if !ga.is_empty() {
                    let d = format!("{ga:?}");
                    self.s(&d);
                }
                self.n(xs.len() as u64);
                for x in xs {
                    self.sub(x, memo);
                }
                self.span(sp);
            }
            Expr::Field(x, f, sp) => {
                self.n(9);
                self.sub(x, memo);
                self.s(f);
                self.span(sp);
            }
            Expr::Index(x, i, sp) => {
                self.n(10);
                self.sub(x, memo);
                self.sub(i, memo);
                self.span(sp);
            }
            Expr::Range(a, b, inc) => {
                self.n(11 + ((*inc as u64) << 8));
                self.opt_e(a, memo);
                self.opt_e(b, memo);
            }
            Expr::Ref(m, x) => {
                self.n(12 + ((*m as u64) << 8));
                self.sub(x, memo);
            }
            Expr::RawRef(m, x) => {
                self.n(13 + ((*m as u64) << 8));
                self.sub(x, memo);
            }
            Expr::Cast(x, t) => {
                self.n(14);
                self.sub(x, memo);
                self.ty(t);
            }
            Expr::Block(l, u, b) => {
                self.n(15 + ((*u as u64) << 8));
                self.opt_s(l);
                self.block(b, memo);
            }
            Expr::Const(b) => {
                self.n(16);
                self.block(b, memo);
            }
            Expr::If(c, t, f) => {
                self.n(17);
                self.sub(c, memo);
                self.block(t, memo);
                self.opt_e(f, memo);
            }
            Expr::Let(p, x) => {
                self.n(18);
                self.pat(p);
                self.sub(x, memo);
            }
            Expr::Match(x, arms) => {
                self.n(19);
                self.sub(x, memo);
                self.n(arms.len() as u64);
                for a in arms {
                    self.pat(&a.pat);
                    match &a.guard {
                        Some(g) => self.sub(g, memo),
                        None => self.n(0xfd),
                    }
                    self.sub(&a.body, memo);
                }
            }
            Expr::While(l, c, b) => {
                self.n(20);
                self.opt_s(l);
                self.sub(c, memo);
                self.block(b, memo);
            }
            Expr::Loop(l, b) => {
                self.n(21);
                self.opt_s(l);
                self.block(b, memo);
            }
            Expr::For(l, p, x, b) => {
                self.n(22);
                self.opt_s(l);
                self.pat(p);
                self.sub(x, memo);
                self.block(b, memo);
            }
            Expr::Break(l, x, _) => {
                self.n(23);
                self.opt_s(l);
                self.opt_e(x, memo);
            }
            Expr::Continue(l, _) => {
                self.n(24);
                self.opt_s(l);
            }
            Expr::Return(x, _) => {
                self.n(25);
                self.opt_e(x, memo);
            }
            Expr::Closure(mv, ps, rt, body, _) => {
                self.n(26 + ((*mv as u64) << 8));
                self.n(ps.len() as u64);
                for (p, t) in ps {
                    self.pat(p);
                    match t {
                        Some(t) => self.ty(t),
                        None => self.n(0xfd),
                    }
                }
                match rt {
                    Some(t) => self.ty(t),
                    None => self.n(0xfd),
                }
                self.sub(body, memo);
            }
            Expr::Tuple(xs) | Expr::Array(xs) => {
                self.n(if matches!(e, Expr::Tuple(..)) { 27 } else { 28 });
                self.n(xs.len() as u64);
                for x in xs {
                    self.sub(x, memo);
                }
            }
            Expr::Repeat(x, n) => {
                self.n(29);
                self.sub(x, memo);
                self.sub(n, memo);
            }
            Expr::Struct(p, fs, rest) => {
                self.n(30);
                self.path(p);
                self.n(fs.len() as u64);
                for (f, x) in fs {
                    self.s(f);
                    self.sub(x, memo);
                }
                self.opt_e(rest, memo);
            }
            Expr::Macro(p, g, sp) => {
                self.n(31);
                self.path(p);
                self.s(g);
                self.span(sp);
            }
            Expr::Try(x) => {
                self.n(32);
                self.sub(x, memo);
            }
            Expr::Paren(x) => {
                self.n(33);
                self.sub(x, memo);
            }
        }
    }
}

/// The pattern binds a part of the value by value (`x`, `Some(x)`), not only by reference or not at all
/// Whether p binds a part of what it matches by `ref mut` (a mutable borrow of it)
pub fn binds_ref_mut(p: &Pat) -> bool {
    match p {
        Pat::Ident(by_ref, m, _, sub) => (*by_ref && *m) || sub.as_deref().is_some_and(binds_ref_mut),
        Pat::Ref(_, q) => binds_ref_mut(q),
        Pat::Or(v, _) | Pat::Tuple(v) | Pat::TupleStruct(_, v) | Pat::Slice(v) => v.iter().any(binds_ref_mut),
        Pat::Struct(_, fs, _) => fs.iter().any(|f| binds_ref_mut(&f.1)),
        _ => false,
    }
}

pub fn binds_by_value(p: &Pat) -> bool {
    match p {
        Pat::Ident(by_ref, _, n, sub) => {
            (!*by_ref && n.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') && n != "_") || sub.as_deref().is_some_and(binds_by_value)
        }
        Pat::Ref(_, q) => binds_by_value(q),
        Pat::Or(v, _) | Pat::Tuple(v) | Pat::TupleStruct(_, v) | Pat::Slice(v) => v.iter().any(binds_by_value),
        Pat::Struct(_, fs, _) => fs.iter().any(|f| binds_by_value(&f.1)),
        _ => false,
    }
}

/// `#![deny(unused_variables)]`: the bindings (and parameters) that nothing after them reads, parameters first and
/// then in order, with the position of each one's `let` (None for a parameter). Names starting with `_` and the
/// compiler's own `__exr_` ones are not reported
pub fn unused_bindings(params: &[&Pat], body: &Block) -> Vec<(Option<Pos>, String)> {
    fn names(p: &Pat, out: &mut Vec<String>) {
        match p {
            Pat::Ident(_, _, n, sub) => {
                out.push(n.clone());
                if let Some(s) = sub {
                    names(s, out);
                }
            }
            Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Slice(ps) => ps.iter().for_each(|q| names(q, out)),
            Pat::Ref(_, q) => names(q, out),
            Pat::Struct(_, fs, _) => fs.iter().for_each(|(_, q)| names(q, out)),
            // (an or-pattern binds the same names in every alternative: the first's)
            Pat::Or(ps, _) => ps.iter().take(1).for_each(|q| names(q, out)),
            _ => {}
        }
    }
    fn reportable(n: &str) -> bool {
        !n.starts_with('_') && n.chars().next().is_some_and(|c| c.is_lowercase())
    }
    // (where an expression starts, when its first part says; else line 0)
    fn expr_pos(e: &Expr) -> Pos {
        match e {
            Expr::Path(_, p) => p.pos,
            Expr::Lit(_, p) => *p,
            Expr::Call(x, _) | Expr::Ref(_, x) | Expr::Paren(x) | Expr::Range(Some(x), _, _) => expr_pos(x),
            Expr::MethodCall(.., s) | Expr::Binary(.., s) | Expr::Field(_, _, s) | Expr::Macro(_, _, s) | Expr::Index(_, _, s) => s.start,
            _ => Pos::default(),
        }
    }
    fn block(b: &Block, found: &mut Vec<(Pos, String)>) {
        for (i, s) in b.stmts.iter().enumerate() {
            if let Stmt::Let(p, _, init, els, pos) = s {
                let mut ns = vec![];
                names(p, &mut ns);
                if ns.iter().any(|n| reportable(n)) {
                    let rest = Expr::Block(None, false, Block { stmts: b.stmts[i + 1..].to_vec(), spos: b.spos.shifted(i + 1, -(i as isize + 1)) });
                    let fv = used_vars(&rest);
                    for n in ns.iter().filter(|n| reportable(n)) {
                        if !fv.contains(n) {
                            found.push((*pos, n.clone()));
                        }
                    }
                }
                if let Some(e) = init {
                    expr(e, found);
                }
                if let Some(eb) = els {
                    block(eb, found);
                }
            } else if let Stmt::Expr(e, _) = s {
                expr(e, found);
            }
        }
    }
    // (the `let`s of a condition: `if let p = x && let q = y`)
    fn let_parts<'a>(c: &'a Expr, out: &mut Vec<(&'a Pat, &'a Expr)>) {
        match c {
            Expr::Let(p, x) => out.push((p, x)),
            Expr::Binary(op, l, r, _) if op == "&&" => {
                let_parts(l, out);
                let_parts(r, out);
            }
            _ => {}
        }
    }
    // (an `if let` / `while let` / guard's bindings that neither its condition nor `body` reads, at the scrutinee)
    fn cond_lets(c: &Expr, body: &dyn Fn() -> Vec<String>, found: &mut Vec<(Pos, String)>) {
        let mut ls = vec![];
        let_parts(c, &mut ls);
        if ls.is_empty() {
            return;
        }
        let mut fv = body();
        fv.extend(used_vars(c));
        for (p, x) in ls {
            let mut ns = vec![];
            names(p, &mut ns);
            let at = Some(crate::parse::pat_pos(p)).filter(|q| q.line > 0).unwrap_or_else(|| expr_pos(x));
            if at.line > 0 {
                found.extend(ns.into_iter().filter(|n| reportable(n) && !fv.contains(n)).map(|n| (at, n)));
            }
        }
    }
    fn expr(e: &Expr, found: &mut Vec<(Pos, String)>) {
        match e {
            Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => block(b, found),
            Expr::If(c, t, els) => {
                cond_lets(c, &|| used_vars(&Expr::Block(None, false, t.clone())), found);
                expr(c, found);
                block(t, found);
                if let Some(x) = els {
                    expr(x, found);
                }
            }
            Expr::While(_, c, b) => {
                cond_lets(c, &|| used_vars(&Expr::Block(None, false, b.clone())), found);
                expr(c, found);
                block(b, found);
            }
            Expr::For(_, p, x, b) => {
                expr(x, found);
                // (the loop's pattern: a binding the body never reads, at the iterator, on the `for`'s line)
                let mut ns = vec![];
                names(p, &mut ns);
                let fv = used_vars(&Expr::Block(None, false, b.clone()));
                let at = expr_pos(x);
                if at.line > 0 {
                    found.extend(ns.into_iter().filter(|n| reportable(n) && !fv.contains(n)).map(|n| (at, n)));
                }
                block(b, found);
            }
            // (a `match` arm's bindings its guard and body never read, at the arm's pattern)
            Expr::Match(x, arms) => {
                expr(x, found);
                for a in arms {
                    let mut ns = vec![];
                    names(&a.pat, &mut ns);
                    if ns.iter().any(|n| reportable(n)) {
                        let mut fv = used_vars(&a.body);
                        if let Some(g) = &a.guard {
                            fv.extend(used_vars(g));
                        }
                        found.extend(ns.into_iter().filter(|n| reportable(n) && !fv.contains(n)).map(|n| (a.pos, n)));
                    }
                    if let Some(g) = &a.guard {
                        cond_lets(g, &|| used_vars(&a.body), found);
                        expr(g, found);
                    }
                    expr(&a.body, found);
                }
            }
            // (a closure's parameters its body never reads, at the closure)
            Expr::Closure(_, ps, _, b, pos) => {
                let fv = used_vars(b);
                for (p, _) in ps {
                    let mut ns = vec![];
                    names(p, &mut ns);
                    found.extend(ns.into_iter().filter(|n| reportable(n) && !fv.contains(n)).map(|n| (*pos, n)));
                }
                expr(b, found);
            }
            Expr::Macro(..) => {}
            _ => each_child(e, &mut |c| expr(c, found)),
        }
    }
    let mut found = vec![];
    block(body, &mut found);
    let fv = used_vars(&Expr::Block(None, false, body.clone()));
    let mut ps = vec![];
    params.iter().for_each(|p| names(p, &mut ps));
    found.sort_by_key(|(p, _)| (p.line, p.col));
    ps.into_iter().filter(|n| reportable(n) && !fv.contains(n)).map(|n| (None, n)).chain(found.into_iter().map(|(p, n)| (Some(p), n))).collect()
}

/// Every `let` binding of a body (nested blocks too) with its `let`'s position, in order (the naming lint)
pub fn let_bindings(body: &Block) -> Vec<(Pos, String)> {
    fn names(p: &Pat, out: &mut Vec<String>) {
        match p {
            Pat::Ident(_, _, n, sub) => {
                out.push(n.clone());
                if let Some(s) = sub {
                    names(s, out);
                }
            }
            Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Slice(ps) => ps.iter().for_each(|q| names(q, out)),
            Pat::Ref(_, q) => names(q, out),
            Pat::Struct(_, fs, _) => fs.iter().for_each(|(_, q)| names(q, out)),
            _ => {}
        }
    }
    fn block(b: &Block, out: &mut Vec<(Pos, String)>) {
        for s in &b.stmts {
            match s {
                Stmt::Let(p, _, init, els, pos) => {
                    let mut ns = vec![];
                    names(p, &mut ns);
                    out.extend(ns.into_iter().map(|n| (*pos, n)));
                    if let Some(e) = init {
                        expr(e, out);
                    }
                    if let Some(eb) = els {
                        block(eb, out);
                    }
                }
                Stmt::Expr(e, _) => expr(e, out),
                Stmt::Item(_) => {}
            }
        }
    }
    fn expr(e: &Expr, out: &mut Vec<(Pos, String)>) {
        match e {
            Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => block(b, out),
            Expr::If(c, t, els) => {
                expr(c, out);
                block(t, out);
                if let Some(x) = els {
                    expr(x, out);
                }
            }
            Expr::While(_, c, b) => {
                expr(c, out);
                block(b, out);
            }
            Expr::For(_, _, x, b) => {
                expr(x, out);
                block(b, out);
            }
            Expr::Macro(..) => {}
            _ => each_child(e, &mut |c| expr(c, out)),
        }
    }
    let mut out = vec![];
    block(body, &mut out);
    out
}
