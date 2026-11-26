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

/// The lint's levels at a place: its own (0 allow / expect, 1 warn, 2 deny, 3 forbid) and whether `warnings` is denied,
/// which makes it an error while it is at warn; rustc keeps the two apart (`#[allow(warnings)]` leaves a deny of it,
/// `#[warn(unused_parens)]` is an error under `#![deny(warnings)]`)
pub(super) type Levels = (u8, bool);

/// Whether the lint is an error at those levels
pub(super) fn denied((lv, warnings): Levels) -> bool {
    lv >= 2 || (lv == 1 && warnings)
}

/// The levels after an attribute (super::lint_attr's level and names): a forbid of the lint stays
fn apply(at: &mut Levels, level: u8, names: u8) {
    if names & 1 != 0 && at.0 != 3 {
        at.0 = match level {
            2 => 3,
            1 => 2,
            _ if names & 4 != 0 => 1,
            _ => 0,
        };
    }
    if names & 2 != 0 {
        at.1 = level > 0;
    }
}

/// The first unnecessary parentheses in the crate's own fns (on its first `own_lines` lines; all of them at 0) and its
/// consts' and statics' values, where the lint is denied: by the crate's levels, then those of the modules, impls and
/// traits around (levels_around: line ranges), then the attributes in the item's text. `line:col: unnecessary
/// parentheses around ..`, at the `(`
pub(super) fn first_unused(src: &str, src0: &str, items: &[SrcItem], own_lines: usize, crate_levels: Levels, around: &[(usize, usize, u8, u8)]) -> Option<String> {
    // (the starts of the lines of the source as written, read once something is found)
    let mut lines0: Option<Vec<usize>> = None;
    for it in items.iter().filter(|it| own_lines == 0 || it.line <= own_lines) {
        let text = src.get(it.start..it.end).unwrap_or("");
        // (the levels of the blocks it is in, or that are in it (a fn's inner attribute), outer ones first)
        let mut base = crate_levels;
        let last = it.line + text.matches('\n').count();
        for &(_, _, level, names) in around.iter().filter(|r| r.0 <= last && it.line <= r.1) {
            apply(&mut base, level, names);
        }
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
        if !denied(base) && !attrs.iter().any(|a| a.2 > 0) {
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
        // (at a place of the item's text: the levels around it, then each attribute that covers it, in order)
        let denied_at = |o: usize| {
            let mut at = base;
            for &(_, _, level, names) in attrs.iter().filter(|a| a.0 <= o && o < a.1) {
                apply(&mut at, level, names);
            }
            denied(at)
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
            if !ok || !denied_at(o) {
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

/// Past spaces and comments from b[i]
fn skip_space(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        match super::literal_or_comment_end(b, i) {
            Some(e) if b[i] == b'/' => i = e,
            _ if b[i].is_ascii_whitespace() => i += 1,
            _ => break,
        }
    }
    i
}

/// Where a source's text starts: past a byte order mark and a `#!` line (not `#![`)
fn head_start(b: &[u8]) -> usize {
    let i = if b.starts_with(b"\xEF\xBB\xBF") { 3 } else { 0 };
    if b[i..].starts_with(b"#!") && !b[i..].starts_with(b"#![") {
        return i + b[i..].iter().position(|&c| c == b'\n').unwrap_or(b.len() - i);
    }
    i
}

/// The crate's levels of the lint: by its inner attributes, in its head (before its first item), in order
pub(super) fn crate_levels(src0: &str) -> Levels {
    let b = src0.as_bytes();
    let mut at = (1, false);
    let mut i = skip_space(b, head_start(b));
    while b[i..].starts_with(b"#![") {
        let Some(close) = super::attr_close(b, i + 2) else { break };
        let text: String = src0[i + 3..close].chars().filter(|c| !c.is_whitespace()).collect();
        if let Some((level, names)) = super::lint_attr(&text, "unused_parens") {
            apply(&mut at, level, names);
        }
        i = skip_space(b, close + 1);
    }
    at
}

/// The `allow` / `expect` / `warn` of the lint (of unused_parens, `unused` or `warnings`) that cover more than the item
/// or statement they are written on, which the items' text does not show, read from the source as written past its
/// literals and comments: one on a `mod` (its header is gone from the flattened text), an `impl` or a `trait` (their
/// members are items of their own), or an inner one past the crate's head. Each as the lines of the block it covers,
/// its level and names (super::lint_attr), outer ones first. None where what one covers is not known (`#[allow(..)]
/// mod x;`): the lint is left off then. A deny there is not followed (what it would report is missed, never more)
pub(super) fn levels_around(src0: &str) -> Option<Vec<(usize, usize, u8, u8)>> {
    let b = src0.as_bytes();
    let id = |c: u8| c == b'_' || c.is_ascii_alphanumeric();
    let line = |at: usize| 1 + b[..at].iter().filter(|&&c| c == b'\n').count();
    let skip = |i: usize| skip_space(b, i);
    let word = |i: usize, w: &str| b[i..].starts_with(w.as_bytes()) && !b.get(i + w.len()).is_some_and(|&c| id(c));
    // (the blocks open, and the attributes waiting for the end of theirs: where each is, its `{`, level and names)
    let (mut open, mut waiting, mut out) = (vec![], vec![], vec![]);
    // (in the crate's head, before its first item: its inner attributes are the crate's levels (crate_levels))
    let mut head = true;
    let mut i = head_start(b);
    while i < b.len() {
        if let Some(e) = super::literal_or_comment_end(b, i) {
            head = head && b[i] == b'/';
            i = e.max(i + 1);
            continue;
        }
        match b[i] {
            b'{' => open.push(i),
            b'}' => {
                let o = open.pop()?;
                out.extend(waiting.iter().filter(|w: &&(usize, usize, u8, u8)| w.1 == o).map(|w| (w.0, line(o), line(i), w.2, w.3)));
            }
            b'#' if b.get(i + 1) == Some(&b'[') || (b.get(i + 1) == Some(&b'!') && b.get(i + 2) == Some(&b'[')) => {
                let at = i;
                let inner = b[i + 1] == b'!';
                let close = super::attr_close(b, if inner { i + 2 } else { i + 1 })?;
                i = close + 1;
                let text: String = src0[if inner { at + 3 } else { at + 2 }..close].chars().filter(|c| !c.is_whitespace()).collect();
                let lint = super::lint_attr(&text, "unused_parens").filter(|l| l.0 == 0);
                if inner {
                    match (lint, open.last()) {
                        (None, _) => {}
                        // (the crate's own, in its levels)
                        (Some(_), None) if head => {}
                        (Some(_), None) => return None,
                        (Some((level, names)), Some(&o)) => waiting.push((at, o, level, names)),
                    }
                    continue;
                }
                head = false;
                let Some((level, names)) = lint else { continue };
                // (what it is on: past the attributes after it, the visibility and `unsafe`)
                let mut j = skip(i);
                while b[j..].starts_with(b"#[") {
                    j = skip(super::attr_close(b, j + 1)? + 1);
                }
                if word(j, "pub") {
                    j = skip(j + 3);
                    if b.get(j) == Some(&b'(') {
                        j = skip(j + b[j..].iter().position(|&c| c == b')')? + 1);
                    }
                }
                if word(j, "unsafe") {
                    j = skip(j + 6);
                }
                if !(word(j, "mod") || word(j, "impl") || word(j, "trait") || word(j, "auto")) {
                    continue;
                }
                // (its block: the first `{` on, before any `;`)
                while j < b.len() && b[j] != b'{' {
                    if b[j] == b';' {
                        return None;
                    }
                    j = super::literal_or_comment_end(b, j).unwrap_or(j + 1).max(j + 1);
                }
                waiting.push((at, j, level, names));
                continue;
            }
            c if !c.is_ascii_whitespace() => head = false,
            _ => {}
        }
        i += 1;
    }
    if !open.is_empty() {
        return None;
    }
    out.sort_by_key(|r| r.0);
    Some(out.into_iter().map(|(_, a, z, level, names)| (a, z, level, names)).collect())
}
