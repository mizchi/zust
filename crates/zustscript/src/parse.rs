//! Lexer and parser. The lexer runs on demand (one token of lookahead), so the parser can switch to reading JSX
//! text or a raw Rust body (`wasm fn`) at any point: it rewinds to the start of the peeked token and reads characters.

use crate::ast::*;

#[derive(Clone, Debug, PartialEq)]
pub enum T {
    Ident(String),
    Int(i64),
    Float(String),
    Str(String),
    P(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Tok {
    pub t: T,
    pub pos: Pos,
    /// offset of the token's first char
    pub start: usize,
}

pub struct Parser {
    src: Vec<char>,
    i: usize,
    line: u32,
    col: u32,
    peeked: Option<Tok>,
    next_id: Id,
}

pub type PResult<X> = Result<X, (Pos, String)>;

const PUNCTS: &[&str] = &[
    "|>", "..", "::", "->", "=>", "+=", "-=", "*=", "/=", "%=", "==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "=", "<", ">", "!", "(", ")",
    "{", "}", "[", "]", ",", ";", ":", ".", "|", "&", "#", "@",
];

impl Parser {
    pub fn new(src: &str) -> Parser {
        Parser { src: src.chars().collect(), i: 0, line: 1, col: 0, peeked: None, next_id: 0 }
    }

    fn here(&self) -> Pos {
        Pos { line: self.line, col: self.col }
    }

    fn bump(&mut self) -> char {
        let c = self.src[self.i];
        self.i += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 0;
        } else {
            self.col += 1;
        }
        c
    }

    fn at(&self, k: usize) -> char {
        *self.src.get(self.i + k).unwrap_or(&'\0')
    }

    fn skip_ws(&mut self) {
        loop {
            if self.i >= self.src.len() {
                return;
            }
            let c = self.at(0);
            if c.is_whitespace() {
                self.bump();
            } else if c == '/' && self.at(1) == '/' {
                while self.i < self.src.len() && self.at(0) != '\n' {
                    self.bump();
                }
            } else if c == '/' && self.at(1) == '*' {
                while self.i < self.src.len() && !(self.at(0) == '*' && self.at(1) == '/') {
                    self.bump();
                }
                if self.i < self.src.len() {
                    self.bump();
                    self.bump();
                }
            } else {
                return;
            }
        }
    }

    fn lex(&mut self) -> PResult<Tok> {
        self.skip_ws();
        let pos = self.here();
        let start = self.i;
        if self.i >= self.src.len() {
            return Ok(Tok { t: T::Eof, pos, start });
        }
        let c = self.at(0);
        let t = if c.is_alphabetic() || c == '_' {
            let mut s = String::new();
            while self.at(0).is_alphanumeric() || self.at(0) == '_' {
                s.push(self.bump());
            }
            T::Ident(s)
        } else if c.is_ascii_digit() {
            let mut s = String::new();
            let mut float = false;
            while self.at(0).is_ascii_digit() || self.at(0) == '_' || (self.at(0) == '.' && self.at(1).is_ascii_digit() && !float) {
                if self.at(0) == '.' {
                    float = true;
                }
                let ch = self.bump();
                if ch != '_' {
                    s.push(ch);
                }
            }
            // (an exponent: `1.0e10`, `2e-3`)
            if (self.at(0) == 'e' || self.at(0) == 'E')
                && (self.at(1).is_ascii_digit() || ((self.at(1) == '-' || self.at(1) == '+') && self.at(2).is_ascii_digit()))
            {
                float = true;
                s.push(self.bump());
                if !self.at(0).is_ascii_digit() {
                    s.push(self.bump());
                }
                while self.at(0).is_ascii_digit() {
                    s.push(self.bump());
                }
            }
            if float {
                T::Float(s)
            } else {
                T::Int(s.parse().map_err(|_| (pos, format!("integer literal out of range: {s}")))?)
            }
        } else if c == '`' {
            // (a template: `primary` reads its text and holes from here)
            self.bump();
            T::P("`")
        } else if c == '"' {
            self.bump();
            let mut s = String::new();
            loop {
                if self.i >= self.src.len() {
                    return Err((pos, "unterminated string".into()));
                }
                match self.bump() {
                    '"' => break,
                    '\\' => {
                        if self.i >= self.src.len() {
                            return Err((pos, "unterminated string".into()));
                        }
                        let e = self.bump();
                        s.push(match e {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            '0' => '\0',
                            o => o,
                        })
                    }
                    o => s.push(o),
                }
            }
            T::Str(s)
        } else {
            let mut found = None;
            for p in PUNCTS {
                if p.chars().enumerate().all(|(k, pc)| self.at(k) == pc) {
                    found = Some(*p);
                    break;
                }
            }
            let Some(p) = found else { return Err((pos, format!("unexpected character `{c}`"))) };
            for _ in 0..p.len() {
                self.bump();
            }
            T::P(p)
        };
        Ok(Tok { t, pos, start })
    }

    fn peek(&mut self) -> PResult<&Tok> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lex()?);
        }
        Ok(self.peeked.as_ref().unwrap())
    }

    fn next(&mut self) -> PResult<Tok> {
        self.peek()?;
        Ok(self.peeked.take().unwrap())
    }

    /// Drops the peeked token and rewinds to its start (to read characters from there)
    fn unpeek(&mut self) {
        if let Some(t) = self.peeked.take() {
            self.i = t.start;
            self.line = t.pos.line;
            self.col = t.pos.col;
        }
    }

    fn is(&mut self, p: &str) -> PResult<bool> {
        Ok(matches!(&self.peek()?.t, T::P(q) if *q == p))
    }

    fn is_kw(&mut self, k: &str) -> PResult<bool> {
        Ok(matches!(&self.peek()?.t, T::Ident(q) if q == k))
    }

    fn eat(&mut self, p: &str) -> PResult<bool> {
        if self.is(p)? {
            self.next()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect(&mut self, p: &str) -> PResult<Pos> {
        let t = self.next()?;
        match &t.t {
            T::P(q) if *q == p => Ok(t.pos),
            o => Err((t.pos, format!("expected `{p}`, found {}", show(o)))),
        }
    }

    fn ident(&mut self) -> PResult<(String, Pos)> {
        let t = self.next()?;
        match t.t {
            T::Ident(s) => Ok((s, t.pos)),
            o => Err((t.pos, format!("expected a name, found {}", show(&o)))),
        }
    }

    fn mk(&mut self, pos: Pos, kind: EK) -> Expr {
        self.next_id += 1;
        Expr { id: self.next_id, pos, kind }
    }

    // ------------------------------------------------------------ items

    pub fn file(&mut self) -> PResult<Vec<Item>> {
        let mut items = vec![];
        while self.peek()?.t != T::Eof {
            items.push(self.item()?);
        }
        Ok(items)
    }

    fn generics(&mut self) -> PResult<Vec<String>> {
        Ok(self.generics_bounds()?.0)
    }

    /// `<T, N: Num>`
    fn generics_bounds(&mut self) -> PResult<(Vec<String>, Vec<(String, String)>)> {
        let (mut g, mut b) = (vec![], vec![]);
        if self.eat("<")? {
            while !self.eat(">")? {
                let n = self.ident()?.0;
                if self.eat(":")? {
                    b.push((n.clone(), self.ident()?.0));
                }
                g.push(n);
                self.eat(",")?;
            }
        }
        Ok((g, b))
    }

    fn item(&mut self) -> PResult<Item> {
        // `@wasm fn ..`: a zustscript function that runs as wasm
        // `@simd fn ..`: an array kernel vectorized with v128
        let (wasm, simd) = if self.eat("@")? {
            let (a, apos) = self.ident()?;
            match a.as_str() {
                "wasm" => (true, false),
                "simd" => (false, true),
                _ => return Err((apos, format!("unknown attribute `@{a}` (there are `@wasm` and `@simd`)"))),
            }
        } else {
            (false, false)
        };
        // `@simd(relaxed)`
        let relaxed = if simd && self.eat("(")? {
            let (a, apos) = self.ident()?;
            if a != "relaxed" {
                return Err((apos, format!("unknown option `{a}` of @simd (there is `relaxed`)")));
            }
            self.expect(")")?;
            true
        } else {
            false
        };
        let wasm_attr = wasm || simd;
        let public = if self.is_kw("export")? || self.is_kw("pub")? {
            self.next()?;
            true
        } else {
            false
        };
        let is_async = if self.is_kw("async")? {
            let t = self.next()?;
            if !self.is_kw("fn")? {
                return Err((t.pos, "`async` goes on a `fn` (`async fn load() -> String { .. }`) or a closure (`async () => ..`)".into()));
            }
            true
        } else {
            false
        };
        let (kw, pos) = self.ident()?;
        match kw.as_str() {
            _ if wasm_attr && kw != "fn" => Err((pos, "`@wasm` / `@simd` go on a `fn`".into())),
            "type" => {
                let (name, _) = self.ident()?;
                let params = self.generics()?;
                self.expect("=")?;
                self.expect("{")?;
                let mut fields = vec![];
                while !self.eat("}")? {
                    let (f, _) = self.ident()?;
                    self.expect(":")?;
                    fields.push((f, self.ty()?));
                    self.eat(",")?;
                }
                Ok(Item::Record { name, params, fields, pos, from: None })
            }
            "enum" => {
                let (name, _) = self.ident()?;
                let params = self.generics()?;
                self.expect("{")?;
                let mut variants = vec![];
                while !self.eat("}")? {
                    let (v, _) = self.ident()?;
                    let mut tys = vec![];
                    if self.eat("(")? {
                        while !self.eat(")")? {
                            tys.push(self.ty()?);
                            self.eat(",")?;
                        }
                    }
                    let ret = if self.eat("->")? { Some(self.ty()?) } else { None };
                    variants.push((v, tys, ret));
                    self.eat(",")?;
                }
                Ok(Item::Enum { name, params, variants, pos, from: None })
            }
            "fn" => {
                let (name, _) = self.ident()?;
                let (generics, bounds) = self.generics_bounds()?;
                self.expect("(")?;
                let mut args = vec![];
                while !self.eat(")")? {
                    let (a, _) = self.ident()?;
                    self.expect(":")?;
                    args.push((a, self.ty()?));
                    self.eat(",")?;
                }
                let ret = if self.eat("->")? { Some(self.ty()?) } else { None };
                let body = self.block()?;
                Ok(Item::Fn { name, public, wasm, simd, relaxed, is_async, generics, bounds, args, ret, body, pos })
            }
            // `import type { User, Post } from "./model.ts";` / `import { greet, Shape } from "./lib.zsc";`
            "import" => {
                let values = self.is("{")?;
                if !values {
                    let (t, tpos) = self.ident()?;
                    if t != "type" {
                        return Err((tpos, "expected `import { .. } from \"./lib.zsc\";` or `import type { .. } from \"./model.ts\";`".into()));
                    }
                }
                self.expect("{")?;
                let mut names = vec![];
                while !self.eat("}")? {
                    names.push(self.ident()?);
                    self.eat(",")?;
                }
                let (f, fpos) = self.ident()?;
                if f != "from" {
                    return Err((fpos, "expected `from \"./file.ts\"`".into()));
                }
                let from = match self.next()? {
                    Tok { t: T::Str(s), .. } => s,
                    t => return Err((t.pos, "expected the file's path as a string".into())),
                };
                self.expect(";")?;
                Ok(Item::ImportType { names, from, values, pos })
            }
            "extern" => {
                let module = match &self.peek()?.t {
                    T::Str(s) => {
                        let s = s.clone();
                        self.next()?;
                        Some(s)
                    }
                    _ => None,
                };
                let (f, fpos) = self.ident()?;
                if f != "fn" {
                    return Err((fpos, "expected `fn` after `extern`".into()));
                }
                let (name, _) = self.ident()?;
                let generics = self.generics()?;
                self.expect("(")?;
                let mut args = vec![];
                while !self.eat(")")? {
                    self.ident()?;
                    self.expect(":")?;
                    args.push(self.ty()?);
                    self.eat(",")?;
                }
                let ret = if self.eat("->")? { self.ty()? } else { TyS::Unit };
                let js = if self.eat("=")? {
                    match self.next()? {
                        Tok { t: T::Str(s), .. } => s,
                        t => return Err((t.pos, "expected the JS name as a string".into())),
                    }
                } else {
                    name.clone()
                };
                self.expect(";")?;
                Ok(Item::Extern { name, module, generics, args, ret, js, pos })
            }
            "wasm" => {
                if self.is("{")? {
                    let text = self.raw_braces()?;
                    return Ok(Item::WasmRaw(text[1..text.len() - 1].to_string()));
                }
                // the Rust text starts at `fn`
                let fstart = self.peek()?.start;
                let (f, fpos) = self.ident()?;
                if f != "fn" {
                    return Err((fpos, "expected `fn` or `{` after `wasm`".into()));
                }
                let (name, _) = self.ident()?;
                self.expect("(")?;
                let mut args = vec![];
                while !self.is(")")? {
                    let (a, _) = self.ident()?;
                    self.expect(":")?;
                    args.push((a, self.wty(&[",", ")"])?));
                    self.eat(",")?;
                }
                self.expect(")")?;
                let ret = if self.eat("->")? { self.wty(&["{"])? } else { WTy::Unit };
                let body = self.raw_braces()?;
                let head: String = self.src[fstart..self.i - body.chars().count()].iter().collect();
                Ok(Item::WasmFn { name, public, args, ret, text: format!("{head}{body}"), pos })
            }
            _ => Err((pos, format!("expected an item (`type`, `enum`, `fn`, `extern`, `wasm`), found `{kw}`"))),
        }
    }

    /// A boundary type, spelled as in Rust: the tokens up to one of `ends` at depth 0
    fn wty(&mut self, ends: &[&str]) -> PResult<WTy> {
        let pos = self.peek()?.pos;
        let mut s = String::new();
        let mut depth = 0i32;
        loop {
            let t = self.peek()?.clone();
            if let T::P(p) = &t.t {
                if depth == 0 && ends.contains(p) {
                    break;
                }
                match *p {
                    "<" | "[" | "(" => depth += 1,
                    ">" | "]" | ")" => depth -= 1,
                    _ => {}
                }
            }
            if t.t == T::Eof {
                return Err((t.pos, "unexpected end of file".into()));
            }
            s.push_str(&match self.next()?.t {
                T::Ident(x) => x,
                T::P(p) => p.to_string(),
                o => show(&o),
            });
        }
        if let Some(cb) = callback_ty(&s) {
            return Ok(cb);
        }
        Ok(match s.as_str() {
            "i32" => WTy::I32,
            "i64" => WTy::I64,
            "u32" => WTy::U32,
            "f32" => WTy::F32,
            "f64" => WTy::F64,
            "bool" => WTy::Bool,
            "()" => WTy::Unit,
            _ if s.starts_with("&[") && s.ends_with(']') && Elem::parse(&s[2..s.len() - 1]).is_some() => WTy::Slice(Elem::parse(&s[2..s.len() - 1]).unwrap()),
            _ if s.starts_with("Vec<") && s.ends_with('>') && Elem::parse(&s[4..s.len() - 1]).is_some() => WTy::Vec(Elem::parse(&s[4..s.len() - 1]).unwrap()),
            "&str" => WTy::Str,
            "String" => WTy::String,
            _ => {
                return Err((
                    pos,
                    format!(
                        "`{s}` cannot cross the JS / wasm boundary (allowed: i32 u32 i64 f64 bool (); &[T] (T: i32 u32 i64 f64), &str and &dyn Fn(..) / Box<dyn Fn(..)> of scalars as arguments; String, Vec<T> as results)"
                    ),
                ));
            }
        })
    }

    /// The text of a `{ .. }` group, read as characters (Rust strings, chars and comments do not end it)
    fn raw_braces(&mut self) -> PResult<String> {
        let pos = self.peek()?.pos;
        self.unpeek();
        self.skip_ws();
        let start = self.i;
        if self.at(0) != '{' {
            return Err((pos, "expected `{`".into()));
        }
        let mut depth = 0;
        loop {
            if self.i >= self.src.len() {
                return Err((pos, "unclosed `{`".into()));
            }
            let c = self.at(0);
            if c == '"' {
                self.bump();
                while self.i < self.src.len() && self.at(0) != '"' {
                    if self.bump() == '\\' {
                        self.bump();
                    }
                }
                self.bump();
                continue;
            }
            if c == '\'' {
                // a char literal ('x', '\n'); otherwise a lifetime
                if self.at(1) == '\\' {
                    self.bump();
                    while self.i < self.src.len() && self.at(0) != '\'' {
                        self.bump();
                    }
                    self.bump();
                    continue;
                }
                if self.at(2) == '\'' {
                    self.bump();
                    self.bump();
                    self.bump();
                    continue;
                }
            }
            if c == '/' && (self.at(1) == '/' || self.at(1) == '*') {
                self.skip_ws();
                continue;
            }
            self.bump();
            if c == '{' {
                depth += 1;
            } else if c == '}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
        Ok(self.src[start..self.i].iter().collect())
    }

    // ------------------------------------------------------------ types

    fn ty(&mut self) -> PResult<TyS> {
        if self.eat("(")? {
            let mut ts = vec![];
            while !self.eat(")")? {
                ts.push(self.ty()?);
                self.eat(",")?;
            }
            return Ok(if ts.is_empty() { TyS::Unit } else { TyS::Tuple(ts) });
        }
        if self.is_kw("fn")? {
            self.next()?;
            self.expect("(")?;
            let mut ts = vec![];
            while !self.eat(")")? {
                ts.push(self.ty()?);
                self.eat(",")?;
            }
            let r = if self.eat("->")? { self.ty()? } else { TyS::Unit };
            return Ok(TyS::Fn(ts, Box::new(r)));
        }
        let (n, pos) = self.ident()?;
        let mut args = vec![];
        if self.eat("<")? {
            while !self.eat(">")? {
                args.push(self.ty()?);
                self.eat(",")?;
            }
        }
        Ok(TyS::Name(n, args, pos))
    }

    // ------------------------------------------------------------ expressions

    fn block(&mut self) -> PResult<Expr> {
        let pos = self.expect("{")?;
        let mut stmts = vec![];
        let mut tail = None;
        loop {
            if self.eat("}")? {
                break;
            }
            if self.is_kw("let")? {
                self.next()?;
                if self.is_kw("mut")? {
                    self.next()?;
                    let (n, _) = self.ident()?;
                    let t = if self.eat(":")? { Some(self.ty()?) } else { None };
                    self.expect("=")?;
                    let e = self.expr(false)?;
                    self.expect(";")?;
                    stmts.push(Stmt::LetMut(n, t, e));
                    continue;
                }
                let p = self.pat()?;
                let t = if self.eat(":")? { Some(self.ty()?) } else { None };
                self.expect("=")?;
                let e = self.expr(false)?;
                self.expect(";")?;
                stmts.push(Stmt::Let(p, t, e));
                continue;
            }
            let mut e = self.expr(false)?;
            // `x = e;` / `x += e;`
            let t = self.peek()?.clone();
            if let T::P(op @ ("=" | "+=" | "-=" | "*=" | "/=" | "%=")) = t.t {
                let EK::Var(n) = &e.kind else { return Err((t.pos, "only a local variable can be assigned".into())) };
                let n = n.clone();
                self.next()?;
                let mut r = self.expr(false)?;
                if op != "=" {
                    let lhs = self.mk(e.pos, EK::Var(n.clone()));
                    let bop: &'static str = &op[..1];
                    let bop = match bop {
                        "+" => "+",
                        "-" => "-",
                        "*" => "*",
                        "/" => "/",
                        _ => "%",
                    };
                    r = self.mk(t.pos, EK::Bin(bop, Box::new(lhs), Box::new(r)));
                }
                e = self.mk(t.pos, EK::Assign(n, Box::new(r)));
            }
            if self.eat(";")? {
                stmts.push(Stmt::Expr(e));
            } else if self.is("}")? {
                self.next()?;
                tail = Some(Box::new(e));
                break;
            } else if matches!(e.kind, EK::If(..) | EK::Match(..) | EK::Block(..) | EK::While(..) | EK::For(..)) {
                stmts.push(Stmt::Expr(e));
            } else {
                let t = self.peek()?.clone();
                return Err((t.pos, format!("expected `;` or `}}`, found {}", show(&t.t))));
            }
        }
        Ok(self.mk(pos, EK::Block(stmts, tail)))
    }

    fn pat(&mut self) -> PResult<Pat> {
        let t = self.next()?;
        Ok(match t.t {
            T::P("(") => {
                let mut ps = vec![];
                while !self.eat(")")? {
                    ps.push(self.pat()?);
                    self.eat(",")?;
                }
                if ps.is_empty() { Pat::Lit(Lit::Unit) } else { Pat::Tuple(ps) }
            }
            T::P("-") => match self.next()?.t {
                T::Int(n) => Pat::Lit(Lit::Int(-n)),
                T::Float(f) => Pat::Lit(Lit::Float(format!("-{f}"))),
                _ => return Err((t.pos, "expected a number after `-`".into())),
            },
            T::Int(n) => Pat::Lit(Lit::Int(n)),
            T::Float(f) => Pat::Lit(Lit::Float(f)),
            T::Str(s) => Pat::Lit(Lit::Str(s)),
            T::Ident(n) if n == "_" => Pat::Wild,
            T::Ident(n) if n == "true" || n == "false" => Pat::Lit(Lit::Bool(n == "true")),
            T::Ident(n) => {
                let (en, vn) = if self.eat("::")? { (Some(n), self.ident()?.0) } else { (None, n) };
                let upper = vn.chars().next().is_some_and(|c| c.is_uppercase());
                if !upper && en.is_none() {
                    return Ok(Pat::Bind(vn));
                }
                let mut subs = vec![];
                if self.eat("(")? {
                    while !self.eat(")")? {
                        subs.push(self.pat()?);
                        self.eat(",")?;
                    }
                }
                Pat::Variant(en, vn, subs, t.pos)
            }
            o => return Err((t.pos, format!("expected a pattern, found {}", show(&o)))),
        })
    }

    /// `no_rec`: a `Name {` does not start a record literal (the condition of `if`, the scrutinee of `match`)
    pub fn expr(&mut self, no_rec: bool) -> PResult<Expr> {
        self.binary(0, no_rec)
    }

    fn binary(&mut self, min: u8, no_rec: bool) -> PResult<Expr> {
        let mut l = self.unary(no_rec)?;
        loop {
            let t = self.peek()?.clone();
            if matches!(&t.t, T::Ident(k) if k == "as") {
                if min >= 7 {
                    break;
                }
                self.next()?;
                let ty = self.ty()?;
                l = self.mk(t.pos, EK::Cast(Box::new(l), ty));
                continue;
            }
            let T::P(op) = t.t else { break };
            let prec = match op {
                "|>" => 1,
                "||" => 2,
                "&&" => 3,
                "==" | "!=" | "<" | "<=" | ">" | ">=" => 4,
                "+" | "-" => 5,
                "*" | "/" | "%" => 6,
                _ => break,
            };
            if prec <= min {
                break;
            }
            self.next()?;
            if op == "|>" {
                l = self.pipe(l, no_rec)?;
                continue;
            }
            let r = self.binary(prec, no_rec)?;
            l = self.mk(t.pos, EK::Bin(op, Box::new(l), Box::new(r)));
        }
        Ok(l)
    }

    /// `x |> f(a)` is `f(x, a)`, `x |> f` is `f(x)`, `x |> .m(a)` is `x.m(a)` (the left side becomes the first argument)
    fn pipe(&mut self, l: Expr, no_rec: bool) -> PResult<Expr> {
        if self.is(".")? {
            self.next()?;
            let (name, pos) = self.ident()?;
            self.expect("(")?;
            let args = self.args(")")?;
            return Ok(self.mk(pos, EK::Method(Box::new(l), name, args)));
        }
        let r = self.postfix(no_rec)?;
        Ok(match r.kind {
            EK::Call(f, mut args) => {
                args.insert(0, l);
                self.mk(r.pos, EK::Call(f, args))
            }
            EK::Method(recv, m, mut args) => {
                args.insert(0, l);
                self.mk(r.pos, EK::Method(recv, m, args))
            }
            _ => {
                let pos = r.pos;
                self.mk(pos, EK::Call(Box::new(r), vec![l]))
            }
        })
    }

    fn unary(&mut self, no_rec: bool) -> PResult<Expr> {
        let t = self.peek()?.clone();
        // `await e` binds as the prefix operators do: `await f() + 1` is `(await f()) + 1`
        if matches!(&t.t, T::Ident(k) if k == "await") {
            self.next()?;
            let e = self.unary(no_rec)?;
            return Ok(self.mk(t.pos, EK::Await(Box::new(e))));
        }
        if let T::P(op @ ("-" | "!")) = t.t {
            self.next()?;
            let e = self.unary(no_rec)?;
            return Ok(self.mk(t.pos, EK::Un(op, Box::new(e))));
        }
        self.postfix(no_rec)
    }

    fn postfix(&mut self, no_rec: bool) -> PResult<Expr> {
        let mut e = self.primary(no_rec)?;
        loop {
            if self.is("(")? {
                let pos = self.next()?.pos;
                let args = self.args(")")?;
                e = self.mk(pos, EK::Call(Box::new(e), args));
            } else if self.is("[")? {
                let pos = self.next()?.pos;
                let i = self.expr(false)?;
                self.expect("]")?;
                e = self.mk(pos, EK::Index(Box::new(e), Box::new(i)));
            } else if self.is(".")? {
                self.next()?;
                let t = self.next()?;
                let name = match t.t {
                    T::Ident(n) => n,
                    T::Int(n) => n.to_string(),
                    o => return Err((t.pos, format!("expected a field or method name, found {}", show(&o)))),
                };
                if self.is("(")? {
                    self.next()?;
                    let args = self.args(")")?;
                    e = self.mk(t.pos, EK::Method(Box::new(e), name, args));
                } else {
                    e = self.mk(t.pos, EK::Field(Box::new(e), name));
                }
            } else {
                return Ok(e);
            }
        }
    }

    fn args(&mut self, close: &str) -> PResult<Vec<Expr>> {
        let mut v = vec![];
        while !self.eat(close)? {
            v.push(self.expr(false)?);
            if !self.eat(",")? {
                self.expect(close)?;
                break;
            }
        }
        Ok(v)
    }

    fn primary(&mut self, no_rec: bool) -> PResult<Expr> {
        let t = self.next()?;
        let pos = t.pos;
        Ok(match t.t {
            T::Int(n) => self.mk(pos, EK::Lit(Lit::Int(n))),
            T::Float(f) => self.mk(pos, EK::Lit(Lit::Float(f))),
            T::Str(s) => self.mk(pos, EK::Lit(Lit::Str(s))),
            T::P("(") => {
                if let Some(ps) = self.try_arrow_params()? {
                    let body = self.expr(no_rec)?;
                    return Ok(self.mk(pos, EK::Closure(ps, Box::new(body))));
                }
                if self.eat(")")? {
                    return Ok(self.mk(pos, EK::Lit(Lit::Unit)));
                }
                let e = self.expr(false)?;
                if self.eat(")")? {
                    return Ok(e);
                }
                let mut es = vec![e];
                while self.eat(",")? {
                    if self.is(")")? {
                        break;
                    }
                    es.push(self.expr(false)?);
                }
                self.expect(")")?;
                self.mk(pos, EK::Tuple(es))
            }
            T::P("[") => {
                let es = self.args("]")?;
                self.mk(pos, EK::Array(es))
            }
            T::P("`") => {
                let (parts, holes) = self.template(pos)?;
                self.mk(pos, EK::Interp(parts, holes))
            }
            T::P("{") => {
                self.peeked = Some(Tok { t: T::P("{"), pos, start: t.start });
                self.block()?
            }
            T::P("|") | T::P("||") => return Err((pos, "a closure is written `(x) => e`".into())),
            T::P("<") => {
                self.peeked = Some(Tok { t: T::P("<"), pos, start: t.start });
                let j = self.jsx()?;
                self.mk(pos, EK::Jsx(Box::new(j)))
            }
            T::Ident(k) if k == "true" || k == "false" => self.mk(pos, EK::Lit(Lit::Bool(k == "true"))),
            // `async (x) => e`
            T::Ident(k) if k == "async" && self.is("(")? => {
                let c = self.primary(no_rec)?;
                if !matches!(c.kind, EK::Closure(..)) {
                    return Err((pos, "`async` goes on a closure: `async (x) => ..`".into()));
                }
                self.mk(pos, EK::Async(Box::new(c)))
            }
            T::Ident(k) if k == "if" => {
                let c = self.expr(true)?;
                let th = self.block()?;
                let el = if self.is_kw("else")? {
                    self.next()?;
                    if self.is_kw("if")? { Some(Box::new(self.primary(no_rec)?)) } else { Some(Box::new(self.block()?)) }
                } else {
                    None
                };
                self.mk(pos, EK::If(Box::new(c), Box::new(th), el))
            }
            T::Ident(k) if k == "while" => {
                let c = self.expr(true)?;
                let body = self.block()?;
                self.mk(pos, EK::While(Box::new(c), Box::new(body)))
            }
            T::Ident(k) if k == "for" => {
                let (x, _) = self.ident()?;
                let (kw, kpos) = self.ident()?;
                if kw != "in" {
                    return Err((kpos, "expected `in`".into()));
                }
                let it = self.expr(true)?;
                let body = self.block()?;
                self.mk(pos, EK::For(x, Box::new(it), Box::new(body)))
            }
            T::Ident(k) if k == "break" => self.mk(pos, EK::Break),
            T::Ident(k) if k == "continue" => self.mk(pos, EK::Continue),
            T::Ident(k) if k == "perform" => {
                let (n, npos) = self.ident()?;
                let (en, v) = if self.eat("::")? { (Some(n), self.ident()?.0) } else { (None, n) };
                let args = if self.eat("(")? { self.args(")")? } else { vec![] };
                self.mk(npos, EK::Perform(en, v, args))
            }
            T::Ident(k) if k == "handle" => {
                let body = self.expr(true)?;
                self.expect("{")?;
                let mut arms = vec![];
                while !self.eat("}")? {
                    let (v, apos) = self.ident()?;
                    let mut pats = vec![];
                    if self.eat("(")? {
                        while !self.eat(")")? {
                            pats.push(self.pat()?);
                            self.eat(",")?;
                        }
                    }
                    self.expect(",")?;
                    let (k, _) = self.ident()?;
                    self.expect("=>")?;
                    let b = self.expr(false)?;
                    arms.push(HArm { variant: v, pats, k, body: b, pos: apos });
                    if !self.eat(",")? {
                        self.expect("}")?;
                        break;
                    }
                }
                self.mk(pos, EK::Handle(Box::new(body), arms))
            }
            T::Ident(k) if k == "match" => {
                let s = self.expr(true)?;
                self.expect("{")?;
                let mut arms = vec![];
                while !self.eat("}")? {
                    let p = self.pat()?;
                    self.expect("=>")?;
                    let body = self.expr(false)?;
                    arms.push((p, body));
                    if !self.eat(",")? {
                        self.expect("}")?;
                        break;
                    }
                }
                self.mk(pos, EK::Match(Box::new(s), arms))
            }
            T::Ident(n) => {
                if self.eat("::")? {
                    let (v, _) = self.ident()?;
                    return Ok(self.mk(pos, EK::Path(n, v)));
                }
                let upper = n.chars().next().is_some_and(|c| c.is_uppercase());
                if upper && !no_rec && self.is("{")? {
                    self.next()?;
                    let mut fields = vec![];
                    let mut base = None;
                    while !self.eat("}")? {
                        if self.eat("..")? {
                            base = Some(Box::new(self.expr(false)?));
                            self.eat(",")?;
                            continue;
                        }
                        let (f, fpos) = self.ident()?;
                        let v = if self.eat(":")? {
                            self.expr(false)?
                        } else {
                            self.mk(fpos, EK::Var(f.clone()))
                        };
                        fields.push((f, v));
                        self.eat(",")?;
                    }
                    return Ok(self.mk(pos, EK::Record(n, fields, base)));
                }
                self.mk(pos, EK::Var(n))
            }
            o => return Err((pos, format!("expected an expression, found {}", show(&o)))),
        })
    }

    /// After `` ` ``: the text of a template up to its closing `` ` ``, and the expressions of its `${..}` holes
    fn template(&mut self, pos: Pos) -> PResult<(Vec<String>, Vec<Expr>)> {
        let mut parts = vec![String::new()];
        let mut holes = vec![];
        loop {
            if self.i >= self.src.len() {
                return Err((pos, "unterminated template string".into()));
            }
            match self.bump() {
                '`' => break,
                '$' if self.at(0) == '{' => {
                    self.bump();
                    let hpos = self.here();
                    if self.is("}")? {
                        return Err((hpos, "an empty `${}` in a template string".into()));
                    }
                    holes.push(self.expr(false)?);
                    // (the `}` is read as a token: the text goes on right after it)
                    self.expect("}")?;
                    parts.push(String::new());
                }
                '\\' => {
                    if self.i >= self.src.len() {
                        return Err((pos, "unterminated template string".into()));
                    }
                    let e = self.bump();
                    parts.last_mut().unwrap().push(match e {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        o => o,
                    })
                }
                o => parts.last_mut().unwrap().push(o),
            }
        }
        Ok((parts, holes))
    }

    /// After `(`: the parameters of an arrow function `(a, b: i32) => ..`, up to and including `=>`; otherwise
    /// None, with the parser back where it was
    fn try_arrow_params(&mut self) -> PResult<Option<Vec<(String, Option<TyS>)>>> {
        let save = (self.i, self.line, self.col, self.peeked.clone(), self.next_id);
        let r = (|| -> PResult<Option<Vec<(String, Option<TyS>)>>> {
            let mut ps = vec![];
            while !self.eat(")")? {
                let T::Ident(n) = self.next()?.t else { return Ok(None) };
                let t = if self.eat(":")? { Some(self.ty()?) } else { None };
                ps.push((n, t));
                if !self.eat(",")? && !self.is(")")? {
                    return Ok(None);
                }
            }
            Ok(if self.eat("=>")? { Some(ps) } else { None })
        })();
        match r {
            Ok(Some(ps)) => Ok(Some(ps)),
            _ => {
                (self.i, self.line, self.col, self.peeked, self.next_id) = save;
                Ok(None)
            }
        }
    }

    // ------------------------------------------------------------ JSX

    fn jsx(&mut self) -> PResult<Jsx> {
        let pos = self.expect("<")?;
        if self.eat(">")? {
            let children = self.jsx_children("")?;
            return Ok(Jsx { tag: String::new(), attrs: vec![], children, pos });
        }
        let (mut tag, _) = self.ident()?;
        while self.eat(".")? {
            tag = format!("{tag}.{}", self.ident()?.0);
        }
        let mut attrs = vec![];
        loop {
            if self.eat("/")? {
                self.expect(">")?;
                return Ok(Jsx { tag, attrs, children: vec![], pos });
            }
            if self.eat(">")? {
                break;
            }
            let (mut a, apos) = self.ident()?;
            // (data-x / aria-x)
            while self.is("-")? {
                self.next()?;
                a = format!("{a}-{}", self.ident()?.0);
            }
            let v = if self.eat("=")? {
                let t = self.peek()?.clone();
                match t.t {
                    T::Str(s) => {
                        self.next()?;
                        self.mk(t.pos, EK::Lit(Lit::Str(s)))
                    }
                    T::P("{") => {
                        self.next()?;
                        let e = self.expr(false)?;
                        self.expect("}")?;
                        e
                    }
                    o => return Err((t.pos, format!("expected a string or `{{..}}` as the attribute's value, found {}", show(&o)))),
                }
            } else {
                self.mk(apos, EK::Lit(Lit::Bool(true)))
            };
            attrs.push((a, v));
        }
        let children = self.jsx_children(&tag)?;
        Ok(Jsx { tag, attrs, children, pos })
    }

    fn jsx_children(&mut self, tag: &str) -> PResult<Vec<JsxChild>> {
        let mut out = vec![];
        loop {
            self.unpeek();
            let mut text = String::new();
            while self.i < self.src.len() && self.at(0) != '<' && self.at(0) != '{' {
                text.push(self.bump());
            }
            // (JSX's whitespace rule, simplified: lines are trimmed and joined by a space, blank text dropped)
            let lines: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
            if !lines.is_empty() {
                let mut s = lines.join(" ");
                if text.starts_with([' ', '\t']) && !text.trim_start_matches([' ', '\t']).starts_with('\n') {
                    s.insert(0, ' ');
                }
                if text.ends_with([' ', '\t']) && !text.trim_end_matches([' ', '\t']).ends_with('\n') {
                    s.push(' ');
                }
                out.push(JsxChild::Text(s));
            }
            if self.i >= self.src.len() {
                return Err((self.here(), format!("unclosed <{tag}>")));
            }
            if self.at(0) == '{' {
                self.next()?;
                let e = self.expr(false)?;
                self.expect("}")?;
                out.push(JsxChild::Expr(e));
                continue;
            }
            // `<`: a closing tag or a child element
            let save = (self.i, self.line, self.col);
            self.next()?;
            if self.eat("/")? {
                let mut close = String::new();
                if !self.is(">")? {
                    close = self.ident()?.0;
                    while self.eat(".")? {
                        close = format!("{close}.{}", self.ident()?.0);
                    }
                }
                let p = self.expect(">")?;
                if close != tag {
                    return Err((p, format!("expected </{tag}>, found </{close}>")));
                }
                return Ok(out);
            }
            self.peeked = None;
            (self.i, self.line, self.col) = save;
            let pos = self.here();
            let j = self.jsx()?;
            let e = self.mk(pos, EK::Jsx(Box::new(j)));
            out.push(JsxChild::Expr(e));
        }
    }
}

pub fn show(t: &T) -> String {
    match t {
        T::Ident(s) => format!("`{s}`"),
        T::Int(n) => format!("`{n}`"),
        T::Float(f) => format!("`{f}`"),
        T::Str(s) => format!("{s:?}"),
        T::P(p) => format!("`{p}`"),
        T::Eof => "end of file".into(),
    }
}

fn scalar_wty(s: &str) -> Option<WTy> {
    Some(match s {
        "i32" => WTy::I32,
        "u32" => WTy::U32,
        "f32" => WTy::F32,
        "f64" => WTy::F64,
        "bool" => WTy::Bool,
        "()" => WTy::Unit,
        _ => return None,
    })
}

/// `&dyn Fn(f64, i32) -> f64` / `Box<dyn Fn(i32)>` (the tokens joined without spaces)
fn callback_ty(s: &str) -> Option<WTy> {
    let (owned, rest) = if let Some(r) = s.strip_prefix("&dynFn(") {
        (false, r)
    } else {
        (true, s.strip_prefix("Box<dynFn(")?.strip_suffix('>')?)
    };
    let (args, ret) = rest.split_once(')')?;
    let args = if args.is_empty() { vec![] } else { args.split(',').map(scalar_wty).collect::<Option<Vec<_>>>()? };
    let ret = match ret.strip_prefix("->") {
        Some(r) => scalar_wty(r)?,
        None if ret.is_empty() => WTy::Unit,
        None => return None,
    };
    Some(WTy::Callback(args, Box::new(ret), owned))
}
