//! `import type { User, Post } from "./model.ts";`: record types read from a TS file.
//!
//! zsc reads the subset of TS a zustscript record can be: an `interface` (or a `type X = { .. }`) without type
//! parameters, `extends` or optional fields, whose fields are `string`, `number` (`f64`), `boolean`, `bigint`
//! (`i64`), arrays of those (`T[]`, `readonly T[]`, `Array<T>`, `ReadonlyArray<T>`), and other such types of the
//! same file. A type the import needs is read with it (and can be named in the zsc file too). Everything else in
//! the file is skipped; a type the import needs that is not in the subset is an error at its place in the TS file.
//!
//! The JS output needs nothing (a record is a plain object); the `.d.ts` imports the TS type instead of declaring
//! its own, so TS sees one type.

use crate::ast::{Item, Pos, TsEnum, TsFrom, TyS};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// An error in a TS file (or, with `file: None`, at an import of the zsc file)
pub struct TsError {
    pub file: Option<(String, String)>,
    pub pos: Pos,
    pub msg: String,
}

#[derive(Clone, Debug, PartialEq)]
enum T {
    Ident(String),
    Str(String),
    P(char),
    /// `=>`, `...`, `?.`, numbers and other text the subset never uses
    Other(String),
    Eof,
}

#[derive(Clone, Debug)]
struct Tok {
    t: T,
    pos: Pos,
    /// the token's text: `src[start..end]` (in chars)
    start: usize,
    end: usize,
    /// a line ends between this token and the one before (a field's separator, as in TS)
    nl: bool,
}

/// A line terminator, as TS counts them (LF, CR, U+2028, U+2029)
fn line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Whether a `/` after token `prev` starts a regular expression (not a division): after an operator, an opening
/// bracket, a keyword, or the `)` that closes the condition of `if` / `while` / `for` / `with` (`ctrl`), as JS
/// decides
fn regex_after(prev: Option<&T>, ctrl: bool) -> bool {
    match prev {
        None => true,
        Some(T::P(')')) => ctrl,
        Some(T::P(c)) => !matches!(c, ')' | ']'),
        Some(T::Ident(k)) => matches!(
            k.as_str(),
            "return" | "typeof" | "instanceof" | "in" | "of" | "new" | "delete" | "void" | "throw" | "case" | "do" | "else" | "yield" | "await"
        ),
        Some(_) => false,
    }
}

fn lex(src: &str) -> Vec<Tok> {
    let s: Vec<char> = src.chars().collect();
    let (mut i, mut line, mut col) = (0usize, 1u32, 0u32);
    let mut out: Vec<Tok> = vec![];
    let mut nl = false;
    // (each open `(`: whether it starts the condition of `if` / `while` / `for` / `with`; and whether the last
    // token is the `)` that closed one)
    let mut parens: Vec<bool> = vec![];
    let mut ctrl = false;
    let adv = |i: &mut usize, line: &mut u32, col: &mut u32, s: &[char]| {
        // (CR LF is one line end)
        if line_end(s[*i]) && !(s[*i] == '\r' && s.get(*i + 1) == Some(&'\n')) {
            *line += 1;
            *col = 0;
        } else {
            *col += 1;
        }
        *i += 1;
    };
    while i < s.len() {
        let c = s[i];
        let pos = Pos { line, col };
        let start = i;
        let before = out.len();
        if c.is_whitespace() {
            nl |= line_end(c);
            adv(&mut i, &mut line, &mut col, &s);
        } else if c == '/' && s.get(i + 1) == Some(&'/') {
            while i < s.len() && !line_end(s[i]) {
                adv(&mut i, &mut line, &mut col, &s);
            }
        } else if c == '/' && s.get(i + 1) == Some(&'*') {
            while i < s.len() && !(s[i] == '*' && s.get(i + 1) == Some(&'/')) {
                nl |= line_end(s[i]);
                adv(&mut i, &mut line, &mut col, &s);
            }
            for _ in 0..2 {
                if i < s.len() {
                    adv(&mut i, &mut line, &mut col, &s);
                }
            }
        } else if c == '"' || c == '\'' || c == '`' {
            // (a template literal's `${..}` is skipped with it: the subset has none at the top level)
            adv(&mut i, &mut line, &mut col, &s);
            let mut v = String::new();
            while i < s.len() && s[i] != c {
                if s[i] == '\\' && i + 1 < s.len() {
                    // (an escape: its character, so `"\u0064one"` is the tag `done`)
                    adv(&mut i, &mut line, &mut col, &s);
                    let e = s[i];
                    adv(&mut i, &mut line, &mut col, &s);
                    let hex = |i: &mut usize, line: &mut u32, col: &mut u32, n: Option<usize>| -> Option<u32> {
                        let mut h = String::new();
                        if n.is_none() {
                            // `\u{..}`
                            adv(i, line, col, &s);
                            while *i < s.len() && s[*i] != '}' {
                                h.push(s[*i]);
                                adv(i, line, col, &s);
                            }
                            if *i < s.len() {
                                adv(i, line, col, &s);
                            }
                        } else {
                            for _ in 0..n.unwrap() {
                                if *i < s.len() {
                                    h.push(s[*i]);
                                    adv(i, line, col, &s);
                                }
                            }
                        }
                        u32::from_str_radix(&h, 16).ok()
                    };
                    let ch = match e {
                        'n' => Some('\n'),
                        't' => Some('\t'),
                        'r' => Some('\r'),
                        'b' => Some('\u{8}'),
                        'f' => Some('\u{c}'),
                        'v' => Some('\u{b}'),
                        '0' => Some('\0'),
                        'x' => hex(&mut i, &mut line, &mut col, Some(2)).and_then(char::from_u32),
                        'u' => {
                            let u = if i < s.len() && s[i] == '{' { hex(&mut i, &mut line, &mut col, None) } else { hex(&mut i, &mut line, &mut col, Some(4)) };
                            // (UTF-16: a high surrogate and the low one after it, `\uD835\uDC9C`, are one character; a
                            // lone surrogate is no character: U+FFFD, not dropped)
                            match u {
                                Some(hi @ 0xD800..=0xDBFF) if s.get(i) == Some(&'\\') && s.get(i + 1) == Some(&'u') && s.get(i + 2) != Some(&'{') => {
                                    let save = (i, line, col);
                                    adv(&mut i, &mut line, &mut col, &s);
                                    adv(&mut i, &mut line, &mut col, &s);
                                    match hex(&mut i, &mut line, &mut col, Some(4)) {
                                        Some(lo @ 0xDC00..=0xDFFF) => char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)),
                                        _ => {
                                            (i, line, col) = save;
                                            Some('\u{FFFD}')
                                        }
                                    }
                                }
                                Some(c) => Some(char::from_u32(c).unwrap_or('\u{FFFD}')),
                                None => None,
                            }
                        }
                        // (a line continuation: CR LF is one line end)
                        '\r' if i < s.len() && s[i] == '\n' => {
                            adv(&mut i, &mut line, &mut col, &s);
                            None
                        }
                        c if line_end(c) => None,
                        c => Some(c),
                    };
                    v.extend(ch);
                    continue;
                }
                v.push(s[i]);
                adv(&mut i, &mut line, &mut col, &s);
            }
            if i < s.len() {
                adv(&mut i, &mut line, &mut col, &s);
            }
            out.push(Tok { t: T::Str(v), pos, start, end: i, nl });
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let mut v = String::new();
            while i < s.len() && (s[i].is_alphanumeric() || s[i] == '_' || s[i] == '$') {
                v.push(s[i]);
                adv(&mut i, &mut line, &mut col, &s);
            }
            out.push(Tok { t: T::Ident(v), pos, start, end: i, nl });
        } else if c.is_ascii_digit() {
            let mut v = String::new();
            while i < s.len() && (s[i].is_alphanumeric() || s[i] == '.' || s[i] == '_') {
                v.push(s[i]);
                adv(&mut i, &mut line, &mut col, &s);
            }
            out.push(Tok { t: T::Other(v), pos, start, end: i, nl });
        } else if c == '/' && regex_after(out.last().map(|t| &t.t), ctrl) {
            // (a regular expression: opaque, so its brackets are not structure; `/` inside `[..]` does not end it)
            adv(&mut i, &mut line, &mut col, &s);
            let mut class = false;
            while i < s.len() && !line_end(s[i]) && (class || s[i] != '/') {
                match s[i] {
                    '\\' if i + 1 < s.len() => adv(&mut i, &mut line, &mut col, &s),
                    '[' => class = true,
                    ']' => class = false,
                    _ => {}
                }
                adv(&mut i, &mut line, &mut col, &s);
            }
            if i < s.len() && s[i] == '/' {
                adv(&mut i, &mut line, &mut col, &s);
            }
            while i < s.len() && s[i].is_alphabetic() {
                adv(&mut i, &mut line, &mut col, &s);
            }
            out.push(Tok { t: T::Other(s[start..i].iter().collect()), pos, start, end: i, nl });
        } else {
            let closes_ctrl = match c {
                '(' => {
                    // (the keyword, not a property named like it: `obj.if(4) / 2` divides)
                    let n = out.len();
                    // (`for await (..)`: the keyword is the one before `await`)
                    let is = |j: usize, ks: &[&str]| matches!(&out[j].t, T::Ident(k) if ks.contains(&k.as_str())) && !(j >= 1 && out[j - 1].t == T::P('.'));
                    let kw = n >= 1 && (is(n - 1, &["if", "while", "for", "with"]) || (is(n - 1, &["await"]) && n >= 2 && is(n - 2, &["for"])));
                    parens.push(kw);
                    false
                }
                ')' => parens.pop().unwrap_or(false),
                _ => false,
            };
            adv(&mut i, &mut line, &mut col, &s);
            out.push(Tok { t: T::P(c), pos, start, end: i, nl });
            if out.len() > before {
                nl = false;
            }
            ctrl = closes_ctrl;
            continue;
        }
        if out.len() > before {
            nl = false;
            ctrl = false;
        }
    }
    out.push(Tok { t: T::Eof, pos: Pos { line, col }, start: i, end: i, nl });
    out
}

/// A declaration of the TS file: where its body starts (`{` of an interface, the token after `=` of a type alias)
struct Decl {
    pos: Pos,
    /// `interface X<..>` / `type X<..>`: the type parameters' `<`
    generic: bool,
    extends: bool,
    interface: bool,
    exported: bool,
    body: usize,
    /// another declaration of the same name (TS merges interfaces: not read yet)
    again: Option<Pos>,
}

const PLAIN: &str = "zsc reads plain records and unions from TS for now";
const TYPES: &str = "string, number, boolean, bigint, arrays, and the file's other records and unions";

#[derive(Clone)]
enum Fty {
    Ty(TyS),
    /// a string literal type (a tag)
    Lit(String, Pos),
}

struct Member {
    name: String,
    ty: Fty,
    /// as the file writes it: `readonly tags: string[]`
    text: String,
}

/// A declaration read: a record (its fields, its members' text), or an enum (its variants, how its values look in
/// JS, the union's text)
enum Read {
    Record(Vec<(String, TyS)>, Vec<String>),
    Enum(Vec<(String, Vec<TyS>)>, TsEnum, String),
}

struct TsFile {
    path: String,
    src: String,
    chars: Vec<char>,
    toks: Vec<Tok>,
    decls: HashMap<String, Decl>,
}

impl TsFile {
    fn read(path: &Path) -> Result<TsFile, String> {
        let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let toks = lex(&src);
        // (the top-level declarations: `interface X` and `type X =` outside any braces, brackets or parentheses)
        let mut decls = HashMap::new();
        let mut depth = 0i32;
        let mut k = 0;
        while k < toks.len() {
            match &toks[k].t {
                T::P('{' | '(' | '[') => depth += 1,
                T::P('}' | ')' | ']') => depth -= 1,
                T::Ident(kw) if depth == 0 && (kw == "interface" || kw == "type") => {
                    if let Some(Tok { t: T::Ident(name), pos, .. }) = toks.get(k + 1) {
                        let interface = kw == "interface";
                        // (`export interface`, `export declare interface`)
                        let before = |n: usize, w: &str| k >= n && matches!(&toks[k - n].t, T::Ident(x) if x == w);
                        let exported = before(1, "export") || (before(1, "declare") && before(2, "export"));
                        let mut j = k + 2;
                        let generic = toks[j].t == T::P('<');
                        // (to the body: past `<..>` and `extends ..`)
                        let mut extends = false;
                        let mut angle = 0;
                        while j < toks.len() {
                            match &toks[j].t {
                                T::P('<') => angle += 1,
                                T::P('>') if angle > 0 => angle -= 1,
                                T::Ident(e) if angle == 0 && e == "extends" => extends = true,
                                T::P('{') if angle == 0 && interface => break,
                                T::P('=') if angle == 0 && !interface => {
                                    j += 1;
                                    break;
                                }
                                T::P(';') | T::Eof if angle == 0 => break,
                                _ => {}
                            }
                            j += 1;
                        }
                        // (a `type` that is not `type X = ..`, e.g. `import type`, is not a declaration)
                        if interface || toks.get(j.wrapping_sub(1)).is_some_and(|t| t.t == T::P('=')) {
                            match decls.get_mut(name) {
                                Some(d) => {
                                    let d: &mut Decl = d;
                                    d.again.get_or_insert(*pos);
                                }
                                None => {
                                    decls.insert(name.clone(), Decl { pos: *pos, generic, extends, interface, exported, body: j, again: None });
                                }
                            }
                        }
                        k = j;
                        continue;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        // (`export { A, B }` / `export type { A }` at the top level: those declarations are exported too)
        let mut depth = 0i32;
        let mut k = 0;
        while k < toks.len() {
            match &toks[k].t {
                T::P('{' | '(' | '[') => depth += 1,
                T::P('}' | ')' | ']') => depth -= 1,
                T::Ident(e) if depth == 0 && e == "export" => {
                    let mut j = k + 1;
                    if matches!(&toks[j].t, T::Ident(t) if t == "type") {
                        j += 1;
                    }
                    if toks[j].t == T::P('{') {
                        // (`export { A } from "./other"` re-exports another module's `A`: not the local one)
                        let close = (j..toks.len()).find(|&x| toks[x].t == T::P('}') || toks[x].t == T::Eof).unwrap_or(j);
                        if matches!(toks.get(close + 1).map(|t| &t.t), Some(T::Ident(f)) if f == "from") {
                            k = close + 1;
                            continue;
                        }
                        j += 1;
                        while j < toks.len() && toks[j].t != T::P('}') {
                            // (a plain name; `A as B` exports another name, not read)
                            if let T::Ident(n) = &toks[j].t {
                                let renamed = matches!(&toks[j + 1].t, T::Ident(a) if a == "as");
                                if !renamed && n != "type" {
                                    if let Some(d) = decls.get_mut(n) {
                                        let d: &mut Decl = d;
                                        d.exported = true;
                                    }
                                }
                                if renamed {
                                    j += 2;
                                }
                            }
                            j += 1;
                        }
                        k = j + 1;
                        continue;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        let chars = src.chars().collect();
        Ok(TsFile { path: path.display().to_string(), src, chars, toks, decls })
    }

    fn err(&self, pos: Pos, msg: impl Into<String>) -> TsError {
        TsError { file: Some((self.path.clone(), self.src.clone())), pos, msg: msg.into() }
    }

    /// The checks every declaration passes: one declaration, no type parameters, no `extends`
    fn plain_decl(&self, name: &str) -> Result<&Decl, TsError> {
        let d = &self.decls[name];
        if let Some(p) = d.again {
            return Err(self.err(p, format!("`{name}` is declared more than once: {PLAIN} (TS merges the declarations; write them as one)")));
        }
        if d.generic {
            return Err(self.err(d.pos, format!("`{name}` has type parameters: {PLAIN} (no type parameters, `extends`, optional fields)")));
        }
        if d.extends {
            return Err(self.err(d.pos, format!("`{name}` extends another type: {PLAIN} (write its fields out)")));
        }
        Ok(d)
    }

    /// Declaration `name` as a record or an enum, and the names of the file's types it uses
    fn decl(&self, name: &str) -> Result<(Read, Vec<(String, Pos)>), TsError> {
        let d = self.plain_decl(name)?;
        let mut uses = vec![];
        // (an interface, or `type X = { .. }` that is only the object: a record)
        let k = d.body;
        let object = d.interface || (self.toks[k].t == T::P('{') && !matches!(self.toks[self.close(k)? + 1].t, T::P('|')));
        if object {
            let (members, close) = self.object(if d.interface { k } else { k }, name, &mut uses)?;
            // (`type X = { .. } & ..`: the whole alias must be the object)
            // (or a conditional type: `type X = { .. } extends Y ? .. : ..`)
            let goes_on = matches!(self.toks[close + 1].t, T::P('&' | '[' | '.' | '<')) || matches!(&self.toks[close + 1].t, T::Ident(e) if e == "extends");
            if !d.interface && goes_on {
                return Err(self.err(self.toks[close + 1].pos, format!("`{name}` is not only an object type (an intersection, an array or a conditional type of one): {PLAIN}")));
            }
            let mut fields = vec![];
            let mut texts = vec![];
            for m in members {
                let Fty::Ty(t) = m.ty else {
                    let Fty::Lit(_, p) = m.ty else { unreachable!() };
                    return Err(self.err(p, format!("`{name}.{}` is a string literal type: {PLAIN} (a union of them is an enum)", m.name)));
                };
                texts.push(m.text);
                fields.push((m.name, t));
            }
            return Ok((Read::Record(fields, texts), uses));
        }
        self.union(name, k, &mut uses).map(|r| (r, uses))
    }

    /// The index of the `}` closing the `{` at `k`
    fn close(&self, k: usize) -> Result<usize, TsError> {
        let mut depth = 0;
        let mut j = k;
        while j < self.toks.len() - 1 {
            match self.toks[j].t {
                T::P('{') => depth += 1,
                T::P('}') => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(j);
                    }
                }
                _ => {}
            }
            j += 1;
        }
        Err(self.err(self.toks[k].pos, "this `{` is not closed before the end of the file"))
    }

    /// The members of the object type whose `{` is at `k`, and the index of its `}`
    fn object(&self, mut k: usize, owner: &str, uses: &mut Vec<(String, Pos)>) -> Result<(Vec<Member>, usize), TsError> {
        k += 1;
        let mut members = vec![];
        loop {
            let t = &self.toks[k];
            match &t.t {
                T::P('}') => break,
                T::P(';' | ',') => {
                    k += 1;
                    continue;
                }
                _ => {}
            }
            // `readonly name: T`
            let ro = matches!(&t.t, T::Ident(r) if r == "readonly") && matches!(self.toks[k + 1].t, T::Ident(_));
            if ro {
                k += 1;
            }
            let ft = &self.toks[k];
            let T::Ident(f) = &ft.t else {
                return Err(self.err(ft.pos, format!("a member of `{owner}` that is not `field: type` (methods, index signatures and quoted names are not read): {PLAIN}")));
            };
            if members.iter().any(|m: &Member| m.name == *f) {
                return Err(self.err(ft.pos, format!("`{owner}.{f}` is declared twice")));
            }
            if f == "__proto__" {
                return Err(self.err(ft.pos, format!("`{owner}.__proto__`: a member named `__proto__` is the prototype in JS, not a field: zsc does not read it")));
            }
            k += 1;
            match &self.toks[k].t {
                T::P(':') => {}
                T::P('?') => return Err(self.err(self.toks[k].pos, format!("`{owner}.{f}` is optional: {PLAIN} (an optional field needs `Option`)"))),
                _ => return Err(self.err(self.toks[k].pos, format!("`{owner}.{f}` is not `field: type` (a method is not read): {PLAIN}"))),
            }
            k += 1;
            let from = self.toks[k].start;
            // (a string literal type: the tag of a union's member)
            let (ty, next) = match &self.toks[k].t {
                T::Str(lit) if matches!(self.toks[k + 1].t, T::P(';' | ',' | '}')) || self.toks[k + 1].nl => (Fty::Lit(lit.clone(), self.toks[k].pos), k + 1),
                _ => {
                    let (t, next) = self.ty(k, uses, owner, f)?;
                    (Fty::Ty(t), next)
                }
            };
            let text: String = self.chars[from..self.toks[next - 1].end].iter().collect();
            k = next;
            // (`;`, `,`, or the end of the line, as TS separates members)
            match &self.toks[k].t {
                T::P(';' | ',' | '}') => {}
                _ if self.toks[k].nl => {}
                _ => return Err(self.err(self.toks[k].pos, format!("the type of `{owner}.{f}` is not one zsc reads: {PLAIN} ({TYPES})"))),
            }
            members.push(Member { name: f.clone(), ty, text: format!("{}{f}: {text}", if ro { "readonly " } else { "" }) });
        }
        Ok((members, k))
    }

    /// `type X = "a" | "b"` (an enum without data: the strings are its values) or `type X = { kind: "a"; .. } | ..`
    /// (each member an object, or the name of one in the file, with a field of a string literal type, the same in
    /// every member: the enum's tag)
    fn union(&self, name: &str, body: usize, uses: &mut Vec<(String, Pos)>) -> Result<Read, TsError> {
        let mut k = body;
        // (`type X = ("a" | "b")`: the parentheses close before anything that would go on)
        let paren = self.toks[k].t == T::P('(');
        if paren {
            k += 1;
        }
        if self.toks[k].t == T::P('|') {
            k += 1;
        }
        let src = |a: usize, b: usize| -> String { self.chars[self.toks[a].start..self.toks[b].end].iter().collect() };
        let mut lits: Vec<(String, Pos)> = vec![];
        let mut objs: Vec<(Vec<Member>, Pos)> = vec![];
        // (each member as the file writes it; a named one as its object, so the union reads the same anywhere)
        let mut texts: Vec<String> = vec![];
        loop {
            let t = &self.toks[k];
            match &t.t {
                T::Str(v) => {
                    lits.push((v.clone(), t.pos));
                    texts.push(src(k, k));
                    k += 1;
                }
                T::P('{') => {
                    let (ms, close) = self.object(k, name, uses)?;
                    objs.push((ms, t.pos));
                    texts.push(src(k, close));
                    k = close + 1;
                }
                // (a member named: an object type of the file, read for its shape)
                T::Ident(n) if self.decls.contains_key(n) => {
                    let d = self.plain_decl(n)?;
                    let b = if d.interface || self.toks[d.body].t == T::P('{') { d.body } else {
                        return Err(self.err(t.pos, format!("`{n}`, a member of `{name}`, is not an object type: {PLAIN}")));
                    };
                    let (ms, close) = self.object(b, n, uses)?;
                    // (the whole declaration must be the object: not `type A = { .. } & ..`)
                    let goes_on = matches!(self.toks[close + 1].t, T::P('&' | '[' | '.' | '<' | '|')) || matches!(&self.toks[close + 1].t, T::Ident(e) if e == "extends");
                    if !d.interface && goes_on {
                        return Err(self.err(self.toks[close + 1].pos, format!("`{n}`, a member of `{name}`, is not only an object type: {PLAIN}")));
                    }
                    objs.push((ms, t.pos));
                    texts.push(src(b, close));
                    k += 1;
                }
                _ => return Err(self.err(t.pos, format!("`{name}` is not a record or a union zsc reads: {PLAIN} (an object type, a union of string literals, or a union of objects with a string tag field)"))),
            }
            if self.toks[k].t == T::P('|') {
                k += 1;
                continue;
            }
            break;
        }
        if paren {
            if self.toks[k].t != T::P(')') {
                return Err(self.err(self.toks[k].pos, format!("`{name}` is not a record or a union zsc reads: {PLAIN}")));
            }
            k += 1;
        }
        if matches!(self.toks[k].t, T::P('&' | '[' | '.' | '<' | '|')) || matches!(&self.toks[k].t, T::Ident(e) if e == "extends") {
            return Err(self.err(self.toks[k].pos, format!("`{name}` is not a record or a union zsc reads: {PLAIN}")));
        }
        let text = texts.join(" | ");
        let variant = |tag: &str, p: Pos| -> Result<String, TsError> {
            let mut cs = tag.chars();
            match cs.next() {
                Some(c) if (c.is_alphabetic() || c == '_') && tag.chars().all(|c| c.is_alphanumeric() || c == '_') => Ok(c.to_uppercase().chain(cs).collect()),
                _ => Err(self.err(p, format!("the tag \"{tag}\" of `{name}` is not a name: zsc names the variant after it, capitalized (\"circle\" is `Circle`)"))),
            }
        };
        let mut seen = HashSet::new();
        let mut check = |v: &str, p: Pos| -> Result<(), TsError> {
            if seen.insert(v.to_string()) { Ok(()) } else { Err(self.err(p, format!("two members of `{name}` are the variant `{v}`: their tags must differ"))) }
        };
        match (lits.is_empty(), objs.is_empty()) {
            (false, true) => {
                let mut vs = vec![];
                for (l, p) in &lits {
                    let v = variant(l, *p)?;
                    check(&v, *p)?;
                    vs.push((v, vec![]));
                }
                let repr = TsEnum { key: None, tags: lits.into_iter().map(|l| l.0).collect(), fields: vec![vec![]; vs.len()] };
                Ok(Read::Enum(vs, repr, text))
            }
            (true, false) => {
                // (the tag: the first field of a string literal type that every member has)
                let key = objs[0].0.iter().find(|m| matches!(m.ty, Fty::Lit(..)) && objs.iter().all(|(ms, _)| ms.iter().any(|x| x.name == m.name && matches!(x.ty, Fty::Lit(..)))));
                let Some(key) = key.map(|m| m.name.clone()) else {
                    return Err(self.err(objs[0].1, format!("the members of `{name}` have no common tag: zsc reads a union of objects that all have a field of a string literal type (`kind: \"circle\"`)")));
                };
                let (mut vs, mut tags, mut fields) = (vec![], vec![], vec![]);
                for (ms, p) in objs {
                    let Some(Fty::Lit(tag, tp)) = ms.iter().find(|m| m.name == key).map(|m| m.ty.clone()) else { unreachable!() };
                    let v = variant(&tag, tp)?;
                    check(&v, p)?;
                    let mut tys = vec![];
                    let mut names = vec![];
                    for m in ms.into_iter().filter(|m| m.name != key) {
                        match m.ty {
                            Fty::Ty(t) => tys.push(t),
                            Fty::Lit(_, lp) => return Err(self.err(lp, format!("`{}` in a member of `{name}` is a second string literal field: zsc reads one tag", m.name))),
                        }
                        names.push(m.name);
                    }
                    vs.push((v, tys));
                    tags.push(tag);
                    fields.push(names);
                }
                Ok(Read::Enum(vs, TsEnum { key: Some(key), tags, fields }, text))
            }
            _ => Err(self.err(self.toks[body].pos, format!("`{name}` mixes string literals and objects: zsc reads a union of either"))),
        }
    }

    /// A field's type from token `k`: the type and the token after it
    fn ty(&self, mut k: usize, uses: &mut Vec<(String, Pos)>, rec: &str, f: &str) -> Result<(TyS, usize), TsError> {
        let bad = |t: &Tok| self.err(t.pos, format!("the type of `{rec}.{f}` is not one zsc reads: {PLAIN} ({TYPES})"));
        let t = &self.toks[k];
        let readonly = matches!(&t.t, T::Ident(r) if r == "readonly");
        if readonly {
            k += 1;
        }
        let t = &self.toks[k];
        let pos = t.pos;
        let mut ty = match &t.t {
            T::Ident(n) => {
                k += 1;
                match n.as_str() {
                    "string" => TyS::Name("String".into(), vec![], pos),
                    "number" => TyS::Name("f64".into(), vec![], pos),
                    "boolean" => TyS::Name("bool".into(), vec![], pos),
                    "bigint" => TyS::Name("i64".into(), vec![], pos),
                    "Array" | "ReadonlyArray" => {
                        if self.toks[k].t != T::P('<') {
                            return Err(bad(&self.toks[k]));
                        }
                        let (e, next) = self.ty(k + 1, uses, rec, f)?;
                        if self.toks[next].t != T::P('>') {
                            return Err(bad(&self.toks[next]));
                        }
                        k = next + 1;
                        TyS::Name("Array".into(), vec![e], pos)
                    }
                    _ if self.decls.contains_key(n) && self.toks[k].t != T::P('<') => {
                        uses.push((n.clone(), pos));
                        TyS::Name(n.clone(), vec![], pos)
                    }
                    _ if self.toks[k].t == T::P('<') || matches!(self.toks[k].t, T::P('.')) => return Err(bad(t)),
                    _ => return Err(self.err(pos, format!("`{n}` (the type of `{rec}.{f}`) is not declared in this file: zsc reads the types of one TS file for now"))),
                }
            }
            T::P('(') => {
                let (inner, next) = self.ty(k + 1, uses, rec, f)?;
                if self.toks[next].t != T::P(')') {
                    return Err(bad(&self.toks[next]));
                }
                k = next + 1;
                inner
            }
            _ => return Err(bad(t)),
        };
        let mut arrays = 0;
        while self.toks[k].t == T::P('[') && self.toks[k + 1].t == T::P(']') {
            ty = TyS::Name("Array".into(), vec![ty], pos);
            arrays += 1;
            k += 2;
        }
        // (`readonly` goes on an array type only)
        if readonly && arrays == 0 {
            return Err(bad(&self.toks[k]));
        }
        if matches!(self.toks[k].t, T::P('|' | '&' | '[' | '<' | '.')) {
            return Err(bad(&self.toks[k]));
        }
        Ok((ty, k))
    }
}

/// Replaces each `import type` with the records it names (and the records they use), read from the TS files.
/// `base`: the zsc file's directory (an import's path is relative to it)
pub fn expand(items: Vec<Item>, base: &Path) -> Result<Vec<Item>, Vec<TsError>> {
    let mut out = vec![];
    let mut errs = vec![];
    let mut files: HashMap<PathBuf, TsFile> = HashMap::new();
    // (a type read once even when two imports, or two records, need it)
    let mut done: HashSet<(PathBuf, String)> = HashSet::new();
    for it in items {
        let Item::ImportType { names, from, values, pos } = it else {
            out.push(it);
            continue;
        };
        if values {
            errs.push(TsError { file: None, pos, msg: format!("`{from}`: a TS file gives types only: `import type {{ .. }} from \"{from}\";` (functions from JS come in as `extern fn`)") });
            continue;
        }
        let path = base.join(&from);
        if !from.starts_with("./") && !from.starts_with("../") {
            errs.push(TsError { file: None, pos, msg: format!("`{from}`: zsc reads TS files by a relative path for now (`./model.ts`), not packages") });
            continue;
        }
        if !files.contains_key(&path) {
            match TsFile::read(&path) {
                Ok(f) => {
                    files.insert(path.clone(), f);
                }
                Err(m) => {
                    errs.push(TsError { file: None, pos, msg: m });
                    continue;
                }
            }
        }
        let f = &files[&path];
        let abs = std::fs::canonicalize(&path).unwrap_or(path.clone());
        let mut todo: Vec<(String, Pos, bool)> = names.into_iter().map(|(n, p)| (n, p, true)).collect();
        let mut recs = vec![];
        while let Some((n, p, named)) = todo.pop() {
            if !f.decls.contains_key(&n) {
                errs.push(if named {
                    TsError { file: None, pos: p, msg: format!("`{from}` declares no type `{n}` (an `interface` or a `type`)") }
                } else {
                    f.err(p, format!("`{n}` is not declared in this file"))
                });
                continue;
            }
            if named && !f.decls[&n].exported {
                errs.push(TsError { file: None, pos: p, msg: format!("`{from}` does not export `{n}` (TS could not import it either)") });
                continue;
            }
            if !done.insert((abs.clone(), n.clone())) {
                continue;
            }
            let exported = f.decls[&n].exported;
            // (a type read because another uses it is placed at the import, for errors in the zsc file)
            let at = if named { p } else { pos };
            match f.decl(&n) {
                Ok((Read::Record(fields, members), uses)) => {
                    todo.extend(uses.into_iter().map(|(u, up)| (u, up, false)));
                    recs.push(Item::Record { name: n, params: vec![], fields, pos: at, from: Some(TsFrom { path: abs.clone(), exported, members, text: String::new(), repr: None, zsc: None }) });
                }
                Ok((Read::Enum(variants, repr, text), uses)) => {
                    todo.extend(uses.into_iter().map(|(u, up)| (u, up, false)));
                    let variants = variants.into_iter().map(|(v, ts)| (v, ts, None)).collect();
                    recs.push(Item::Enum { name: n, params: vec![], variants, pos: at, from: Some(TsFrom { path: abs.clone(), exported, members: vec![], text, repr: Some(repr), zsc: None }) });
                }
                Err(e) => errs.push(e),
            }
        }
        out.extend(recs);
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

/// The module specifier of TS file `ts` from directory `dir` (where the `.d.ts` is written), as TS resolves it from
/// a `.d.ts` next to ES modules: `./model.ts` → `./model.js`
pub fn specifier(dir: &Path, ts: &Path) -> String {
    // (a type from another zustscript module: its specifier as the import wrote it, `./lib.js`)
    if let Some(s) = ts.to_str().filter(|s| s.starts_with("./") || s.starts_with("../")) {
        return s.to_string();
    }
    let dir = std::fs::canonicalize(dir).unwrap_or(dir.to_path_buf());
    let a: Vec<_> = dir.components().collect();
    let b: Vec<_> = ts.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); a.len() - common];
    parts.extend(b[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    let mut s = parts.join("/");
    for (from, to) in [(".d.ts", ".js"), (".d.mts", ".mjs"), (".d.cts", ".cjs"), (".tsx", ".js"), (".mts", ".mjs"), (".cts", ".cjs"), (".ts", ".js")] {
        if let Some(stem) = s.strip_suffix(from) {
            s = format!("{stem}{to}");
            break;
        }
    }
    if s.starts_with("..") { s } else { format!("./{s}") }
}
