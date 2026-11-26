//! `#![deny(unused_parens)]`: parentheses around an expression that rustc finds unnecessary, in the places its lint
//! looks at (an `if` / `while` condition, a `match` scrutinee or arm, a `let` scrutinee or value, a `for` iterator, a
//! `return` / `break` value, an assigned value, an index, an argument, a closure's body, a block's value, a const's
//! value), and not where removing them would read differently (a struct literal in a condition, `({ 0 } + 1)`).
//!
//! The parser keeps no place for parentheses: the `(` is found from the first token inside, in the item's text, and
//! when that has no place, they are left. Only a line the expansion left as written is reported: what a macro's
//! expansion put there (`({ .. })`, a fragment in parentheses) is not the program's.

use crate::lexer::{ItemKind, SrcItem};
use crate::parse::{Block, Expr, Pos, Stmt};

/// The places rustc's lint looks at
#[derive(Clone, Copy, PartialEq)]
enum At {
    If,
    While,
    Match,
    Arm,
    /// (the scrutinee of `if let` / `while let`)
    Let,
    For,
    Return,
    Break,
    /// (a `let`'s value, an assignment's, a const's)
    Assigned,
    /// (the value of a `let .. else`)
    LetElse,
    Index,
    FnArg,
    MethodArg,
    Closure,
    BlockValue,
}

impl At {
    fn what(self) -> &'static str {
        match self {
            At::If => "`if` condition",
            At::While => "`while` condition",
            At::Match => "`match` scrutinee expression",
            At::Arm => "match arm expression",
            At::Let => "`let` scrutinee expression",
            At::For => "`for` iterator expression",
            At::Return => "`return` value",
            At::Break => "`break` value",
            At::Assigned | At::LetElse => "assigned value",
            At::Index => "index expression",
            At::FnArg => "function argument",
            At::MethodArg => "method argument",
            At::Closure => "closure body",
            At::BlockValue => "block return value",
        }
    }
}

/// Parentheses rustc finds unnecessary, in the order its lint pass meets them: where the first token inside with a
/// place is, the tokens before it back to the `(` around (that one included, nearest first: the inner's own `(`, a
/// `&`, a keyword), and the place
type Found = Vec<(Pos, Vec<&'static str>, At)>;

/// Where an expression's first token with a place is, and the tokens of its own before that (nearest first); None
/// when it has none
fn lead(e: &Expr) -> Option<(Pos, Vec<&'static str>)> {
    let with = |x: &Expr, before: &[&'static str]| {
        lead(x).map(|(p, mut v)| {
            v.extend_from_slice(before);
            (p, v)
        })
    };
    match e {
        Expr::Paren(x) => with(x, &["("]),
        Expr::Tuple(xs) => with(xs.first()?, &["("]),
        Expr::Ref(m, x) => with(x, if *m { &["mut", "&"] } else { &["&"] }),
        Expr::If(c, ..) => with(c, &["if"]),
        Expr::Match(x, _) => with(x, &["match"]),
        Expr::Call(x, _) | Expr::MethodCall(x, ..) | Expr::Field(x, ..) | Expr::Index(x, ..) | Expr::Cast(x, _) | Expr::Try(x) | Expr::Assign(x, _) | Expr::AssignOp(_, x, ..) | Expr::Range(Some(x), ..) => lead(x),
        Expr::Unary(_, _, s) | Expr::Binary(_, _, _, s) | Expr::Macro(_, _, s) => Some((s.start, vec![])),
        Expr::Lit(_, p) | Expr::Closure(.., p) | Expr::Return(_, p) => Some((*p, vec![])),
        Expr::Path(None, p) | Expr::Struct(p, ..) => Some((p.pos, vec![])),
        // (at the label, when there is one)
        Expr::Break(l, _, p) => Some((*p, if l.is_some() { vec!["break"] } else { vec![] })),
        Expr::Continue(l, p) => Some((*p, if l.is_some() { vec!["continue"] } else { vec![] })),
        _ => None,
    }
    .filter(|(p, _)| p.line > 0)
}

/// Whether a struct literal is in e outside other brackets (rustc's contains_exterior_struct_lit; also through `&`,
/// a range and a call's callee, where it does not look)
fn struct_lit(e: &Expr) -> bool {
    match e {
        Expr::Struct(..) => true,
        Expr::Binary(_, l, r, _) | Expr::AssignOp(_, l, r, _) | Expr::Assign(l, r) => struct_lit(l) || struct_lit(r),
        Expr::Range(l, r, _) => l.as_deref().is_some_and(struct_lit) || r.as_deref().is_some_and(struct_lit),
        Expr::Unary(_, x, _) | Expr::Cast(x, _) | Expr::Field(x, ..) | Expr::Index(x, ..) | Expr::Try(x) | Expr::Ref(_, x) | Expr::RawRef(_, x) | Expr::MethodCall(x, ..) | Expr::Call(x, _) | Expr::Let(_, x) => struct_lit(x),
        _ => false,
    }
}

/// Whether e ends with a `}` (rustc's expr_trailing_brace, and a macro's call, whose brackets are not known)
fn ends_with_brace(e: &Expr) -> bool {
    match e {
        Expr::Struct(..) | Expr::Block(..) | Expr::Const(..) | Expr::If(..) | Expr::Match(..) | Expr::Loop(..) | Expr::While(..) | Expr::For(..) | Expr::Macro(..) => true,
        Expr::Binary(_, _, r, _) | Expr::Unary(_, r, _) | Expr::Ref(_, r) | Expr::RawRef(_, r) | Expr::Assign(_, r) | Expr::AssignOp(_, _, r, _) | Expr::Closure(_, _, _, r, _) | Expr::Range(_, Some(r), _) | Expr::Return(Some(r), _) | Expr::Break(_, Some(r), _) => ends_with_brace(r),
        _ => false,
    }
}

/// Whether rustc keeps the parentheses around e at a place (is_expr_delims_necessary): removing them would change
/// how it reads, or they are kept on purpose (`&raw`). Where it is not sure, kept
fn kept(e: &Expr, at: At) -> bool {
    match e {
        Expr::RawRef(..) => return true,
        Expr::Range(..) if at == At::Let => return true,
        Expr::Binary(op, ..) if matches!(op.as_str(), "&&" | "||") && matches!(at, At::Let | At::LetElse) => return true,
        Expr::Assign(..) if matches!(at, At::Return | At::Break) => return true,
        _ => {}
    }
    if at == At::LetElse && ends_with_brace(e) {
        return true;
    }
    // (`({ 0 } + 1)`: what the operation, the call or the cast starts with would end the statement there; a macro's
    // call may be one)
    let mut x = e;
    loop {
        x = match x {
            Expr::Binary(_, l, ..) | Expr::Call(l, _) | Expr::Cast(l, _) | Expr::Index(l, ..) => l,
            _ => break,
        };
        if matches!(x, Expr::If(..) | Expr::Match(..) | Expr::Block(..) | Expr::Loop(..) | Expr::While(..) | Expr::For(..) | Expr::Const(..) | Expr::Macro(..)) {
            return true;
        }
    }
    if !matches!(at, At::If | At::While | At::Match | At::Let | At::For) {
        return false;
    }
    // (a block follows: a struct literal outside other brackets would take it, and so would what the expression ends
    // with: `if (x == return) {}`, `if (break x) {}`, `for _ in (0..{ 2 }) {}`)
    if struct_lit(e) {
        return true;
    }
    let mut x = e;
    loop {
        x = match x {
            Expr::Unary(_, y, _) | Expr::Binary(_, _, y, _) | Expr::AssignOp(_, _, y, _) | Expr::Assign(_, y) | Expr::Ref(_, y) | Expr::RawRef(_, y) => y,
            Expr::Return(..) | Expr::Closure(..) | Expr::Macro(..) => return true,
            Expr::Break(_, v, _) => return v.as_deref().is_some_and(|v| matches!(v, Expr::Block(..) | Expr::Const(..) | Expr::Path(..) | Expr::Struct(..))),
            Expr::Range(_, Some(r), _) => return matches!(**r, Expr::Block(..) | Expr::Const(..)),
            _ => return false,
        };
    }
}

/// The parentheses directly at a place, unless rustc keeps them
fn check(v: &Expr, at: At, out: &mut Found) {
    let Expr::Paren(inner) = v else { return };
    if kept(inner, at) {
        return;
    }
    if let Some((p, mut before)) = lead(inner) {
        before.push("(");
        out.push((p, before, at));
    }
}

/// A condition: the scrutinee of a `let` in it, or itself
fn cond(c: &Expr, at: At, out: &mut Found) {
    match c {
        Expr::Let(_, x) => check(x, At::Let, out),
        _ => check(c, at, out),
    }
}

/// rustc's check of a statement: a `let`'s value, a block's value
fn stmt_own(s: &Stmt, out: &mut Found) {
    match s {
        Stmt::Let(_, _, Some(e), els, _) => check(e, if els.is_some() { At::LetElse } else { At::Assigned }, out),
        Stmt::Expr(e, false) => check(e, At::BlockValue, out),
        _ => {}
    }
}

/// rustc's check of an expression (UnusedParens::check_expr): an `if let` looks at its block's statements and its
/// `else` there too, a `match` at its arms before its scrutinee
fn own(e: &Expr, out: &mut Found) {
    match e {
        Expr::If(c, t, els) if matches!(**c, Expr::Let(..)) => {
            cond(c, At::If, out);
            for s in &t.stmts {
                stmt_own(s, out);
            }
            if let Some(e) = els {
                delims(e, out);
            }
            return;
        }
        Expr::Match(_, arms) => {
            for a in arms {
                check(&a.body, At::Arm, out);
            }
        }
        _ => {}
    }
    delims(e, out);
}

/// UnusedDelimLint::check_expr: the places an expression has
fn delims(e: &Expr, out: &mut Found) {
    match e {
        Expr::If(c, ..) => cond(c, At::If, out),
        Expr::While(_, c, _) => cond(c, At::While, out),
        Expr::For(_, _, x, _) => check(x, At::For, out),
        Expr::Match(x, _) => check(x, At::Match, out),
        Expr::Return(Some(v), _) => check(v, At::Return, out),
        // (`break 'a ({ .. })`: without them the block would be the loop's label's)
        Expr::Break(l, Some(v), _) if !(l.is_some() && matches!(&**v, Expr::Paren(i) if matches!(**i, Expr::Block(..)))) => check(v, At::Break, out),
        Expr::Index(_, i, _) => check(i, At::Index, out),
        Expr::Assign(_, r) | Expr::AssignOp(_, _, r, _) => check(r, At::Assigned, out),
        Expr::Call(_, args) => args.iter().for_each(|a| check(a, At::FnArg, out)),
        Expr::MethodCall(_, _, _, args, _) => args.iter().for_each(|a| check(a, At::MethodArg, out)),
        Expr::Closure(_, _, None, body, _) => check(body, At::Closure, out),
        _ => {}
    }
}

fn block(b: &Block, out: &mut Found) {
    for s in &b.stmts {
        stmt_own(s, out);
        match s {
            Stmt::Let(_, _, init, els, _) => {
                if let Some(e) = init {
                    expr(e, out);
                }
                if let Some(b) = els {
                    block(b, out);
                }
            }
            Stmt::Expr(e, _) => expr(e, out),
            Stmt::Item(_) => {}
        }
    }
}

/// An expression's checks, then its parts' in the order rustc's visitor walks them
fn expr(e: &Expr, out: &mut Found) {
    own(e, out);
    match e {
        Expr::Unary(_, x, _) | Expr::Try(x) | Expr::Paren(x) | Expr::Ref(_, x) | Expr::RawRef(_, x) | Expr::Cast(x, _) | Expr::Field(x, ..) | Expr::Let(_, x) | Expr::Closure(_, _, _, x, _) => expr(x, out),
        Expr::Binary(_, l, r, _) | Expr::Assign(l, r) | Expr::AssignOp(_, l, r, _) | Expr::Index(l, r, _) | Expr::Repeat(l, r) => {
            expr(l, out);
            expr(r, out);
        }
        Expr::Call(x, args) | Expr::MethodCall(x, _, _, args, _) => {
            expr(x, out);
            args.iter().for_each(|a| expr(a, out));
        }
        Expr::Range(a, b, _) => a.iter().chain(b.iter()).for_each(|x| expr(x, out)),
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => block(b, out),
        Expr::If(c, t, els) => {
            expr(c, out);
            block(t, out);
            if let Some(e) = els {
                expr(e, out);
            }
        }
        Expr::Match(x, arms) => {
            expr(x, out);
            for a in arms {
                if let Some(g) = &a.guard {
                    expr(g, out);
                }
                expr(&a.body, out);
            }
        }
        Expr::While(_, c, b) | Expr::For(_, _, c, b) => {
            expr(c, out);
            block(b, out);
        }
        Expr::Break(_, Some(v), _) | Expr::Return(Some(v), _) => expr(v, out),
        Expr::Tuple(xs) | Expr::Array(xs) => xs.iter().for_each(|x| expr(x, out)),
        Expr::Struct(_, fs, base) => {
            fs.iter().for_each(|f| expr(&f.1, out));
            if let Some(b) = base {
                expr(b, out);
            }
        }
        _ => {}
    }
}

/// The first unnecessary parentheses in the program's fns and its consts' and statics' values, where the lint is
/// denied (for the crate, or by an attribute around them): `line:col: unnecessary parentheses around ..`, at the `(`
pub(super) fn first_unused(src: &str, src0: &str, items: &[SrcItem], crate_deny: bool) -> Option<String> {
    // (the starts of the lines of the source as written, read once something is found)
    let mut lines0: Option<Vec<usize>> = None;
    for it in items {
        let text = src.get(it.start..it.end).unwrap_or("");
        let body = text[super::leading_attrs(text).len()..].trim_start();
        // (past the visibility: `pub`, `pub(crate)`)
        let body = match body.strip_prefix("pub").map(str::trim_start) {
            Some(r) if r.starts_with('(') => r.find(')').map_or(r, |k| r[k + 1..].trim_start()),
            Some(r) => r,
            None => body,
        };
        // (a const's or a static's value is looked at at the top level, not in an impl or a trait)
        let value = it.kind == ItemKind::Normal && (body.starts_with("const ") || body.starts_with("static "));
        if !(value || text.contains("fn ")) || text.contains("#[cfg") {
            continue;
        }
        let attrs = if text.contains("unused") || text.contains("warnings") { super::lint_attrs(text, "unused_parens") } else { vec![] };
        if !crate_deny && !attrs.iter().any(|a| a.2 > 0) {
            continue;
        }
        let Ok(f) = crate::parse::parse_file(text) else { continue };
        let mut found = vec![];
        match f.items.first().map(|i| &i.kind) {
            Some(crate::parse::ItemKind::Fn(_, Some(b))) => block(b, &mut found),
            Some(crate::parse::ItemKind::Const(_, _, Some(e)) | crate::parse::ItemKind::Static(_, _, _, e)) if value => {
                check(e, At::Assigned, &mut found);
                expr(e, &mut found);
            }
            _ => {}
        }
        if found.is_empty() {
            continue;
        }
        // (the level at a place of the item's text: the crate's, then each attribute that covers it, in order)
        let denied = |o: usize| {
            let (mut on, mut forbid) = (crate_deny, false);
            for &(_, _, l) in attrs.iter().filter(|a| a.0 <= o && o < a.1) {
                if l == 2 {
                    on = true;
                    forbid = true;
                } else if !forbid {
                    on = l == 1;
                }
            }
            on
        };
        let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
        let tb = text.as_bytes();
        let id = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
        for (p, before, at) in found {
            let Some(&ls) = starts.get(p.line as usize - 1) else { continue };
            let Some(mut o) = text[ls..].char_indices().nth(p.col as usize).map(|(i, _)| ls + i) else { continue };
            // (back from that token over the ones before it, each after any space or `/* */` comment; a word not the end
            // of a longer one)
            let ok = before.iter().all(|t| {
                loop {
                    while o > 0 && tb[o - 1].is_ascii_whitespace() {
                        o -= 1;
                    }
                    match text[..o].strip_suffix("*/").and_then(|r| r.rfind("/*")) {
                        Some(k) => o = k,
                        None => break,
                    }
                }
                if !text[..o].ends_with(t) || (id(t.as_bytes()[0]) && o > t.len() && id(tb[o - t.len() - 1])) {
                    return false;
                }
                o -= t.len();
                true
            });
            if !ok || !denied(o) {
                continue;
            }
            let abs = it.start + o;
            let ls = src[..abs].rfind('\n').map_or(0, |k| k + 1);
            let le = src[abs..].find('\n').map_or(src.len(), |k| abs + k);
            let line = it.line + text[..o].matches('\n').count();
            // (on a line as written: not one an expansion, the moving of a nested item or a rewriting changed)
            let l0 = lines0.get_or_insert_with(|| std::iter::once(0).chain(src0.match_indices('\n').map(|(i, _)| i + 1)).collect());
            if !l0.get(line - 1).is_some_and(|&s| src0[s..].split('\n').next() == Some(&src[ls..le])) {
                continue;
            }
            let col = src[ls..abs].chars().count() + 1;
            return Some(format!("{line}:{col}: unnecessary parentheses around {}", at.what()));
        }
    }
    None
}

/// Whether an `allow` / `expect` / `warn` of unused_parens (or of `unused`) covers more than the item or statement it
/// is written on, which the items' text does not show: one on a `mod` (its header is gone from the flattened text), an
/// `impl` or a `trait` (their members are items of their own), or an inner one past the crate's head
pub(super) fn allowed_wide(src0: &str) -> bool {
    let b = src0.as_bytes();
    let id = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
    // (the crate's head: the inner attributes the crate's level was read from)
    let head = {
        let mut i = 0;
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if b[i..].starts_with(b"//") {
                i += b[i..].iter().position(|&c| c == b'\n').unwrap_or(b.len() - i);
            } else if b[i..].starts_with(b"/*") {
                i += src0[i + 2..].find("*/").map_or(b.len() - i, |k| k + 4);
            } else if b[i..].starts_with(b"#![") {
                i += src0[i..].find(']').map_or(b.len() - i, |k| k + 1);
            } else {
                break;
            }
        }
        i.min(4096)
    };
    let word = |t: &str, w: &str| t.starts_with(w) && !t.as_bytes().get(w.len()).is_some_and(|&c| id(c));
    src0.match_indices("unused").any(|(i, _)| {
        let end = i + 6;
        let name = (i == 0 || !id(b[i - 1])) && (!b.get(end).is_some_and(|&c| id(c)) || word(&src0[end..], "_parens"));
        if !name || i < head {
            return false;
        }
        // (in the list of an attribute's level: `#[allow(` / `#![expect(`, other names before it)
        let Some(open) = src0[..i].rfind('(') else { return false };
        if !src0[open + 1..i].bytes().all(|c| id(c) || c == b',' || c.is_ascii_whitespace()) {
            return false;
        }
        let before = src0[..open].trim_end();
        let Some(lv) = ["allow", "expect", "warn"].into_iter().find(|l| before.ends_with(l)) else { return false };
        let Some(before) = before[..before.len() - lv.len()].trim_end().strip_suffix('[') else { return false };
        let before = before.trim_end();
        if let Some(inner) = before.strip_suffix('!') {
            return inner.trim_end().ends_with('#');
        }
        if !before.ends_with('#') {
            return false;
        }
        // (an outer one: what it is on, past its list, the attributes after it and the visibility)
        let Some(close) = src0[i..].find(']') else { return false };
        let mut rest = src0[i + close + 1..].trim_start();
        while rest.starts_with("#[") {
            rest = rest.find(']').map_or("", |k| rest[k + 1..].trim_start());
        }
        if let Some(r) = rest.strip_prefix("pub") {
            let r = r.trim_start();
            rest = if r.starts_with('(') { r.find(')').map_or("", |k| r[k + 1..].trim_start()) } else { r };
        }
        if let Some(r) = rest.strip_prefix("unsafe") {
            rest = r.trim_start();
        }
        word(rest, "mod") || word(rest, "impl") || word(rest, "trait") || word(rest, "auto")
    })
}
