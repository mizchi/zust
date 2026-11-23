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

use crate::ast::{Item, Pos, TsFrom, TyS};
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
                    adv(&mut i, &mut line, &mut col, &s);
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
        let chars = src.chars().collect();
        Ok(TsFile { path: path.display().to_string(), src, chars, toks, decls })
    }

    fn err(&self, pos: Pos, msg: impl Into<String>) -> TsError {
        TsError { file: Some((self.path.clone(), self.src.clone())), pos, msg: msg.into() }
    }

    /// The fields of record `name`, its members as the file writes them, and the names of the file's types they use
    #[allow(clippy::type_complexity)]
    fn record(&self, name: &str) -> Result<(Vec<(String, TyS)>, Vec<String>, Vec<(String, Pos)>), TsError> {
        let d = &self.decls[name];
        let plain = "zsc reads plain record types from TS for now";
        if let Some(p) = d.again {
            return Err(self.err(p, format!("`{name}` is declared more than once: {plain} (TS merges the declarations; write them as one)")));
        }
        if d.generic {
            return Err(self.err(d.pos, format!("`{name}` has type parameters: {plain} (no type parameters, `extends`, optional fields, unions)")));
        }
        if d.extends {
            return Err(self.err(d.pos, format!("`{name}` extends another type: {plain} (write its fields out)")));
        }
        let mut k = d.body;
        if !d.interface && self.toks[k].t != T::P('{') {
            return Err(self.err(self.toks[k].pos, format!("`{name}` is not an object type: {plain}")));
        }
        k += 1;
        let mut fields = vec![];
        let mut members = vec![];
        let mut uses = vec![];
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
                return Err(self.err(ft.pos, format!("a member of `{name}` that is not `field: type` (methods, index signatures and quoted names are not read): {plain}")));
            };
            k += 1;
            match &self.toks[k].t {
                T::P(':') => {}
                T::P('?') => return Err(self.err(self.toks[k].pos, format!("`{name}.{f}` is optional: {plain} (an optional field needs `Option`)"))),
                _ => return Err(self.err(self.toks[k].pos, format!("`{name}.{f}` is not `field: type` (a method is not read): {plain}"))),
            }
            k += 1;
            let from = self.toks[k].start;
            let (ty, next) = self.ty(k, &mut uses, name, f)?;
            let text: String = self.chars[from..self.toks[next - 1].end].iter().collect();
            members.push(format!("{}{f}: {text}", if ro { "readonly " } else { "" }));
            k = next;
            // (`;`, `,`, or the end of the line, as TS separates members)
            match &self.toks[k].t {
                T::P(';' | ',' | '}') => {}
                _ if self.toks[k].nl => {}
                _ => return Err(self.err(self.toks[k].pos, format!("the type of `{name}.{f}` is not one zsc reads: {plain} (string, number, boolean, bigint, arrays, and the file's other record types)"))),
            }
            fields.push((f.clone(), ty));
        }
        // (`type X = { .. } | ..`: the whole alias must be the object)
        // (or a conditional type: `type X = { .. } extends Y ? .. : ..`)
        let goes_on = matches!(self.toks[k + 1].t, T::P('|' | '&' | '[' | '.' | '<')) || matches!(&self.toks[k + 1].t, T::Ident(e) if e == "extends");
        if !d.interface && goes_on {
            return Err(self.err(self.toks[k + 1].pos, format!("`{name}` is not only an object type (a union, an intersection or an array of one): {plain}")));
        }
        Ok((fields, members, uses))
    }

    /// A field's type from token `k`: the type and the token after it
    fn ty(&self, mut k: usize, uses: &mut Vec<(String, Pos)>, rec: &str, f: &str) -> Result<(TyS, usize), TsError> {
        let bad = |t: &Tok| self.err(t.pos, format!("the type of `{rec}.{f}` is not one zsc reads: zsc reads plain record types from TS for now (string, number, boolean, bigint, arrays, and the file's other record types)"));
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
        let Item::ImportType { names, from, pos } = it else {
            out.push(it);
            continue;
        };
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
            match f.record(&n) {
                Ok((fields, members, uses)) => {
                    todo.extend(uses.into_iter().map(|(u, up)| (u, up, false)));
                    // (a type read because another uses it is placed at the import, for errors in the zsc file)
                    let at = if named { p } else { pos };
                    recs.push(Item::Record { name: n, params: vec![], fields, pos: at, from: Some(TsFrom { path: abs.clone(), exported, members }) });
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
