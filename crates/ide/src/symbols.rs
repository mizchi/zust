//! The list of a source's symbols (items and the members in them): used by LSP documentSymbol / workspace/symbol.
//! It reads the token sequence rather than the syntax tree, so even if the syntax is broken mid-edit it outputs as far as it can read. Positions are bytes of the original text
//! (before preprocessing: the text the editor holds, as it is, before macro expansion / flattening `mod`).
//!
//! Output: mod / struct / enum / union / trait / impl / fn / const / static / type / macro_rules!,
//! struct fields, enum variants, items inside impl / trait / mod / extern. The inside of fn bodies is not read.

use zust_syntax::lexer::tokenize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymKind {
    Module,
    Struct,
    Enum,
    Union,
    Trait,
    Impl,
    Function,
    Method,
    Const,
    Static,
    TypeAlias,
    Field,
    Variant,
    Macro,
    /// An `extern "C" { .. }` block (the declarations inside are children; the name is `extern "C"`)
    Extern,
}

impl SymKind {
    /// LSP SymbolKind number
    pub fn lsp(self) -> u32 {
        match self {
            SymKind::Module => 2,
            SymKind::Struct | SymKind::Union => 23,
            SymKind::Enum => 10,
            SymKind::Trait => 11,
            SymKind::Impl => 19,
            SymKind::Function | SymKind::Macro => 12,
            SymKind::Method => 6,
            // (Like rust-analyzer, static is Constant too)
            SymKind::Const | SymKind::Static => 14,
            SymKind::Extern => 3,
            SymKind::TypeAlias => 26,
            SymKind::Field => 8,
            SymKind::Variant => 22,
        }
    }
}

impl Symbol {
    /// documentSymbol's SymbolKind: like rust-analyzer, a fn inside impl / trait that does not take `self` (`fn new() -> Self`) is
    /// Function
    pub fn lsp_kind(&self) -> u32 {
        if self.kind == SymKind::Method && !takes_self(&self.detail) {
            return SymKind::Function.lsp();
        }
        self.kind.lsp()
    }
}

/// Whether the first parameter of a fn signature (`fn f<T>(&mut self, ..)`) is self
pub fn takes_self(sig: &str) -> bool {
    let Some(i) = sig.find('(') else { return false };
    let first = sig[i + 1..].split([',', ')']).next().unwrap_or("").trim();
    let first = first.trim_start_matches('&').trim_start();
    let first = if first.starts_with('\'') { first.split_once(' ').map(|x| x.1).unwrap_or("") } else { first };
    let first = first.trim_start_matches("mut ").trim_start();
    first == "self" || first.starts_with("self:") || first.starts_with("self ")
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: SymKind,
    /// Extra info (the fn signature, the field type, etc.; put on one line)
    pub detail: String,
    /// Byte ranges of the whole item (including attributes) and of the name
    pub start: usize,
    pub end: usize,
    pub name_start: usize,
    pub name_end: usize,
    pub children: Vec<Symbol>,
    /// Something inside a macro (`thread_local! { static X .. }`): used for definitions, but not output in documentSymbol (same as rust-analyzer)
    pub in_macro: bool,
}

type Tok = (u8, usize, usize);

struct Scan<'a> {
    s: &'a [u8],
    t: Vec<Tok>,
}

impl Scan<'_> {
    fn text(&self, k: usize) -> &[u8] {
        let (_, a, b) = self.t[k];
        &self.s[a..b]
    }
    fn is(&self, k: usize, w: &[u8]) -> bool {
        k < self.t.len() && self.text(k) == w
    }
    fn sym(&self, k: usize, c: u8) -> bool {
        k < self.t.len() && self.t[k].0 == 1 && self.s[self.t[k].1] == c
    }
    fn ident(&self, k: usize) -> bool {
        k < self.t.len() && self.t[k].0 == 0
    }
    /// The closing bracket matching the opening bracket at k. For `{`, only braces are counted (the end of the body is found even if parentheses inside are unclosed).
    /// If unclosed, up to before the next item that starts at a line start (no indentation) (end - 1 if none)
    fn close(&self, k: usize, end: usize) -> usize {
        let brace = self.sym(k, b'{');
        let mut d = 0i32;
        let mut j = k;
        while j < end {
            if self.t[j].0 == 1 {
                match self.s[self.t[j].1] {
                    b'{' => d += 1,
                    b'(' | b'[' if !brace => d += 1,
                    b'}' => {
                        d -= 1;
                        if d == 0 {
                            return j;
                        }
                    }
                    b')' | b']' if !brace => {
                        d -= 1;
                        if d == 0 {
                            return j;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        let mut j = k + 1;
        while j < end {
            let a = self.t[j].1;
            let at_col0 = a == 0 || self.s[a - 1] == b'\n';
            let w = self.text(j);
            if at_col0 && matches!(w, b"fn" | b"pub" | b"struct" | b"enum" | b"union" | b"impl" | b"trait" | b"mod" | b"const" | b"static" | b"type" | b"use" | b"#" | b"macro_rules") {
                return j - 1;
            }
            j += 1;
        }
        end.saturating_sub(1)
    }

    /// From k, the `;` outside brackets or (if want_brace) the end of the first `{ .. }` outside brackets
    fn item_end(&self, k: usize, end: usize, want_brace: bool) -> usize {
        let mut j = k;
        while j < end {
            if self.t[j].0 == 1 {
                match self.s[self.t[j].1] {
                    b';' => return j,
                    b'{' if want_brace => return self.close(j, end),
                    b'(' | b'[' | b'{' => {
                        j = self.close(j, end);
                    }
                    // (A `}` at the start of the next item: an unclosed bracket)
                    b'}' => return j.saturating_sub(1).max(k),
                    _ => {}
                }
            }
            j += 1;
        }
        end.saturating_sub(1)
    }
    /// The text of the token range a..=b put on one line with whitespace (newlines, comments too) squeezed out (cut if long)
    fn join(&self, a: usize, b: usize) -> String {
        if a >= self.t.len() || b < a {
            return String::new();
        }
        let b = b.min(self.t.len() - 1);
        let mut out = String::new();
        let mut j = a;
        while j <= b {
            if j > a && self.t[j].1 > self.t[j - 1].2 {
                out.push(' ');
            }
            out.push_str(&String::from_utf8_lossy(&self.s[self.t[j].1..self.t[j].2]));
            // (A `'` (lifetime) between tokens is not in the token sequence, so add it from the original text)
            if j < b && self.t[j + 1].1 > self.t[j].2 {
                let gap = &self.s[self.t[j].2..self.t[j + 1].1];
                if gap.ends_with(b"'") {
                    if gap.len() > 1 {
                        out.push(' ');
                    }
                    out.push('\'');
                    j += 1;
                    out.push_str(&String::from_utf8_lossy(&self.s[self.t[j].1..self.t[j].2]));
                }
            }
            if out.len() > 1000 {
                out.push('…');
                break;
            }
            j += 1;
        }
        out
    }

    /// Reads the items in a..end. in_impl: inside impl / trait (fns are methods)
    /// Items inside a fn body (tokens a..end): the ones with a keyword at the start of a statement (after `{` / `}` / `;` / an attribute's `]`)
    fn nested_items(&self, a: usize, end: usize) -> Vec<Symbol> {
        let mut out = vec![];
        let mut j = a;
        while j < end {
            let at_stmt = j == a || self.sym(j - 1, b'{') || self.sym(j - 1, b'}') || self.sym(j - 1, b';') || self.sym(j - 1, b']');
            let item = self.ident(j)
                && at_stmt
                && match self.text(j) {
                    b"fn" | b"struct" | b"enum" | b"union" | b"trait" | b"mod" | b"type" => self.ident(j + 1),
                    b"impl" => true,
                    b"const" | b"static" => self.ident(j + 1) && !self.is(j + 1, b"fn") || self.is(j + 1, b"fn"),
                    b"macro_rules" => self.sym(j + 1, b'!'),
                    b"pub" | b"unsafe" | b"async" | b"extern" => true,
                    _ => false,
                }
                // (A lifetime like `'static` is, lexically, an identifier after `'`)
                && !(self.t[j].1 >= 1 && self.s[self.t[j].1 - 1] == b'\'');
            if !item {
                j += 1;
                continue;
            }
            let n = out.len();
            let e = self.item_end(j, end, !matches!(self.text(j), b"const" | b"static" | b"type"));
            self.items(j, (e + 1).min(end), false, &mut out);
            // (For blocks that were not items, like `unsafe { .. }` / `async move { .. }`, look inside too)
            j = if out.len() > n { e + 1 } else { j + 1 };
        }
        out
    }

    fn items(&self, a: usize, end: usize, in_impl: bool, out: &mut Vec<Symbol>) {
        let mut k = a;
        while k < end {
            let start = k;
            // Attributes `#[..]` / `#![..]`
            let mut j = k;
            while self.sym(j, b'#') {
                let mut o = j + 1;
                if self.sym(o, b'!') {
                    o += 1;
                }
                if !self.sym(o, b'[') {
                    break;
                }
                j = self.close(o, end) + 1;
            }
            // Modifiers (pub / pub(crate) / unsafe / async / default / extern "C" (before fn) / const (before fn))
            loop {
                if self.is(j, b"pub") {
                    j += 1;
                    if self.sym(j, b'(') {
                        j = self.close(j, end) + 1;
                    }
                } else if self.is(j, b"unsafe") || self.is(j, b"async") || self.is(j, b"default") || (self.is(j, b"const") && (self.is(j + 1, b"fn") || self.is(j + 1, b"unsafe") || self.is(j + 1, b"extern") || self.is(j + 1, b"async") || self.is(j + 1, b"impl") || self.is(j + 1, b"trait"))) {
                    j += 1;
                } else if self.is(j, b"extern") && j + 1 < end && self.t[j + 1].0 == 2 && self.is(j + 2, b"fn") {
                    j += 2;
                } else {
                    break;
                }
            }
            if j >= end {
                break;
            }
            let kw = self.text(j).to_vec();
            let named = |kind: SymKind, nk: usize, e: usize, detail: String, children: Vec<Symbol>| -> Symbol {
                let (na, nb) = if nk < self.t.len() { (self.t[nk].1, self.t[nk].2) } else { (self.t[j].1, self.t[j].2) };
                Symbol {
                    name: String::from_utf8_lossy(&self.s[na..nb]).into_owned(),
                    kind,
                    detail,
                    start: self.t[start].1,
                    end: self.t[e.min(self.t.len() - 1)].2,
                    name_start: na,
                    name_end: nb,
                    children,
                    in_macro: false,
                }
            };
            let next = match kw.as_slice() {
                b"fn" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    // (Signature: up to before the body's `{` or `;`)
                    let mut se = j;
                    while se < e && !self.sym(se + 1, b'{') && !self.sym(se + 1, b';') {
                        se += 1;
                    }
                    // (Items inside the body `fn helper() {}` / `struct S;` ..: made children, like rust-analyzer)
                    let ch = if self.sym(se + 1, b'{') { self.nested_items(se + 2, e) } else { vec![] };
                    out.push(named(if in_impl { SymKind::Method } else { SymKind::Function }, j + 1, e, self.join(j, se), ch));
                    e + 1
                }
                b"struct" | b"union" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    let mut fields = vec![];
                    // Fields of `struct S { a: T, .. }`
                    let mut b = j + 2;
                    while b < e && !self.sym(b, b'{') && !self.sym(b, b'(') && !self.sym(b, b';') {
                        if self.sym(b, b'<') {
                            // (A `{` inside type arguments is not counted: the expression of a const argument can be ignored)
                        }
                        b += 1;
                    }
                    if self.sym(b, b'{') {
                        self.fields(b + 1, self.close(b, end), &mut fields);
                    }
                    let kind = if kw == b"struct" { SymKind::Struct } else { SymKind::Union };
                    out.push(named(kind, j + 1, e, String::new(), fields));
                    e + 1
                }
                b"enum" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    let mut vars = vec![];
                    let mut b = j + 2;
                    while b < e && !self.sym(b, b'{') {
                        b += 1;
                    }
                    if self.sym(b, b'{') {
                        self.variants(b + 1, self.close(b, end), &mut vars);
                    }
                    out.push(named(SymKind::Enum, j + 1, e, String::new(), vars));
                    e + 1
                }
                b"trait" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    let mut ch = vec![];
                    let mut b = j + 2;
                    while b < e && !self.sym(b, b'{') {
                        b += 1;
                    }
                    if self.sym(b, b'{') {
                        self.items(b + 1, self.close(b, end), true, &mut ch);
                    }
                    out.push(named(SymKind::Trait, j + 1, e, String::new(), ch));
                    e + 1
                }
                b"impl" => {
                    let e = self.item_end(j, end, true);
                    let mut b = j + 1;
                    while b < e && !self.sym(b, b'{') {
                        b += 1;
                    }
                    let mut ch = vec![];
                    if self.sym(b, b'{') {
                        self.items(b + 1, self.close(b, end), true, &mut ch);
                    }
                    // (Name: `Tr for S<T>` / `S`, without the type params of `impl<T> Tr for S<T>`)
                    let mut h = j + 1;
                    if self.sym(h, b'<') {
                        let mut d = 0i32;
                        while h < b {
                            if self.sym(h, b'<') {
                                d += 1;
                            } else if self.sym(h, b'>') {
                                d -= 1;
                                if d == 0 {
                                    h += 1;
                                    break;
                                }
                            }
                            h += 1;
                        }
                    }
                    let mut he = b.saturating_sub(1);
                    // (Nothing from `where` on goes into the name)
                    let mut w = h;
                    while w < b {
                        if self.is(w, b"where") {
                            he = w.saturating_sub(1);
                            break;
                        }
                        w += 1;
                    }
                    let name = if h <= he { self.join(h, he) } else { String::from("impl") };
                    let (na, nb) = if h <= he && he < self.t.len() { (self.t[h].1, self.t[he].2) } else { (self.t[j].1, self.t[j].2) };
                    out.push(Symbol {
                        name: format!("impl {name}"),
                        kind: SymKind::Impl,
                        detail: String::new(),
                        start: self.t[start].1,
                        end: self.t[e.min(self.t.len() - 1)].2,
                        name_start: na,
                        name_end: nb,
                        children: ch,
                        in_macro: false,
                    });
                    e + 1
                }
                b"mod" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, true);
                    let mut ch = vec![];
                    if self.sym(j + 2, b'{') {
                        self.items(j + 3, self.close(j + 2, end), false, &mut ch);
                    }
                    out.push(named(SymKind::Module, j + 1, e, String::new(), ch));
                    e + 1
                }
                b"extern" if self.sym(j + 1, b'{') || (j + 2 < end && self.t[j + 1].0 == 2 && self.sym(j + 2, b'{')) => {
                    // `extern "C" { fn f(); }`: made children of `extern "C"`, like rust-analyzer (navigate treats them as top-level items)
                    let o = if self.sym(j + 1, b'{') { j + 1 } else { j + 2 };
                    let c = self.close(o, end);
                    let mut ch = vec![];
                    self.items(o + 1, c, false, &mut ch);
                    let name = if o == j + 2 { format!("extern {}", String::from_utf8_lossy(self.text(j + 1))) } else { "extern".to_string() };
                    out.push(Symbol { name, kind: SymKind::Extern, detail: String::new(), start: self.t[start].1, end: self.t[c.min(self.t.len() - 1)].2, name_start: self.t[j].1, name_end: self.t[o - 1].2, children: ch, in_macro: false });
                    c + 1
                }
                b"const" | b"static" if self.ident(j + 1) => {
                    let nk = if self.is(j + 1, b"mut") { j + 2 } else { j + 1 };
                    let e = self.item_end(j, end, false);
                    if self.is(nk, b"_") {
                        e + 1
                    } else {
                        // (Type: from `:` up to before `=` / `;`)
                        let mut ta = nk + 1;
                        let detail = if self.sym(ta, b':') {
                            ta += 1;
                            let mut tb = ta;
                            while tb < e && !self.sym(tb, b'=') {
                                tb += 1;
                            }
                            if tb > ta { self.join(ta, tb - 1) } else { String::new() }
                        } else {
                            String::new()
                        };
                        out.push(named(if kw == b"const" { SymKind::Const } else { SymKind::Static }, nk, e, detail, vec![]));
                        e + 1
                    }
                }
                b"type" if self.ident(j + 1) => {
                    let e = self.item_end(j, end, false);
                    // (detail: the type to the right of `=`. With type params, `<K, V> = right`)
                    let mut q = j + 2;
                    let mut generics = String::new();
                    if self.sym(q, b'<') {
                        let g0 = q;
                        let mut d = 0;
                        while q < e {
                            if self.sym(q, b'<') {
                                d += 1;
                            } else if self.sym(q, b'>') && !self.sym(q - 1, b'-') {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            q += 1;
                        }
                        generics = self.join(g0, q);
                    }
                    while q < e && !self.sym(q, b'=') {
                        q += 1;
                    }
                    let rhs = if q + 1 < e { self.join(q + 1, e - 1) } else { String::new() };
                    let detail = if generics.is_empty() || rhs.is_empty() { rhs } else { format!("{generics} = {rhs}") };
                    out.push(named(SymKind::TypeAlias, j + 1, e, detail, vec![]));
                    e + 1
                }
                b"macro_rules" if self.sym(j + 1, b'!') && self.ident(j + 2) => {
                    let mut e = if self.sym(j + 3, b'{') || self.sym(j + 3, b'(') || self.sym(j + 3, b'[') { self.close(j + 3, end) } else { j + 2 };
                    if self.sym(e + 1, b';') {
                        e += 1;
                    }
                    out.push(named(SymKind::Macro, j + 2, e, String::from("macro_rules!"), vec![]));
                    // (The methods of core's number types are in the bodies of the macros `uint_impl!` / `int_impl!`: the items in their arms are made
                    // members of `impl __uint` / `impl __int` (navigate adds them to the owners u8 .. / i8 ..))
                    let mname = self.text(j + 2).to_vec();
                    if matches!(mname.as_slice(), b"uint_impl" | b"int_impl" | b"from_str_int_impl" | b"generate_pattern_iterators") && self.sym(j + 3, b'{') {
                        let mut ch = vec![];
                        let mut q = j + 4;
                        while q < e {
                            if self.sym(q, b'=') && self.sym(q + 1, b'>') && self.sym(q + 2, b'{') {
                                let c = self.close(q + 2, e);
                                let mut part = vec![];
                                self.items(q + 3, c, true, &mut part);
                                // (The members of the impl inside `$( impl $int_ty { fn .. } )+` too)
                                part = self.nested_items(q + 3, c).into_iter().chain(part).collect();
                                for s in part {
                                    if s.kind == SymKind::Impl {
                                        ch.extend(s.children.into_iter().map(|mut x| {
                                            if x.kind == SymKind::Function {
                                                x.kind = SymKind::Method;
                                            }
                                            x
                                        }));
                                    } else if !ch.iter().any(|x: &Symbol| x.name_start == s.name_start) {
                                        ch.push(s);
                                    }
                                }
                                q = c + 1;
                            } else {
                                q += 1;
                            }
                        }
                        ch.sort_by_key(|s| s.name_start);
                        ch.dedup_by_key(|s| s.name_start);
                        for s in &mut ch {
                            s.in_macro = true;
                            // (Members of the number types' impls)
                            if s.kind == SymKind::Function {
                                s.kind = SymKind::Method;
                            }
                        }
                        let owner = match mname.as_slice() {
                            b"uint_impl" => "__uint",
                            b"int_impl" => "__int",
                            // (Iterator impls such as str's `Split<'a, P>`: navigate adds them to the owners of those types)
                            b"generate_pattern_iterators" => "__str_pat_iter",
                            _ => "__num",
                        };
                        let (na, nb) = (self.t[j + 2].1, self.t[j + 2].2);
                        out.push(Symbol { name: format!("impl {owner}"), kind: SymKind::Impl, detail: String::new(), start: self.t[start].1, end: self.t[e.min(self.t.len() - 1)].2, name_start: na, name_end: nb, children: ch, in_macro: true });
                    }
                    e + 1
                }
                // (Declarative macros 2.0 `macro name(..) { .. }` / `macro name { .. }`: std's derives, etc.)
                b"macro" if self.ident(j + 1) => {
                    let mut b = j + 2;
                    if self.sym(b, b'(') {
                        b = self.close(b, end) + 1;
                    }
                    let e = if self.sym(b, b'{') { self.close(b, end) } else { self.item_end(j, end, true) };
                    out.push(named(SymKind::Macro, j + 1, e, String::from("macro"), vec![]));
                    e + 1
                }
                b"use" | b"extern" => self.item_end(j, end, false) + 1,
                _ => {
                    // (Macro calls like `m! { .. }` / `m!(..);` are skipped. An unknown token advances by one)
                    if self.ident(j) && self.sym(j + 1, b'!') && j + 2 < end && self.t[j + 2].0 == 1 {
                        let c = self.close(j + 2, end);
                        // (The static in `thread_local! { static X: T = ..; }` is an item: rust-analyzer expands the macro to see it)
                        if self.is(j, b"thread_local") && self.sym(j + 2, b'{') {
                            let n = out.len();
                            self.items(j + 3, c, false, out);
                            for s in &mut out[n..] {
                                s.in_macro = true;
                                // (The type of a thread_local static is `LocalKey<T>`)
                                if s.kind == SymKind::Static {
                                    s.detail = format!("LocalKey<{}>", s.detail);
                                }
                            }
                        }
                        if self.sym(c + 1, b';') { c + 2 } else { c + 1 }
                    } else if self.sym(j, b'{') {
                        self.close(j, end) + 1
                    } else {
                        j + 1
                    }
                }
            };
            k = next.max(k + 1);
        }
    }

    /// Past the attributes at j (`#[..] #[..]`)
    fn skip_attrs(&self, mut j: usize, end: usize) -> usize {
        while self.sym(j, b'#') && self.sym(j + 1, b'[') {
            j = self.close(j + 1, end) + 1;
        }
        j
    }

    /// The next `,` from j outside brackets, or end; with `angles`, `<..>` is a bracket too (a field's type)
    fn next_comma(&self, j: usize, end: usize, angles: bool) -> usize {
        let mut e = j;
        while e < end && !self.sym(e, b',') {
            if self.sym(e, b'(') || self.sym(e, b'[') || self.sym(e, b'{') {
                e = self.close(e, end);
            } else if angles && self.sym(e, b'<') {
                let mut d = 0i32;
                while e < end {
                    if self.sym(e, b'<') {
                        d += 1;
                    } else if self.sym(e, b'>') {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    e += 1;
                }
            }
            e += 1;
        }
        e
    }

    /// The fields of `{ a: T, pub b: U }`
    fn fields(&self, a: usize, end: usize, out: &mut Vec<Symbol>) {
        let mut k = a;
        while k < end {
            let start = k;
            let mut j = self.skip_attrs(k, end);
            if self.is(j, b"pub") {
                j += 1;
                if self.sym(j, b'(') {
                    j = self.close(j, end) + 1;
                }
            }
            let e = self.next_comma(j, end, true);
            if self.ident(j) && self.sym(j + 1, b':') && j + 2 < e {
                let (na, nb) = (self.t[j].1, self.t[j].2);
                out.push(Symbol {
                    name: String::from_utf8_lossy(&self.s[na..nb]).into_owned(),
                    kind: SymKind::Field,
                    detail: self.join(j + 2, e - 1),
                    start: self.t[start].1,
                    end: self.t[(e - 1).min(self.t.len() - 1)].2,
                    name_start: na,
                    name_end: nb,
                    children: vec![],
                    in_macro: false,
                });
            }
            k = e + 1;
        }
    }

    /// The variants of `{ A, B(T), C { x: U } = 3 }`
    fn variants(&self, a: usize, end: usize, out: &mut Vec<Symbol>) {
        let mut k = a;
        while k < end {
            let start = k;
            let j = self.skip_attrs(k, end);
            let e = self.next_comma(j, end, false);
            if self.ident(j) {
                let (na, nb) = (self.t[j].1, self.t[j].2);
                let mut ch = vec![];
                if self.sym(j + 1, b'{') {
                    self.fields(j + 2, self.close(j + 1, end), &mut ch);
                }
                out.push(Symbol {
                    name: String::from_utf8_lossy(&self.s[na..nb]).into_owned(),
                    kind: SymKind::Variant,
                    detail: String::new(),
                    start: self.t[start].1,
                    end: self.t[(e.max(j + 1) - 1).min(self.t.len() - 1)].2,
                    name_start: na,
                    name_end: nb,
                    children: ch,
                    in_macro: false,
                });
            }
            k = e + 1;
        }
    }
}

/// The tree of a source's symbols
pub fn document_symbols(src: &str) -> Vec<Symbol> {
    let s = src.as_bytes();
    let t = tokenize(s);
    let sc = Scan { s, t };
    let mut out = vec![];
    let n = sc.t.len();
    sc.items(0, n, false, &mut out);
    out
}

/// Symbols flattened into a list: (symbol, name of what encloses it)
pub fn flatten(syms: &[Symbol]) -> Vec<(Symbol, String)> {
    fn go(syms: &[Symbol], parent: &str, out: &mut Vec<(Symbol, String)>) {
        for s in syms {
            let mut x = s.clone();
            x.children = vec![];
            out.push((x, parent.to_string()));
            go(&s.children, &s.name, out);
        }
    }
    let mut out = vec![];
    go(syms, "", &mut out);
    out
}

/// byte position → (line, column). Columns are in UTF-16 units (same as LSP)
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(src: &str) -> LineIndex {
        let mut starts = vec![0];
        for (i, &c) in src.as_bytes().iter().enumerate() {
            if c == b'\n' {
                starts.push(i + 1);
            }
        }
        LineIndex { starts }
    }
    pub fn pos(&self, src: &str, at: usize) -> (u32, u32) {
        let at = at.min(src.len());
        let line = match self.starts.binary_search(&at) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let ls = self.starts[line];
        let mut col = 0u32;
        let mut i = ls;
        while i < at {
            if !src.is_char_boundary(i) {
                i += 1;
                continue;
            }
            let c = src[i..].chars().next().unwrap_or(' ');
            col += c.len_utf16() as u32;
            i += c.len_utf8();
        }
        (line as u32, col)
    }
}

/// Whether a name matches a query, and how good the match is (smaller is better). Case-insensitive.
/// 0: equal, 1: prefix match, 2: substring match (at a word start), 3: substring match, 4: contains the characters in order (`fbar` → `foo_bar`). An empty query is 5 for everything
pub fn match_score(query: &str, name: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(5);
    }
    let q = query.to_lowercase();
    let n = name.to_lowercase();
    if n == q {
        return Some(0);
    }
    if n.starts_with(&q) {
        return Some(1);
    }
    if let Some(i) = n.find(&q) {
        let b = name.as_bytes();
        let word_head = i > 0 && (b[i - 1] == b'_' || b[i - 1] == b':' || b[i - 1] == b' ' || (b[i].is_ascii_uppercase() && b[i - 1].is_ascii_lowercase()));
        return Some(if word_head { 2 } else { 3 });
    }
    let mut it = n.chars();
    for c in q.chars() {
        if !it.any(|x| x == c) {
            return None;
        }
    }
    Some(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[Symbol]) -> Vec<String> {
        v.iter().map(|s| format!("{:?} {}", s.kind, s.name)).collect()
    }

    #[test]
    fn items_and_members() {
        let src = "#[derive(Debug)]\npub struct P { pub x: i64, y: Vec<u8> }\nenum E { A, B(u8), C { z: u32 } }\n\
                   impl<T: Clone> Tr for W<T> where T: Copy {\n    fn m(&self) -> u32 { 1 }\n    const K: u8 = 1;\n}\n\
                   mod m { pub fn f() {} macro_rules! mm { () => {} } }\nconst C: &str = \"{\";\nfn g() { fn inner() {} }\n";
        let s = document_symbols(src);
        assert_eq!(names(&s), ["Struct P", "Enum E", "Impl impl Tr for W<T>", "Module m", "Const C", "Function g"]);
        assert_eq!(document_symbols("impl<'a> Cx<'a> { fn f(&'a self) {} }")[0].name, "impl Cx<'a>");
        assert_eq!(names(&s[0].children), ["Field x", "Field y"]);
        assert_eq!(s[0].children[1].detail, "Vec<u8>");
        assert_eq!(names(&s[1].children), ["Variant A", "Variant B", "Variant C"]);
        assert_eq!(names(&s[1].children[2].children), ["Field z"]);
        assert_eq!(names(&s[2].children), ["Method m", "Const K"]);
        assert_eq!(s[2].children[0].detail, "fn m(&self) -> u32");
        assert_eq!(names(&s[3].children), ["Function f", "Macro mm"]);
        // (The range starts at the attribute, the name covers only the name)
        assert_eq!(&src[s[0].start..s[0].start + 2], "#[");
        assert_eq!(&src[s[0].name_start..s[0].name_end], "P");
        assert!(src[s[5].start..s[5].end].ends_with('}'));
    }

    #[test]
    fn broken_source() {
        // (Past an unclosed fn too, as far as it can read)
        let s = document_symbols("fn a() {\n    let x = (1;\n}\nstruct S;\nfn b(");
        assert_eq!(names(&s), ["Function a", "Struct S", "Function b"]);
    }

    #[test]
    fn scores() {
        assert_eq!(match_score("foo", "foo"), Some(0));
        assert_eq!(match_score("foo", "FooBar"), Some(1));
        assert_eq!(match_score("bar", "foo_bar"), Some(2));
        assert_eq!(match_score("oob", "foobar"), Some(3));
        assert_eq!(match_score("fbr", "foo_bar"), Some(4));
        assert_eq!(match_score("xyz", "foo"), None);
        // Word starts: a camelCase capital / after `::` / after whitespace (containerName's `impl Point`). Others are 3
        assert_eq!(match_score("bar", "fooBar"), Some(2));
        assert_eq!(match_score("bar", "a::bar"), Some(2));
        assert_eq!(match_score("point", "impl Point"), Some(2));
        assert_eq!(match_score("bar", "FOOBAR"), Some(3));
        assert_eq!(match_score("ar", "fooBar"), Some(3));
        assert_eq!(match_score("bar", "fooBAR"), Some(2));
    }

    #[test]
    fn utf16_columns() {
        let src = "// あ\nfn ü() {}";
        let li = LineIndex::new(src);
        assert_eq!(li.pos(src, src.find('ü').unwrap()), (1, 3));
        assert_eq!(li.pos(src, src.find("()").unwrap()), (1, 4));
    }
}
