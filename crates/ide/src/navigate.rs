//! Go to definition and references (LSP textDocument/definition / references).
//! Looked up from the token list and the list of symbols (symbols) without the syntax tree or type inference: fast, and works even on source broken in the middle of an edit.
//! No answer when it does not come down to one (better not to jump than to jump to the wrong place).
//!
//! What the identifier at the cursor refers to:
//! - on the name of a definition: that definition
//! - a local (let / parameter / closure parameter / for / if let / while let / match arm pattern in a fn): following the
//!   scopes of the blocks, the nearest binding before the cursor
//! - a path `A::b`: with A as a type (struct / enum / trait / type), a module or `Self`, the b among its members (variants, the inside of impls,
//!   items in the module)
//! - a method / field `x.b`: if x is `self`, the type of the enclosing impl; for a local, the type is decided from the binding's type annotation, the `T::..` / `T { .. }` initializer
//!   or the parameter's type, then the b among that type's members. If the type is unknown, when it is the only one in the workspace
//! - other names: the same module → what `use` brought in → the only one in the workspace
//! References are the identifiers of the same name that resolve to the same definition

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use zust_syntax::fxhash::{FxMap, FxSet};
use zust_syntax::lexer::tokenize;
use crate::symbols::{SymKind, Symbol, document_symbols};

/// A definition (a symbol flattened, with its owner and module attached)
#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    pub kind: SymKind,
    /// The name of the type / trait owning the member (the self type for an impl, the enum for an enum variant). Empty for top-level items
    pub owner: String,
    /// The enclosing thing (for display: `impl Point` / a module / a struct name)
    pub container: Arc<str>,
    /// The enclosing module (the file's module path followed by the `mod x { .. }` in the file)
    pub module: Arc<[String]>,
    /// The byte ranges of the name and of the whole item
    pub start: usize,
    pub end: usize,
    pub item_start: usize,
    pub item_end: usize,
    /// The head name of the type: the type for a field / const / static, the return type for a fn / method, the right-hand type for a type alias (`&mut Vec<T>` → Vec)
    pub ty: String,
    /// The names of a type alias's type parameters (`type R<T> = Result<T, E>` → [T])
    pub params: Vec<String>,
    /// The parameters of a fn / method that take a closure: (parameter index (excluding self), type parameter name (`F`, empty for `impl Fn`),
    /// the closure's return shape (`U` / `Option<B>`), the closure's parameter shape (`T` / `&Self::Item` / `B, Self::Item`)).
    /// `fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Option<U>` → [(0, F, U, T)]
    pub fnret: Vec<(u8, String, String, String)>,
    /// Whether the method takes self (the ones offered in completion after `.`)
    pub has_self: bool,
    /// When slim (the text was dropped): (line, UTF-16 column) of both ends of the name
    pub pos: [u32; 4],
}

/// The index of one file
pub struct FileIndex {
    pub text: String,
    /// A library (std / core / alloc) file: holds only definitions (no token list). Used only when the workspace has no candidate
    pub lib: bool,
    /// Whether it is a file of the package's main crate (lib / main / bin in src). Items of the tests / examples / benches / build.rs crates
    /// are visible only from inside the same crate (krate)
    pub in_src: bool,
    /// The number of the target crate (files with the same number see each other)
    pub krate: u32,
    /// The file's module path (within the workspace: `src/a/b.rs` → [a, b])
    pub module: Vec<String>,
    pub defs: Vec<Def>,
    /// (kind, start, end). Held as u32 (small, since it is held for every file in the workspace)
    toks: Vec<(u8, u32, u32)>,
    /// Name → the indices of the tokens of that identifier
    idents: FxMap<String, Vec<u32>>,
    /// What `use` brought in: (the name in this file ("*" for a glob), path)
    uses: Vec<(String, Vec<String>)>,
    /// For each of uses, the byte position of the last name of the path (usize::MAX for a glob)
    use_at: Vec<usize>,
    /// The byte ranges of `macro_rules!` bodies (including the ones in fns)
    macros: Vec<(usize, usize)>,
    /// The hashes of definition names and the defs indices (in hash order): for looking up definitions by name with a binary search
    name_ix: Vec<(u64, u32)>,
    /// The hashes of owner names and the defs indices (in hash order): looked up per owner for completion after `.`
    owner_ix: Vec<(u64, u32)>,
    /// The uses shown in workspace/symbol: aliases `use a::B as C` (C) and re-exports `pub use a::B` (B).
    /// (name, line, UTF-16 column, name length (UTF-16)) (kept even when made slim)
    pub use_syms: Vec<(String, u32, u32, u32)>,
    /// The text and the token list were dropped (libraries, and workspace files taken out of memory). Only definitions, uses and the set of names
    pub slim: bool,
    /// (head of the self type, head of the trait): `impl Tr for S` and `#[derive(Tr)] struct S`
    pub impls: Vec<(String, String)>,
    /// (S, Tr, byte position of Tr's name) of `#[derive(Tr)] struct S`
    pub derives: Vec<(String, String, u32)>,
    /// The byte ranges of items excluded by cfg (decided in apply_cfg; the definitions inside are removed from defs)
    inactive: Vec<(usize, usize)>,
    /// The enabled features passed to apply_cfg (applied again when reloaded)
    features: Option<Vec<String>>,
    /// When slim: the hashes of the names of the identifiers that occur (sorted; for choosing the files to reload when searching references)
    names: Vec<u64>,
}

/// A position in a file (ws index, byte range of the name)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loc {
    pub file: usize,
    pub start: usize,
    pub end: usize,
}

/// (byte position in the literal, name) of each `{name}` / `{name:..}` (captured format argument) in a string literal
fn captures(lit: &str) -> Vec<(usize, &str)> {
    let b = lit.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            if i + 1 < b.len() && b[i + 1] == b'{' {
                i += 2;
                continue;
            }
            let a = i + 1;
            let mut e = a;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            if e > a && e < b.len() && (b[e] == b'}' || b[e] == b':') && !b[a].is_ascii_digit() {
                out.push((a, &lit[a..e]));
            }
            i = e;
            continue;
        }
        i += 1;
    }
    out
}

/// The names of the type parameters ([T, N]) of the generics declaration `<'a, T: X, const N: usize>` after a name (from byte at)
fn generic_names(text: &str, at: usize) -> Vec<String> {
    let rest = text.get(at..).unwrap_or("");
    let r = rest.trim_start();
    if !r.starts_with('<') {
        return vec![];
    }
    let mut d = 0;
    let mut end = r.len();
    for (i, c) in r.char_indices() {
        match c {
            '<' => d += 1,
            '>' if i > 0 && r.as_bytes()[i - 1] != b'-' => {
                d -= 1;
                if d == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    targs(&format!("X{}>", &r[..end]))
        .into_iter()
        .filter(|p| !p.starts_with('\''))
        .map(|p| p.trim_start_matches("const ").split([':', '=', ' ']).next().unwrap_or("").to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// The parameters that take a closure in a fn signature `fn m<B, F: FnMut(T) -> B>(self, x: u8, f: F) -> R where F: ..`
/// (parameter index (excluding self), type parameter name, the closure's return shape)
fn fn_bounds(sig: &str) -> Vec<(u8, String, String, String)> {
    // (`X: Fn*(..) -> R` / `X: Fn*(..)` in the type parameters `<..>` and in where: (X, return, parameters))
    let mut bounds: Vec<(String, String, String)> = vec![];
    let fn_ret = |b: &str| -> Option<(String, String)> {
        let b = b.trim().trim_start_matches("[const]").trim_start_matches("~const").trim();
        let rest = ["FnOnce", "FnMut", "Fn"].iter().find_map(|k| b.strip_prefix(k))?;
        let rest = rest.trim_start();
        if !rest.starts_with('(') {
            return None;
        }
        // (the `-> R` after the matching `)`)
        let mut d = 0;
        let mut e = None;
        for (i, c) in rest.char_indices() {
            match c {
                '(' | '<' | '[' => d += 1,
                ')' | ']' => {
                    d -= 1;
                    if d == 0 {
                        e = Some(i + 1);
                        break;
                    }
                }
                '>' if i > 0 && rest.as_bytes()[i - 1] != b'-' => d -= 1,
                _ => {}
            }
        }
        // (unknown if not closed (a long signature cut off with `…`))
        let e = e?;
        let args = rest.get(1..e - 1)?.trim().to_string();
        let after = rest[e..].trim_start();
        Some((args, match after.strip_prefix("->") {
            Some(r) => {
                // (the return type runs up to the next `+` / `,` (at depth 0))
                let r = r.trim();
                let mut d = 0;
                let mut end = r.len();
                for (i, c) in r.char_indices() {
                    match c {
                        '<' | '(' | '[' => d += 1,
                        '>' if i > 0 && r.as_bytes()[i - 1] != b'-' => d -= 1,
                        ')' | ']' => d -= 1,
                        '+' | ',' if d == 0 => {
                            end = i;
                            break;
                        }
                        _ => {}
                    }
                }
                r[..end].trim().to_string()
            }
            None => "()".to_string(),
        }))
    };
    let split_top = |s: &str| -> Vec<String> {
        let mut out = vec![];
        let mut d = 0;
        let mut a = 0;
        let b = s.as_bytes();
        for i in 0..b.len() {
            match b[i] {
                b'<' | b'(' | b'[' => d += 1,
                b'>' if i > 0 && b[i - 1] != b'-' => d -= 1,
                b')' | b']' => d -= 1,
                b',' if d == 0 => {
                    out.push(s[a..i].trim().to_string());
                    a = i + 1;
                }
                _ => {}
            }
        }
        if a < s.len() && !s[a..].trim().is_empty() {
            out.push(s[a..].trim().to_string());
        }
        out
    };
    let Some(name_end) = sig.find(|c: char| c == '<' || c == '(') else { return vec![] };
    let mut rest = &sig[name_end..];
    let mut gnames: Vec<String> = vec![];
    if rest.starts_with('<') {
        let g = targs(&format!("X{rest}"));
        for p in &g {
            let n = p.split([':', '=']).next().unwrap_or("").trim();
            if !n.is_empty() && !n.starts_with('\'') && !n.starts_with("const ") {
                gnames.push(n.to_string());
            }
        }
        for p in g {
            if let Some((n, b)) = p.split_once(':') {
                for part in b.split('+') {
                    if let Some((a, r)) = fn_ret(part) {
                        bounds.push((n.trim().to_string(), r, a));
                    }
                }
            }
        }
        // (the `(` after `>`)
        let mut d = 0;
        let mut cut = rest.len();
        for (i, c) in rest.char_indices() {
            match c {
                '<' => d += 1,
                '>' if i > 0 && rest.as_bytes()[i - 1] != b'-' => {
                    d -= 1;
                    if d == 0 {
                        cut = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = rest[cut..].trim_start();
    }
    if let Some(w) = sig.rfind(" where ") {
        for p in split_top(&sig[w + 7..]) {
            if let Some((n, b)) = p.split_once(':') {
                for part in b.split('+') {
                    if let Some((a, r)) = fn_ret(part) {
                        bounds.push((n.trim().to_string(), r, a));
                    }
                }
            }
        }
    }
    // (the parameter list)
    let Some(inner) = rest.strip_prefix('(') else { return vec![] };
    let mut d = 0;
    let mut e = inner.len();
    for (i, c) in inner.char_indices() {
        match c {
            '(' | '[' | '<' => d += 1,
            '>' if i > 0 && inner.as_bytes()[i - 1] != b'-' => d -= 1,
            ']' => d -= 1,
            ')' => {
                if d == 0 {
                    e = i;
                    break;
                }
                d -= 1;
            }
            _ => {}
        }
    }
    let mut out = vec![];
    let mut idx = 0u8;
    for p in split_top(&inner[..e]) {
        let Some((_, ty)) = p.split_once(':') else {
            // (`self` / `&self` / `mut self`)
            continue;
        };
        if p.trim_start().trim_start_matches('&').trim_start_matches("mut ").trim_start().starts_with("self") && !p.contains("self:") {
            continue;
        }
        if p.trim_start().starts_with("self:") || p.trim_start().starts_with("mut self:") {
            continue;
        }
        let ty = ty.trim();
        if let Some((a, r)) = ty.strip_prefix("impl ").and_then(|b| b.split('+').find_map(|x| fn_ret(x))) {
            out.push((idx, String::new(), r, a));
        } else if let Some((_, r, a)) = bounds.iter().find(|(n, _, _)| n == ty.trim_start_matches("&mut ").trim_start_matches('&')) {
            out.push((idx, ty.to_string(), r.clone(), a.clone()));
        } else if gnames.iter().any(|g| g == ty) || (ty.len() == 1 && ty.starts_with(|c: char| c.is_ascii_uppercase())) {
            // (a parameter whose type is a type parameter itself, like `other: U`: decided by the type of the call's argument expression. The mark is "=")
            out.push((idx, ty.to_string(), "=".to_string(), String::new()));
        } else if let Some(t) = ty.strip_prefix("&mut ").or_else(|| ty.strip_prefix('&')).map(|t| t.trim()).filter(|t| gnames.iter().any(|g| g == t)) {
            // (`dest: &mut T` (`mem::take`): the type behind the argument's reference. A reference mark in the 4th)
            out.push((idx, t.to_string(), "=".to_string(), "&".to_string()));
        }
        idx += 1;
    }
    out
}

/// The name of the self type (`S`) of `impl Tr for S<T>` / `impl S`
fn impl_self(name: &str) -> String {
    let n = name.strip_prefix("impl ").unwrap_or(name);
    let n = match n.rfind(" for ") {
        Some(i) => &n[i + 5..],
        None => n,
    };
    let n = n.trim_start_matches('&').trim_start_matches("mut ").trim();
    let n = n.split('<').next().unwrap_or(n);
    n.rsplit("::").next().unwrap_or(n).trim().to_string()
}

impl FileIndex {
    pub fn new(text: String, module: Vec<String>) -> FileIndex {
        let toks: Vec<(u8, u32, u32)> = tokenize(text.as_bytes()).into_iter().map(|(a, b, c)| (a, b as u32, c as u32)).collect();
        let syms = document_symbols(&text);
        let mut defs = vec![];
        // (the containers and module paths are shared: since one is held for each definition of libraries and large workspaces)
        fn walk(text: &str, v: &[Symbol], owner: &str, container: &Arc<str>, module: &Arc<[String]>, outer: &[String], out: &mut Vec<Def>) {
            for s in v {
                // (an `extern "C" { .. }` block: the declarations inside are items of this level as they are)
                if s.kind == SymKind::Extern {
                    walk(text, &s.children, owner, container, module, outer, out);
                    continue;
                }
                // (type parameter names: a type alias's own, a struct / enum / union's own, and for a field / variant those of the owner type)
                let params = match s.kind {
                    SymKind::TypeAlias => alias_parts(&s.detail).0,
                    SymKind::Struct | SymKind::Enum | SymKind::Union => generic_names(text, s.name_end),
                    SymKind::Field | SymKind::Variant => outer.to_vec(),
                    _ => vec![],
                };
                {
                    out.push(Def {
                        name: s.name.clone(),
                        kind: s.kind,
                        owner: if s.kind == SymKind::Impl { String::new() } else { owner.to_string() },
                        container: container.clone(),
                        module: module.clone(),
                        start: s.name_start,
                        end: s.name_end,
                        item_start: s.start,
                        item_end: s.end,
                        ty: match s.kind {
                            SymKind::Field | SymKind::Const | SymKind::Static => s.detail.trim().to_string(),
                            SymKind::TypeAlias => alias_parts(&s.detail).1,
                            SymKind::Function | SymKind::Method => {
                                let t = ret_type(&s.detail);
                                // (Self inside an impl is the impl's self type)
                                if container.starts_with("impl") { subst(&t, &["Self".to_string()], &[impl_self_full(&container[..]).to_string()]) } else { t }
                            }
                            SymKind::Variant => owner.to_string(),
                            _ => String::new(),
                        },
                        params: params.clone(),
                        fnret: if matches!(s.kind, SymKind::Function | SymKind::Method) {
                            fn_bounds(&s.detail)
                        } else if s.kind == SymKind::Struct {
                            // (a tuple struct `struct P<'a>(A, B);`: the way its fields are written as (255, "", "A, B", ""))
                            let after = text.get(s.name_end..s.end).unwrap_or("");
                            let a = after.trim_start();
                            let a = if a.starts_with('<') { close_angle(a).map(|c| &a[c + 1..]).unwrap_or("") } else { a };
                            match a.trim_start().strip_prefix('(').and_then(|r| Some(&r[..r.rfind(')')?])) {
                                Some(inner) => vec![(255, String::new(), inner.to_string(), String::new())],
                                None => vec![],
                            }
                        } else if s.kind == SymKind::Macro {
                            // (a hint at the result type of a macro_rules! (so that it can be used even in libraries whose text was dropped): if most of the body is
                            // `$crate::T::..` (`json_internal!` → Value), T; if the body is only another macro `$crate::m!(..)`, "!m")
                            match macro_hint(text.get(s.start..s.end).unwrap_or("")) {
                                Some(h) => vec![(255, String::new(), h, String::new())],
                                None => vec![],
                            }
                        } else if s.kind == SymKind::Variant {
                            // (a tuple variant `V(A, B)`: the way its fields are written goes into (255, "", "A, B", ""), so that even in libraries whose text was dropped
                            // the types of a pattern's fields are known)
                            match text.get(s.start..s.end).filter(|_| text.get(s.name_end..).is_some_and(|r| r.trim_start().starts_with('('))).and_then(|v| Some((v.find('(')?, v.rfind(')')?))).filter(|(a, b)| a < b) {
                                Some((a, b)) => vec![(255, String::new(), text[s.start + a + 1..s.start + b].to_string(), String::new())],
                                None => vec![],
                            }
                        } else {
                            vec![]
                        },
                        has_self: s.kind == SymKind::Method && crate::symbols::takes_self(&s.detail),
                        pos: [0; 4],
                    });
                }
                if s.children.is_empty() {
                    continue;
                }
                let c: Arc<str> = Arc::from(s.name.as_str());
                match s.kind {
                    SymKind::Impl => walk(text, &s.children, &impl_self(&s.name), &c, module, &[], out),
                    SymKind::Module => {
                        let mut m = module.to_vec();
                        m.push(s.name.clone());
                        walk(text, &s.children, "", &c, &Arc::from(m), &[], out);
                    }
                    // (items in a fn body: items without an owner)
                    SymKind::Function | SymKind::Method => walk(text, &s.children, "", &c, module, &[], out),
                    _ => walk(text, &s.children, &s.name, &c, module, &params, out),
                }
            }
        }
        walk(&text, &syms, "", &Arc::from(""), &Arc::from(module.clone()), &[], &mut defs);
        let mut idents: FxMap<String, Vec<u32>> = FxMap::default();
        for (i, t) in toks.iter().enumerate() {
            if t.0 == 0 {
                idents.entry(text[t.1 as usize..t.2 as usize].to_string()).or_default().push(i as u32);
            }
        }
        let mut f = FileIndex { text, lib: false, in_src: true, krate: 0, module, defs, toks, idents, uses: vec![], use_at: vec![], macros: vec![], use_syms: vec![], name_ix: vec![], owner_ix: vec![], slim: false, names: vec![], impls: vec![], derives: vec![], inactive: vec![], features: None };
        (f.uses, f.use_at) = f.collect_uses(false).into_iter().map(|(a, b, c)| ((a, b), c)).unzip();
        f.use_syms = f.collect_use_syms();
        f.macros = f.macro_bodies();
        f.index_names();
        f.derives = f.collect_derives();
        f.impls = f.collect_impls();
        f
    }

    /// Rebuilds name_ix (when defs changed)
    fn index_names(&mut self) {
        let mut v: Vec<(u64, u32)> = self.defs.iter().enumerate().map(|(i, d)| (name_hash(&d.name), i as u32)).collect();
        v.sort_unstable();
        v.shrink_to_fit();
        self.name_ix = v;
        let mut o: Vec<(u64, u32)> = self.defs.iter().enumerate().filter(|(_, d)| !d.owner.is_empty()).map(|(i, d)| (name_hash(&d.owner), i as u32)).collect();
        o.sort_unstable();
        o.shrink_to_fit();
        self.owner_ix = o;
    }

    /// The defs indices of the definitions whose owner is owner
    fn defs_owned<'a>(&'a self, owner: &'a str) -> impl Iterator<Item = usize> + 'a {
        let h = name_hash(owner);
        let a = self.owner_ix.partition_point(|x| x.0 < h);
        self.owner_ix[a..].iter().take_while(move |x| x.0 == h).map(|x| x.1 as usize).filter(move |&i| self.defs[i].owner == owner)
    }

    /// The defs indices of the definitions named name
    fn defs_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = usize> + 'a {
        let h = name_hash(name);
        let a = self.name_ix.partition_point(|x| x.0 < h);
        self.name_ix[a..].iter().take_while(move |x| x.0 == h).map(|x| x.1 as usize).filter(move |&i| self.defs[i].name == name)
    }

    /// The T of the struct expression `T { .. }` enclosing token k
    fn struct_lit_at(&self, k: usize) -> Option<String> {
        let mut d = 0;
        let mut j = k;
        while j > 0 {
            j -= 1;
            if self.sym(j, b'}') || self.sym(j, b')') || self.sym(j, b']') {
                d += 1;
            } else if self.sym(j, b'(') || self.sym(j, b'[') {
                if d == 0 {
                    return None;
                }
                d -= 1;
            } else if self.sym(j, b'{') {
                if d == 0 {
                    // (the bodies of declarations `enum E {` / `struct S {` / `impl T {` / `trait T {` / `impl X for T {` are not struct expressions)
                    let decl = j >= 2 && self.ident(j - 2) && matches!(self.tok(j - 2), "enum" | "struct" | "union" | "trait" | "impl" | "for" | "mod" | "dyn");
                    return (j >= 1 && !decl && self.ident(j - 1) && self.tok(j - 1).starts_with(|c: char| c.is_ascii_uppercase())).then(|| self.tok(j - 1).to_string());
                }
                d -= 1;
            }
        }
        None
    }

    fn collect_impls(&self) -> Vec<(String, String)> {
        let mut out = vec![];
        for d in &self.defs {
            if d.kind == SymKind::Impl {
                let n = d.name.strip_prefix("impl ").unwrap_or(&d.name);
                if let Some(i) = n.rfind(" for ") {
                    let s = type_head(&n[i + 5..]);
                    let t = type_head(n[..i].trim_start_matches('!'));
                    // (an impl for everything (blanket) like `impl<T: X> Tr for T` goes to "*": Tr's methods go into the members of every type)
                    if !s.is_empty() && !t.is_empty() {
                        let blanket = s.len() == 1 && s.starts_with(|c: char| c.is_ascii_uppercase());
                        out.push((if blanket { "*".to_string() } else { s }, t));
                    }
                }
            }
        }
        for (t, tr, _) in &self.derives {
            out.push((t.clone(), tr.clone()));
        }
        out
    }

    /// The struct / enum / union after `#[derive(A, B)]`, and the positions of the derive names
    fn collect_derives(&self) -> Vec<(String, String, u32)> {
        let mut out = vec![];
        let n = self.toks.len();
        let mut k = 0;
        while k < n {
            if self.ident(k) && self.tok(k) == "derive" && self.sym(k + 1, b'(') && k >= 2 && self.sym(k - 1, b'[') && !self.in_inactive(self.toks[k].1 as usize) {
                let mut j = k + 2;
                let mut names = vec![];
                while j < n && !self.sym(j, b')') {
                    if self.ident(j) && !self.sym(j + 1, b':') {
                        names.push((self.tok(j).to_string(), self.toks[j].1));
                    }
                    j += 1;
                }
                // (the name of the next item: skipping attributes and pub)
                let mut q = j + 1;
                while q < n && !(self.ident(q) && matches!(self.tok(q), "struct" | "enum" | "union")) && q < j + 64 {
                    q += 1;
                }
                if q + 1 < n && self.ident(q + 1) {
                    let s = self.tok(q + 1).to_string();
                    for (t, at) in names {
                        out.push((s.clone(), t, at));
                    }
                }
                k = j;
            }
            k += 1;
        }
        out
    }

    /// A library file: only definitions and uses (the text, token list and name table are dropped; positions are turned into lines and columns first)
    pub fn library(text: String, module: Vec<String>) -> FileIndex {
        let mut f = FileIndex::new(text, module);
        f.lib = true;
        // (only `pub use` (re-exports) is visible from other files. Collected before the token list is dropped)
        f.uses = f.collect_uses(true).into_iter().map(|(a, b, _)| (a, b)).collect();
        f.toks = vec![];
        f.idents = FxMap::default();
        f.make_slim();
        f.names = vec![];
        f
    }

    /// Drops the text, token list and name table (definition positions are turned into lines and columns; of the names only a set of hashes is kept)
    pub fn make_slim(&mut self) {
        if self.slim {
            return;
        }
        let mut names: Vec<u64> = self.idents.keys().map(|k| name_hash(k)).collect();
        names.sort_unstable();
        names.dedup();
        self.names = names;
        let li = crate::symbols::LineIndex::new(&self.text);
        for d in self.defs.iter_mut() {
            let (a, b) = (li.pos(&self.text, d.start), li.pos(&self.text, d.end));
            d.pos = [a.0, a.1, b.0, b.1];
        }
        self.text = String::new();
        self.toks = vec![];
        self.idents = FxMap::default();
        self.macros = vec![];
        self.use_at = vec![];
        self.uses.shrink_to_fit();
        self.defs.shrink_to_fit();
        self.slim = true;
    }

    /// One holding only the text of a library file (used by hover / signatureHelp to read signatures and doc comments; the definitions
    /// of the original index are used, so neither a token list nor definitions are made)
    pub fn text_only(&self, text: String) -> FileIndex {
        FileIndex { text, lib: self.lib, in_src: self.in_src, krate: self.krate, module: self.module.clone(), defs: vec![], toks: vec![], idents: FxMap::default(), uses: vec![], use_at: vec![], macros: vec![], use_syms: vec![], name_ix: vec![], owner_ix: vec![], slim: false, names: vec![], impls: vec![], derives: vec![], inactive: vec![], features: None }
    }

    /// A slim file rebuilt from its text (same module and crate)
    pub fn reload(&self, text: String) -> FileIndex {
        let mut f = FileIndex::new(text, self.module.clone());
        f.lib = self.lib;
        f.in_src = self.in_src;
        f.krate = self.krate;
        if let Some(fs) = &self.features {
            f.apply_cfg(fs.clone());
        }
        f
    }

    /// Removes the items excluded by `#[cfg(..)]` (features: the features enabled in that crate). Called before making it slim.
    /// Understood are feature = ".." / test / debug_assertions / unix / windows / target_os / target_arch etc. and all / any / not.
    /// Other cfgs are taken as false (as in rustc)
    pub fn apply_cfg(&mut self, features: Vec<String>) {
        if self.toks.is_empty() && !self.text.is_empty() {
            return;
        }
        let mut inactive: Vec<(usize, usize)> = vec![];
        for d in &self.defs {
            if inactive.iter().any(|&(a, b)| a <= d.item_start && d.item_end <= b) {
                continue;
            }
            // (the attributes from the start of the item to the name)
            let a = self.toks.partition_point(|t| (t.1 as usize) < d.item_start);
            let e = self.toks.partition_point(|t| (t.1 as usize) < d.start);
            let mut k = a;
            while k + 3 < e {
                if self.sym(k, b'#') && self.sym(k + 1, b'[') && self.tok(k + 2) == "cfg" && self.sym(k + 3, b'(') {
                    let mut j = k + 4;
                    if !self.cfg_pred(&mut j, &features) {
                        inactive.push((d.item_start, d.item_end));
                        break;
                    }
                }
                k += 1;
            }
        }
        self.inactive = inactive;
        if !self.inactive.is_empty() {
            let ina = std::mem::take(&mut self.inactive);
            self.defs.retain(|d| !ina.iter().any(|&(a, b)| a <= d.item_start && d.item_end <= b));
            self.index_names();
            self.inactive = ina;
            self.derives = self.collect_derives();
            self.impls = self.collect_impls();
        }
        self.features = Some(features);
    }

    /// A cfg predicate (from token *j; advances *j past what was read)
    fn cfg_pred(&self, j: &mut usize, fs: &[String]) -> bool {
        if !self.ident(*j) {
            return true;
        }
        let w = self.tok(*j).to_string();
        *j += 1;
        if self.sym(*j, b'(') {
            *j += 1;
            let mut vals = vec![];
            while *j < self.toks.len() && !self.sym(*j, b')') {
                vals.push(self.cfg_pred(j, fs));
                if self.sym(*j, b',') {
                    *j += 1;
                }
            }
            *j += 1;
            return match w.as_str() {
                "all" => vals.iter().all(|&v| v),
                "any" => vals.iter().any(|&v| v),
                "not" => !vals.first().copied().unwrap_or(true),
                _ => true,
            };
        }
        if self.sym(*j, b'=') && *j + 1 < self.toks.len() && self.toks[*j + 1].0 == 2 {
            let v = self.tok(*j + 1).trim_matches('"').to_string();
            *j += 2;
            return match w.as_str() {
                "feature" => fs.contains(&v),
                "target_os" => v == std::env::consts::OS,
                "target_family" => v == std::env::consts::FAMILY,
                "target_arch" => v == std::env::consts::ARCH,
                "target_pointer_width" => v == (usize::BITS).to_string(),
                "target_endian" => v == if cfg!(target_endian = "little") { "little" } else { "big" },
                "target_env" => v == if cfg!(target_env = "gnu") { "gnu" } else if cfg!(target_env = "musl") { "musl" } else if cfg!(target_env = "msvc") { "msvc" } else { "" },
                "target_vendor" => v == if cfg!(target_vendor = "apple") { "apple" } else if cfg!(target_vendor = "pc") { "pc" } else { "unknown" },
                "target_has_atomic" => matches!(v.as_str(), "8" | "16" | "32" | "64" | "ptr"),
                "panic" => v == "unwind",
                // (as in rustc, a cfg that is not set is false)
                _ => false,
            };
        }
        match w.as_str() {
            "unix" => cfg!(unix),
            "windows" => cfg!(windows),
            // (rust-analyzer sees the workspace crates with test and debug_assertions on. `miri` too
            // (the default of rust-analyzer's setting `cargo.cfgs` is `["debug_assertions", "miri"]`))
            "test" | "debug_assertions" | "miri" => true,
            // (cfgs that are not set: `no_global_oom_handling` / `doc` / `bootstrap` etc.)
            _ => false,
        }
    }

    fn in_inactive(&self, at: usize) -> bool {
        self.inactive.iter().any(|&(a, b)| a <= at && at < b)
    }

    /// Whether token k is a name in a binding pattern (`let (a, b) =` / `for (i, x) in` / `if let Some(x) =` / `|a, b|`)
    fn pat_binding(&self, k: usize) -> bool {
        let w = self.tok(k);
        if !w.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') || PRIMS.contains(&w) || matches!(w, "self" | "mut" | "ref") {
            return false;
        }
        if self.sym(k + 1, b'(') || self.sym(k + 1, b'!') || (self.sym(k + 1, b':') && self.sym(k + 2, b':')) || self.after_path(k) || (k > 0 && self.sym(k - 1, b'.')) {
            return false;
        }
        let mut j = k;
        for _ in 0..64 {
            if j == 0 {
                return false;
            }
            j -= 1;
            if self.ident(j) {
                match self.tok(j) {
                    "let" | "for" => return true,
                    "in" | "if" | "match" | "return" | "else" | "while" | "fn" | "move" | "as" => return false,
                    _ => continue,
                }
            }
            if self.sym(j, b'|') {
                return self.closure_start(j) && !(j > 0 && self.sym(j - 1, b'|'));
            }
            if self.toks[j].0 == 1 && matches!(self.text.as_bytes()[self.toks[j].1 as usize], b',' | b'(' | b')' | b'[' | b']' | b'&' | b'{' | b'}' | b'@') {
                continue;
            }
            // (the `::` of `Foo::Bar(x)`, and the `:` of a closure parameter's type `|a: T, b|`)
            if self.sym(j, b':') {
                continue;
            }
            return false;
        }
        false
    }

    /// Whether token k is the name of a fn parameter (`fn f(a: T, mut b: U)`)
    fn fn_param(&self, k: usize) -> bool {
        if !(self.sym(k + 1, b':') && !self.sym(k + 2, b':')) || k == 0 || !(self.sym(k - 1, b'(') || self.sym(k - 1, b',') || self.tok(k - 1) == "mut") {
            return false;
        }
        let Some(f) = self.enclosing_fn(self.toks[k].1 as usize) else { return false };
        // (before the body's `{`)
        let mut j = self.toks.partition_point(|t| (t.1 as usize) < f.start);
        let mut d = 0;
        while j < self.toks.len() && j < k {
            if self.sym(j, b'(') || self.sym(j, b'[') {
                d += 1;
            } else if self.sym(j, b')') || self.sym(j, b']') {
                d -= 1;
            } else if d == 0 && self.sym(j, b'{') {
                return false;
            }
            j += 1;
        }
        true
    }

    /// If byte position at is on a name inside `format!("{name}")`: (the literal's token, the token of the local binding of that name, the start of the name)
    fn capture_at(&self, at: usize) -> Option<(usize, usize, usize)> {
        let i = self.toks.partition_point(|t| (t.2 as usize) <= at);
        let t = *self.toks.get(i)?;
        if t.0 != 2 || (t.1 as usize) > at || !self.text[t.1 as usize..].starts_with('"') {
            return None;
        }
        let lit = &self.text[t.1 as usize..t.2 as usize];
        let (a, name) = captures(lit).into_iter().map(|(a, n)| (t.1 as usize + a, n)).find(|&(a, n)| a <= at && at <= a + n.len())?;
        let b = self.local(i, name)?;
        Some((i, b, a))
    }

    /// The opening bracket matching closing bracket p
    fn open_of(&self, p: usize) -> Option<usize> {
        let mut d = 0i32;
        let mut j = p;
        loop {
            if self.toks[j].0 == 1 {
                match self.text.as_bytes()[self.toks[j].1 as usize] {
                    b')' | b']' | b'}' => d += 1,
                    b'(' | b'[' | b'{' => {
                        d -= 1;
                        if d == 0 {
                            return Some(j);
                        }
                    }
                    _ => {}
                }
            }
            if j == 0 {
                return None;
            }
            j -= 1;
        }
    }

    /// The token range of the pattern of a match arm's `=>` (token arrow is the `=`) (excluding a guard `if ..`)
    fn arm_pat(&self, arrow: usize) -> (usize, usize) {
        let mut j = arrow;
        let mut start = 0;
        while j > 0 {
            let p = j - 1;
            if self.toks[p].0 == 1 {
                match self.text.as_bytes()[self.toks[p].1 as usize] {
                    b',' | b';' | b'{' | b'(' | b'[' => {
                        start = j;
                        break;
                    }
                    b'>' if p >= 1 && self.sym(p - 1, b'=') => {
                        start = j;
                        break;
                    }
                    b')' | b']' | b'}' => {
                        let Some(o) = self.open_of(p) else { break };
                        // (a `}` is either a struct pattern `Path { .. }` or the block body of the previous arm)
                        if self.sym(p, b'}') && !(o >= 1 && self.ident(o - 1) && self.tok(o - 1).starts_with(|c: char| c.is_ascii_uppercase())) {
                            start = j;
                            break;
                        }
                        j = o;
                        continue;
                    }
                    _ => {}
                }
            }
            j = p;
        }
        let mut end = arrow;
        let mut d = 0i32;
        for q in start..arrow {
            if self.sym(q, b'(') || self.sym(q, b'[') || self.sym(q, b'{') {
                d += 1;
            } else if self.sym(q, b')') || self.sym(q, b']') || self.sym(q, b'}') {
                d -= 1;
            } else if d == 0 && self.ident(q) && self.tok(q) == "if" {
                end = q;
                break;
            }
        }
        (start, end)
    }

    /// Whether token k is a binding in a match arm's pattern (`Some(x) =>` / `T { a, b: c } =>`)
    fn arm_binding(&self, k: usize) -> bool {
        let w = self.tok(k);
        if !w.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') || PRIMS.contains(&w) || matches!(w, "self" | "mut" | "ref" | "_") {
            return false;
        }
        if self.sym(k + 1, b'(') || self.sym(k + 1, b'!') || self.sym(k + 1, b'.') || (self.sym(k + 1, b':') && self.sym(k + 2, b':')) || self.after_path(k) || (k > 0 && self.sym(k - 1, b'.')) {
            return false;
        }
        if self.sym(k + 1, b':') {
            return false;
        }
        let n = self.toks.len();
        let mut j = k + 1;
        let mut d = 0i32;
        while j < n && j < k + 256 {
            if self.toks[j].0 == 1 {
                match self.text.as_bytes()[self.toks[j].1 as usize] {
                    b'(' | b'[' => d += 1,
                    b'{' => {
                        if d == 0 && !(j >= 1 && self.ident(j - 1) && self.tok(j - 1).starts_with(|c: char| c.is_ascii_uppercase())) {
                            return false;
                        }
                        d += 1;
                    }
                    b')' | b']' | b'}' => d -= 1,
                    b';' => return false,
                    b'=' if self.sym(j + 1, b'>') => {
                        let (a, e) = self.arm_pat(j);
                        return a <= k && k < e;
                    }
                    b'=' => return false,
                    _ => {}
                }
            }
            j += 1;
        }
        false
    }

    /// The token of name in the generics declarations of the items (fn / type / trait / impl) enclosing at (from the innermost)
    fn generic_param(&self, at: usize, name: &str) -> Option<usize> {
        let mut ds: Vec<&Def> = self
            .defs
            .iter()
            .filter(|d| matches!(d.kind, SymKind::Function | SymKind::Method | SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::Trait | SymKind::Impl | SymKind::TypeAlias) && d.item_start <= at && at < d.item_end)
            .collect();
        ds.sort_by_key(|d| d.item_end - d.item_start);
        let n = self.toks.len();
        for d in ds {
            // (the `<` is right after the name, or right after `impl` for an impl)
            let mut j = self.toks.partition_point(|t| (t.1 as usize) < d.start);
            if d.kind == SymKind::Impl {
                let mut q = self.toks.partition_point(|t| (t.1 as usize) < d.item_start);
                while q < j && self.tok(q) != "impl" {
                    q += 1;
                }
                j = q;
            }
            j += 1;
            if !self.sym(j, b'<') {
                continue;
            }
            let mut dd = 0;
            let mut sep = false;
            while j < n {
                if self.sym(j, b'<') {
                    dd += 1;
                    sep = dd == 1;
                } else if self.sym(j, b'>') && !self.sym(j - 1, b'-') {
                    dd -= 1;
                    if dd == 0 {
                        break;
                    }
                } else if dd == 1 && self.sym(j, b',') {
                    sep = true;
                } else if dd == 1 && sep && self.ident(j) && self.tok(j) == "const" {
                } else {
                    if dd == 1 && sep && self.ident(j) && self.tok(j) == name {
                        return Some(j);
                    }
                    sep = false;
                }
                j += 1;
            }
        }
        None
    }

    /// The trait names of the bounds of type parameter name of the items enclosing at (`<T: Clone + Debug>` and `where T: Hash`)
    fn param_bounds(&self, at: usize, name: &str) -> Vec<String> {
        let Some(j) = self.generic_param(at, name) else { return vec![] };
        let mut out = vec![];
        let n = self.toks.len();
        // (after `T:`: up to the `,` / `>` at depth 0. The head name of each trait separated by `+`)
        let take = |mut q: usize, out: &mut Vec<String>| {
            let mut d = 0;
            let mut first = true;
            while q < n {
                if self.sym(q, b'<') || self.sym(q, b'(') {
                    d += 1;
                } else if (self.sym(q, b'>') && !self.sym(q - 1, b'-')) || self.sym(q, b')') {
                    if d == 0 {
                        break;
                    }
                    d -= 1;
                } else if d == 0 && (self.sym(q, b',') || self.sym(q, b'{') || self.sym(q, b';') || self.sym(q, b'=')) {
                    break;
                } else if d == 0 && self.sym(q, b'+') {
                    first = true;
                } else if d == 0 && first && self.ident(q) && !self.before_path(q) && !matches!(self.tok(q), "dyn" | "const" | "for") && !(q >= 1 && self.sym(q - 1, b'?')) {
                    out.push(self.tok(q).to_string());
                    first = false;
                }
                q += 1;
            }
        };
        if self.sym(j + 1, b':') && !self.sym(j + 2, b':') {
            take(j + 2, &mut out);
        }
        // (the where of the enclosing item: before the body's `{`)
        for d in self.defs.iter().filter(|d| matches!(d.kind, SymKind::Function | SymKind::Method | SymKind::Impl | SymKind::Struct | SymKind::Enum | SymKind::Trait) && d.item_start <= at && at < d.item_end) {
            let mut q = self.toks.partition_point(|t| (t.1 as usize) < d.item_start);
            while q < n && (self.toks[q].1 as usize) < d.item_end && !self.sym(q, b'{') {
                if self.ident(q) && self.tok(q) == "where" {
                    let mut w = q + 1;
                    while w < n && !self.sym(w, b'{') && !self.sym(w, b';') {
                        if self.ident(w) && self.tok(w) == name && self.sym(w + 1, b':') && !self.sym(w + 2, b':') {
                            take(w + 2, &mut out);
                        }
                        w += 1;
                    }
                    break;
                }
                q += 1;
            }
        }
        out
    }

    /// The original name B of an alias C brought in by `use a::B as C`
    fn use_alias(&self, name: &str) -> Option<&str> {
        self.uses.iter().find(|u| u.0 == name && u.1.len() >= 2 && u.1.last().is_some_and(|l| l != name)).map(|u| u.1[u.1.len() - 1].as_str())
    }

    /// Whether an identifier name may occur (by the name table if not slim, by the hashes if slim)
    fn may_have(&self, name: &str) -> bool {
        if self.slim { self.names.binary_search(&name_hash(name)).is_ok() } else { self.idents.contains_key(name) }
    }

    /// An estimate of the heap size (bytes): (definitions, the text + token list + name table, uses and the rest)
    pub fn heap_bytes(&self) -> (usize, usize, usize) {
        let vs = |v: &Vec<String>| v.capacity() * 24 + v.iter().map(|x| x.capacity()).sum::<usize>();
        let defs = self.defs.capacity() * std::mem::size_of::<Def>()
            + self.defs.iter().map(|d| d.name.capacity() + d.owner.capacity() + d.ty.capacity() + vs(&d.params)).sum::<usize>();
        let body = self.text.capacity() + self.toks.capacity() * 12 + self.idents.iter().map(|(k, v)| k.capacity() + v.capacity() * 4 + 48).sum::<usize>();
        let other = self.uses.iter().map(|(a, b)| a.capacity() + vs(b) + 48).sum::<usize>()
            + self.impls.iter().map(|(a, b)| a.capacity() + b.capacity() + 48).sum::<usize>()
            + self.derives.iter().map(|(a, b, _)| a.capacity() + b.capacity() + 56).sum::<usize>()
            + self.names.capacity() * 8
            + self.module.capacity() * 24;
        (defs, body, other)
    }

    /// The size of the text (for the memory estimate)
    pub fn text_len(&self) -> usize {
        self.text.len()
    }

    /// (line, UTF-16 column) of an end of a definition name in a library file
    pub fn lib_pos(&self, at: usize) -> Option<(u32, u32)> {
        self.defs.iter().find_map(|d| {
            if d.start == at {
                Some((d.pos[0], d.pos[1]))
            } else if d.end == at {
                Some((d.pos[2], d.pos[3]))
            } else {
                None
            }
        })
    }

    fn tok(&self, k: usize) -> &str {
        let t = self.toks[k];
        &self.text[t.1 as usize..t.2 as usize]
    }
    fn sym(&self, k: usize, c: u8) -> bool {
        k < self.toks.len() && self.toks[k].0 == 1 && self.text.as_bytes()[self.toks[k].1 as usize] == c
    }
    fn ident(&self, k: usize) -> bool {
        k < self.toks.len() && self.toks[k].0 == 0
    }
    /// Whether k is preceded by `::`
    fn after_path(&self, k: usize) -> bool {
        k >= 2 && self.sym(k - 1, b':') && self.sym(k - 2, b':')
    }
    /// Whether k is followed by `::`
    fn before_path(&self, k: usize) -> bool {
        self.sym(k + 1, b':') && self.sym(k + 2, b':')
    }
    /// The identifier token at a byte position
    fn tok_at(&self, off: usize) -> Option<usize> {
        let i = self.toks.partition_point(|t| (t.2 as usize) < off);
        for k in [i, i + 1, i.wrapping_sub(1)] {
            if k < self.toks.len() && self.toks[k].0 == 0 && (self.toks[k].1 as usize) <= off && off <= (self.toks[k].2 as usize) {
                return Some(k);
            }
        }
        None
    }

    /// `use a::b::{C, D as E, f::*};` into a list of (name, path)
    fn macro_bodies(&self) -> Vec<(usize, usize)> {
        let mut out = vec![];
        let n = self.toks.len();
        for k in 0..n {
            if self.tok(k) == "macro_rules" && self.sym(k + 1, b'!') && self.ident(k + 2) && (self.sym(k + 3, b'{') || self.sym(k + 3, b'(')) {
                let mut d = 0i32;
                let mut j = k + 3;
                while j < n {
                    if self.toks[j].0 == 1 {
                        match self.text.as_bytes()[self.toks[j].1 as usize] {
                            b'{' | b'(' | b'[' => d += 1,
                            b'}' | b')' | b']' => {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    j += 1;
                }
                let end = if j < n { self.toks[j].2 as usize } else { self.text.len() };
                out.push(((self.toks[k + 3].1 as usize), end));
            }
        }
        out
    }

    /// Makes use_syms (uses with an alias, and pub uses; globs and `self` excluded)
    fn collect_use_syms(&self) -> Vec<(String, u32, u32, u32)> {
        let pubs: Vec<usize> = self.collect_uses(true).into_iter().map(|x| x.2).collect();
        let mut out = vec![];
        for ((local, path), &at) in self.uses.iter().zip(&self.use_at) {
            if at == usize::MAX || local == "self" || local == "_" {
                continue;
            }
            let alias = path.last().is_some_and(|l| l != local);
            if !(alias || pubs.contains(&at)) {
                continue;
            }
            // (for an alias, the position of the name after `as`)
            let a = if alias { self.text[at..].find(&format!(" {local}")).map(|i| at + i + 1).unwrap_or(at) } else { at };
            let ls = self.text[..a].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let line = self.text[..ls].matches('\n').count() as u32;
            out.push((local.clone(), line, self.text[ls..a].encode_utf16().count() as u32, local.encode_utf16().count() as u32));
        }
        out
    }

    fn collect_uses(&self, only_pub: bool) -> Vec<(String, Vec<String>, usize)> {
        let mut out = vec![];
        let n = self.toks.len();
        let mut k = 0;
        while k < n {
            if self.tok(k) == "use" && (!only_pub || (k >= 1 && self.tok(k - 1) == "pub")) && (k == 0 || self.sym(k - 1, b';') || self.sym(k - 1, b'}') || self.sym(k - 1, b'{') || self.sym(k - 1, b']') || self.tok(k - 1) == "pub" || self.sym(k - 1, b')')) {
                let mut e = k + 1;
                while e < n && !self.sym(e, b';') {
                    e += 1;
                }
                self.use_tree(k + 1, e, &mut vec![], &mut out);
                k = e;
            }
            k += 1;
        }
        out
    }
    fn use_tree(&self, a: usize, e: usize, prefix: &mut Vec<String>, out: &mut Vec<(String, Vec<String>, usize)>) {
        let mut path = prefix.clone();
        let mut k = a;
        while k < e {
            if self.ident(k) {
                let w = self.tok(k).to_string();
                if w == "as" && self.ident(k + 1) {
                    out.push((self.tok(k + 1).to_string(), path.clone(), (self.toks[k - 1].1 as usize)));
                    path = prefix.clone();
                    k += 2;
                    continue;
                }
                path.push(w);
                k += 1;
                if !(self.sym(k, b':') && self.sym(k + 1, b':')) && !(self.ident(k) && self.tok(k) == "as") {
                    out.push((path.last().cloned().unwrap_or_default(), path.clone(), (self.toks[k - 1].1 as usize)));
                    path = prefix.clone();
                }
            } else if self.sym(k, b'{') {
                // (the matching `}`)
                let mut d = 0;
                let mut c = k;
                while c < e {
                    if self.sym(c, b'{') {
                        d += 1;
                    } else if self.sym(c, b'}') {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    c += 1;
                }
                let mut p = path.clone();
                // (the inside is a comma-separated tree)
                let mut s = k + 1;
                let mut j = k + 1;
                let mut dd = 0;
                while j <= c {
                    if self.sym(j, b'{') {
                        dd += 1;
                    } else if self.sym(j, b'}') {
                        dd -= 1;
                    }
                    if (self.sym(j, b',') && dd == 0) || j == c {
                        self.use_tree(s, j, &mut p, out);
                        s = j + 1;
                    }
                    j += 1;
                }
                path = prefix.clone();
                k = c + 1;
            } else if self.sym(k, b'*') {
                out.push(("*".to_string(), path.clone(), usize::MAX));
                path = prefix.clone();
                k += 1;
            } else {
                k += 1;
            }
        }
    }

    /// The innermost fn definition containing off
    fn enclosing_fn(&self, off: usize) -> Option<&Def> {
        self.defs
            .iter()
            .filter(|d| matches!(d.kind, SymKind::Function | SymKind::Method) && d.item_start <= off && off < d.item_end)
            .min_by_key(|d| d.item_end - d.item_start)
    }
    /// The self type of the impl enclosing off (the owner of methods)
    fn enclosing_owner(&self, off: usize) -> Option<String> {
        self.enclosing_fn(off).filter(|d| !d.owner.is_empty()).map(|d| d.owner.clone())
    }
    /// The module enclosing off (including `mod x { .. }` in the file)
    fn module_at(&self, off: usize) -> Vec<String> {
        let mut m = self.module.clone();
        let mut best: Option<&Def> = None;
        for d in &self.defs {
            if d.kind == SymKind::Module && d.item_start <= off && off < d.item_end && best.is_none_or(|b| d.item_end - d.item_start < b.item_end - b.item_start) {
                best = Some(d);
            }
        }
        if let Some(b) = best {
            m = b.module.to_vec();
            m.push(b.name.clone());
        }
        m
    }

    /// The local binding (token index) of token k (named name). The nearest one in the fn before the cursor
    fn local(&self, k: usize, name: &str) -> Option<usize> {
        self.live(k).into_iter().rev().find(|&j| self.tok(j) == name && j != k)
    }

    /// The local bindings visible at the position of token k (token indices, from the outermost)
    fn live(&self, k: usize) -> Vec<usize> {
        let Some(f) = self.enclosing_fn(self.toks[k].1 as usize) else { return vec![] };
        // (from the token of the fn name)
        let mut i = self.toks.partition_point(|t| (t.1 as usize) < f.start);
        let is_bind = |j: usize| -> bool {
            let w = self.tok(j);
            self.ident(j)
                && w.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                && !matches!(w, "mut" | "ref" | "box" | "in" | "if" | "let" | "true" | "false" | "self" | "_")
                && !self.after_path(j)
                && !self.before_path(j)
                && !self.sym(j + 1, b'(')
                && !self.sym(j + 1, b'!')
                && !(j > 0 && self.sym(j - 1, b'.'))
        };
        // Live bindings: (name, token, the depth at which it goes away, whether it goes away at a comma, whether it goes away at ;)
        let mut live: Vec<(usize, i32, u8)> = vec![];
        // (the ones that become live in the next block: for / if let / while let)
        let mut pend_block: Vec<usize> = vec![];
        // (the ones that become live at the end of the statement `;`: let)
        let mut pend_let: Vec<(usize, i32)> = vec![];
        let mut depth = 0i32;
        // (fn parameters: the `x:` in the `(` .. `)` after the name)
        i += 1;
        if self.sym(i, b'<') {
            let mut d = 0;
            while i < k {
                if self.sym(i, b'<') {
                    d += 1;
                } else if self.sym(i, b'>') {
                    d -= 1;
                    if d == 0 {
                        i += 1;
                        break;
                    }
                }
                i += 1;
            }
        }
        if self.sym(i, b'(') {
            let mut d = 0;
            let mut seg_start = i + 1;
            while i < k {
                if self.sym(i, b'(') || self.sym(i, b'[') {
                    d += 1;
                } else if self.sym(i, b')') || self.sym(i, b']') {
                    d -= 1;
                    if d == 0 {
                        break;
                    }
                }
                if d == 1 && self.sym(i, b':') && !self.sym(i + 1, b':') && !(i > 0 && self.sym(i - 1, b':')) {
                    for j in seg_start..i {
                        if is_bind(j) {
                            pend_block.push(j);
                        }
                    }
                }
                if d == 1 && self.sym(i, b',') {
                    seg_start = i + 1;
                }
                i += 1;
            }
        }
        // (the body)
        let mut stmt_kind: u8 = 0; // 1: a let pattern, 2: a for / if let / while let pattern, 3: closure parameters
        let mut body_seen = false;
        let mut pat_start = 0usize;
        let mut arm_start = i;
        while i < k {
            let t = self.toks[i];
            if t.0 == 1 {
                match self.text.as_bytes()[t.1 as usize] {
                    b'{' => {
                        depth += 1;
                        body_seen = true;
                        for j in pend_block.drain(..) {
                            live.push((j, depth, 0));
                        }
                        arm_start = i + 1;
                        if stmt_kind == 2 {
                            stmt_kind = 0;
                        }
                    }
                    b'(' | b'[' => depth += 1,
                    b'}' | b')' | b']' => {
                        depth -= 1;
                        live.retain(|l| l.1 <= depth);
                        if self.sym(i, b'}') {
                            arm_start = i + 1;
                        }
                    }
                    b';' => {
                        live.retain(|l| !(l.2 == 2 && l.1 == depth));
                        // (a `let x;` without an initializer)
                        if stmt_kind == 1 {
                            for j in pat_start..i {
                                if is_bind(j) {
                                    pend_let.push((j, depth));
                                }
                            }
                        }
                        for (j, d) in pend_let.drain(..) {
                            live.push((j, d, 0));
                        }
                        stmt_kind = 0;
                        arm_start = i + 1;
                    }
                    b',' => {
                        live.retain(|l| !(l.2 == 1 && l.1 == depth));
                        arm_start = i + 1;
                    }
                    b'=' if self.sym(i + 1, b'>') => {
                        // A match arm pattern (the x of `T { x: a, .. }` is a field name)
                        let (pa, pe) = self.arm_pat(i);
                        for j in pa.max(arm_start.min(pa))..pe {
                            // (the 2nd m of `(m, 0) | (0, m)` is the same binding as the 1st: to the 1st, as in rust-analyzer)
                            if is_bind(j) && !self.sym(j + 1, b'{') && !(self.sym(j + 1, b':') && !self.sym(j + 2, b':')) && !(pa..j).any(|q| self.ident(q) && self.tok(q) == self.tok(j)) {
                                live.push((j, depth, 1));
                            }
                        }
                        i += 1;
                    }
                    b'=' if !self.sym(i + 1, b'=') && !(i > 0 && matches!(self.text.as_bytes()[self.toks[i - 1].1 as usize], b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')) => {
                        if stmt_kind == 1 || stmt_kind == 2 {
                            for j in pat_start..i {
                                if is_bind(j) {
                                    if stmt_kind == 1 {
                                        pend_let.push((j, depth));
                                    } else {
                                        pend_block.push(j);
                                    }
                                }
                            }
                            if stmt_kind == 1 {
                                stmt_kind = 4; // (the initializer expression after the type annotation)
                            } else {
                                stmt_kind = 0;
                            }
                        }
                    }
                    b':' if stmt_kind == 1 && !self.sym(i + 1, b':') && !(i > 0 && self.sym(i - 1, b':')) => {
                        // The pattern is everything before the type annotation of `let x: T = ..`
                        for j in pat_start..i {
                            if is_bind(j) {
                                pend_let.push((j, depth));
                            }
                        }
                        stmt_kind = 4;
                    }
                    b'|' if stmt_kind != 3 && (i == 0 || !self.sym(i - 1, b'|')) && !self.sym(i + 1, b'|') && self.closure_start(i) => {
                        stmt_kind = 3;
                        pat_start = i + 1;
                    }
                    b'|' if stmt_kind == 3 => {
                        for j in pat_start..i {
                            if is_bind(j) && !(j > 0 && self.sym(j - 1, b':')) {
                                live.push((j, depth, 2));
                            }
                        }
                        stmt_kind = 0;
                    }
                    _ => {}
                }
            } else if t.0 == 0 {
                match self.tok(i) {
                    // A match arm guard `P(x) if x > 0 =>`: the pattern's bindings are visible from the guard
                    "if" => {
                        if let Some(arrow) = self.guard_arrow(i) {
                            let (pa, pe) = self.arm_pat(arrow);
                            if pe == i {
                                for j in pa..pe {
                                    if is_bind(j) && !self.sym(j + 1, b'{') && !(self.sym(j + 1, b':') && !self.sym(j + 2, b':')) {
                                        live.push((j, depth, 1));
                                    }
                                }
                            }
                        }
                    }
                    // (`matches!(EXPR, PAT if GUARD)`: the pattern's bindings are visible from the guard (they go away at `)`))
                    "matches" if self.sym(i + 1, b'!') && self.sym(i + 2, b'(') => {
                        let mut d = 0i32;
                        let mut comma = None;
                        let mut j = i + 3;
                        while j < k && j < i + 512 {
                            if self.sym(j, b'(') || self.sym(j, b'[') || self.sym(j, b'{') {
                                d += 1;
                            } else if self.sym(j, b')') || self.sym(j, b']') || self.sym(j, b'}') {
                                if d == 0 {
                                    break;
                                }
                                d -= 1;
                            } else if d == 0 && self.sym(j, b',') && comma.is_none() {
                                comma = Some(j);
                            } else if d == 0 && self.ident(j) && self.tok(j) == "if" {
                                if let Some(c) = comma {
                                    for q in c + 1..j {
                                        if is_bind(q) && !self.sym(q + 1, b'{') && !(self.sym(q + 1, b':') && !self.sym(q + 2, b':')) {
                                            live.push((q, depth + 1, 0));
                                        }
                                    }
                                }
                                break;
                            }
                            j += 1;
                        }
                    }
                    "let" => {
                        // (`if let` / `while let` become live in the next block)
                        let cond = i > 0 && matches!(self.tok(i - 1), "if" | "while");
                        stmt_kind = if cond { 2 } else { 1 };
                        pat_start = i + 1;
                    }
                    "for" if !self.sym(i + 1, b'<') => {
                        // `for PAT in`
                        let mut j = i + 1;
                        while j < k && self.tok(j) != "in" {
                            if is_bind(j) {
                                pend_block.push(j);
                            }
                            j += 1;
                        }
                        i = j;
                    }
                    _ => {}
                }
            }
            i += 1;
        }
        // (if still before the body's `{` (in the fn parameters), the parameters are visible. What is left inside the body is the bindings
        // of `if let` / `while let` / `for` patterns visible from the next block, so they are not visible yet (the x on the right of `if let Some(x) = (x, ..)` is the outer x))
        if !body_seen {
            live.extend(pend_block.iter().map(|&j| (j, depth, 0)));
        }
        live.into_iter().map(|l| l.0).collect()
    }

    /// If `if` (token i) is a match arm guard, the `=` of the `=>` after it (the one before a block `{` or a `;`)
    fn guard_arrow(&self, i: usize) -> Option<usize> {
        let mut d = 0i32;
        for j in i + 1..(i + 128).min(self.toks.len()) {
            if self.toks[j].0 != 1 {
                continue;
            }
            match self.text.as_bytes()[self.toks[j].1 as usize] {
                b'(' | b'[' => d += 1,
                b')' | b']' => {
                    d -= 1;
                    if d < 0 {
                        return None;
                    }
                }
                b'{' | b'}' | b';' => return None,
                b',' if d == 0 => return None,
                b'=' if d == 0 && self.sym(j + 1, b'>') => return Some(j),
                _ => {}
            }
        }
        None
    }

    /// Whether k is a name inside `#[derive(..)]`
    fn in_derive(&self, k: usize) -> bool {
        let mut j = k;
        while j > 0 && (self.ident(j - 1) || self.sym(j - 1, b',') || self.sym(j - 1, b':')) {
            j -= 1;
        }
        j >= 4 && self.sym(j - 1, b'(') && self.ident(j - 2) && self.tok(j - 2) == "derive" && self.sym(j - 3, b'[') && self.sym(j - 4, b'#')
    }

    /// Whether byte position at is inside a macro_rules! body (excluding the name)
    fn in_macro_body(&self, at: usize) -> bool {
        self.macros.iter().any(|&(a, b)| a <= at && at < b) || self.defs.iter().any(|d| d.kind == SymKind::Macro && d.item_start <= at && at < d.item_end && at != d.start)
    }

    /// Whether `|` starts closure parameters (preceded by a symbol that can start an expression, or by `move`)
    fn closure_start(&self, i: usize) -> bool {
        if i == 0 {
            return true;
        }
        let p = i - 1;
        if self.ident(p) {
            return self.tok(p) == "move" || self.tok(p) == "return";
        }
        // (a closure taken by reference `f(&|t: &str| ..)`)
        if self.sym(p, b'&') && p >= 1 && !self.sym(p - 1, b'&') {
            return self.closure_start(p);
        }
        self.toks[p].0 == 1 && matches!(self.text.as_bytes()[self.toks[p].1 as usize], b'(' | b',' | b'=' | b'{' | b';' | b'[' | b':')
    }

    /// The type name of local binding b (type annotation, a `T::..` / `T { .. }` / `T(..)` initializer, the parameter's type)
    fn local_type(&self, b: usize) -> Option<String> {
        let n = self.toks.len();
        let mut j = b + 1;
        if self.sym(j, b':') && !self.sym(j + 1, b':') {
            j += 1;
            // (skips `&` / `mut` / a lifetime `'a` (an identifier after `'` in the tokens))
            while j < n && (self.sym(j, b'&') || self.tok(j) == "mut" || self.toks[j].0 == 2 || (self.ident(j) && self.toks[j].1 >= 1 && self.text.as_bytes()[self.toks[j].1 as usize - 1] == b'\'')) {
                j += 1;
            }
            // (the last of `a::b::T<..>`)
            while self.ident(j) && self.before_path(j) {
                j += 3;
            }
            if self.ident(j) {
                return Some(self.tok(j).to_string());
            }
            return None;
        }
        if self.sym(j, b'=') && !self.sym(j + 1, b'=') {
            j += 1;
            while j < n && (self.sym(j, b'&') || self.tok(j) == "mut") {
                j += 1;
            }
            let up = |j: usize| self.tok(j).starts_with(|c: char| c.is_ascii_uppercase());
            // (skips the module part of `a::b::T`; stops at the T of `T::new`)
            while self.ident(j) && self.before_path(j) && self.ident(j + 3) && !(up(j) && !up(j + 3)) {
                j += 3;
            }
            // (`T::..` / `T { .. }` / `T(..)`, a unit struct `a::T;` (except all-uppercase constants))
            if self.ident(j) && up(j) && (self.before_path(j) || self.sym(j + 1, b'{') || self.sym(j + 1, b'(') || (self.sym(j + 1, b';') && (self.tok(j).len() == 1 || self.tok(j).contains(|c: char| c.is_ascii_lowercase())))) {
                return Some(self.tok(j).to_string());
            }
        }
        None
    }
}

fn loc(file: usize, d: &Def) -> Loc {
    Loc { file, start: d.start, end: d.end }
}

const ITEM_KINDS: [SymKind; 10] = [
    SymKind::Function,
    SymKind::Const,
    SymKind::Static,
    SymKind::Struct,
    SymKind::Enum,
    SymKind::Union,
    SymKind::Trait,
    SymKind::TypeAlias,
    SymKind::Macro,
    SymKind::Module,
];

/// For a method made by derive (`default` / `clone` / `eq` / `fmt` / ..), the position of the derive name of the owner type (one of owners)
fn derive_method(ws: &[&FileIndex], file: usize, owners: &[String], name: &str) -> Option<Loc> {
    let tr = match name {
        "default" => "Default",
        "clone" | "clone_from" => "Clone",
        "eq" | "ne" => "PartialEq",
        "fmt" => "Debug",
        "partial_cmp" | "lt" | "le" | "gt" | "ge" => "PartialOrd",
        "cmp" | "max" | "min" | "clamp" => "Ord",
        "hash" => "Hash",
        _ => return None,
    };
    let kr = ws[file].krate;
    ws.iter().enumerate().filter(|(i, w)| !w.lib && (*i == file || w.in_src || w.krate == kr)).find_map(|(i, w)| {
        w.derives.iter().find(|(t, x, _)| x == tr && owners.contains(t)).map(|&(_, _, at)| Loc { file: i, start: at as usize, end: at as usize + tr.len() })
    })
}

/// Answers if there is one candidate. If there are several, the one in the same file if there is exactly one such
fn pick(c: Vec<(usize, &Def)>, file: usize) -> Vec<Loc> {
    if c.len() == 1 {
        return vec![loc(c[0].0, c[0].1)];
    }
    let same: Vec<&(usize, &Def)> = c.iter().filter(|x| x.0 == file).collect();
    if same.len() == 1 {
        return vec![loc(same[0].0, same[0].1)];
    }
    vec![]
}

/// The workspace candidates if there are any, else the library candidates (those with a short module path first: rather than the ones inside, e.g. under `sys`,
/// the public ones)
fn prefer_ws(ws: &[&FileIndex], c: Vec<(usize, &Def)>, file: usize) -> Vec<Loc> {
    let (w, l): (Vec<(usize, &Def)>, Vec<(usize, &Def)>) = c.into_iter().partition(|x| !ws[x.0].lib);
    if !w.is_empty() {
        return pick(w, file);
    }
    if l.is_empty() {
        return vec![];
    }
    let score = lib_rank;
    let best = l.iter().map(|x| score(x.1)).min().unwrap();
    let l: Vec<(usize, &Def)> = l.into_iter().filter(|x| score(x.1) == best).collect();
    // (several methods of the same name in the same file: impls of a trait with different type arguments (`Extend<T>` / `Extend<&T>` for Vec).
    // Take the first one (usually the generic one))
    // (they may be in different files too: `impl From<String> for Arc<str>` (string.rs) / `impl<T> From<T> for Arc<T>` (sync.rs))
    if l.len() > 1 && l.iter().all(|x| x.1.kind == SymKind::Method && x.1.name == l[0].1.name && x.1.owner == l[0].1.owner) {
        return vec![loc(l[0].0, l[0].1)];
    }
    // (a trait method's declaration and the methods of impls of that trait (`impl<T: Display> ToString for T`): the declaration (as in rust-analyzer))
    if l.len() > 1 {
        if let Some(decl) = l.iter().find(|x| x.1.kind == SymKind::Method && !x.1.container.starts_with("impl") && *x.1.container == *x.1.owner) {
            if l.iter().all(|x| std::ptr::eq(x.1, decl.1) || impl_trait(&x.1.container) == Some(decl.1.owner.as_str())) {
                return vec![loc(decl.0, decl.1)];
            }
        }
    }
    pick(l, file)
}

/// The rank of a library definition (smaller first): the ones with a short module path. Those under `sys` / `os`, non-default ones like `nonpoison` etc. go later
fn lib_rank(d: &Def) -> usize {
    let bad = d.module.first().is_some_and(|m| m == "sys" || m == "os" || m.starts_with("sys_")) || d.module.iter().any(|m| m == "nonpoison" || m == "unix" || m == "windows" || m == "wasi");
    d.module.len() + if bad { 100 } else { 0 }
}

/// The path of a `use` (the module part without the last name) → (module path, whether it is a path inside the crate)
fn use_module(here: &[String], path: &[String]) -> (Vec<String>, bool) {
    match path.first().map(|s| s.as_str()) {
        Some("crate") => (path[1..].to_vec(), true),
        Some("self") => {
            let mut m = here.to_vec();
            m.extend(path[1..].iter().cloned());
            (m, true)
        }
        Some("super") => {
            let mut m = here.to_vec();
            let mut i = 0;
            while i < path.len() && path[i] == "super" {
                m.pop();
                i += 1;
            }
            m.extend(path[i..].iter().cloned());
            (m, true)
        }
        // (another crate `std::sync` / `zust_core::session`: strips the crate name)
        Some(_) => (path[1..].to_vec(), false),
        None => (vec![], true),
    }
}

/// Whether d's module matches target (smaller is better). If exact, only the same path. Otherwise the tail is the same, or it contains it in order
fn mod_score(d: &[String], target: &[String], exact: bool) -> Option<usize> {
    if d == target {
        return Some(0);
    }
    if exact {
        return None;
    }
    if d.ends_with(target) {
        return Some(1 + d.len() - target.len());
    }
    let mut it = d.iter();
    if target.iter().all(|t| it.any(|x| x == t)) {
        return Some(10 + d.len());
    }
    None
}

/// Narrows the candidates by the module of a `use` (the best matching ones)
fn by_use<'a>(cands: &[(usize, &'a Def)], target: &[String], exact: bool) -> Vec<(usize, &'a Def)> {
    let scored: Vec<(usize, (usize, &'a Def))> = cands.iter().filter_map(|c| mod_score(&c.1.module[..], target, exact).map(|s| (s, *c))).collect();
    let Some(best) = scored.iter().map(|x| x.0).min() else { return vec![] };
    scored.into_iter().filter(|x| x.0 == best).map(|x| x.1).collect()
}

/// The definition of the identifier at off (byte)
/// (the token of the header's type / trait name, the range of the item) of the impl / trait enclosing the `Self` at off (where rust-analyzer's `Self` goes)
fn self_owner(f: &FileIndex, off: usize) -> Option<(usize, usize, usize)> {
    let k = f.tok_at(off)?;
    if f.tok(k) != "Self" {
        return None;
    }
    let d = f.defs.iter().filter(|d| matches!(d.kind, SymKind::Impl | SymKind::Trait) && d.item_start <= off && off < d.item_end).min_by_key(|d| d.item_end - d.item_start)?;
    if d.kind == SymKind::Trait {
        return Some((f.toks.partition_point(|t| (t.1 as usize) < d.start), d.item_start, d.item_end));
    }
    // (the last identifier at depth 0 before the `{` / where of an impl header `impl<T> Tr for a::S<T> where .. {`)
    let mut j = f.toks.partition_point(|t| (t.1 as usize) < d.item_start);
    while j < f.toks.len() && f.tok(j) != "impl" {
        j += 1;
    }
    let mut dd = 0;
    let mut last = None;
    j += 1;
    while j < f.toks.len() && (f.toks[j].1 as usize) < d.item_end {
        if f.sym(j, b'<') {
            dd += 1;
        } else if f.sym(j, b'>') && !f.sym(j - 1, b'-') {
            dd -= 1;
        } else if dd == 0 && (f.sym(j, b'{') || f.tok(j) == "where") {
            break;
        } else if dd == 0 && f.ident(j) && !matches!(f.tok(j), "for" | "dyn" | "mut" | "const" | "unsafe") {
            last = Some(j);
        }
        j += 1;
    }
    Some((last?, d.item_start, d.item_end))
}

/// The references of `Self` inside an impl / trait: the header's name and the `Self`s inside it (as in rust-analyzer)
fn self_refs(ws: &[&FileIndex], file: usize, off: usize, include_decl: bool) -> Option<Vec<Loc>> {
    let f = ws[file];
    let (h, a, e) = self_owner(f, off)?;
    let mut out = vec![];
    if include_decl {
        out.push(Loc { file, start: f.toks[h].1 as usize, end: f.toks[h].2 as usize });
    }
    for &k in f.idents.get("Self").map(|v| v.as_slice()).unwrap_or(&[]) {
        let at = f.toks[k as usize].1 as usize;
        if a <= at && at < e && !f.in_macro_body(at) {
            out.push(Loc { file, start: at, end: f.toks[k as usize].2 as usize });
        }
    }
    Some(out)
}

/// If on the keyword of an item (`fn` / `struct` / `enum` / `trait` ..), the position of the name after it (rust-analyzer answers with that item even on
/// the keyword)
fn kw_to_name(f: &FileIndex, off: usize) -> usize {
    if let Some(k) = f.tok_at(off) {
        if matches!(f.tok(k), "fn" | "struct" | "enum" | "union" | "trait" | "type" | "const" | "static" | "mod") && f.ident(k + 1) {
            let a = f.toks[k + 1].1 as usize;
            if f.defs.iter().any(|d| d.start == a) {
                return a;
            }
        }
    }
    off
}

pub fn definition(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    let off = kw_to_name(ws[file], off);
    if let Some((h, _, _)) = self_owner(ws[file], off) {
        let f = ws[file];
        return vec![Loc { file, start: f.toks[h].1 as usize, end: f.toks[h].2 as usize }];
    }
    clear_memo();
    let d: Vec<Loc> = def_at(ws, file, off).into_iter().map(|l| mod_file(ws, l).unwrap_or(l)).collect();
    // (the x of the struct shorthand `T { x }`: both the local and the field (the local first, as in rust-analyzer))
    if let (Some(k), [local]) = (ws[file].tok_at(off), d.as_slice()) {
        if !ws[local.file].defs.iter().any(|x| x.start == local.start) {
            if let Some(&fl) = shorthand_field(ws, file, k).first() {
                return vec![*local, fl];
            }
        }
    }
    d
}

/// Where `recv.into()` goes (token k of into, the receiver type recv): the from of the `impl From<R> for T` of the expected type T (the annotation of `let x: T = ..`)
/// (the ones whose R matches the receiver type first)
fn into_from(ws: &[&FileIndex], file: usize, k: usize, recv: Option<&str>) -> Option<Loc> {
    let f = ws[file];
    // (goes back to the start of the statement for `let NAME: TYPE =`)
    let mut j = k;
    let mut d = 0;
    let mut eq = None;
    while j > 0 {
        j -= 1;
        if f.sym(j, b')') || f.sym(j, b']') {
            d += 1;
        } else if f.sym(j, b'(') || f.sym(j, b'[') {
            if d == 0 {
                return None;
            }
            d -= 1;
        } else if d == 0 && (f.sym(j, b';') || f.sym(j, b'{') || f.sym(j, b'}')) {
            return None;
        } else if d == 0 && f.sym(j, b'=') && !f.sym(j + 1, b'=') && !f.sym(j - 1, b'=') {
            eq = Some(j);
        } else if d == 0 && f.ident(j) && f.tok(j) == "let" {
            break;
        }
    }
    let eq = eq?;
    let c = (j + 1..eq).find(|&q| f.sym(q, b':') && !f.sym(q + 1, b':') && !f.sym(q - 1, b':'))?;
    let ty = type_head(&f.text[f.toks[c + 1].1 as usize..f.toks[eq - 1].2 as usize]);
    let rh = recv.map(type_head);
    let mut best: Option<(bool, Loc)> = None;
    for (i, d) in named(ws, file, "from") {
        if d.owner != ty || !d.container.starts_with("impl") {
            continue;
        }
        let Some(tr) = d.container.strip_prefix("impl ").and_then(|n| n.split(" for ").next()) else { continue };
        if type_head(tr) != "From" {
            continue;
        }
        let fits = rh.as_ref().is_some_and(|r| targs(tr).first().is_some_and(|a| type_head(a) == *r));
        if best.is_none_or(|b| fits && !b.0) {
            best = Some((fits, loc(i, d)));
        }
    }
    best.map(|b| b.1)
}

/// If token k is the x of a struct shorthand `T { x }` and refers to the local x, the definition of that field
fn shorthand_pair(ws: &[&FileIndex], file: usize, k: usize) -> Option<Loc> {
    let f = ws[file];
    let l = *def_at(ws, file, f.toks[k].1 as usize).first()?;
    if ws[l.file].defs.iter().any(|x| x.start == l.start) {
        return None;
    }
    shorthand_field(ws, file, k).first().copied()
}

/// The definition of `mod x;` (a file module) goes to the start of that file (as in rust-analyzer)
fn mod_file(ws: &[&FileIndex], l: Loc) -> Option<Loc> {
    let d = ws[l.file].defs.iter().find(|d| d.start == l.start && d.kind == SymKind::Module)?;
    let kr = ws[l.file].krate;
    let lib = ws[l.file].lib;
    let i = ws.iter().position(|w| (if lib { w.lib && w.krate == kr } else { !w.lib }) && w.module.len() == d.module.len() + 1 && w.module.starts_with(&d.module) && w.module.last() == Some(&d.name))?;
    Some(Loc { file: i, start: 0, end: 0 })
}

fn def_at(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    if ws.is_empty() {
        return vec![];
    }
    let f = ws[file];
    if let Some(k) = f.tok_at(off) {
        if f.in_inactive(f.toks[k].1 as usize) {
            return inactive_def_at(ws, file, k);
        }
    }
    def_at1(ws, file, off)
}

/// Token k in code excluded by cfg: as in rust-analyzer, resolves only paths to items visible from the outer scope
/// (`used()` / `P` / `sub::deep` / `crate::P`). No answer for locals, fields, methods, associated items (`P::new`) or
/// the names of excluded items
fn inactive_def_at(ws: &[&FileIndex], file: usize, k: usize) -> Vec<Loc> {
    let f = ws[file];
    let decl = k > 0 && f.ident(k - 1) && matches!(f.tok(k - 1), "fn" | "struct" | "enum" | "union" | "trait" | "type" | "const" | "static" | "mod" | "let" | "mut");
    if decl || (k > 0 && f.sym(k - 1, b'.')) || (f.sym(k + 1, b':') && !f.sym(k + 2, b':') && f.struct_lit_at(k).is_some()) {
        return vec![];
    }
    // (if the T of `T::name` is a type (starts with an uppercase letter, or Self), an associated item)
    if f.after_path(k) && k >= 3 && (f.tok(k - 3) == "Self" || f.tok(k - 3).starts_with(|c: char| c.is_ascii_uppercase())) {
        return vec![];
    }
    let r = def_at1(ws, file, f.toks[k].1 as usize);
    r.into_iter()
        .filter(|l| ws[l.file].defs.iter().any(|d| d.start == l.start && (d.owner.is_empty() || d.kind == SymKind::Variant) && d.kind != SymKind::Field) || (l.start == 0 && l.end == 0))
        .collect()
}

/// Whether token k is in a cfg predicate (`#[cfg(..)]` / the pred of `#[cfg_attr(pred, ..)]` / `cfg!(..)`). Also nested `any` / `all` / `not`
fn in_cfg_pred(f: &FileIndex, k: usize) -> bool {
    let mut j = k;
    let mut d = 0i32;
    let lo = k.saturating_sub(128);
    while j > lo {
        j -= 1;
        if f.sym(j, b')') {
            d += 1;
        } else if f.sym(j, b'(') {
            if d > 0 {
                d -= 1;
                continue;
            }
            // (`cfg!(..)`)
            if j >= 2 && f.sym(j - 1, b'!') && f.tok(j - 2) == "cfg" {
                return true;
            }
            if j == 0 || !f.ident(j - 1) {
                return false;
            }
            match f.tok(j - 1) {
                "any" | "all" | "not" => continue,
                "cfg" => return j >= 2 && f.sym(j - 2, b'['),
                "cfg_attr" => {
                    // (only within the first argument (the predicate): the Debug of `#[cfg_attr(test, derive(Debug))]` is not a predicate)
                    let mut dd = 0;
                    for q in j + 1..k {
                        if f.sym(q, b'(') {
                            dd += 1;
                        } else if f.sym(q, b')') {
                            dd -= 1;
                        } else if dd == 0 && f.sym(q, b',') {
                            return false;
                        }
                    }
                    return j >= 2 && f.sym(j - 2, b'[');
                }
                _ => return false,
            }
        } else if f.sym(j, b';') || f.sym(j, b'{') || f.sym(j, b'}') {
            return false;
        }
    }
    false
}

fn def_at1(ws: &[&FileIndex], file: usize, off: usize) -> Vec<Loc> {
    let f = ws[file];
    let Some(k) = f.tok_at(off) else { return f.capture_at(off).map(|(_, b, _)| vec![Loc { file, start: f.toks[b].1 as usize, end: f.toks[b].2 as usize }]).unwrap_or_default() };
    let name = f.tok(k).to_string();
    let start = f.toks[k].1 as usize;
    // (built-in types have no definition)
    if PRIMS.contains(&name.as_str()) {
        return vec![];
    }
    // (names in a cfg predicate (the test of `#[cfg(test)]`, `cfg!(unix)`) refer to nothing (as in rust-analyzer))
    if in_cfg_pred(f, k) {
        return vec![];
    }
    // On the name of a definition
    // (the name range of an impl is the self type's name, so impls are skipped: the P of `impl P` is struct P)
    if let Some(d) = f.defs.iter().find(|d| d.start == start && d.kind != SymKind::Impl) {
        return vec![loc(file, d)];
    }
    // (a crate name `std::..` / `extern crate x;`: the start of that crate's root file (as in rust-analyzer))
    if (f.before_path(k) && !f.after_path(k)) || (k >= 2 && f.tok(k - 1) == "crate" && f.tok(k - 2) == "extern") {
        if !f.defs.iter().any(|d| d.name == name && d.kind == SymKind::Module) {
            if let Some(r) = crate_root(ws, &name) {
                return vec![Loc { file: r, start: 0, end: 0 }];
            }
        }
    }
    // (visible definitions: libraries, the files under src, and this file)
    // A macro call `name!(..)` / `a::name!(..)`: macros only (`vec!` is not `mod vec`, `format!` is not `fn format`)
    if f.sym(k + 1, b'!') && !f.sym(k + 2, b'=') && !(k >= 1 && f.sym(k - 1, b'.')) {
        let c: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| d.kind == SymKind::Macro && d.owner.is_empty()).collect();
        if !c.is_empty() {
            // (this file's macro_rules! first)
            let here: Vec<(usize, &Def)> = c.iter().copied().filter(|x| x.0 == file).collect();
            return prefer_ws(ws, if here.is_empty() { c } else { here }, file);
        }
    }
    // A method / field `x.name`
    if k >= 1 && f.sym(k - 1, b'.') && !(k >= 2 && f.sym(k - 2, b'.')) {
        let call = f.sym(k + 1, b'(') || (f.before_path(k) && f.sym(k + 3, b'<'));
        let kinds: &[SymKind] = if call { &[SymKind::Method] } else { &[SymKind::Field] };
        let owner = k.checked_sub(2).and_then(|r| expr_type(ws, file, r, 0)).filter(|t| !type_head(t).is_empty() && type_head(t) != "_");
        let (mut owners, concrete) = owner.as_ref().map(|o| owners_of2(ws, file, o)).unwrap_or_default();
        // (if the receiver type is a type parameter `T`, the methods of the traits of its bounds (`impl<T: Clone>` / `where T: Clone`))
        if let Some(o) = owner.as_deref() {
            owners.extend(f.param_bounds(start, strip_ref(o)));
        }
        let c: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| kinds.contains(&d.kind) && (owner.is_none() || owners.contains(&d.owner))).collect();
        let mut c = prefer_concrete(c, &concrete);
        // (impls of another type with the same name (an impl of slice's `Split<'a, T, P>` for str's `Split<'a, P>`): if some have a matching number of type parameters,
        // the impls that do not match are dropped)
        if let Some(o) = owner.as_deref() {
            let n = targs(strip_ref(o)).len();
            let fits = |d: &Def| !d.container.starts_with("impl") || d.owner != type_head(o) || targs(impl_self_full(&d.container)).len() == n;
            if n > 0 && c.iter().any(|x| x.1.owner == type_head(o) && fits(x.1)) || c.iter().any(|x| x.1.owner == "__str_pat_iter") {
                c.retain(|x| fits(x.1));
            }
            // (the methods of str's `Split<'a, P>` (1 type parameter) are the ones inside a core macro (not `io::Split<B>`))
            if n == 1 && c.iter().any(|x| x.1.owner == "__str_pat_iter") {
                c.retain(|x| x.1.owner == "__str_pat_iter" || x.1.owner != type_head(o));
            }
        }
        // (the ones on both the receiver type and its Deref target (`String::len` and `str::len`): the one closer to the receiver)
        if let Some(first) = concrete.iter().find(|o| c.iter().any(|x| &x.1.owner == *o)) {
            let first = first.clone();
            c.retain(|x| x.1.owner == first || !concrete.contains(&x.1.owner));
        }
        // (`let h: Hidden = n.into();`: the from of `impl From<..> for Hidden` of the expected type Hidden (as in rust-analyzer))
        if call && name == "into" {
            if let Some(l) = into_from(ws, file, k, owner.as_deref()) {
                return vec![l];
            }
        }
        // (methods made by derive: if there is no candidate whose owner is a concrete type, the derive name rather than the library trait's declaration)
        if owner.is_some() && !c.iter().any(|x| concrete.contains(&x.1.owner)) {
            if let Some(l) = derive_method(ws, file, &concrete, &name) {
                return vec![l];
            }
        }
        if c.is_empty() && owner.is_some() {
            // (methods of the traits of blanket impls: `"x".to_string()`)
            let bl = blanket_traits(ws, file, false);
            c = named(ws, file, &*name).into_iter().filter(|(_, d)| kinds.contains(&d.kind) && bl.contains(&d.owner)).collect();
        }
        if owner.is_none() {
            // (when the receiver type is unknown, only when it is the only one, libraries included)
            return pick(c, file);
        }
        return prefer_ws(ws, c, file);
    }
    // A path `Q::name`
    if f.after_path(k) && k >= 3 && f.ident(k - 3) {
        let q = f.tok(k - 3).to_string();
        let mut q = if q == "Self" { f.enclosing_owner(off).unwrap_or(q) } else { q };
        // (the default of `T { .., ..Default::default() }` is T's)
        if (q == "Default" && name == "default") && k >= 5 && f.sym(k - 4, b'.') && f.sym(k - 5, b'.') {
            if let Some(t) = f.struct_lit_at(k - 5) {
                q = if t == "Self" { f.enclosing_owner(off).unwrap_or(t) } else { t };
            }
        }
        // (the default of `T { field: Default::default() }` is the field type's)
        if (q == "Default" && name == "default") && k >= 5 && f.sym(k - 4, b':') && !f.sym(k - 5, b':') && f.ident(k - 5) {
            if let Some(t) = f.struct_lit_at(k - 5) {
                let field = f.tok(k - 5);
                if let Some(ty) = named(ws, file, field).into_iter().map(|x| x.1).find(|d| d.kind == SymKind::Field && d.owner == t).map(|d| type_head(&d.ty)).filter(|h| !h.is_empty()) {
                    q = ty;
                }
            }
        }
        if !matches!(q.as_str(), "crate" | "super" | "self") {
            // (a member of the type, or an item in the module)
            let (qs, nc) = owner_split(ws, file, &q);
            // (`std::collections` / `fixture::Counter`: if Q is a crate name, the items of its root file)
            let root = crate_root(ws, &q);
            let c: Vec<(usize, &Def)> = named(ws, file, &*name)
                .into_iter()
                .filter(|(i, d)| qs.contains(&d.owner) || (d.owner.is_empty() && d.module.last() == Some(&q)) || root.is_some_and(|r| *i == r && d.owner.is_empty() && d.module[..] == ws[r].module[..]))
                .collect();
            let mut c = prefer_concrete(c, &qs[..nc]);
            // (the ones on both the type and its Deref target (`String::as_str` and `str::as_str`): the type itself)
            if let Some(first) = qs[..nc].iter().find(|o| c.iter().any(|x| &x.1.owner == *o)).cloned() {
                c.retain(|x| x.1.owner == first || !qs[..nc].contains(&x.1.owner));
            }
            // (a method made by derive `Point::default()`: if there is no candidate whose owner is a concrete type, the derive name)
            if !c.iter().any(|x| qs[..nc].contains(&x.1.owner)) {
                if let Some(l) = derive_method(ws, file, &qs[..nc], &name) {
                    return vec![l];
                }
            }
            // (not found: if it is a re-exported name like `use std::sync::Mutex`, followed through the uses below.
            // If Q is an unknown name (one of an external crate, `syn::Pat` / `Delimiter::None`), no answer)
            if !c.is_empty() {
                return prefer_ws(ws, c, file);
            }
            // (a method made by derive `T::default()` goes to the derive name: as in rust-analyzer)
            if let Some(l) = derive_method(ws, file, &qs[..nc], &name) {
                return vec![l];
            }
            let known = ws.iter().any(|w| w.module.contains(&q) || w.defs.iter().any(|d| d.name == q && matches!(d.kind, SymKind::Module | SymKind::Struct | SymKind::Enum | SymKind::Trait | SymKind::TypeAlias | SymKind::Union)))
                || matches!(q.as_str(), "std" | "core" | "alloc");
            if !known {
                return vec![];
            }
        }
    }
    // `self::` / `super::` (at the start of a path): that module
    if (name == "self" || name == "super") && f.before_path(k) {
        let mut segs = vec![name.as_str()];
        let mut j = k;
        while j >= 3 && f.after_path(j) && matches!(f.tok(j - 3), "self" | "super") {
            j -= 3;
            segs.push(f.tok(j));
        }
        if f.after_path(j) {
            return vec![];
        }
        let mut m = f.module_at(off);
        for s in segs {
            if s == "super" && m.pop().is_none() {
                return vec![];
            }
        }
        let Some(last) = m.pop() else { return vec![] };
        let c: Vec<(usize, &Def)> = named(ws, file, &last).into_iter().filter(|(_, d)| d.kind == SymKind::Module && d.module[..] == m[..]).collect();
        return prefer_ws(ws, c, file);
    }
    if name == "Self" {
        if let Some(o) = f.enclosing_owner(off) {
            let c: Vec<(usize, &Def)> = named(ws, file, &*o).into_iter().filter(|(_, d)| d.owner.is_empty() && matches!(d.kind, SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::TypeAlias)).collect();
            return prefer_ws(ws, c, file);
        }
        return vec![];
    }
    // The Name of `#[derive(Name)]` is the derive macro
    if f.in_derive(k) {
        let c: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| d.kind == SymKind::Macro).collect();
        return prefer_ws(ws, c, file);
    }
    // (the x of `let x` / `let mut x` is itself)
    if k >= 1
        && (f.tok(k - 1) == "let" || (f.tok(k - 1) == "mut" && k >= 2 && f.tok(k - 2) == "let"))
        && !name.starts_with(|c: char| c.is_ascii_uppercase())
        && !f.sym(k + 1, b'(')
        && !f.sym(k + 1, b'{')
        && !(f.sym(k + 1, b':') && f.sym(k + 2, b':'))
    {
        return vec![Loc { file, start, end: (f.toks[k].2 as usize) }];
    }
    // The name of a field in a struct expression / pattern `T { x: .. }` (the shorthand `T { x }` goes to the binding / local side: as in rust-analyzer)
    if k >= 1 && (f.sym(k - 1, b'{') || f.sym(k - 1, b',')) && f.sym(k + 1, b':') && !f.sym(k + 2, b':') {
        if let Some(t) = f.struct_lit_at(k) {
            let t = if t == "Self" { f.enclosing_owner(off).unwrap_or(t) } else { t };
            let (os, _) = owner_split(ws, file, &t);
            let c: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| d.kind == SymKind::Field && os.contains(&d.owner)).collect();
            return prefer_ws(ws, c, file);
        }
    }
    // (the name of a binding in a pattern / parameters is itself: `let (a, b)` / `fn f(x: T)` / `Some(x) =>`)
    if f.pat_binding(k) || f.fn_param(k) || f.arm_binding(k) {
        return vec![Loc { file, start, end: (f.toks[k].2 as usize) }];
    }
    // A local
    if !f.after_path(k) {
        if let Some(b) = f.local(k, &name) {
            return vec![Loc { file, start: (f.toks[b].1 as usize), end: (f.toks[b].2 as usize) }];
        }
    }
    // A type parameter (the T of `fn f<T>(x: T)` / `impl<T> S<T>`)
    if name.starts_with(|c: char| c.is_ascii_uppercase()) && !f.after_path(k) {
        if let Some(g) = f.generic_param(start, &name) {
            return vec![Loc { file, start: f.toks[g].1 as usize, end: f.toks[g].2 as usize }];
        }
    }
    // Items: the same module → use → the only one in the workspace → libraries (Option / Some etc. of the prelude)
    let mut cands: Vec<(usize, &Def)> = named(ws, file, &*name).into_iter().filter(|(_, d)| d.owner.is_empty() && ITEM_KINDS.contains(&d.kind)).collect();
    if cands.is_empty() {
        // (the Pt of `use a::Point as Pt`: the definition of the original name, looked up through that use)
        if let Some(real) = f.use_alias(&name) {
            cands = named(ws, file, &*real).into_iter().filter(|(_, d)| d.owner.is_empty() && ITEM_KINDS.contains(&d.kind)).collect();
        }
    }
    if cands.is_empty() {
        // (`Some` / `None` / `Ok` / `Err`: variants of the prelude)
        cands = named(ws, file, &name).into_iter().filter(|(i, d)| ws[*i].lib && d.kind == SymKind::Variant && (d.owner == "Option" || d.owner == "Result")).collect();
    }
    if cands.is_empty() {
        return vec![];
    }
    let here = f.module_at(off);
    // (a name inside a `use` (the Ty of `use a::Ty as PTy`): looked up by the path of that use)
    if let Some(j) = f.use_at.iter().position(|&a| a == start) {
        let one = [(name.clone(), f.uses[j].1.clone())];
        return resolve_use(ws, file, &one, &here, &name, &cands, 0).unwrap_or_else(|| prefer_ws(ws, cands, file));
    }
    let same: Vec<(usize, &Def)> = cands.iter().filter(|(i, d)| !ws[*i].lib && d.module[..] == here[..]).cloned().collect();
    if !same.is_empty() {
        return pick(same, file);
    }
    // (use: what brought in the name, what is in the modules brought in by glob, what those modules brought in by use)
    if let Some(r) = resolve_use(ws, file, &f.uses, &here, &name, &cands, 0) {
        return r;
    }
    prefer_ws(ws, cands, file)
}

/// Looks up name with uses (the uses of module here). depth: the number of levels of uses of glob-imported modules followed
fn resolve_use(ws: &[&FileIndex], file: usize, uses: &[(String, Vec<String>)], here: &[String], name: &str, cands: &[(usize, &Def)], depth: u32) -> Option<Vec<Loc>> {
    for (local, path) in uses {
        if local == name && path.len() >= 2 {
            let (m, exact) = use_module(here, &path[..path.len() - 1]);
            let c = by_use(cands, &m, exact);
            // (two or more candidates under the module: follows that module's `pub use`. std::sync::Mutex → sync::poison::mutex)
            if c.len() > 1 && !exact {
                if let Some(r) = reexport(ws, file, &m, &path[path.len() - 1], cands, 0) {
                    return Some(r);
                }
            }
            if !c.is_empty() {
                return Some(prefer_ws(ws, c, file));
            }
        }
    }
    for (local, path) in uses {
        if local == "*" && !path.is_empty() {
            let (m, exact) = use_module(here, path);
            let c = by_use(cands, &m, exact);
            if !c.is_empty() {
                return Some(prefer_ws(ws, c, file));
            }
            // (what the module's file brought in by use: with `use super::*` the parent's uses are visible too)
            if depth < 2 && exact {
                for w in ws.iter().filter(|w| !w.lib && w.module == m) {
                    if let Some(r) = resolve_use(ws, file, &w.uses, &m, name, cands, depth + 1) {
                        return Some(r);
                    }
                }
            }
        }
    }
    None
}

/// Follows the name that module m (its file) brought in by `use`
fn reexport(ws: &[&FileIndex], file: usize, m: &[String], name: &str, cands: &[(usize, &Def)], depth: u32) -> Option<Vec<Loc>> {
    if depth > 4 {
        return None;
    }
    for w in ws.iter().filter(|w| w.module == m) {
        for (local, path) in &w.uses {
            let (base, last) = if local == name && path.len() >= 2 {
                (&path[..path.len() - 1], path[path.len() - 1].as_str())
            } else if local == "*" && !path.is_empty() {
                (&path[..], name)
            } else {
                continue;
            };
            let (m2, exact) = use_module(m, base);
            if !exact {
                continue;
            }
            let c = by_use(cands, &m2, true);
            if !c.is_empty() {
                return Some(pick(c, file));
            }
            if let Some(r) = reexport(ws, file, &m2, last, cands, depth + 1) {
                return Some(r);
            }
        }
    }
    None
}

/// The references of the identifier at off (the identifiers of the same name that resolve to the same definition). If include_decl, the definition's name is included too
pub fn references(ws: &[&FileIndex], file: usize, off: usize, include_decl: bool, load: Load) -> Vec<Loc> {
    references_in(ws, file, off, include_decl, load, None)
}

/// textDocument/documentHighlight: only the references in file file (other files are not searched)
pub fn highlights(ws: &[&FileIndex], file: usize, off: usize, load: Load) -> Vec<Loc> {
    let mut v = references_in(ws, file, off, true, load, Some(file));
    // (on the x of a shorthand `T { x }`: both the local's references and the field's references (as in rust-analyzer))
    if let Some(k) = ws[file].tok_at(off) {
        if let Some(fl) = shorthand_pair(ws, file, k) {
            if !ws[fl.file].slim {
                for l in references_in(ws, fl.file, fl.start, true, load, Some(file)) {
                    if l.file == file && !v.contains(&l) {
                        v.push(l);
                    }
                }
                v.sort_by_key(|l| l.start);
            }
        }
    }
    v
}

/// The body of references. If only is given, searches only inside that file
/// On the `fn` keyword: the fn's exit points (`fn` itself, `return ..`, `?`, the last expression of the body). Same as rust-analyzer's references /
/// documentHighlight
fn exit_points(f: &FileIndex, file: usize, off: usize) -> Option<Vec<Loc>> {
    let k = f.tok_at(off)?;
    if f.tok(k) != "fn" || !f.ident(k + 1) {
        return None;
    }
    let a = f.toks[k + 1].1 as usize;
    let d = f.defs.iter().find(|d| d.start == a && matches!(d.kind, SymKind::Function | SymKind::Method))?;
    let n = f.toks.len();
    // (the body's `{`: the first `{` outside the signature's brackets)
    let mut j = k + 2;
    let mut dd = 0;
    while j < n && (f.toks[j].1 as usize) < d.item_end {
        if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'<') {
            dd += 1;
        } else if f.sym(j, b')') || f.sym(j, b']') || (f.sym(j, b'>') && !f.sym(j - 1, b'-')) {
            dd -= 1;
        } else if dd <= 0 && (f.sym(j, b'{') || f.sym(j, b';')) {
            break;
        }
        j += 1;
    }
    if !f.sym(j, b'{') {
        return Some(vec![Loc { file, start: f.toks[k].1 as usize, end: f.toks[k].2 as usize }]);
    }
    let open = j;
    let mut close = open;
    let mut dd = 0;
    while close < n {
        if f.sym(close, b'{') {
            dd += 1;
        } else if f.sym(close, b'}') {
            dd -= 1;
            if dd == 0 {
                break;
            }
        }
        close += 1;
    }
    let inner = |at: usize| f.defs.iter().any(|x| x.start != d.start && matches!(x.kind, SymKind::Function | SymKind::Method) && x.item_start <= at && at < x.item_end);
    let mut out = vec![Loc { file, start: f.toks[k].1 as usize, end: f.toks[k].2 as usize }];
    for q in open + 1..close.min(n) {
        let at = f.toks[q].1 as usize;
        if inner(at) || f.in_macro_body(at) {
            continue;
        }
        if f.sym(q, b'?') {
            out.push(Loc { file, start: at, end: at + 1 });
        } else if f.ident(q) && f.tok(q) == "return" {
            out.push(Loc { file, start: at, end: f.toks[q].2 as usize });
        }
    }
    // (the last expression: for match / if / a block, follows the arms inside / the last expression of the block)
    block_tails(f, file, open, close, &mut out);
    Some(out)
}

/// The type of the last expression of the body of this file's `macro_rules! name { (..) => {{ ..; tail }} }` (with one arm)
/// A hint at the result type from the text of a macro definition (kept in the Def by FileIndex::new): if half or more of the arm bodies start with `$crate::T::` / `$crate::T(`
/// (T starts with an uppercase letter), T. If there is one arm and the body is `$crate::m!(..)`, "!m"
fn macro_hint(text: &str) -> Option<String> {
    let bodies: Vec<&str> = text.match_indices("=>").map(|(i, _)| text[i + 2..].trim_start().trim_start_matches(['{', '(']).trim_start()).collect();
    if bodies.is_empty() {
        return None;
    }
    if bodies.len() == 1 {
        if let Some(r) = bodies[0].strip_prefix("$crate::") {
            let n: String = r.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if r[n.len()..].starts_with('!') && !n.is_empty() {
                return Some(format!("!{n}"));
            }
        }
    }
    let mut counts: Vec<(String, usize)> = vec![];
    for b in &bodies {
        let Some(r) = b.strip_prefix("$crate::") else { continue };
        let n: String = r.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if n.starts_with(|c: char| c.is_ascii_uppercase()) && (r[n.len()..].starts_with("::") || r[n.len()..].starts_with('(')) {
            match counts.iter_mut().find(|x| x.0 == n) {
                Some(x) => x.1 += 1,
                None => counts.push((n, 1)),
            }
        }
    }
    let (t, c) = counts.into_iter().max_by_key(|x| x.1)?;
    // (half or more of the arms, excluding bodies that only call a macro (the intermediate arms of a TT muncher `json_internal!(@object ..)` / `vec![..]`))
    let finals = bodies
        .iter()
        .filter(|b| {
            let r = b.strip_prefix("$crate::").unwrap_or(b);
            let r = r.strip_prefix("__private::").unwrap_or(r);
            let n = r.chars().take_while(|c| c.is_alphanumeric() || *c == '_').count();
            !(n > 0 && r[n..].starts_with('!')) && !b.is_empty() && !b.starts_with('}')
        })
        .count();
    (c * 2 >= finals.max(1)).then_some(t)
}

fn macro_result_type(ws: &[&FileIndex], file: usize, name: &str, depth: u32) -> Option<String> {
    let f = ws[file];
    let Some(d) = f.defs.iter().find(|d| d.kind == SymKind::Macro && d.name == name) else {
        // (a macro_rules! of another file / a library: the hint kept in the definition)
        if depth > 8 {
            return None;
        }
        let d = named(ws, file, name).into_iter().map(|x| x.1).find(|d| d.kind == SymKind::Macro && d.fnret.iter().any(|x| x.0 == 255))?;
        let h = &d.fnret.iter().find(|x| x.0 == 255)?.2;
        return match h.strip_prefix('!') {
            Some(m) => macro_result_type(ws, file, m, depth + 1),
            None => Some(h.clone()),
        };
    };
    let a = f.toks.partition_point(|t| (t.1 as usize) < d.start);
    let e = f.toks.partition_point(|t| (t.1 as usize) < d.item_end);
    // (only one `=>` of an arm)
    let arrows: Vec<usize> = (a..e).filter(|&q| f.sym(q, b'=') && f.sym(q + 1, b'>')).collect();
    let mut q = *arrows.first()?;
    if arrows.len() != 1 {
        return None;
    }
    q += 2;
    if !(f.sym(q, b'{') || f.sym(q, b'(')) {
        return None;
    }
    // (`{{ .. }}` is the inner block)
    let (o, c) = if f.sym(q + 1, b'{') && close_of(f, q + 1) + 1 == close_of(f, q) { (q + 1, close_of(f, q + 1)) } else { (q, close_of(f, q)) };
    let mut tails = vec![];
    block_tails(f, file, o, c, &mut tails);
    let l = tails.first()?;
    let k = f.toks.partition_point(|t| (t.2 as usize) < l.end);
    // (a `$x` inside the body is unknown)
    if (f.toks.partition_point(|t| (t.1 as usize) < l.start)..=k).any(|q| f.sym(q, b'$')) {
        return None;
    }
    expr_type(ws, file, k, depth + 1)
}

/// The start of an expression ending in a block (opening bracket p): the match of `match x {`, the if of `if c { } else {`, `loop {` / `unsafe {`; for a plain block
/// p. None for the `{` of a `fn` body or of an item such as `impl`
fn block_expr_start(f: &FileIndex, p: usize) -> Option<usize> {
    let mut q = p;
    loop {
        // (`} else {` / `} else if c {`: to the previous block)
        if q >= 2 && f.ident(q - 1) && f.tok(q - 1) == "else" && f.sym(q - 2, b'}') {
            q = f.open_of(q - 2)?;
            continue;
        }
        let mut r = q;
        let mut d = 0;
        while r > 0 {
            r -= 1;
            if f.sym(r, b')') || f.sym(r, b']') {
                d += 1;
            } else if f.sym(r, b'(') || f.sym(r, b'[') {
                if d == 0 {
                    return Some(q);
                }
                d -= 1;
            } else if d == 0 && (f.sym(r, b';') || f.sym(r, b'{') || f.sym(r, b'}') || f.sym(r, b',')) {
                return Some(q);
            } else if d == 0 && f.ident(r) {
                match f.tok(r) {
                    "match" | "loop" | "unsafe" | "async" => return Some(r),
                    "if" => {
                        // (`else if`: further back to the previous if)
                        if r >= 2 && f.ident(r - 1) && f.tok(r - 1) == "else" && f.sym(r - 2, b'}') {
                            q = f.open_of(r - 2)?;
                            break;
                        }
                        return Some(r);
                    }
                    "fn" | "impl" | "struct" | "enum" | "trait" | "mod" | "union" | "where" | "while" | "for" => return None,
                    _ => {}
                }
            }
        }
        if r == 0 {
            return Some(q);
        }
    }
}

/// The closing bracket matching the opening bracket of token j
fn close_of(f: &FileIndex, j: usize) -> usize {
    let mut d = 0;
    let mut q = j;
    while q < f.toks.len() {
        if f.sym(q, b'{') || f.sym(q, b'(') || f.sym(q, b'[') {
            d += 1;
        } else if f.sym(q, b'}') || f.sym(q, b')') || f.sym(q, b']') {
            d -= 1;
            if d == 0 {
                return q;
            }
        }
        q += 1;
    }
    f.toks.len().saturating_sub(1)
}

/// Adds the last expression (its exit points) of a block `{ .. }` (open..close) to out
fn block_tails(f: &FileIndex, file: usize, open: usize, close: usize, out: &mut Vec<Loc>) {
    if close >= f.toks.len() || close <= open + 1 || f.sym(close - 1, b';') {
        return;
    }
    // (the start of the last statement: at depth 0, preceded by `;` or following a block expression `}` (not followed by `.` / `else`))
    let mut s = close - 1;
    let mut dd = 0;
    while s > open + 1 {
        if f.sym(s, b'}') || f.sym(s, b')') || f.sym(s, b']') {
            dd += 1;
        } else if f.sym(s, b'{') || f.sym(s, b'(') || f.sym(s, b'[') {
            dd -= 1;
        }
        if dd == 0 && (f.sym(s - 1, b';') || (f.sym(s - 1, b'}') && !f.sym(s, b'.') && !f.sym(s, b'?') && !(f.ident(s) && f.tok(s) == "else"))) {
            break;
        }
        s -= 1;
    }
    expr_tails(f, file, s, close - 1, out);
}

/// The type of a range expression (tokens s..=e, with `..` / `..=` at depth 0): `Range<T>` / `RangeInclusive<T>`. T is the type of whichever end is known;
/// if both are numbers without a suffix, i32 (the same default as rust-analyzer). An open-ended `a..` is `RangeFrom<T>`
fn range_type(ws: &[&FileIndex], file: usize, s: usize, e: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let mut dd = 0;
    for q in s..=e {
        if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
            dd += 1;
        } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
            dd -= 1;
        } else if dd == 0 && f.sym(q, b'.') && f.sym(q + 1, b'.') && !f.sym(q + 2, b'.') {
            let incl = f.sym(q + 2, b'=');
            let hi_s = if incl { q + 3 } else { q + 2 };
            let hi = if hi_s <= e { expr_type(ws, file, e, depth + 1) } else { None };
            let lo = if q > s { expr_type(ws, file, q - 1, depth + 1) } else { None };
            let tt = hi.or(lo).unwrap_or_else(|| "i32".to_string());
            let tt = strip_ref(&tt).to_string();
            let kind = if hi_s > e { "RangeFrom" } else if incl { "RangeInclusive" } else { "Range" };
            return Some(format!("{kind}<{tt}>"));
        }
    }
    None
}

/// The type of an expression (tokens s..=e) starting with prefix operators (`&` / `&mut` / `*`): applied from the inside to the type t of the expression the operators apply to (the type of the expression's last token)
/// (the p of `&*p`: `*const T` → `&T`). If there is a binary operator in between (`*a + b`), unchanged
fn with_prefix(f: &FileIndex, s: usize, e: usize, t: String) -> String {
    let mut ops = vec![];
    let mut q = s;
    while q < e {
        if f.sym(q, b'&') {
            if f.ident(q + 1) && f.tok(q + 1) == "mut" {
                ops.push("&mut");
                q += 2;
            } else {
                ops.push("&");
                q += 1;
            }
        } else if f.sym(q, b'*') {
            ops.push("*");
            q += 1;
        } else {
            break;
        }
    }
    if ops.is_empty() || (q..e).any(|j| f.toks[j].0 == 1 && b"+-*/%<>=!|^".contains(&f.text.as_bytes()[f.toks[j].1 as usize]) && !f.sym(j, b'>') && !(f.sym(j, b'-') && f.sym(j + 1, b'>'))) {
        return t;
    }
    let mut t = t;
    for op in ops.iter().rev() {
        t = match *op {
            "&" => format!("&{t}"),
            "&mut" => format!("&mut {t}"),
            _ => {
                let x = t.trim();
                if let Some(r) = x.strip_prefix("&mut ").or_else(|| x.strip_prefix('&')).or_else(|| x.strip_prefix("*const ")).or_else(|| x.strip_prefix("*mut ")) {
                    r.trim().to_string()
                } else if matches!(type_head(x).as_str(), "Box" | "Rc" | "Arc") && !x.starts_with('&') {
                    targs(x).into_iter().next().unwrap_or_else(|| x.to_string())
                } else {
                    x.to_string()
                }
            }
        };
    }
    t
}

/// The exit points of an expression (tokens s..=e): for match arms, if / else blocks and blocks, the last expression inside; otherwise the expression itself
fn expr_tails(f: &FileIndex, file: usize, s: usize, e: usize, out: &mut Vec<Loc>) {
    let whole = |out: &mut Vec<Loc>| {
        let a = f.toks[s].1 as usize;
        if !out.iter().any(|l| l.start == a) {
            out.push(Loc { file, start: a, end: f.toks[e].2 as usize });
        }
    };
    if f.sym(s, b'{') && close_of(f, s) == e {
        block_tails(f, file, s, e, out);
    } else if f.ident(s) && f.tok(s) == "unsafe" && f.sym(s + 1, b'{') && close_of(f, s + 1) == e {
        block_tails(f, file, s + 1, e, out);
    } else if f.ident(s) && matches!(f.tok(s), "while" | "for") {
        // (a loop without a value: not an exit point (as in rust-analyzer))
    } else if f.ident(s) && f.tok(s) == "match" {
        // (arms: `pat => expr,` / `pat => { .. }`)
        let mut b = s + 1;
        while b < e && !(f.sym(b, b'{') && !(b >= 1 && f.ident(b - 1) && f.tok(b - 1).starts_with(|c: char| c.is_ascii_uppercase()))) {
            b += 1;
        }
        if b >= e || close_of(f, b) != e {
            return whole(out);
        }
        let mut q = b + 1;
        while q < e {
            // (finds `=>`, then the end of the arm's expression after it (a `,` at depth 0, or before a closing bracket))
            if f.sym(q, b'=') && f.sym(q + 1, b'>') {
                let a = q + 2;
                let mut z = a;
                let mut dd = 0;
                while z < e {
                    if f.sym(z, b'{') || f.sym(z, b'(') || f.sym(z, b'[') {
                        dd += 1;
                    } else if f.sym(z, b'}') || f.sym(z, b')') || f.sym(z, b']') {
                        dd -= 1;
                    }
                    if dd == 0 && (f.sym(z + 1, b',') || z + 1 == e || (f.sym(z, b'}') && f.sym(a, b'{'))) {
                        break;
                    }
                    z += 1;
                }
                if a <= z && z < e {
                    expr_tails(f, file, a, z, out);
                }
                q = z + 1;
            } else {
                q += 1;
            }
        }
    } else if f.ident(s) && f.tok(s) == "if" {
        // (`if c { A } else if d { B } else { C }`: each block)
        let mut q = s + 1;
        let mut any = false;
        while q <= e {
            if f.sym(q, b'{') {
                let c = close_of(f, q);
                block_tails(f, file, q, c, out);
                any = true;
                q = c + 1;
                if !(q <= e && f.ident(q) && f.tok(q) == "else") {
                    break;
                }
            }
            q += 1;
        }
        if !any {
            whole(out);
        }
    } else {
        whole(out);
    }
}

fn references_in(ws: &[&FileIndex], file: usize, off: usize, include_decl: bool, load: Load, only: Option<usize>) -> Vec<Loc> {
    clear_memo();
    if let Some(r) = exit_points(ws[file], file, off) {
        return r;
    }
    let off = kw_to_name(ws[file], off);
    if let Some(r) = self_refs(ws, file, off, include_decl) {
        return r;
    }
    let d = def_at(ws, file, off);
    let Some(&target) = d.first() else {
        if ws[file].tok_at(off).is_some_and(|k| ws[file].in_inactive(ws[file].toks[k].1 as usize) && !PRIMS.contains(&ws[file].tok(k))) {
            return vec![];
        }
        return prim_refs(ws, file, off, load, only);
    };
    let tf = ws[target.file];
    // (if the target is the name in `#[derive(Default)]` (a method made by derive): searched by the queried name (`default`))
    let via_derive = !tf.slim && tf.tok_at(target.start).is_some_and(|j| tf.in_derive(j));
    let qname = ws[file].tok_at(off).map(|k| ws[file].tok(k).to_string()).unwrap_or_default();
    // (a crate root (`std` / `fixture`): position 0..0. The name is the queried name)
    let crate_ref = target.start == 0 && target.end == 0;
    let name = match tf.defs.iter().find(|x| x.start == target.start) {
        _ if via_derive || crate_ref => qname.as_str(),
        Some(d) if tf.slim => d.name.as_str(),
        _ => &tf.text[target.start..target.end],
    };
    // (for a local, only inside the fn that has the definition)
    let is_local = !via_derive && !crate_ref && !tf.defs.iter().any(|x| x.start == target.start && x.kind != SymKind::Impl);
    let mut out = vec![];
    // (library indexes have no tokens, so the declaration is added here. The derive name too (it has a different name, so it is not found below))
    if include_decl && ((tf.lib && only.is_none()) || ((via_derive || crate_ref) && only.is_none_or(|o| o == target.file))) {
        out.push(target);
    }
    // (for a type, the `Self`s inside its impls are references too: as in rust-analyzer)
    let tdef = tf.defs.iter().find(|x| x.start == target.start && x.kind != SymKind::Impl);
    let is_type = tdef.is_some_and(|x| matches!(x.kind, SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::TypeAlias));
    // (for a module, `self::` / `super::` too)
    let is_mod = tdef.is_some_and(|x| x.kind == SymKind::Module);
    let is_field = tdef.is_some_and(|x| x.kind == SymKind::Field);
    // (for a trait method, the impls' methods and their calls too: as in rust-analyzer)
    let mut targets = vec![target];
    if let Some(td) = tdef.filter(|x| x.kind == SymKind::Method && !x.container.starts_with("impl")) {
        for (i, w) in ws.iter().enumerate() {
            for d in &w.defs {
                if d.kind == SymKind::Method && d.name == td.name && impl_trait(&d.container) == Some(&td.container[..]) {
                    targets.push(loc(i, d));
                }
            }
        }
    }
    // (for a member, the name of the owner type and the names of the concrete types (struct / enum / union) of the workspace (type aliases and traits excluded))
    let owner: Option<String> = tdef.filter(|x| !x.owner.is_empty() && matches!(x.kind, SymKind::Method | SymKind::Function | SymKind::Const)).map(|x| x.owner.clone());
    let mut concrete: FxSet<&str> = if owner.is_some() {
        let aliases: FxSet<&str> = ws.iter().filter(|w| !w.lib).flat_map(|w| w.defs.iter()).filter(|d| matches!(d.kind, SymKind::TypeAlias | SymKind::Trait)).map(|d| d.name.as_str()).collect();
        ws.iter().filter(|w| !w.lib).flat_map(|w| w.defs.iter()).filter(|d| matches!(d.kind, SymKind::Struct | SymKind::Enum | SymKind::Union) && !aliases.contains(d.name.as_str())).map(|d| d.name.as_str()).collect()
    } else {
        FxSet::default()
    };
    // (if the owner is a trait, `P::default()` can also be the trait's method: not narrowed)
    let owner = owner.filter(|o| concrete.contains(o.as_str()));
    if owner.is_none() {
        concrete.clear();
    }
    // (the files to search. Files taken out of memory (slim) are reloaded first, and those are used to resolve identifiers)
    let files: Vec<usize> = ws
        .iter()
        .enumerate()
        .filter(|&(i, &w0)| {
            !((is_local && i != target.file) || only.is_some_and(|o| o != i))
                && !w0.lib
                && (w0.may_have(name) || (is_type && w0.may_have("Self")) || (is_mod && (w0.may_have("self") || w0.may_have("super"))) || (crate_ref && w0.krate == tf.krate && (w0.may_have("crate") || w0.may_have("super"))))
        })
        .map(|(i, _)| i)
        .collect();
    let held: Vec<(usize, Rc<FileIndex>)> = files.iter().filter(|&&i| ws[i].slim).filter_map(|&i| load(i).map(|r| (i, r))).collect();
    let mut wsx: Vec<&FileIndex> = ws.to_vec();
    for (i, r) in &held {
        wsx[*i] = r;
    }
    let files: Vec<usize> = files.into_iter().filter(|&i| !ws[i].slim || held.iter().any(|(j, _)| *j == i)).collect();
    let wsx: &[&FileIndex] = &wsx;
    let scan = |i: usize| -> Vec<Loc> {
        let w = wsx[i];
        let mut out = vec![];
        let none: Vec<u32> = vec![];
        let ks = w.idents.get(name).unwrap_or(&none);
        let selfs = if is_type { w.idents.get("Self").unwrap_or(&none) } else { &none };
        let mods = if is_mod { w.idents.get("self").into_iter().chain(w.idents.get("super")).flatten().collect() } else { vec![] };
        // (the Pt of `use a::Point as Pt` is not counted: as in rust-analyzer. The references of Pt are looked up with Pt)
        for &k in ks.iter().chain(selfs.iter()).chain(mods) {
            let at = w.toks[k as usize].1 as usize;
            // (names in macro_rules! bodies are not resolved: as in rust-analyzer. Inside items excluded by cfg, def_at resolves only paths)
            if w.in_macro_body(at) {
                continue;
            }
            if !include_decl && i == target.file && at == target.start {
                continue;
            }
            if i == target.file && at == target.start {
                out.push(target);
                continue;
            }
            let here = Loc { file: i, start: at, end: (w.toks[k as usize].2 as usize) };
            // (for speed: for references of a member, if the Q of `Q::name` is a concrete type (struct / enum) that is not the owner, there is no need to resolve it)
            if let Some(o) = owner.as_deref() {
                let k = k as usize;
                if w.after_path(k) && k >= 3 && w.ident(k - 3) {
                    let q = w.tok(k - 3);
                    if q != o && q != "Self" && concrete.contains(q) {
                        continue;
                    }
                }
            }
            if def_at(wsx, i, at).first().is_some_and(|l| targets.contains(l)) {
                out.push(here);
            } else if is_field && shorthand_field(wsx, i, k as usize).first() == Some(&target) {
                // (the x of a shorthand `T { x }` is also a reference of the field)
                out.push(here);
            }
        }
        // (a workspace crate root: the `crate::` / `pub(crate)` inside that crate, and also `super` pointing at the root (as in rust-analyzer))
        if crate_ref && !tf.lib && w.krate == tf.krate && !w.lib {
            for (k, _) in w.toks.iter().enumerate() {
                if !w.ident(k) {
                    continue;
                }
                let at = w.toks[k].1 as usize;
                let hit = match w.tok(k) {
                    "crate" => w.before_path(k) && !w.after_path(k) || (k >= 2 && w.sym(k - 1, b'(') && w.tok(k - 2) == "pub"),
                    "super" => w.before_path(k) && !w.after_path(k) && w.module_at(at).len() == 1,
                    _ => false,
                };
                if hit && !w.in_macro_body(at) {
                    out.push(Loc { file: i, start: at, end: w.toks[k].2 as usize });
                }
            }
        }
        // (for a local, the x inside `format!("{x}")` too)
        if is_local {
            for (t, tk) in w.toks.iter().enumerate() {
                if tk.0 != 2 || !w.text[tk.1 as usize..].starts_with('"') {
                    continue;
                }
                for (a, n) in captures(&w.text[tk.1 as usize..tk.2 as usize]) {
                    if n == name && w.local(t, n).is_some_and(|b| w.toks[b].1 as usize == target.start) {
                        let a = tk.1 as usize + a;
                        out.push(Loc { file: i, start: a, end: a + n.len() });
                    }
                }
            }
        }
        out
    };
    // (each file is independent, so if there are many they are split across threads. The memo of the name table (thread_local) is per thread; the crate roots are copied)
    let par = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
    if files.len() >= 8 && par > 1 {
        let roots = CRATE_ROOTS.with(|c| c.borrow().clone());
        let mut parts: Vec<Vec<(usize, Vec<Loc>)>> = vec![];
        std::thread::scope(|sc| {
            let hs: Vec<_> = (0..par)
                .map(|t| {
                    let files = &files;
                    let scan = &scan;
                    let roots = roots.clone();
                    sc.spawn(move || {
                        set_crate_roots(roots);
                        files.iter().skip(t).step_by(par).map(|&i| (i, scan(i))).collect::<Vec<_>>()
                    })
                })
                .collect();
            for h in hs {
                parts.push(h.join().unwrap_or_default());
            }
        });
        let mut all: Vec<(usize, Vec<Loc>)> = parts.into_iter().flatten().collect();
        all.sort_by_key(|x| x.0);
        for (_, v) in all {
            out.extend(v);
        }
    } else {
        for &i in &files {
            out.extend(scan(i));
        }
    }
    out
}

/// The Tr of `impl Tr for S` / `impl<T> Tr<T> for S<T>`
fn impl_trait(container: &str) -> Option<&str> {
    let n = container.strip_prefix("impl ")?;
    let i = n.rfind(" for ")?;
    let t = n[..i].trim_start_matches('!');
    Some(t.split('<').next().unwrap_or(t).rsplit("::").next().unwrap_or(t).trim())
}

/// The definition of the field of the x (token k) of a struct expression / pattern shorthand `T { x }`
fn shorthand_field(ws: &[&FileIndex], file: usize, k: usize) -> Vec<Loc> {
    let f = ws[file];
    if !(k >= 1 && (f.sym(k - 1, b'{') || f.sym(k - 1, b',')) && (f.sym(k + 1, b',') || f.sym(k + 1, b'}'))) {
        return vec![];
    }
    let Some(t) = f.struct_lit_at(k) else { return vec![] };
    let t = if t == "Self" { f.enclosing_owner(f.toks[k].1 as usize).unwrap_or(t) } else { t };
    let (os, _) = owner_split(ws, file, &t);
    let name = f.tok(k);
    let kr = f.krate;
    let c: Vec<(usize, &Def)> = ws
        .iter()
        .enumerate()
        .filter(|(i, w)| *i == file || w.in_src || w.lib || w.krate == kr)
        .flat_map(|(i, w)| w.defs.iter().map(move |d| (i, d)))
        .filter(|(_, d)| d.name == name && d.kind == SymKind::Field && os.contains(&d.owner))
        .collect();
    prefer_ws(ws, c, file)
}

/// References of a built-in type (`u8` etc.): the identifiers of the same name in the workspace that do not resolve to another definition
fn prim_refs(ws: &[&FileIndex], file: usize, off: usize, load: Load, only: Option<usize>) -> Vec<Loc> {
    let f = ws[file];
    let Some(k) = f.tok_at(off) else { return vec![] };
    let name = f.tok(k).to_string();
    if !PRIMS.contains(&name.as_str()) {
        return vec![];
    }
    let mut out = vec![];
    for (i, &w0) in ws.iter().enumerate() {
        if w0.lib || !w0.may_have(&name) || only.is_some_and(|o| o != i) {
            continue;
        }
        let held;
        let w: &FileIndex = if w0.slim {
            let Some(r) = load(i) else { continue };
            held = r;
            &held
        } else {
            w0
        };
        let Some(ks) = w.idents.get(&name) else { continue };
        for &k in ks {
            let k = k as usize;
            let at = w.toks[k].1 as usize;
            // (excluding method / field names `x.u8` and macro bodies)
            // (also excluding the suffix of `0u8`: right after a number. The str of `std::str::from_utf8` is a module)
            if w.in_macro_body(at) || w.after_path(k) || (k > 0 && (w.sym(k - 1, b'.') || (w.toks[k - 1].2 as usize == at && w.text.as_bytes()[w.toks[k - 1].1 as usize].is_ascii_digit()))) {
                continue;
            }
            // (the name of a definition with the same name `fn u8(..)`)
            if w.defs.iter().any(|d| d.start == at) {
                continue;
            }
            out.push(Loc { file: i, start: at, end: w.toks[k].2 as usize });
        }
    }
    out
}

/// Reloads a slim file (ws index)
pub type Load<'a> = &'a dyn Fn(usize) -> Option<Rc<FileIndex>>;

fn name_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A type as written with references, `dyn`, `impl` and lifetimes removed
fn strip_ref(t: &str) -> &str {
    let mut t = t.trim();
    loop {
        let before = t;
        t = t.trim_start_matches('&').trim_start();
        if t.starts_with('\'') {
            t = t.find(' ').map(|i| t[i..].trim_start()).unwrap_or("");
        }
        for kw in ["mut ", "dyn ", "impl ", "const "] {
            if let Some(r) = t.strip_prefix(kw) {
                t = r.trim_start();
            }
        }
        if t == before {
            break;
        }
    }
    t
}

/// The head name of a type as written (`&'a mut a::Vec<T>` → Vec, `impl Iterator<..>` → Iterator, `[T]` / `[T; N]` → `[T]`, a tuple → empty)
pub fn type_head(t: &str) -> String {
    let t = strip_ref(t);
    if t.starts_with('[') {
        return "[T]".to_string();
    }
    let end = t.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(t.len());
    let p = &t[..end];
    p.rsplit("::").next().unwrap_or("").to_string()
}

/// The type arguments (`HashMap<K, V>` → [K, V], `[T]` / `[T; N]` → [T], lifetimes removed)
fn targs(t: &str) -> Vec<String> {
    let t = strip_ref(t);
    let (inner, sep_semi) = if let Some(r) = t.strip_prefix('[') {
        (r.strip_suffix(']').unwrap_or(r), true)
    } else {
        match t.find('<') {
            Some(i) if !t[..i].contains('(') => {
                let r = &t[i + 1..];
                // (up to the matching `>`)
                let mut d = 0;
                let mut e = r.len();
                for (j, c) in r.char_indices() {
                    match c {
                        '<' | '(' | '[' => d += 1,
                        ')' | ']' => d -= 1,
                        '>' if j > 0 && r.as_bytes()[j - 1] == b'-' => {}
                        '>' => {
                            if d == 0 {
                                e = j;
                                break;
                            }
                            d -= 1;
                        }
                        _ => {}
                    }
                }
                (&r[..e], false)
            }
            _ => return vec![],
        }
    };
    let mut out = vec![];
    let mut d = 0;
    let mut s = 0;
    let b = inner.as_bytes();
    for j in 0..=b.len() {
        let c = if j < b.len() { b[j] } else { b',' };
        match c {
            b'<' | b'(' | b'[' => d += 1,
            b'>' if j > 0 && b[j - 1] == b'-' => {}
            b'>' | b')' | b']' => d -= 1,
            b',' | b';' if d == 0 => {
                let a = inner[s..j].trim();
                if !a.is_empty() && !a.starts_with('\'') {
                    out.push(a.to_string());
                }
                s = j + 1;
                if c == b';' && sep_semi {
                    break;
                }
            }
            _ => {}
        }
    }
    out
}

/// Replaces the identifiers params[i] in t with args[i]
fn subst(t: &str, params: &[String], args: &[String]) -> String {
    if params.is_empty() {
        return t.to_string();
    }
    let b = t.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let w = &t[s..i];
            let prev_colon = s >= 2 && &t[s - 2..s] == "::";
            match params.iter().position(|p| p == w) {
                Some(k) if !prev_colon && k < args.len() => out.push_str(&args[k]),
                _ => out.push_str(w),
            }
        } else {
            let c = t[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// A type alias's detail (`<K, V> = HashMap<K, V>` / `HashMap<..>`) → (type parameter names, the right-hand type)
fn alias_parts(detail: &str) -> (Vec<String>, String) {
    let d = detail.trim();
    if !d.starts_with('<') {
        return (vec![], d.to_string());
    }
    let b = d.as_bytes();
    let mut depth = 0;
    let mut e = 0;
    for i in 0..b.len() {
        match b[i] {
            b'<' => depth += 1,
            b'>' if i > 0 && b[i - 1] == b'-' => {}
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    e = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let params = targs(&format!("X{}", &d[..=e])).into_iter().map(|p| p.split([':', '=', ' ']).next().unwrap_or("").to_string()).filter(|p| !p.is_empty()).collect();
    let rhs = d[e + 1..].trim_start().trim_start_matches('=').trim().to_string();
    (params, rhs)
}

/// For a type alias, turns it into the right-hand type (type arguments replaced too: `LockResult<G>` → `Result<G, PoisonError<G>>`). For a wrapper that Derefs
/// (`MutexGuard<T>` / `Box<T>` / `Rc<T>` / `Ref<T>` ..), its contents
fn expand(ws: &[&FileIndex], file: usize, t: &str) -> Vec<String> {
    let mut out = vec![t.to_string()];
    let mut cur = t.to_string();
    for _ in 0..4 {
        let h = type_head(&cur);
        if h.is_empty() {
            break;
        }
        // (if there are several aliases with the same name (`io::Result<T>` and `fmt::Result`): the one whose qualifying module matches, the one whose number of type parameters matches)
        let qual = strip_ref(&cur).split('<').next().unwrap_or("").rsplit("::").nth(1).map(|s| s.to_string());
        let nargs = targs(&cur).len();
        // (even if the right-hand side is a type of the same name, an alias with a different number of type parameters (`io::Result<T>` = `Result<T, Error>`) is expanded)
        let aliases: Vec<(usize, &Def)> = named(ws, file, &h).into_iter().filter(|(_, d)| d.kind == SymKind::TypeAlias && d.owner.is_empty() && !d.ty.is_empty() && (type_head(&d.ty) != h || targs(&d.ty).len() != d.params.len())).collect();
        // (without a qualifier, only the ones whose number of type parameters matches: `Result<X, Y>` is the enum Result, not the alias `io::Result<T>`)
        // (without a qualifier, the ones visible from the use site: in the order same file, same crate, workspace)
        let kr = ws[file].krate;
        let best = aliases.iter().copied().filter(|(_, d)| qual.is_some() || d.params.len() == nargs || nargs == 0).min_by_key(|(i, d)| (qual.as_ref().is_some_and(|q| d.module.last() != Some(q)), d.params.len() != nargs, *i != file, ws[*i].krate != kr, ws[*i].lib)).map(|x| x.1);
        let next = if let Some(d) = best {
            subst(&d.ty, &d.params, &targs(&cur))
        } else if matches!(h.as_str(), "MutexGuard" | "RwLockReadGuard" | "RwLockWriteGuard" | "Box" | "Rc" | "Arc" | "Ref" | "RefMut" | "ManuallyDrop") {
            match targs(&cur).into_iter().next() {
                Some(a) => a,
                None => break,
            }
        } else if let Some(tg) = assoc_type(ws, file, strip_ref(&cur), "Deref", "Target").filter(|tg| type_head(tg) != h && !tg.is_empty()) {
            // (the Deref target, e.g. of `impl Deref for PathBuf { type Target = Path; }`)
            tg
        } else {
            break;
        };
        if out.contains(&next) {
            break;
        }
        out.push(next.clone());
        cur = next;
    }
    out
}

/// The self type (`S<T>`) of an impl name (`impl Tr for S<T>` / `impl S<T>`)
fn impl_self_full(container: &str) -> &str {
    let n = container.strip_prefix("impl ").unwrap_or(container);
    match n.rfind(" for ") {
        Some(i) => n[i + 5..].trim(),
        None => n.trim(),
    }
}

/// The return type of a fn signature (`fn f(..) -> T where ..`)
fn ret_type(sig: &str) -> String {
    let b = sig.as_bytes();
    let mut d = 0i32;
    let mut i = 0;
    while i + 1 < b.len() {
        match b[i] {
            b'(' | b'[' | b'<' => d += 1,
            b')' | b']' => d -= 1,
            b'>' => d -= 1,
            b'-' if b[i + 1] == b'>' && d == 0 => {
                let r = &sig[i + 2..];
                let r = match r.find(" where ") {
                    Some(w) => &r[..w],
                    None => r,
                };
                return r.trim().to_string();
            }
            b'-' if b[i + 1] == b'>' => i += 1,
            _ => {}
        }
        i += 1;
    }
    String::new()
}

/// The element type of an iteration (the x of `for x in v`)
fn elem_type(t: &str) -> Option<String> {
    let h = type_head(t);
    let a = targs(t);
    match h.as_str() {
        "Chars" => Some("char".to_string()),
        "Bytes" => Some("u8".to_string()),
        "CharIndices" => Some("(usize, char)".to_string()),
        "Vec" | "VecDeque" | "[T]" | "HashSet" | "BTreeSet" | "BinaryHeap" | "Iter" | "IterMut" | "IntoIter" | "Drain" | "Option" | "Range" | "Rev" | "Skip" | "Take" | "Peekable" | "Box" | "Split" | "Lines" | "SplitWhitespace" => {
            match h.as_str() {
                "Split" | "Lines" | "SplitWhitespace" => Some("&str".to_string()),
                _ => a.into_iter().next(),
            }
        }
        _ => None,
    }
}

/// The type of `t[..]` (the same sequence for a range)
fn index_type(t: &str, range: bool) -> Option<String> {
    let h = type_head(t);
    let a = targs(t);
    if h == "String" || h == "str" {
        return Some("str".to_string());
    }
    if h.ends_with("Map") {
        return a.get(1).cloned();
    }
    if h == "Vec" || h == "VecDeque" || h == "[T]" || h.ends_with("Vec") {
        let e = a.into_iter().next()?;
        return Some(if range { format!("[{e}]") } else { e });
    }
    None
}

thread_local! {
    /// Name → the definitions with that name (file index, defs index). Only for the duration of one query (references looks up the same name hundreds of times).
    /// The key is the number of workspace files and the name
    static NAMED: RefCell<FxMap<(usize, String), Vec<(u32, u32)>>> = RefCell::new(FxMap::default());
}

/// The definitions of name name visible from file file (same crate, src, libraries)
fn named<'a>(ws: &[&'a FileIndex], file: usize, name: &str) -> Vec<(usize, &'a Def)> {
    let kr = ws[file].krate;
    let key = (ws.len(), name.to_string());
    let ids = NAMED.with(|m| m.borrow().get(&key).cloned());
    // (if the remembered indices do not match the current workspace (a reloaded file replaced, etc.), looked up again)
    let ok = |ids: &[(u32, u32)]| ids.iter().all(|&(i, j)| ws.get(i as usize).and_then(|w| w.defs.get(j as usize)).is_some_and(|d| d.name == name));
    let ids = match ids {
        Some(v) if ok(&v) => v,
        _ => {
            let v: Vec<(u32, u32)> = ws.iter().enumerate().flat_map(|(i, w)| w.defs_named(name).map(move |j| (i as u32, j as u32))).collect();
            NAMED.with(|m| m.borrow_mut().insert(key, v.clone()));
            v
        }
    };
    ids.into_iter()
        .map(|(i, j)| (i as usize, &ws[i as usize].defs[j as usize]))
        .filter(|(i, _)| { let w = ws[*i]; *i == file || w.in_src || w.lib || w.krate == kr })
        .collect()
}

thread_local! {
    /// The results of owner_names (only for the duration of one query: cleared at the entry of definition / references / hover / complete)
    static OWNERS: RefCell<FxMap<(String, u32), (Vec<String>, usize)>> = RefCell::new(FxMap::default());
}

/// The start of the `krate` numbers of library crates (so that they do not overlap the numbers of the workspace crates)
pub const LIB_KRATE_BASE: u32 = 1_000_000;

thread_local! {
    /// Crate name → root file (ws index). Set each time the LSP builds the list of the workspace
    static CRATE_ROOTS: RefCell<Vec<(String, usize)>> = const { RefCell::new(Vec::new()) };
}

/// Sets the crate names and root files (ws indices) (`std` / dependency crates / workspace crates)
pub fn set_crate_roots(v: Vec<(String, usize)>) {
    CRATE_ROOTS.with(|c| *c.borrow_mut() = v);
}

fn crate_root(ws: &[&FileIndex], name: &str) -> Option<usize> {
    CRATE_ROOTS.with(|c| c.borrow().iter().find(|(n, _)| n == name).map(|x| x.1)).filter(|&i| i < ws.len())
}

fn clear_memo() {
    OWNERS.with(|m| m.borrow_mut().clear());
    NAMED.with(|m| m.borrow_mut().clear());
}

/// The owner names used when looking up members of type o (head name): o itself, the right side of a type alias, Vec → slice, String → str (Deref targets),
/// the traits impl'd / derived for that type
fn owner_names(ws: &[&FileIndex], file: usize, o: &str) -> Vec<String> {
    owner_split(ws, file, o).0
}

/// owner_names, and how many of them are not traits (the type itself, aliases, Deref targets)
fn owner_split(ws: &[&FileIndex], file: usize, o: &str) -> (Vec<String>, usize) {
    // (an alias from `use a::Point as Pt` by its original name)
    let o = ws[file].use_alias(o).unwrap_or(o);
    let kr = ws[file].krate;
    let key = (o.to_string(), kr);
    if let Some(v) = OWNERS.with(|m| m.borrow().get(&key).cloned()) {
        return v;
    }
    let vis = || ws.iter().enumerate().filter(move |(i, w)| *i == file || w.in_src || w.lib || w.krate == kr).map(|x| x.1);
    let mut v = vec![o.to_string()];
    let mut cur = o.to_string();
    for _ in 0..3 {
        let next = vis()
            .flat_map(|w| w.defs.iter())
            .find(|d| d.kind == SymKind::TypeAlias && d.name == cur && d.owner.is_empty() && !d.ty.is_empty())
            .map(|d| type_head(&d.ty))
            .filter(|n| !n.is_empty() && *n != cur);
        match next {
            Some(n) if !v.contains(&n) => {
                v.push(n.clone());
                cur = n;
            }
            _ => break,
        }
    }
    let mut extra: Vec<String> = vec![];
    for x in &v {
        match x.as_str() {
            "Vec" => extra.push("[T]".to_string()),
            "String" => extra.push("str".to_string()),
            // (the methods of the number types are inside the core macros `uint_impl!` / `int_impl!`: symbols turns them into `impl __uint` / `impl __int`)
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => extra.extend(["__uint".to_string(), "__num".to_string()]),
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => extra.extend(["__int".to_string(), "__num".to_string()]),
            // (the pattern iterators of str: the impls inside the core macro `generate_pattern_iterators!` (symbols turns them into `impl __str_pat_iter`))
            "Split" | "RSplit" | "SplitN" | "RSplitN" | "SplitTerminator" | "RSplitTerminator" | "Matches" | "RMatches" | "MatchIndices" | "RMatchIndices" => extra.push("__str_pat_iter".to_string()),
            _ => {}
        }
    }
    // (the traits of built-in types (`impl Ord for usize` etc. are inside core macros and not visible): give them the methods of commonly used traits)
    let prim_traits: &[&str] = if v.iter().any(|x| matches!(x.as_str(), "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "char" | "bool")) {
        &["Ord", "PartialOrd", "PartialEq", "Eq", "Clone", "Copy", "Hash", "Default", "Debug", "Display", "ToString"]
    } else if v.iter().any(|x| matches!(x.as_str(), "f32" | "f64")) {
        &["PartialOrd", "PartialEq", "Clone", "Copy", "Default", "Debug", "Display", "ToString"]
    } else {
        &[]
    };
    v.extend(extra);
    let nc = v.len();
    let mut traits: Vec<String> = vec![];
    for w in vis() {
        for (s, t) in &w.impls {
            if v.contains(s) && !v.contains(t) && !traits.contains(t) {
                traits.push(t.clone());
            }
        }
    }
    for tr in prim_traits {
        if !traits.iter().any(|t| t == tr) {
            traits.push(tr.to_string());
        }
    }
    // (iterator types: the `impl Iterator` of slice's Iter etc. are inside core macros and not visible, so give them Iterator's methods)
    if v.iter().any(|x| is_iter_head(x)) {
        for tr in ["Iterator", "DoubleEndedIterator", "ExactSizeIterator"] {
            if !traits.iter().any(|t| t == tr) {
                traits.push(tr.to_string());
            }
        }
    }
    v.extend(traits);
    OWNERS.with(|m| m.borrow_mut().insert(key, (v.clone(), nc)));
    (v, nc)
}

/// The trait names of blanket impls (`impl<T: Display> ToString for T`). If only_common, only the commonly used ones offered in completion
fn blanket_traits(ws: &[&FileIndex], file: usize, only_common: bool) -> Vec<String> {
    let kr = ws[file].krate;
    let mut out: Vec<String> = vec![];
    for (i, w) in ws.iter().enumerate() {
        if !(i == file || w.in_src || w.lib || w.krate == kr) {
            continue;
        }
        for (s, t) in &w.impls {
            if s == "*" && !out.contains(t) && (!only_common || !w.lib || matches!(t.as_str(), "ToString" | "Into" | "TryInto" | "ToOwned" | "Borrow" | "BorrowMut")) {
                out.push(t.clone());
            }
        }
    }
    out
}

/// Of the candidates, if there are any whose owner is the type itself (concrete), only those (for `FxMap::default`, HashMap's impl rather than the trait's declaration)
fn prefer_concrete<'a>(c: Vec<(usize, &'a Def)>, concrete: &[String]) -> Vec<(usize, &'a Def)> {
    if c.iter().any(|x| concrete.contains(&x.1.owner)) {
        c.into_iter().filter(|x| concrete.contains(&x.1.owner) || x.1.owner.is_empty()).collect()
    } else {
        c
    }
}

/// The owner names of the members of type t (aliases and Deref targets too)
fn owners_of(ws: &[&FileIndex], file: usize, t: &str) -> Vec<String> {
    owners_of2(ws, file, t).0
}

/// owners_of, and those of them that are not traits
fn owners_of2(ws: &[&FileIndex], file: usize, t: &str) -> (Vec<String>, Vec<String>) {
    let mut v: Vec<String> = vec![];
    let mut c: Vec<String> = vec![];
    for x in expand(ws, file, t) {
        let (all, nc) = owner_split(ws, file, &type_head(&x));
        for (i, o) in all.into_iter().enumerate() {
            if i < nc && !c.contains(&o) {
                c.push(o.clone());
            }
            if !v.contains(&o) {
                v.push(o);
            }
        }
    }
    (v, c)
}

/// The type of member name of type o (looked up by its head name). The impl's type parameters are replaced by o's arguments (unwrap of `Option<Sig>` → Sig)
fn member_type(ws: &[&FileIndex], file: usize, o: &str, name: &str, kinds: &[SymKind]) -> Option<String> {
    expand(ws, file, o).iter().find_map(|t| member_type1(ws, file, t, name, kinds))
}

fn member_type1(ws: &[&FileIndex], file: usize, o: &str, name: &str, kinds: &[SymKind]) -> Option<String> {
    member_def(ws, file, o, name, kinds).map(|x| x.0)
}

/// (type, definition) of member name of receiver type o
fn member_def<'a>(ws: &[&'a FileIndex], file: usize, o: &str, name: &str, kinds: &[SymKind]) -> Option<(String, &'a Def)> {
    let (t, d) = member_def0(ws, file, o, name, kinds)?;
    Some((qualify_local_type(ws, file, d, t), d))
}

/// When the head name of type t of definition d is a type in the same file as d and there are other types with the same name (serde_json's `map::Iter` and
/// slice / path's `Iter`): qualifies it with the module (`map::Iter`). So that later lookups of associated types / methods do not mix them up
fn qualify_local_type(ws: &[&FileIndex], file: usize, d: &Def, t: String) -> String {
    let body = t.trim_start_matches('&').trim_start_matches("mut ").trim_start();
    let pre = &t[..t.len() - body.len()];
    let h = type_head(body);
    // (types with type parameters can be told apart by the number of type parameters (`Iter<'a, T>` / `Iter<'a, K, V>`), so only those without type parameters)
    if h.is_empty() || body.starts_with(|c: char| !c.is_ascii_uppercase()) || body[..body.find('<').unwrap_or(body.len())].contains("::") || !targs(body).is_empty() {
        return t;
    }
    let defs: Vec<(usize, &Def)> = named(ws, file, &h).into_iter().filter(|(_, x)| x.owner.is_empty() && matches!(x.kind, SymKind::Struct | SymKind::Enum | SymKind::Union)).collect();
    if defs.len() < 2 {
        return t;
    }
    let Some(i) = named(ws, file, &d.name).into_iter().find(|(_, x)| std::ptr::eq(*x, d)).map(|x| x.0) else { return t };
    match defs.iter().find(|(j, _)| *j == i).filter(|(_, x)| !x.module.is_empty()) {
        Some((_, x)) => format!("{pre}{}::{body}", x.module.join("::")),
        None => t,
    }
}

fn member_def0<'a>(ws: &[&'a FileIndex], file: usize, o: &str, name: &str, kinds: &[SymKind]) -> Option<(String, &'a Def)> {
    let os = owner_names(ws, file, &type_head(o));
    let kr = ws[file].krate;
    // (workspace ones first. Libraries in lib_rank order)
    let _ = kr;
    let cands = named(ws, file, name);
    let find = |os: &[String]| {
        cands
            .iter()
            .map(|(i, d)| (ws[*i].lib, *d))
            .filter(|(_, d)| kinds.contains(&d.kind) && os.contains(&d.owner) && !d.ty.is_empty())
            // (the ones whose owner is the type itself (`Option::as_ref`) before the trait ones (`AsRef::as_ref`))
            // (for other types with the same name (str's `Split<'a, P>` and slice's `Split<'a, T, P>`), the impls whose number of type parameters matches first)
            .min_by_key(|(lib, d)| {
                let mismatch = d.container.starts_with("impl") && targs(impl_self_full(&d.container)).len() != targs(o).len();
                // (for a receiver without type parameters (serde_json's `map::Iter`), impls of a type with the same name that needs type parameters (slice's `Iter<'a, T>`) are
                // not used: trait methods (`Iterator::find`) first)
                (*lib, targs(o).is_empty() && mismatch, d.owner != type_head(o), mismatch, if *lib { lib_rank(d) } else { 0 })
            })
            .map(|x| x.1)
    };
    // (if none, the Deref target (the `as_ref` of `RefMut<Option<T>>` is Option's); if none either, the methods of the traits of blanket impls)
    let d = match find(&os) {
        Some(d) => d,
        None if kinds.contains(&SymKind::Method) => {
            for x in expand(ws, file, o).into_iter().skip(1) {
                if let Some(d) = find(&owner_names(ws, file, &type_head(&x))) {
                    return Some((def_subst(d, &x, &d.ty), d));
                }
            }
            find(&blanket_traits(ws, file, false))?
        }
        None => return None,
    };
    // (a receiver of an alias `type Cur = Option<..>` is expanded up to the owner type before replacing. An alias with the same name as the owner
    // `io::Result<T>` (= `Result<T, Error>`) is also expanded until it matches the number of the impl's type parameters)
    let arity_differs = || d.container.starts_with("impl") && targs(impl_self_full(&d.container)).len() != targs(o).len();
    if d.owner == type_head(o) && arity_differs() {
        let n = targs(impl_self_full(&d.container)).len();
        if let Some(x) = expand(ws, file, o).into_iter().skip(1).find(|x| type_head(x) == d.owner && targs(x).len() == n) {
            let x = if o.starts_with('&') && !x.starts_with('&') { format!("&{x}") } else { x };
            return Some((def_subst(d, &x, &d.ty), d));
        }
    }
    if d.owner != type_head(o) && d.container.starts_with("impl") {
        if let Some(x) = expand(ws, file, o).into_iter().skip(1).find(|x| type_head(x) == d.owner) {
            let x = if o.starts_with('&') && !x.starts_with('&') { format!("&{x}") } else { x };
            return Some((def_subst(d, &x, &d.ty), d));
        }
    }
    Some((def_subst(d, o, &d.ty), d))
}

/// Replaces the type parameters in s, the written type of definition d (a member of receiver type o), from the receiver type (the type parameters of the impl's self type, Self,
/// the owners of the number types' macros, the undetermined type parameters of Iterator impls)
fn def_subst(d: &Def, o: &str, s: &str) -> String {
    let (params, args) = if d.container.starts_with("impl") {
        // (fits the impl's self type `Option<&T>` to the receiver type `Option<&Ty>` (aligned by position, `&T` would not be a name))
        let selfty = impl_self_full(&d.container);
        let vars: Vec<String> = selfty.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty() && w.len() <= 2 && w.starts_with(|c: char| c.is_ascii_uppercase())).map(|w| w.to_string()).collect();
        let mut b = vec![];
        unify(selfty, strip_ref(o), &vars, &mut b);
        if b.is_empty() {
            (targs(selfty), targs(o))
        } else {
            b.into_iter().unzip()
        }
    } else if d.kind == SymKind::Field {
        (d.params.clone(), targs(o))
    } else {
        (vec![], targs(o))
    };
    let mut t = subst(s, &params, &args);
    // (the impl's type parameters not determined by the self type (the B of `impl<B, I, F> Iterator for Map<I, F>`): if the receiver is an iterator,
    // its element type (usually Item in an Iterator impl))
    // (an impl's name does not have the type parameters of `impl<..>`, so they are recognized as names of 1 uppercase letter)
    if d.container.starts_with("impl") && d.container.contains("Iterator for ") && is_iter_head(&type_head(o)) {
        // (from the declared type before substitution: so as not to mix them up with the user's types like `D`)
        let mut unbound: Vec<String> = d.ty.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() == 1 && w.starts_with(|c: char| c.is_ascii_uppercase()) && !params.iter().any(|p| p == w)).map(|w| w.to_string()).collect();
        unbound.dedup();
        if unbound.len() == 1 {
            if let Some(item) = iter_item(o) {
                t = subst(&t, &unbound, &[item]);
            }
        }
    }
    // (the Self of methods written in a trait is the receiver type. Also the owners of methods inside the number types' macros (`uint_impl!`))
    subst(&t, &["Self".to_string(), "__uint".to_string(), "__int".to_string(), "__num".to_string()], &[o.to_string(), o.to_string(), o.to_string(), o.to_string()]).replace("$SelfT", o).replace("$ActualT", o).replace("$int_ty", o)
}

/// The type of the expression ending at token k (as far as known: locals, self, fields, return values of methods and fns, struct expressions, indexing, `?`)
fn expr_type(ws: &[&FileIndex], file: usize, k: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    if depth > 32 || k >= f.toks.len() {
        return None;
    }
    let self_ty = |at: usize| -> Option<String> { f.enclosing_fn(at).filter(|d| !d.owner.is_empty()).map(|d| impl_self_full(&d.container).to_string()) };
    let at = f.toks[k].1 as usize;
    // (a tuple index `x.0`: a number token preceded by `.`)
    if f.toks[k].0 == 1 && f.text.as_bytes()[f.toks[k].1 as usize].is_ascii_digit() && k >= 2 && f.sym(k - 1, b'.') && !(k >= 3 && f.sym(k - 2, b'.')) {
        let i: usize = f.tok(k).parse().ok()?;
        let base = expr_type(ws, file, k - 2, depth + 1)?;
        let base = expand(ws, file, strip_ref(&base)).into_iter().last()?;
        return match tuple_items(strip_ref(&base)) {
            Some(v) => v.get(i).cloned(),
            // (a field of a tuple struct `struct P(A, B)`: from the way the fields are written, kept in the definition)
            None => {
                let h = type_head(&base);
                let d = named(ws, file, &h).into_iter().map(|x| x.1).find(|d| d.kind == SymKind::Struct && d.owner.is_empty() && d.fnret.iter().any(|x| x.0 == 255))?;
                let inner = &d.fnret.iter().find(|x| x.0 == 255)?.2;
                let tys = targs(&format!("X<{inner}>"));
                let t = tys.get(i)?.trim();
                let t = t.strip_prefix("pub(crate)").or_else(|| t.strip_prefix("pub(super)")).or_else(|| t.strip_prefix("pub ")).unwrap_or(t).trim();
                let params = &d.params;
                Some(strip_lifetimes(&if params.is_empty() { t.to_string() } else { subst(t, params, &targs(&base)) }))
            }
        };
    }
    if let Some(t) = literal_type(f, k) {
        return Some(t);
    }
    if f.ident(k) {
        let r = f.tok(k);
        if k >= 1 && f.ident(k - 1) && f.tok(k - 1) == "as" {
            return Some(r.to_string());
        }
        // `a.b`: a field
        if k >= 1 && f.sym(k - 1, b'.') && !(k >= 2 && f.sym(k - 2, b'.')) {
            let o = expr_type(ws, file, k - 2, depth + 1)?;
            return member_type(ws, file, &o, r, &[SymKind::Field]);
        }
        if r == "self" {
            return self_ty(at);
        }
        if f.after_path(k) {
            // `Q::CONST` / `Enum::Variant`
            let q = f.tok(k.checked_sub(3)?).to_string();
            let q = if q == "Self" { self_ty(at)? } else { q };
            // (`a::b::P` / `m::CONST`: a unit struct / const under a module. The u32 of `u32::MAX` is a type (not the module's `core::char::MAX`))
            if q.starts_with(|c: char| c.is_ascii_lowercase()) && !PRIMS.contains(&q.as_str()) {
                let kr = f.krate;
                return named(ws, file, r)
                    .into_iter()
                    .filter(|(_, d)| d.owner.is_empty() && matches!(d.kind, SymKind::Struct | SymKind::Const | SymKind::Static))
                    .min_by_key(|(i, d)| (d.module.last().map(|m| m.as_str()) != Some(q.as_str()), ws[*i].krate != kr, ws[*i].lib))
                    .map(|(_, d)| if d.kind == SymKind::Struct { r.to_string() } else { d.ty.clone() });
            }
            return member_type(ws, file, &q, r, &[SymKind::Const, SymKind::Variant]).or(Some(q).filter(|_| r.starts_with(|c: char| c.is_ascii_uppercase())));
        }
        if let Some(b) = f.local(k, r) {
            return binding_type(ws, file, b, depth + 1);
        }
        // (`None`: Option)
        if r == "None" && !f.after_path(k) {
            return Some("Option<_>".to_string());
        }
        // (the value of a unit struct `K`)
        if r.starts_with(|c: char| c.is_ascii_uppercase()) && named(ws, file, r).iter().any(|(_, d)| d.kind == SymKind::Struct && d.owner.is_empty()) {
            return Some(r.to_string());
        }
        // (const / static)
        // (if things with the same name are in other files too (`const PRELUDE`), the one in the same file / same crate)
        let kr = f.krate;
        return named(ws, file, r)
            .into_iter()
            .filter(|(_, d)| d.owner.is_empty() && matches!(d.kind, SymKind::Const | SymKind::Static) && !d.ty.is_empty())
            .min_by_key(|(i, _)| (*i != file, ws[*i].krate != kr, ws[*i].lib))
            .map(|(_, d)| d.ty.clone());
    }
    if f.sym(k, b'?') && k >= 1 {
        let t = expr_type(ws, file, k - 1, depth + 1)?;
        return expand(ws, file, &t).iter().find_map(|t| match type_head(t).as_str() {
            "Option" | "Result" => targs(t).into_iter().next(),
            _ => None,
        });
    }
    if f.sym(k, b')') || f.sym(k, b'}') || f.sym(k, b']') {
        let close = f.text.as_bytes()[f.toks[k].1 as usize];
        let open = match close {
            b')' => b'(',
            b'}' => b'{',
            _ => b'[',
        };
        let mut d = 0;
        let mut p = k;
        let mut range = false;
        loop {
            if f.sym(p, close) {
                d += 1;
            } else if f.sym(p, open) {
                d -= 1;
                if d == 0 {
                    break;
                }
            } else if d == 1 && f.sym(p, b'.') && p >= 1 && f.sym(p - 1, b'.') {
                range = true;
            }
            if p == 0 {
                return None;
            }
            p -= 1;
        }
        let m = p.checked_sub(1)?;
        // Macros `vec![..]` / `format!(..)`
        if f.sym(m, b'!') && m >= 1 && f.ident(m - 1) {
            return match f.tok(m - 1) {
                // (`vec![a, b]` / `vec![x; n]`: the type of the first element)
                "vec" => {
                    let mut dd = 0;
                    let mut e0 = None;
                    for q in p + 1..k {
                        if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                            dd += 1;
                        } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                            dd -= 1;
                        } else if dd == 0 && (f.sym(q, b',') || f.sym(q, b';')) {
                            e0 = Some(q - 1);
                            break;
                        }
                    }
                    let e0 = e0.unwrap_or(k - 1);
                    let et = if k > p + 1 { expr_type(ws, file, e0, depth + 1) } else { None };
                    Some(format!("Vec<{}>", et.unwrap_or_else(|| "_".to_string())))
                }
                "format" => Some("String".to_string()),
                // (a `macro_rules!` of this file with one arm: the type of the last expression of the body block)
                mname => macro_result_type(ws, file, mname, depth),
            };
        }
        // Turbofish `x.collect::<Vec<_>>()` / `x.parse::<u8>()`
        if open == b'(' && f.sym(m, b'>') && !(m >= 1 && (f.sym(m - 1, b'=') || f.sym(m - 1, b'-'))) {
            let mut q = m;
            let mut dd = 0;
            while q > 0 {
                if f.sym(q, b'>') && !f.sym(q - 1, b'-') {
                    dd += 1;
                } else if f.sym(q, b'<') {
                    dd -= 1;
                    if dd == 0 {
                        break;
                    }
                }
                q -= 1;
            }
            if q >= 4 && f.sym(q - 1, b':') && f.sym(q - 2, b':') && f.ident(q - 3) && f.sym(q - 4, b'.') && q >= 5 {
                let tb = f.text[f.toks[q + 1].1 as usize..f.toks[m - 1].2 as usize].to_string();
                let o = expr_type(ws, file, q - 5, depth + 1)?;
                return method_call_type_at(ws, file, &o, f.tok(q - 3), m + 1, Some(&tb), depth);
            }
            // (the turbofish of a fn `serde_json::from_str::<Value>(s)` / `f::<T>(x)`: into the first type parameter of the return type)
            if q >= 3 && f.sym(q - 1, b':') && f.sym(q - 2, b':') && f.ident(q - 3) {
                let tb = f.text[f.toks[q + 1].1 as usize..f.toks[m - 1].2 as usize].to_string();
                let name = f.tok(q - 3);
                let qm = if q >= 6 && f.after_path(q - 3) { Some(f.tok(q - 6).to_string()) } else { None };
                let cands: Vec<&Def> = named(ws, file, name).into_iter().map(|x| x.1).filter(|d| d.owner.is_empty() && d.kind == SymKind::Function && !d.ty.is_empty()).collect();
                let d = cands.iter().find(|d| qm.as_ref().is_some_and(|qm| d.module.last() == Some(qm))).or(cands.first())?;
                let mut ret = d.ty.clone();
                if let Some(g) = ret.split(|c: char| !(c.is_alphanumeric() || c == '_')).find(|w| w.len() == 1 && w.starts_with(|c: char| c.is_ascii_uppercase())) {
                    let g = g.to_string();
                    ret = subst(&ret, &[g], &[tb]);
                }
                return Some(strip_lifetimes(&ret));
            }
            return None;
        }
        // A parenthesized expression `(e)` and a tuple expression `(a, b)` (when not preceded by something being called (a name, `)`, `]`))
        if open == b'(' && !(f.ident(m) && !KEYWORDS.contains(&f.tok(m))) && !f.sym(m, b')') && !f.sym(m, b']') && !f.sym(m, b'!') {
            let mut ends = vec![];
            let mut dd = 0;
            for q in p + 1..k {
                if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                    dd += 1;
                } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                    dd -= 1;
                } else if dd == 0 && f.sym(q, b',') {
                    ends.push(q - 1);
                }
            }
            let trailing = ends.last().is_some_and(|&e| e + 2 == k);
            if !trailing && k > p + 1 {
                ends.push(k - 1);
            }
            if ends.is_empty() {
                return Some("()".to_string());
            }
            if ends.len() == 1 && !trailing {
                // (a range expression `(a..b)` / `(a..=b)`: Range<T> (T is the type of whichever end is known))
                if let Some(t) = range_type(ws, file, p + 1, k - 1, depth) {
                    return Some(t);
                }
                return expr_type(ws, file, ends[0], depth + 1);
            }
            // (no answer if not a single element type is known. Unknown elements are `_`)
            let ts: Vec<Option<String>> = ends.iter().map(|&e| expr_type(ws, file, e, depth + 1)).collect();
            if ts.iter().all(|x| x.is_none()) {
                return None;
            }
            return Some(format!("({})", ts.into_iter().map(|x| x.unwrap_or_else(|| "_".to_string())).collect::<Vec<_>>().join(", ")));
        }
        if open == b'[' {
            // Indexing `a[i]` (when preceded by the end of an expression; also the tuple index in `t.1[0]`)
            let tuple_idx = f.toks[m].0 == 1 && f.text.as_bytes()[f.toks[m].1 as usize].is_ascii_digit() && m >= 1 && f.sym(m - 1, b'.');
            if f.ident(m) && (!KEYWORDS.contains(&f.tok(m)) || matches!(f.tok(m), "self" | "Self")) || f.sym(m, b')') || f.sym(m, b']') || tuple_idx {
                let t = expr_type(ws, file, m, depth + 1)?;
                // (also the user's types' `impl Index<..> for T { type Output = .. }`)
                return expand(ws, file, &t).iter().find_map(|t| index_type(t, range).or_else(|| assoc_type(ws, file, strip_ref(t), "Index", "Output")));
            }
            // An array expression `[a, b]` / `[x; n]`: `[T; N]` (the element type is the first element's)
            let mut first_end = None;
            let mut n = 0;
            let mut rep: Option<usize> = None;
            let mut dd = 0;
            for q in p + 1..k {
                if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                    dd += 1;
                } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                    dd -= 1;
                } else if dd == 0 && f.sym(q, b',') {
                    first_end.get_or_insert(q - 1);
                    n += 1;
                } else if dd == 0 && f.sym(q, b';') {
                    first_end.get_or_insert(q - 1);
                    rep = Some(q);
                }
            }
            if k == p + 1 {
                return None;
            }
            if !f.sym(k - 1, b',') && rep.is_none() {
                n += 1;
            }
            let e0 = first_end.unwrap_or(k - 1);
            let et = expr_type(ws, file, e0, depth + 1)?;
            let len = match rep {
                Some(q) => f.text[f.toks[q + 1].1 as usize..f.toks[k - 1].2 as usize].trim().to_string(),
                None => n.to_string(),
            };
            return Some(format!("[{et}; {len}]"));
        }
        // A match / if / block expression (one that is not a struct expression `T { .. }`): the first of the last expressions of the arms / blocks whose type is known
        if open == b'{' && !(f.ident(m) && (f.tok(m).starts_with(|c: char| c.is_ascii_uppercase()) || f.tok(m) == "Self")) {
            let s = block_expr_start(f, p)?;
            let mut tails = vec![];
            expr_tails(f, file, s, k, &mut tails);
            // (types without `_` first (the `Option<Type>` of `clif_ty(..)` rather than the `Option<_>` of `None`))
            let mut rough = None;
            for l in tails {
                let e = f.toks.partition_point(|t| (t.2 as usize) < l.end);
                if e < k {
                    if let Some(t) = expr_type(ws, file, e, depth + 1) {
                        let ts = f.toks.partition_point(|t| (t.1 as usize) < l.start);
                        let t = with_prefix(f, ts, e, t);
                        if !t.contains('_') {
                            return Some(t);
                        }
                        rough.get_or_insert(t);
                    }
                }
            }
            return rough;
        }
        if !f.ident(m) {
            return None;
        }
        let name = f.tok(m);
        if open == b'{' {
            // A struct expression `T { .. }` (only names starting with an uppercase letter: to tell it from blocks)
            if name == "Self" {
                return self_ty(at);
            }
            if !name.starts_with(|c: char| c.is_ascii_uppercase()) {
                return None;
            }
            // (a struct with type parameters: the type parameters are decided from the field initializer expressions, `Wrapper { inner: Point::new() }` → Wrapper<Point>)
            let params = named(ws, file, name).into_iter().map(|x| x.1).find(|d| d.owner.is_empty() && d.kind == SymKind::Struct).map(|d| d.params.clone()).unwrap_or_default();
            if params.is_empty() {
                return Some(name.to_string());
            }
            let mut args = vec!["_".to_string(); params.len()];
            let mut j = p + 1;
            while j < k {
                if f.ident(j) && (f.sym(j - 1, b'{') || f.sym(j - 1, b',')) && (f.sym(j + 1, b':') && !f.sym(j + 2, b':')) {
                    // (the end of the EXPR of `field: EXPR`)
                    let mut e = j + 2;
                    let mut d = 0;
                    while e < k {
                        if f.sym(e, b'(') || f.sym(e, b'[') || f.sym(e, b'{') {
                            d += 1;
                        } else if f.sym(e, b')') || f.sym(e, b']') || f.sym(e, b'}') {
                            d -= 1;
                        } else if d == 0 && f.sym(e, b',') {
                            break;
                        }
                        e += 1;
                    }
                    let field = f.tok(j);
                    if let Some(fd) = named(ws, file, field).into_iter().map(|x| x.1).find(|d| d.kind == SymKind::Field && d.owner == name) {
                        if let Some(pi) = params.iter().position(|p| *p == fd.ty) {
                            if let Some(t) = expr_type(ws, file, e - 1, depth + 1) {
                                args[pi] = t;
                            }
                        }
                    }
                    j = e;
                }
                j += 1;
            }
            return Some(format!("{name}<{}>", args.join(", ")));
        }
        if m >= 1 && f.sym(m - 1, b'.') {
            let o = expr_type(ws, file, m - 2, depth + 1)?;
            return method_call_type(ws, file, &o, name, m, depth);
        }
        if f.after_path(m) && m >= 3 && f.ident(m - 3) {
            let q = f.tok(m - 3).to_string();
            let q = if q == "Self" { self_ty(at)? } else { q };
            // (`a::b::T(..)` / `m::f(..)`: a tuple struct / fn under a module)
            if q.starts_with(|c: char| c.is_ascii_lowercase()) && q != "self" && q != "super" && q != "crate" {
                // (if there are several fns with the same name, the one whose module name is the qualifier q: `zustc::macros::expand` is the expand of macros)
                let cands: Vec<&Def> = named(ws, file, name).into_iter().map(|x| x.1).filter(|d| d.owner.is_empty() && matches!(d.kind, SymKind::Struct | SymKind::Function)).collect();
                let pick = cands.iter().find(|d| d.module.last() == Some(&q)).or(cands.first());
                if let Some(t) = pick.map(|d| if d.kind == SymKind::Struct { name.to_string() } else { apply_call_args(ws, file, d.ty.clone(), &d.fnret, m + 1, depth) }).filter(|t| !t.is_empty()) {
                    return Some(strip_lifetimes(&norm_proj_ws(ws, file, &t)));
                }
            }
            // (an associated fn `Q::f(args)`: the type parameters are decided from the arguments (`Mutex::new(x)` → Mutex<type of x>))
            if let Some((r, d)) = expand(ws, file, &q).iter().find_map(|t| member_def(ws, file, t, name, &[SymKind::Method, SymKind::Function, SymKind::Variant])) {
                let r = apply_call_args(ws, file, r, &d.fnret, m + 1, depth);
                return Some(strip_lifetimes(&norm_proj_ws(ws, file, &r)));
            }
            return name.starts_with(|c: char| c.is_ascii_uppercase()).then(|| q.clone());
        }
        if name == "Some" {
            // (`Some(e)`: if the type of e is known, Option<type of e>)
            let close = close_of(f, m + 1);
            let inner = expr_end(f, m + 2, false).filter(|&e| e + 1 == close).and_then(|e| expr_type(ws, file, e, depth + 1));
            return Some(format!("Option<{}>", inner.unwrap_or_else(|| "_".to_string())));
        }
        if name == "Ok" || name == "Err" {
            return Some("Result<_, _>".to_string());
        }
        if name.starts_with(|c: char| c.is_ascii_uppercase()) {
            return Some(name.to_string());
        }
        // (a call of a local closure `all()`: that closure's return type)
        if let Some(b) = f.local(m, name) {
            if let Some(ty) = binding_type(ws, file, b, depth + 1) {
                if ty.starts_with("impl Fn") {
                    return ty.find(") -> ").map(|i| ty[i + 5..].trim().to_string());
                }
            }
            return None;
        }
        // (the fn called: the one decided by name resolution (use, same module). There are fns with the same name in other crates too (`run`))
        if let Some(t) = def_at(ws, file, f.toks[m].1 as usize).first().and_then(|l| ws[l.file].defs.iter().find(|d| d.start == l.start && d.kind == SymKind::Function)).map(|d| d.ty.clone()) {
            return Some(t).filter(|t| !t.is_empty());
        }
        let kr = f.krate;
        return named(ws, file, name)
            .into_iter()
            .filter(|(_, d)| d.owner.is_empty() && d.kind == SymKind::Function && !d.ty.is_empty())
            .min_by_key(|(i, _)| (*i != file, ws[*i].krate != kr, ws[*i].lib))
            .map(|(_, d)| d.ty.clone());
    }
    if f.toks[k].0 == 2 && f.text.as_bytes()[f.toks[k].1 as usize] == b'"' {
        return Some("&str".to_string());
    }
    None
}

/// The token before the end of the statement (`;` / `{` / `else`, outside brackets). From a
fn expr_end(f: &FileIndex, a: usize, stop_brace: bool) -> Option<usize> {
    let n = f.toks.len();
    let mut j = a;
    let mut d = 0;
    while j < n {
        if f.sym(j, b'{') && d == 0 && stop_brace {
            break;
        }
        if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'{') {
            d += 1;
        } else if f.sym(j, b')') || f.sym(j, b']') || f.sym(j, b'}') {
            if d == 0 {
                break;
            }
            d -= 1;
        } else if d == 0 && (f.sym(j, b';') || (f.ident(j) && f.tok(j) == "else")) {
            break;
        }
        j += 1;
    }
    (j > a).then(|| j - 1)
}

/// The type of local binding b (type annotation, initializer expression, parameter type, the element of `for x in ..`, the contents of `if let Some(x) = ..`)
fn binding_type(ws: &[&FileIndex], file: usize, b: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let n = f.toks.len();
    if f.sym(b + 1, b':') && !f.sym(b + 2, b':') {
        let a = f.toks.get(b + 2)?.1 as usize;
        // (type annotation: up to `=` / `,` / `)` / `;`)
        let mut j = b + 2;
        let mut d = 0;
        while j < n {
            if f.sym(j, b'<') || f.sym(j, b'(') || f.sym(j, b'[') {
                d += 1;
            } else if (f.sym(j, b'>') && !(j > 0 && f.sym(j - 1, b'-'))) || f.sym(j, b')') || f.sym(j, b']') {
                if d == 0 {
                    break;
                }
                d -= 1;
            } else if d == 0 && (f.sym(j, b'=') || f.sym(j, b',') || f.sym(j, b';') || f.sym(j, b'{') || f.sym(j, b'|')) {
                break;
            }
            j += 1;
        }
        let e = if j > b + 2 { f.toks[j - 1].2 as usize } else { a };
        let t = f.text[a..e].to_string();
        if type_head(&t) == "Self" {
            return f.enclosing_fn(a).filter(|d| !d.owner.is_empty()).map(|d| impl_self_full(&d.container).to_string());
        }
        return (!type_head(&t).is_empty() || strip_ref(&t).starts_with('(') || t.starts_with("*const ") || t.starts_with("*mut ")).then_some(t);
    }
    // A binding in a pattern (nested tuple / variant / struct / slice): the path from the outside is applied to the type at the place of the pattern
    if let Some(t) = pattern_type(ws, file, b, depth) {
        return Some(t);
    }
    // `let (a, b) = EXPR;` / `let (a, b): (T, U) = ..;` (a tuple pattern)
    if let Some((after, path)) = let_tuple_pattern(f, b) {
        let ty = if f.sym(after, b':') && !f.sym(after + 1, b':') {
            let mut e = after + 1;
            while e < f.toks.len() && !(f.sym(e, b'=') && !f.sym(e + 1, b'=')) && !f.sym(e, b';') {
                e += 1;
            }
            f.text[f.toks.get(after + 1)?.1 as usize..f.toks.get(e - 1)?.2 as usize].trim().to_string()
        } else if f.sym(after, b'=') {
            let e = expr_end(f, after + 1, false)?;
            expr_type(ws, file, e, depth + 1)?
        } else {
            return None;
        };
        let mut ty = expand(ws, file, &ty).into_iter().last().unwrap_or(ty);
        for &i in &path {
            let inner = strip_ref(&ty).to_string();
            ty = tuple_items(&inner)?.get(i)?.clone();
        }
        return Some(ty);
    }
    // `for x in EXPR {` / `for (i, x) in EXPR {` (for a tuple pattern, the index within the element)
    if let Some((in_tok, path)) = for_pattern(f, b) {
        let e = expr_end(f, in_tok + 1, true)?;
        let t = match range_type(ws, file, in_tok + 1, e, depth) {
            Some(t) => t,
            None => expr_type(ws, file, e, depth + 1)?,
        };
        // (`for x in &v` / `&mut v`: iterating references)
        let t = if f.sym(in_tok + 1, b'&') && !t.starts_with('&') { if f.tok(in_tok + 2) == "mut" { format!("&mut {t}") } else { format!("&{t}") } } else { t };
        let mut ty = if is_iter_head(&type_head(&t)) { iter_item_ws(ws, file, &t) } else { iter_item(&t).or_else(|| elem_type(&t)).or_else(|| iter_item_ws(ws, file, &t)) }?;
        for &i in &path {
            let inner = strip_ref(&ty).to_string();
            ty = tuple_items(&inner)?.get(i)?.clone();
        }
        return Some(ty);
    }
    // `if let Some(x) = EXPR` / `while let Some((a, b)) = EXPR` / `let Ok(x) = EXPR else` (tuple patterns inside too)
    if let Some((wrap, eq, path)) = wrap_pattern(f, b) {
        let e = expr_end(f, eq + 1, true)?;
        let t = expr_type(ws, file, e, depth + 1)?;
        let t = expand(ws, file, &t).into_iter().last().unwrap_or(t);
        let mut ty = match (type_head(&t).as_str(), wrap) {
            ("Option", "Some") | ("Result", "Ok") => targs(&t).into_iter().next(),
            ("Result", "Err") => targs(&t).into_iter().nth(1),
            _ => None,
        }?;
        for &i in &path {
            let inner = strip_ref(&ty).to_string();
            ty = tuple_items(&inner)?.get(i)?.clone();
        }
        return Some(ty);
    }
    // A binding in a `V(a, b)` / `E::V(a, b)` pattern: the type in the variant's declaration
    if let Some(t) = variant_binding_type(ws, file, b, depth) {
        return Some(t);
    }
    // Closure parameters `x.iter().map(|d| ..)` / `opt.map(|v| ..)`: the receiver's element type
    if let Some(t) = closure_param_type(ws, file, b, depth) {
        return Some(t);
    }
    // (`let x;`: the type of the later assignment `x = EXPR;`)
    if f.sym(b + 1, b';') && b >= 1 && (f.tok(b - 1) == "let" || f.tok(b - 1) == "mut") {
        return refine_by_use(ws, file, b, "_", depth);
    }
    // `let x = EXPR;`
    if f.sym(b + 1, b'=') && !f.sym(b + 2, b'=') && b >= 1 && (f.tok(b - 1) == "let" || f.tok(b - 1) == "mut") {
        // (a closure `|a: T, b: U| body` → `impl Fn(T, U) -> R`)
        let c = if f.tok(b + 2) == "move" { b + 3 } else { b + 2 };
        // (also `|| ..` without parameters)
        if f.sym(c, b'|') {
            let mut j = c + 1;
            let mut tys = vec![];
            while j < n && !f.sym(j, b'|') {
                if f.sym(j, b':') {
                    let a = j + 1;
                    while j + 1 < n && !f.sym(j + 1, b',') && !f.sym(j + 1, b'|') {
                        j += 1;
                    }
                    tys.push(f.text[f.toks[a].1 as usize..f.toks[j].2 as usize].to_string());
                } else if f.sym(j, b',') && tys.len() < f.text[f.toks[c].1 as usize..f.toks[j].1 as usize].matches(',').count() {
                    tys.push("_".to_string());
                }
                j += 1;
            }
            // (the return type: `-> T` if written, otherwise the type of the body expression)
            let ret = if f.sym(j + 1, b'-') && f.sym(j + 2, b'>') {
                let mut e = j + 3;
                while e < n && !f.sym(e, b'{') {
                    e += 1;
                }
                f.text.get(f.toks.get(j + 3).map(|x| x.1 as usize).unwrap_or(0)..f.toks.get(e).map(|x| x.1 as usize).unwrap_or(0)).map(|r| format!(" -> {}", r.trim())).unwrap_or_default()
            } else {
                expr_end(f, j + 1, false).and_then(|e| expr_type(ws, file, e, depth + 1)).map(|r| format!(" -> {r}")).unwrap_or_default()
            };
            return Some(format!("impl Fn({}){ret}", tys.join(", ")));
        }
        if let Some(e) = expr_end(f, b + 2, false) {
            if let Some(t) = expr_type(ws, file, e, depth + 1) {
                // (ones whose element is unknown like `let mut v = vec![];` / `Vec::new()`: from a later `v.push(x)` / `v = EXPR`)
                let bare = targs(&t).is_empty() && matches!(type_head(&t).as_str(), "Vec" | "VecDeque" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "Option");
                // (the lifetime `'_` is not a hole)
                if t.replace("'_", "").contains('_') || bare {
                    if let Some(r) = refine_by_use(ws, file, b, &t, depth) {
                        return Some(r);
                    }
                }
                // (`let p = &v["x"];` / `&mut x`: a leading reference. When the type of the end of the expression is the type of the whole as it is (`&a.b` / `&v[i]`))
                if (f.sym(b + 2, b'&') || f.sym(b + 2, b'*')) && !t.starts_with('&') {
                    return Some(with_prefix(f, b + 2, e, t));
                }
                return Some(t);
            }
        }
    }
    let t = f.local_type(b)?;
    // (`let mut v = Vec::new();`: a collection without type arguments, from its later uses)
    if targs(&t).is_empty() && matches!(t.as_str(), "Vec" | "VecDeque" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet") {
        if let Some(r) = refine_by_use(ws, file, b, &t, depth) {
            return Some(r);
        }
    }
    Some(t)
}

/// The type of binding b in a pattern: the type at that position in the declaration of the V of `E::V(.., b, ..)`. If the type of the matched expression is known, type arguments are applied
fn variant_binding_type(ws: &[&FileIndex], file: usize, b: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    if !(b >= 1 && (f.sym(b - 1, b'(') || f.sym(b - 1, b',') || matches!(f.tok(b - 1), "ref" | "mut")) && (f.sym(b + 1, b',') || f.sym(b + 1, b')'))) {
        return None;
    }
    // (the enclosing `(`, and which position b is in it)
    let mut j = b;
    let mut d = 0;
    let mut idx = 0;
    loop {
        if j == 0 {
            return None;
        }
        j -= 1;
        if f.sym(j, b')') || f.sym(j, b']') || f.sym(j, b'}') {
            d += 1;
        } else if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'{') {
            if d == 0 {
                break;
            }
            d -= 1;
        } else if d == 0 && f.sym(j, b',') {
            idx += 1;
        } else if d == 0 && (f.sym(j, b';') || f.sym(j, b'=')) {
            return None;
        }
    }
    if !f.sym(j, b'(') || j == 0 || !f.ident(j - 1) {
        return None;
    }
    let v = f.tok(j - 1);
    if !v.starts_with(|c: char| c.is_ascii_uppercase()) {
        return None;
    }
    let scrut = || scrutinee_type(ws, file, b, depth + 1);
    match v {
        "Some" | "Ok" | "Err" => {
            let t = scrut()?;
            let want = if v == "Some" { "Option" } else { "Result" };
            let t = expand(ws, file, &t).into_iter().find(|t| type_head(t) == want)?;
            return targs(&t).into_iter().nth(if v == "Err" { 1 } else { 0 }).filter(|t| t != "_");
        }
        _ => {}
    }
    let q = if f.after_path(j - 1) && j >= 4 { Some(f.tok(j - 4)) } else { None };
    let kr = f.krate;
    // (if an enum with the same name is in another crate too (`Ty::Array`): the one of the enum reached by following the qualifier `Ty` through use, or the one in this file / the same crate)
    let qdef = if q.is_some() { def_at(ws, file, f.toks[j - 4].1 as usize).first().copied() } else { None };
    let qmod: Option<Vec<String>> = qdef.and_then(|l| ws[l.file].defs.iter().find(|d| d.start == l.start && d.kind == SymKind::Enum).map(|d| d.module.to_vec()));
    let (vi, vd) = named(ws, file, v)
        .into_iter()
        .filter(|(_, d)| d.kind == SymKind::Variant && q.is_none_or(|q| d.owner == q || (q == "Self" && f.enclosing_owner(f.toks[b].1 as usize).as_deref() == Some(&d.owner))))
        .min_by_key(|(i, d)| (qmod.as_ref().is_some_and(|m| d.module[..] != m[..]), *i != file, ws[*i].krate != kr, ws[*i].lib))?;
    let _ = kr;
    let vt = ws[vi].text.get(vd.item_start..vd.item_end)?;
    let inner = &vt[vt.find('(')? + 1..vt.rfind(')')?];
    let tys = targs(&format!("X<{inner}>"));
    let ty = tys.get(idx)?.clone();
    // (applies the type arguments of the matched expression's type to the enum's type parameters)
    if !vd.params.is_empty() {
        if let Some(st) = scrut() {
            if let Some(st) = expand(ws, file, &st).into_iter().find(|t| type_head(t) == vd.owner) {
                return Some(subst(&ty, &vd.params, &targs(&st)));
            }
        }
    }
    Some(ty)
}

/// The type of the expression matched against token k in a pattern (`match EXPR { P => }` / `let P = EXPR` / `if let P = EXPR`)
fn scrutinee_type(ws: &[&FileIndex], file: usize, k: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let n = f.toks.len();
    let mut j = k + 1;
    let mut d = 0i32;
    while j < n && j < k + 256 {
        if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'{') {
            d += 1;
        } else if f.sym(j, b')') || f.sym(j, b']') || f.sym(j, b'}') {
            d -= 1;
        } else if f.sym(j, b';') {
            return None;
        } else if d <= 0 && f.sym(j, b'=') && f.sym(j + 1, b'>') {
            // (match: the matched expression is before the enclosing `{` that precedes the start of the arm)
            let (a, _) = f.arm_pat(j);
            let mut q = a;
            let mut dd = 0;
            while q > 0 {
                q -= 1;
                if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                    dd += 1;
                } else if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                    if dd == 0 {
                        break;
                    }
                    dd -= 1;
                }
            }
            return if f.sym(q, b'{') && q >= 1 { expr_type(ws, file, q - 1, depth + 1) } else { None };
        } else if d <= 0 && f.sym(j, b'=') && !f.sym(j + 1, b'=') && !(f.sym(j - 1, b'=') || f.sym(j - 1, b'!') || f.sym(j - 1, b'<') || f.sym(j - 1, b'>')) {
            // (`==` / `!=` / `<=` / `>=` (in a guard) are not assignments)
            let e = expr_end(f, j + 1, true)?;
            return expr_type(ws, file, e, depth + 1);
        }
        j += 1;
    }
    None
}

const PRIMS: [&str; 17] = ["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64", "bool", "char", "str"];

/// A completion candidate
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    /// The kind of the definition (None for locals and keywords)
    pub kind: Option<SymKind>,
    pub local: bool,
    pub detail: String,
}

const KEYWORDS: [&str; 38] = [
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
];

/// The names of the prelude (std things visible without `use`) and commonly used macros
const PRELUDE: [&str; 31] = [
    "Option", "Some", "None", "Result", "Ok", "Err", "Vec", "String", "Box", "ToString", "ToOwned", "Clone", "Copy", "Default", "Iterator", "IntoIterator", "Extend",
    "Drop", "Fn", "FnMut", "FnOnce", "PartialEq", "Eq", "PartialOrd", "Ord", "Send", "Sync", "Sized", "From", "Into", "AsRef",
];
const MACROS: [&str; 22] = [
    "println", "print", "eprintln", "eprint", "format", "vec", "assert", "assert_eq", "assert_ne", "panic", "write", "writeln", "unreachable", "todo", "unimplemented",
    "matches", "debug_assert", "debug_assert_eq", "include_str", "concat", "stringify", "dbg",
];

/// Completion at off (byte): `x.|` (members of the receiver type), `Q::|` (members of a type / items of a module), anything else (locals, items,
/// prelude, keywords). Narrowed by the part of the identifier before the cursor. Returns (candidates, whether it was cut off)
pub fn complete(ws: &[&FileIndex], file: usize, off: usize) -> (Vec<Item>, bool) {
    clear_memo();
    let f = ws[file];
    let b = f.text.as_bytes();
    let off = off.min(b.len());
    let mut ps = off;
    while ps > 0 && (b[ps - 1].is_ascii_alphanumeric() || b[ps - 1] == b'_' || b[ps - 1] >= 0x80) {
        ps -= 1;
    }
    let prefix = &f.text[ps..off];
    let mut q = ps;
    while q > 0 && (b[q - 1] == b' ' || b[q - 1] == b'\t') {
        q -= 1;
    }
    let kr = f.krate;
    let all = || ws.iter().enumerate().filter(move |(i, w)| *i == file || w.in_src || w.lib || w.krate == kr).flat_map(|(i, w)| w.defs.iter().map(move |d| (i, d)));
    let mut out: Vec<(u32, u8, Item)> = vec![];
    let mut dot_known = false;
    let mut seen: FxMap<String, ()> = FxMap::default();
    let mut add = |out: &mut Vec<(u32, u8, Item)>, prio: u8, name: &str, kind: Option<SymKind>, local: bool, detail: String, loose: bool| {
        let Some(sc) = crate::symbols::match_score(prefix, name) else { return };
        if !loose && sc > 2 && !prefix.is_empty() {
            return;
        }
        if seen.insert(name.to_string(), ()).is_some() {
            return;
        }
        out.push((sc, prio, Item { label: name.to_string(), kind, local, detail }));
    };
    // (the token just before: `.` / `::`)
    let tk = f.toks.partition_point(|t| (t.2 as usize) <= q);
    let before = tk.checked_sub(1);
    let dot = before.filter(|&k| f.sym(k, b'.') && !(k >= 1 && f.sym(k - 1, b'.')) && f.toks[k].2 as usize == q);
    let path = before.filter(|&k| k >= 2 && f.sym(k, b':') && f.sym(k - 1, b':') && f.ident(k - 2) && f.toks[k].2 as usize == q);
    if let Some(k) = dot {
        let kinds = [SymKind::Method, SymKind::Field];
        match k.checked_sub(1).and_then(|r| expr_type(ws, file, r, 0)).filter(|t| !type_head(t).is_empty() && type_head(t) != "_") {
            Some(o) => {
                dot_known = true;
                let mut os = owners_of(ws, file, &o);
                os.extend(blanket_traits(ws, file, true));
                // (looked up per owner with the index. The order is the same as before: file → definition)
                let mut hits: Vec<(usize, usize)> = vec![];
                for (i, w) in ws.iter().enumerate().filter(|(i, w)| *i == file || w.in_src || w.lib || w.krate == kr) {
                    for o in &os {
                        hits.extend(w.defs_owned(o).map(|j| (i, j)));
                    }
                }
                hits.sort_unstable();
                for (i, j) in hits {
                    let d = &ws[i].defs[j];
                    // (associated fns that do not take `self` (`default()`) are not offered after `.`: as in rust-analyzer)
                    if kinds.contains(&d.kind) && (d.kind != SymKind::Method || d.has_self) {
                        add(&mut out, if ws[i].lib { 1 } else { 0 }, &d.name, Some(d.kind), false, d.container.to_string(), true);
                    }
                }
            }
            None => {
                // (type unknown: the names of the methods and fields of the workspace)
                for (i, d) in all() {
                    if !ws[i].lib && kinds.contains(&d.kind) {
                        add(&mut out, 0, &d.name, Some(d.kind), false, d.container.to_string(), false);
                    }
                }
            }
        }
    } else if let Some(k) = path {
        let q = f.tok(k - 2).to_string();
        let q = if q == "Self" { f.enclosing_owner(off).unwrap_or(q) } else { q };
        let os = owner_names(ws, file, &q);
        for (i, d) in all() {
            let mem = os.contains(&d.owner) && d.kind != SymKind::Impl;
            let in_mod = d.owner.is_empty() && d.kind != SymKind::Impl && d.module.last() == Some(&q);
            if mem || in_mod {
                add(&mut out, if ws[i].lib { 1 } else { 0 }, &d.name, Some(d.kind), false, d.container.to_string(), true);
            }
        }
        // (names that module q brought in (re-exported) by use: `std::sync::Mutex`)
        for w in ws.iter().filter(|w| w.module.last() == Some(&q)) {
            for (local, _) in &w.uses {
                if local != "*" && local != "self" {
                    add(&mut out, 1, local, None, false, String::new(), true);
                }
            }
        }
    } else {
        // Locals
        let ck = f.toks.partition_point(|t| (t.1 as usize) < ps).min(f.toks.len().saturating_sub(1));
        if !f.toks.is_empty() {
            for j in f.live(ck).into_iter().rev() {
                if f.toks[j].1 as usize != ps {
                    add(&mut out, 0, f.tok(j), None, true, String::new(), true);
                }
            }
        }
        // Items (workspace, what was brought in by use, prelude)
        let used: Vec<&str> = f.uses.iter().map(|u| u.0.as_str()).collect();
        for (i, d) in all() {
            if d.owner.is_empty() && ITEM_KINDS.contains(&d.kind) && (!ws[i].lib || used.contains(&d.name.as_str())) {
                add(&mut out, if ws[i].lib { 2 } else { 1 }, &d.name, Some(d.kind), false, d.container.to_string(), false);
            }
        }
        for u in &used {
            if *u != "*" && *u != "self" {
                add(&mut out, 2, u, None, false, String::new(), false);
            }
        }
        for p in PRELUDE {
            add(&mut out, 2, p, None, false, String::new(), false);
        }
        for m in MACROS {
            add(&mut out, 3, m, Some(SymKind::Macro), false, String::new(), false);
        }
        for kw in KEYWORDS {
            add(&mut out, 3, kw, None, false, "keyword".to_string(), false);
        }
    }
    out.sort_by(|a, b| (a.0, a.1, a.2.label.len(), &a.2.label).cmp(&(b.0, b.1, b.2.label.len(), &b.2.label)));
    // (all members when the receiver type is known, otherwise up to 200)
    let cap = if dot_known { 2000 } else { 200 };
    let cut = out.len() > cap;
    out.truncate(cap);
    (out.into_iter().map(|x| x.2).collect(), cut)
}

/// Hover on the identifier at off: the definition's signature (and its container, doc comment); for a local, the binding and its type
pub fn hover(ws: &[&FileIndex], file: usize, off: usize, load: Load) -> Option<String> {
    clear_memo();
    // (string / char literals: the type and the value. The same shape as rust-analyzer)
    if ws[file].tok_at(off).is_none() && ws[file].capture_at(off).is_none() {
        if let Some(h) = literal_hover(ws[file], off) {
            return Some(h);
        }
    }
    // (built-in types)
    if let Some(k) = ws[file].tok_at(off) {
        let w = ws[file].tok(k);
        if PRIMS.contains(&w) {
            return Some(format!("```rust\n{w}\n```"));
        }
    }
    let d = def_at(ws, file, off);
    let mut t = *d.first()?;
    // (a method made by derive (it goes to the name in `#[derive(Default)]`): shows the declaration of that trait's method)
    if !ws[t.file].slim && ws[t.file].tok_at(t.start).is_some_and(|j| ws[t.file].in_derive(j)) {
        let tr = ws[t.file].text[t.start..t.end].to_string();
        if let Some(name) = ws[file].tok_at(off).map(|k| ws[file].tok(k).to_string()) {
            if let Some((i, d)) = named(ws, file, &name).into_iter().find(|(_, d)| d.owner == tr) {
                t = loc(i, d);
            }
        }
    }
    let tf = ws[t.file];
    let held;
    let text: &str = if tf.slim {
        held = load(t.file)?;
        &held.text
    } else {
        &tf.text
    };
    if let Some(def) = tf.defs.iter().find(|x| x.start == t.start && x.kind != SymKind::Impl) {
        let head = if !def.owner.is_empty() {
            def.container.to_string()
        } else {
            def.module.join("::")
        };
        let sig = signature(text.get(def.item_start..def.item_end.min(text.len()))?, def.kind);
        let docs = doc_comment(text, def.item_start);
        let mut s = String::from("```rust\n");
        if !head.is_empty() {
            s.push_str(&head);
            s.push('\n');
        }
        s.push_str(&sig);
        s.push_str("\n```");
        if !docs.is_empty() {
            s.push_str("\n\n---\n\n");
            s.push_str(&docs);
        }
        return Some(s);
    }
    // A local
    let name = text.get(t.start..t.end)?;
    let b = tf.toks.partition_point(|x| (x.1 as usize) < t.start);
    let ty = if b < tf.toks.len() { binding_type(ws, t.file, b, 0) } else { None };
    Some(match ty {
        Some(ty) => format!("```rust\n{name}: {ty}\n```"),
        None => format!("```rust\n{name}\n```"),
    })
}

/// The signature from an item's text (attributes removed, up to before the body's `{`; whitespace collapsed)
fn signature(item: &str, kind: SymKind) -> String {
    let mut s = item.trim_start();
    while s.starts_with("#[") || s.starts_with("#![") {
        let mut d = 0;
        let mut cut = s.len();
        for (i, c) in s.char_indices() {
            if c == '[' {
                d += 1;
            } else if c == ']' {
                d -= 1;
                if d == 0 {
                    cut = i + 1;
                    break;
                }
            }
        }
        s = s[cut..].trim_start();
    }
    let body = matches!(kind, SymKind::Function | SymKind::Method | SymKind::Struct | SymKind::Enum | SymKind::Union | SymKind::Trait | SymKind::Module | SymKind::Impl | SymKind::Macro);
    let mut end = s.len();
    if body {
        let mut d = 0i32;
        for (i, c) in s.char_indices() {
            match c {
                '(' | '[' | '<' => d += 1,
                ')' | ']' | '>' => d -= 1,
                '{' if d <= 0 => {
                    end = i;
                    break;
                }
                ';' if d <= 0 => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
    } else if kind == SymKind::Const || kind == SymKind::Static {
        end = s.find(';').unwrap_or(s.len());
    }
    let mut out = String::new();
    for w in s[..end].split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    if out.len() > 400 {
        let mut c = 400;
        while !out.is_char_boundary(c) {
            c -= 1;
        }
        out.truncate(c);
        out.push_str(" …");
    }
    out
}

/// The `///` lines before at (the start of the item)
fn doc_comment(text: &str, at: usize) -> String {
    let mut lines: Vec<&str> = vec![];
    let head = &text[..text[..at.min(text.len())].rfind('\n').map(|i| i + 1).unwrap_or(0)];
    for l in head.lines().rev() {
        let t = l.trim();
        if let Some(d) = t.strip_prefix("///") {
            lines.push(d.strip_prefix(' ').unwrap_or(d));
        } else if t.starts_with("#[") {
            continue;
        } else {
            break;
        }
    }
    lines.reverse();
    lines.join("\n")
}

/// A path relative to the workspace root → the module path (`crates/x/src/a/b.rs` → [a, b], `src/a/mod.rs` → [a])
pub fn module_of(rel: &str) -> Vec<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    let from = parts.iter().rposition(|p| *p == "src").map(|i| i + 1).unwrap_or(0);
    let mut m: Vec<String> = parts[from..].iter().map(|s| s.to_string()).collect();
    if let Some(l) = m.pop() {
        let l = l.trim_end_matches(".rs");
        if !matches!(l, "mod" | "lib" | "main") {
            m.push(l.to_string());
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(src: &str, pat: &str, nth: usize) -> usize {
        src.match_indices(pat).nth(nth).unwrap().0
    }

    fn def_text(ws: &[&FileIndex], file: usize, off: usize) -> Option<(usize, String, usize)> {
        let d = definition(ws, file, off);
        d.first().map(|l| (l.file, ws[l.file].text[l.start..l.end].to_string(), l.start))
    }

    #[test]
    fn locals_paths_methods() {
        let src = "struct P { x: i64 }\nimpl P {\n    fn new(x: i64) -> P { P { x } }\n    fn get(&self) -> i64 { self.x }\n    fn twice(&self) -> i64 { self.get() * 2 }\n}\n\
                   fn f(a: i64) -> i64 {\n    let p = P::new(a);\n    let a = a + 1;\n    for i in 0..3 { let _ = i; }\n    match Some(a) { Some(v) => v, None => p.get() }\n}\n";
        let f = FileIndex::new(src.to_string(), vec![]);
        let ws = [&f];
        // P::new → the new of impl P
        assert_eq!(def_text(&ws, 0, at(src, "new(a)", 0)).unwrap().2, at(src, "new(x", 0));
        // self.get() → the get of impl P, self.x → field x
        assert_eq!(def_text(&ws, 0, at(src, "get() * 2", 0)).unwrap().2, at(src, "get(&self)", 0));
        assert_eq!(def_text(&ws, 0, at(src, "x }\n    fn twice", 0) + 0).unwrap().2, at(src, "x: i64 }", 0));
        // The a on the right of `let a = a + 1` is the parameter a, the a of `Some(a)` is the let's a
        assert_eq!(def_text(&ws, 0, at(src, "a + 1", 0)).unwrap().2, at(src, "a: i64) -> i64 {\n    let p", 0));
        assert_eq!(def_text(&ws, 0, at(src, "a) { Some", 0)).unwrap().2, at(src, "a = a + 1", 0));
        // The i of for, the v of the match arm, p.get() (the type of p from P::new)
        assert_eq!(def_text(&ws, 0, at(src, "i; }", 0)).unwrap().2, at(src, "i in", 0));
        assert_eq!(def_text(&ws, 0, at(src, "v,", 0)).unwrap().2, at(src, "v) =>", 0));
        assert_eq!(def_text(&ws, 0, at(src, "get() }\n}", 0)).unwrap().2, at(src, "get(&self)", 0));
        // References: get is the definition and 2 places
        assert_eq!(references(&ws, 0, at(src, "get(&self)", 0), true, &|_| None).len(), 3);
        assert_eq!(references(&ws, 0, at(src, "get(&self)", 0), false, &|_| None).len(), 2);
    }

    #[test]
    fn across_files_and_uses() {
        let a = "pub fn helper() -> u32 { 1 }\npub struct Thing;\n".to_string();
        let b = "use crate::util::helper;\nfn helper2() {}\nfn main() { helper(); let t: Thing = Thing; }\n".to_string();
        let fa = FileIndex::new(a, module_of("src/util.rs"));
        let fb = FileIndex::new(b.clone(), module_of("src/main.rs"));
        let ws = [&fa, &fb];
        assert_eq!(def_text(&ws, 1, at(&b, "helper();", 0)).map(|x| (x.0, x.1)), Some((0, "helper".to_string())));
        assert_eq!(def_text(&ws, 1, at(&b, "Thing;", 0)).map(|x| x.0), Some(0));
        // (definition, use, call: the use is a reference too, as in rust-analyzer)
        assert_eq!(references(&ws, 0, 7, true, &|_| None).len(), 3);
        assert_eq!(module_of("crates/x/src/a/mod.rs"), vec!["a".to_string()]);
    }

    /// Where the counting of references was matched to rust-analyzer: `Self` inside impls, bindings of patterns / parameters, unknown external paths,
    /// items excluded by cfg, built-in types
    #[test]
    fn references_like_rust_analyzer() {
        let src = "fn concrete() {}
struct P { x: u8 }
impl P {
    fn new() -> Self { Self { x: 0u8 } }
}
                   fn f(concrete: bool) {
    let (a, concrete) = (1, 2);
    let _ = syn::P;
    let _ = Delimiter::None;
}
                   #[cfg(feature = \"off\")]
fn g() -> P { P::new() }
#[cfg(not(feature = \"off\"))]
fn g() -> u8 { 1 }
                   fn u8() {}
";
        let mut fi = FileIndex::new(src.to_string(), vec![]);
        fi.apply_cfg(vec![]);
        let ws = [&fi];
        let no = &|_: usize| None;
        // P: the definition, the impl's name, 2 `Self`s and 2 in g excluded by cfg (`-> P` and `P::new`; rust-analyzer resolves paths even inside excluded
        // items). `syn::P` is an external one
        assert_eq!(references(&ws, 0, at(src, "P {", 0), true, no).len(), 6);
        // `fn f(concrete: ..)` and `let (a, concrete)` are the bindings themselves: not references of the fn concrete
        assert_eq!(references(&ws, 0, 3, true, no).len(), 1);
        assert_eq!(def_text(&ws, 0, at(src, "concrete) =", 0)).map(|x| x.2), Some(at(src, "concrete) =", 0)));
        // `Delimiter::None` is not Option's None (Delimiter is unknown)
        assert!(definition(&ws, 0, at(src, "None", 0)).is_empty());
        // No answer for the associated item `P::new` in g excluded by cfg (the P of the path is answered)
        assert!(definition(&ws, 0, at(src, "new()", 1)).is_empty());
        assert_eq!(definition(&ws, 0, at(src, "P::new", 0)).first().map(|l| l.start), Some(at(src, "P {", 0)));
        // u8: only type positions (the suffix of `0u8` and the name of `fn u8` are excluded)
        assert_eq!(references(&ws, 0, at(src, "u8 }", 0), true, no).len(), 2);
    }
}

#[cfg(test)]
#[path = "navigate_tests.rs"]
mod unit_tests;

/// textDocument/prepareRename: if the name at off is one that can be renamed, the range of that name
pub fn prepare_rename(ws: &[&FileIndex], file: usize, off: usize) -> Result<Loc, String> {
    clear_memo();
    let f = ws[file];
    let k = f.tok_at(off).ok_or("名前の上ではない")?;
    let w = f.tok(k);
    if KEYWORDS.contains(&w) || PRIMS.contains(&w) {
        return Err(format!("`{w}` は名前を変えられない"));
    }
    let t = *def_at(ws, file, off).first().ok_or("定義が分からない")?;
    let tf = ws[t.file];
    // (a library one: can be renamed only on the B of `use a::B;`, in the form of giving it an alias (as in rust-analyzer))
    if tf.lib && lib_use(f, w) != Some(f.toks[k].1 as usize) {
        return Err("ライブラリの定義は名前を変えられない (use の名前の上なら別名を付けられる)".into());
    }
    if mod_file(ws, t).is_some() {
        return Err("ファイルのモジュールの名前は変えられない (ファイルの名前も変わるので)".into());
    }
    // (a method of a trait impl: if the trait is a workspace one, the trait's declaration and all impls are renamed. For a trait of a library / external
    // crate (the default of `impl Default for S`), refused: the declaration cannot be renamed)
    if let Some(d) = tf.defs.iter().find(|d| d.start == t.start && d.kind == SymKind::Method) {
        if let Some(tr) = impl_trait(&d.container) {
            if trait_method(ws, tr, &d.name).is_none() {
                return Err(format!("`{tr}` (ライブラリか外部のクレートの trait) のメソッドは名前を変えられない"));
            }
        }
    }
    // (resolved to the derive name `#[derive(Clone)]`: a method made by derive)
    let is_def = tf.defs.iter().any(|d| d.start == t.start);
    if !is_def && !tf.slim && tf.tok_at(t.start).is_some_and(|j| tf.in_derive(j)) {
        return Err("derive が作るメソッドは名前を変えられない".into());
    }
    let (a, b) = (f.toks[k].1 as usize, f.toks[k].2 as usize);
    Ok(Loc { file, start: a, end: b })
}

/// The method name in the declaration of the workspace trait tr
fn trait_method(ws: &[&FileIndex], tr: &str, name: &str) -> Option<Loc> {
    ws.iter().enumerate().filter(|(_, w)| !w.lib).find_map(|(i, w)| {
        w.defs.iter().find(|d| d.kind == SymKind::Method && d.name == name && !d.container.starts_with("impl") && &d.container[..] == tr && w.defs.iter().any(|x| x.kind == SymKind::Trait && x.name == tr)).map(|d| loc(i, d))
    })
}

/// The byte position of the last name of the path of `use a::name;` (without an alias) in file f
fn lib_use(f: &FileIndex, name: &str) -> Option<usize> {
    f.uses.iter().zip(&f.use_at).find(|(u, a)| u.0 == name && u.1.last().is_some_and(|l| l == name) && **a != usize::MAX).map(|(_, &a)| a)
}

/// Whether the name can be used as an identifier (keywords excluded; ones with `r#` are fine)
fn valid_ident(s: &str) -> bool {
    let n = s.strip_prefix("r#").unwrap_or(s);
    let mut c = n.chars();
    let Some(h) = c.next() else { return false };
    (h == '_' || h.is_alphabetic()) && c.all(|x| x == '_' || x.is_alphanumeric()) && n != "_" && (s.starts_with("r#") || !KEYWORDS.contains(&n))
}

/// textDocument/rename: a list of edits (position, new string). A struct shorthand `S { x }` is rewritten to `S { new: x }` /
/// `S { x: new }`
pub fn rename(ws: &[&FileIndex], file: usize, off: usize, new: &str, load: Load) -> Result<Vec<(Loc, String)>, String> {
    let at = prepare_rename(ws, file, off)?;
    if !valid_ident(new) {
        return Err(format!("`{new}` は名前にできない"));
    }
    let old = ws[file].text[at.start..at.end].to_string();
    let t = *def_at(ws, file, off).first().ok_or("定義が分からない")?;
    // On a name in a `use`: `use std::sync::Mutex;` → `use std::sync::Mutex as New;`, and the Mutex inside this file
    if lib_use(ws[file], &old) == Some(at.start) {
        let f = ws[file];
        let u = at.start;
        let mut out = vec![(Loc { file, start: u + old.len(), end: u + old.len() }, format!(" as {new}"))];
        for &k in f.idents.get(old.as_str()).map(|v| v.as_slice()).unwrap_or(&[]) {
            let a = f.toks[k as usize].1 as usize;
            if a != u && !f.in_inactive(a) && !f.after_path(k as usize) && def_at(ws, file, a).first() == Some(&t) {
                out.push((Loc { file, start: a, end: a + old.len() }, new.to_string()));
            }
        }
        return Ok(out);
    }
    let is_field = ws[t.file].defs.iter().any(|d| d.start == t.start && d.kind == SymKind::Field);
    // (a method of a trait impl: looked up from the trait's declaration (the declaration, all impls, calls))
    let root = ws[t.file].defs.iter().find(|d| d.start == t.start && d.kind == SymKind::Method).and_then(|d| impl_trait(&d.container).and_then(|tr| trait_method(ws, tr, &d.name)));
    let refs = match root {
        Some(r) if (r.file, r.start) != (t.file, t.start) => {
            let held;
            let mut ws2: Vec<&FileIndex> = ws.to_vec();
            if ws[r.file].slim {
                held = load(r.file).ok_or("ファイルを読めない")?;
                ws2[r.file] = &held;
            }
            references(&ws2, r.file, r.start, true, load)
        }
        _ => references(ws, file, off, true, load),
    };
    let mut held: FxMap<usize, Rc<FileIndex>> = FxMap::default();
    let mut out = vec![];
    for l in refs {
        let fi: &FileIndex = if ws[l.file].slim {
            if !held.contains_key(&l.file) {
                let Some(r) = load(l.file) else { continue };
                held.insert(l.file, r);
            }
            &held[&l.file]
        } else {
            ws[l.file]
        };
        if fi.text.get(l.start..l.end) != Some(old.as_str()) {
            continue;
        }
        // (a shorthand `T { x }`: in the form of adding text, as in rust-analyzer. For a field `new: ` before it, for a local `: new` after it)
        match fi.tok_at(l.start) {
            Some(k) if fi.toks[k].0 == 0 && (fi.sym(k.wrapping_sub(1), b'{') || fi.sym(k.wrapping_sub(1), b',')) && (fi.sym(k + 1, b',') || fi.sym(k + 1, b'}')) && fi.struct_lit_at(k).is_some() => {
                if is_field {
                    out.push((Loc { file: l.file, start: l.start, end: l.start }, format!("{new}: ")));
                } else {
                    out.push((Loc { file: l.file, start: l.end, end: l.end }, format!(": {new}")));
                }
            }
            _ => out.push((l, new.to_string())),
        }
    }
    out.sort_by_key(|x| (x.0.file, x.0.start));
    out.dedup_by_key(|x| (x.0.file, x.0.start));
    if out.is_empty() {
        return Err(format!("`{old}` の書き換える所が見つからない (use の別名なら、別名の付いた use の上で変える)"));
    }
    Ok(out)
}

/// textDocument/signatureHelp: (signature, byte ranges of the parameters (in the signature), the current parameter) of the call `f(..|..)` enclosing off
pub fn signature_help(ws: &[&FileIndex], file: usize, off: usize, load: Load) -> Option<(String, Vec<(usize, usize)>, usize, String)> {
    clear_memo();
    let f = ws[file];
    // (goes back from the token before off to find an unclosed `(`. Counted are the commas at depth 0)
    let mut j = f.toks.partition_point(|t| (t.1 as usize) < off);
    let mut d = 0i32;
    let mut commas = 0;
    let open = loop {
        if j == 0 {
            return None;
        }
        j -= 1;
        if f.toks[j].0 != 1 {
            continue;
        }
        match f.text.as_bytes()[f.toks[j].1 as usize] {
            b')' | b']' | b'}' => d += 1,
            b'[' => {
                // (an array / `vec![..]` in the arguments: the commas so far are the ones inside the array)
                if d == 0 {
                    commas = 0;
                } else {
                    d -= 1;
                }
            }
            b'{' => {
                if d == 0 {
                    return None;
                }
                d -= 1;
            }
            b'(' => {
                if d == 0 {
                    break j;
                }
                d -= 1;
            }
            b',' if d == 0 => commas += 1,
            b';' if d == 0 => return None,
            _ => {}
        }
    };
    // (what is called: `f(` / `x.m(` / `T::f(` / `f::<T>(`)
    let mut c = open.checked_sub(1)?;
    if f.sym(c, b'>') {
        let mut dd = 0;
        while c > 0 {
            if f.sym(c, b'>') {
                dd += 1;
            } else if f.sym(c, b'<') {
                dd -= 1;
                if dd == 0 {
                    break;
                }
            }
            c -= 1;
        }
        c = c.checked_sub(3)?;
    }
    // (inside a declaration `fn f(..)` / `struct T(..)` is not a call)
    if !f.ident(c) || KEYWORDS.contains(&f.tok(c)) || (c >= 1 && f.ident(c - 1) && matches!(f.tok(c - 1), "fn" | "struct")) {
        return None;
    }
    let method_call = c >= 1 && f.sym(c - 1, b'.');
    let mut t = *def_at(ws, file, f.toks[c].1 as usize).first()?;
    // (a local holding a fn `let score = lib_rank;`: that fn)
    if !ws[t.file].defs.iter().any(|x| x.start == t.start) && !ws[t.file].slim {
        let tf = ws[t.file];
        let b = tf.toks.partition_point(|x| (x.1 as usize) < t.start);
        if tf.sym(b + 1, b'=') && tf.ident(b + 2) && tf.sym(b + 3, b';') {
            if let Some(&u) = def_at(ws, t.file, tf.toks[b + 2].1 as usize).first() {
                if ws[u.file].defs.iter().any(|x| x.start == u.start && matches!(x.kind, SymKind::Function | SymKind::Method)) {
                    t = u;
                }
            }
        }
    }
    let tf = ws[t.file];
    let (sig, name_at, doc) = match tf.defs.iter().find(|x| x.start == t.start) {
        Some(def) => {
            if !matches!(def.kind, SymKind::Function | SymKind::Method | SymKind::Struct | SymKind::Variant) {
                return None;
            }
            let held;
            let text: &str = if tf.slim {
                held = load(t.file)?;
                &held.text
            } else {
                &tf.text
            };
            let sig = signature(text.get(def.item_start..def.item_end.min(text.len()))?, def.kind);
            let at = sig.find(def.name.as_str()).unwrap_or(0) + def.name.len();
            (sig, at, doc_comment(text, def.item_start))
        }
        // (a local binding: for a closure `let f = |a: T| ..`, `impl Fn(a: T) -> R`)
        None => (closure_sig(ws, t)?, 0, String::new()),
    };
    // (the parameter list: the first `(` .. `)` after the name. Skips the inside of type parameters `<..>`)
    let b = sig.as_bytes();
    let mut i = name_at;
    let mut dg = 0;
    while i < b.len() {
        match b[i] {
            b'<' => dg += 1,
            b'>' if i > 0 && b[i - 1] != b'-' => dg -= 1,
            b'(' if dg == 0 => break,
            _ => {}
        }
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let mut params = vec![];
    let mut dd = 0;
    let mut s = i + 1;
    let mut k = i + 1;
    while k < b.len() {
        match b[k] {
            b'(' | b'[' | b'<' => dd += 1,
            b'>' if b[k - 1] == b'-' => {}
            b')' | b']' | b'>' if dd > 0 => dd -= 1,
            b')' => break,
            b',' if dd == 0 => {
                params.push((s, k));
                s = k + 1;
            }
            _ => {}
        }
        k += 1;
    }
    if k > s && !sig[s..k].trim().is_empty() {
        params.push((s, k));
    }
    let params: Vec<(usize, usize)> = params
        .into_iter()
        .map(|(a, e)| {
            let lead = sig[a..e].len() - sig[a..e].trim_start().len();
            (a + lead, a + sig[a..e].trim_end().len())
        })
        .collect();
    let has_self = params.first().is_some_and(|&(a, e)| {
        let p = &sig[a..e];
        p == "self" || p.ends_with("self") || p.starts_with("self:") || p.starts_with("mut self")
    });
    // (in a method call `x.m(..)` self is not counted as a parameter: as in rust-analyzer)
    let params = if method_call && has_self { params[1..].to_vec() } else { params };
    Some((sig, params, commas, doc))
}

/// The part of closure_param_type that decides from the method signature: among the parameters after the `(` (open) of method m of receiver recv,
/// the idx-th of the parameter shape of the bound of the one that is the closure (its `|` is j) (path is the indices within a tuple pattern)
#[allow(clippy::too_many_arguments)]
fn closure_param_from_sig(ws: &[&FileIndex], file: usize, recv: &str, m: &str, open: usize, j: usize, idx: usize, path: &[usize], depth: u32) -> Option<String> {
    let f = ws[file];
    // (which parameter the closure is, and the starts of the arguments)
    let mut starts = vec![open + 1];
    let mut dd = 0;
    for q in open + 1..j {
        if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
            dd += 1;
        } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
            dd -= 1;
        } else if dd == 0 && f.sym(q, b',') {
            starts.push(q + 1);
        }
    }
    let argi = starts.len() - 1;
    let (o, d) = expand(ws, file, recv).into_iter().find_map(|t| member_def(ws, file, &t, m, &[SymKind::Method]).map(|(_, d)| (t, d)))?;
    let (_, _, _, args) = d.fnret.iter().find(|x| x.0 as usize == argi && x.2 != "=")?;
    let ps = tuple_items(&format!("({args})"))?;
    let pat = ps.get(idx)?.clone();
    let mut ty = def_subst(d, &o, &pat);
    // (a parameter whose type is a type parameter of the method itself `fold(init: B, ..)`: the type of that argument's expression)
    for (i, g, pat, _) in &d.fnret {
        if pat == "=" {
            if let Some(&s) = starts.get(*i as usize) {
                if let Some(at) = expr_end(f, s, false).and_then(|e| expr_type(ws, file, e, depth + 1)) {
                    ty = subst(&ty, &[g.clone()], &[at]);
                }
            }
        }
    }
    let mut ty = strip_lifetimes(&norm_proj_ws(ws, file, &ty));
    // (unknown if a type parameter (1 uppercase letter) in the shape is left unreplaced)
    let words = |s: &str| s.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() == 1 && w.starts_with(|c: char| c.is_ascii_uppercase())).map(|w| w.to_string()).collect::<Vec<_>>();
    let before = words(&pat);
    if words(&ty).iter().any(|w| before.contains(w) && !targs(&o).contains(w)) || has_proj(&ty) {
        return None;
    }
    for &i in &path[1..] {
        let inner = strip_ref(&ty).to_string();
        ty = tuple_items(&inner)?.get(i)?.clone();
    }
    Some(ty)
}

/// Whether there is a projection of an associated type left unreplaced in a written type (`I::Item` / `Self::Output` / `<A as Tr>::B` / `Vec<T>::Item`).
/// A path qualifier (`std::cell::RefCell<..>`) is not a projection
fn has_proj(t: &str) -> bool {
    let b = t.as_bytes();
    t.match_indices("::").any(|(i, _)| {
        if i >= 1 && b[i - 1] == b'>' {
            return true;
        }
        let st = t[..i].rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |x| x + 1);
        let seg = &t[st..i];
        seg == "Self" || seg.len() == 1 && seg.starts_with(|c: char| c.is_ascii_uppercase())
    })
}

/// The elements of a tuple type `(A, B<C, D>)`
fn tuple_items(t: &str) -> Option<Vec<String>> {
    let t = t.trim();
    let inner = t.strip_prefix('(')?.strip_suffix(')')?;
    let mut out = vec![];
    let mut d = 0;
    let mut a = 0;
    let b = inner.as_bytes();
    for i in 0..=b.len() {
        let c = if i < b.len() { b[i] } else { b',' };
        match c {
            b'<' | b'(' | b'[' => d += 1,
            b'>' if i > 0 && b[i - 1] != b'-' => d -= 1,
            b')' | b']' => d -= 1,
            b',' if d == 0 => {
                let x = inner[a..i].trim();
                if !x.is_empty() {
                    out.push(x.to_string());
                }
                a = i + 1;
            }
            _ => {}
        }
    }
    Some(out)
}

/// Whether it is an iteration (iterator) type (one whose element type iter_item knows)
fn is_iter_head(h: &str) -> bool {
    matches!(
        h,
        "Iter" | "IterMut" | "IntoIter" | "Drain" | "Map" | "Filter" | "FilterMap" | "Enumerate" | "Rev" | "Skip" | "Take" | "SkipWhile" | "TakeWhile" | "MapWhile"
            | "Peekable" | "Fuse" | "StepBy" | "Chain" | "Zip" | "Cloned" | "Copied" | "Flatten" | "FlatMap" | "Inspect" | "Scan" | "Cycle" | "Chars" | "CharIndices"
            | "Bytes" | "Lines" | "Split" | "SplitWhitespace" | "SplitN" | "RSplit" | "Keys" | "Values" | "ValuesMut" | "Windows" | "Chunks" | "Range" | "RangeInclusive" | "RangeFrom"
    )
}

/// The element type of iteration type t (`Iter<'_, T>` → &T, `Map<I, impl FnMut() -> R>` → R, `Enumerate<I>` → (usize, element) ..).
/// For collections that are not iterators (Vec / Option / HashMap ..), the IntoIterator element (for a value) / a reference (`&Vec<T>` → &T)
fn iter_item(t: &str) -> Option<String> {
    let t = t.trim();
    let by_ref = t.starts_with('&');
    let h = type_head(t);
    // (the undetermined trailing allocator / hasher type arguments, the A / S of `IntoIter<K, V, A>` / `HashMap<K, V, S>`, are not counted)
    let mut a = targs(t);
    while a.len() > 1 && a.last().is_some_and(|x| x == "A" || x == "S") {
        a.pop();
    }
    let first = || a.first().cloned();
    // (a type of the same name without type arguments (serde_json's `map::Iter`): not a std iterator. From that type's impl (iter_item_ws))
    if a.is_empty() && !matches!(h.as_str(), "Chars" | "CharIndices" | "Bytes" | "Lines" | "SplitWhitespace") {
        return None;
    }
    // (the R of a closure type `impl FnMut(..) -> R`)
    let ret_of = |f: &str| -> Option<String> {
        let i = f.find(") -> ")?;
        Some(f[i + 5..].trim().to_string())
    };
    let deref = |s: String| s.strip_prefix('&').map(|x| x.trim_start_matches("mut ").to_string()).unwrap_or(s);
    Some(match h.as_str() {
        "Iter" if a.len() == 2 => format!("(&{}, &{})", a[0], a[1]),
        "IterMut" if a.len() == 2 => format!("(&{}, &mut {})", a[0], a[1]),
        "Iter" | "Windows" | "Chunks" => format!("&{}", if h == "Iter" { first()? } else { format!("[{}]", first()?) }),
        "IterMut" => format!("&mut {}", first()?),
        "IntoIter" | "Drain" if a.len() == 2 => format!("({}, {})", a[0], a[1]),
        "IntoIter" | "Drain" | "Range" | "RangeInclusive" | "RangeFrom" => first()?,
        "Keys" => format!("&{}", first()?),
        "Values" => format!("&{}", a.get(1)?),
        "ValuesMut" => format!("&mut {}", a.get(1)?),
        "Chars" => "char".to_string(),
        "CharIndices" => "(usize, char)".to_string(),
        "Bytes" => "u8".to_string(),
        "Lines" | "Split" | "SplitWhitespace" | "SplitN" | "RSplit" => "&str".to_string(),
        "Map" | "Scan" | "MapWhile" => {
            let r = ret_of(a.get(1).or(a.last())?)?;
            if h == "Map" { r } else { targs(&r).into_iter().next()? }
        }
        "FilterMap" => targs(&ret_of(a.get(1)?)?).into_iter().next()?,
        "FlatMap" => iter_item(&ret_of(a.get(2).or(a.get(1))?)?)?,
        "Flatten" => iter_item(&iter_item(&first()?)?)?,
        "Enumerate" => format!("(usize, {})", iter_item(&first()?)?),
        "Zip" => format!("({}, {})", iter_item(a.first()?)?, iter_item(a.get(1)?)?),
        "Cloned" | "Copied" => deref(iter_item(&first()?)?),
        "Filter" | "Rev" | "Skip" | "Take" | "SkipWhile" | "TakeWhile" | "Peekable" | "Fuse" | "StepBy" | "Chain" | "Inspect" | "Cycle" => iter_item(&first()?)?,
        "Vec" | "VecDeque" | "HashSet" | "BTreeSet" | "BinaryHeap" | "Option" | "Box" | "[T]" => {
            let e = if h == "[T]" { strip_ref(t).trim_start_matches('[').split([';', ']']).next()?.trim().to_string() } else { first()? };
            if by_ref { format!("&{e}") } else { e }
        }
        "HashMap" | "BTreeMap" => {
            if by_ref { format!("(&{}, &{})", a.first()?, a.get(1)?) } else { format!("({}, {})", a.first()?, a.get(1)?) }
        }
        _ => return None,
    })
}

/// The associated type name of the impl of trait tr for type ty (the Output of `impl Index<usize> for AdtTable { type Output = StructDef; }`,
/// the Item of `impl Iterator for ReadDir { type Item = io::Result<DirEntry>; }`, `impl Deref for W { type Target = T; }`)
fn assoc_type(ws: &[&FileIndex], file: usize, ty: &str, tr: &str, name: &str) -> Option<String> {
    let h = type_head(ty);
    if h.is_empty() {
        return None;
    }
    // (if there are several types with the same name (the `Iter` of slice / option / serde_json): the ones whose impl self type has a matching number of type arguments,
    // the ones in the same file / same crate)
    let n = targs(ty).len();
    let kr = ws[file].krate;
    // (a qualified `map::Iter` (qualify_local_type): if a type of that name is in exactly that module, only that module's impls
    // (the associated types of other `Iter`s are not answered. The impls of serde_json's `map::Iter` are inside a macro and not visible))
    let path = strip_ref(ty).split('<').next().unwrap_or("");
    let qual = path.rsplit_once("::").map(|(q, _)| q.to_string());
    let exact = qual.as_ref().is_some_and(|q| named(ws, file, &h).iter().any(|(_, x)| x.owner.is_empty() && matches!(x.kind, SymKind::Struct | SymKind::Enum) && x.module.join("::") == *q));
    let (_, d) = named(ws, file, name)
        .into_iter()
        .filter(|(_, d)| {
            d.kind == SymKind::TypeAlias && d.owner == h && !d.ty.is_empty() && d.container.strip_prefix("impl ").and_then(|n| n.rsplit_once(" for ")).is_some_and(|(t2, _)| type_head(t2.trim_start_matches('!')) == tr)
        })
        .filter(|(_, d)| !exact || qual.as_ref().is_some_and(|q| d.module.join("::") == *q))
        .min_by_key(|(i, d)| (targs(impl_self_full(&d.container)).len() != n, *i != file, ws[*i].krate != kr))?;
    let t = def_subst(d, ty, &d.ty);
    // (unknown if names of the type parameters of the impl's self type are left)
    Some(t)
}

/// iter_item plus the impls' associated types `type Item = ..` (the user's types, std's ReadDir etc.)
fn iter_item_ws(ws: &[&FileIndex], file: usize, t: &str) -> Option<String> {
    // (when the contents of a std adapter are a user's or std type (`Skip<Args>`): the element of the contents from the impl)
    let adapt = || -> Option<String> {
        let a = targs(strip_ref(t));
        let inner = || iter_item_ws(ws, file, a.first()?);
        match type_head(strip_ref(t)).as_str() {
            "Filter" | "Rev" | "Skip" | "Take" | "SkipWhile" | "TakeWhile" | "Peekable" | "Fuse" | "StepBy" | "Chain" | "Inspect" | "Cycle" => inner(),
            "Enumerate" => Some(format!("(usize, {})", inner()?)),
            "Cloned" | "Copied" => inner().map(|s| s.strip_prefix('&').map(|x| x.trim_start_matches("mut ").to_string()).unwrap_or(s)),
            "Zip" => Some(format!("({}, {})", inner()?, iter_item_ws(ws, file, a.get(1)?)?)),
            "Flatten" => iter_item_ws(ws, file, &inner()?),
            _ => None,
        }
    };
    iter_item(t).or_else(adapt).or_else(|| assoc_type(ws, file, strip_ref(t), "Iterator", "Item")).or_else(|| {
        // (IntoIterator: the coll of `for x in &coll`)
        let it = assoc_type(ws, file, strip_ref(t), "IntoIterator", "Item")?;
        Some(it)
    })
}

/// The position of the `>` matching s, which starts with `<` (the `>` of `->` is not counted)
fn close_angle(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut d = 0;
    for i in 0..b.len() {
        match b[i] {
            b'<' => d += 1,
            b'>' if i >= 1 && b[i - 1] == b'-' => {}
            b'>' => {
                d -= 1;
                if d == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// norm_proj plus the impls' associated types
fn norm_proj_ws(ws: &[&FileIndex], file: usize, t: &str) -> String {
    // (`<A as Tr>::N` (other than Item / IntoIter): the impl's associated type. For ToOwned, `str` → String, `[T]` → Vec<T>, otherwise A by Clone)
    let mut t = t.to_string();
    for _ in 0..4 {
        let Some(st) = t.find("<").filter(|_| t.contains(" as ")) else { break };
        let Some(r) = (st..t.len()).filter(|&i| t[i..].starts_with('<')).find_map(|i| {
            let close = i + close_angle(&t[i..])?;
            let inner = &t[i + 1..close];
            let (a, tr) = inner.split_once(" as ")?;
            let rest = t[close + 1..].strip_prefix("::")?;
            let n: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            if n.is_empty() || n == "Item" || n == "IntoIter" || a.contains('<') && a.contains(" as ") {
                return None;
            }
            let trh = type_head(tr);
            let v = assoc_type(ws, file, a.trim(), &trh, &n).or_else(|| match (trh.as_str(), n.as_str()) {
                ("ToOwned", "Owned") => Some(match a.trim() {
                    "str" => "String".to_string(),
                    x if x.starts_with('[') => format!("Vec<{}>", x[1..].trim_end_matches(']').split(';').next().unwrap_or("").trim()),
                    x => x.to_string(),
                }),
                _ => None,
            })?;
            Some((i, close + 3 + n.len(), v))
        }) else { break };
        t = format!("{}{}{}", &t[..r.0], r.2, &t[r.1..]);
    }
    let t = t.as_str();
    norm_proj_with(t, &|inner: &str, into: bool| {
        if into {
            if assoc_type(ws, file, inner, "Iterator", "Item").is_some() { Some(inner.to_string()) } else { assoc_type(ws, file, inner, "IntoIterator", "IntoIter") }
        } else {
            iter_item_ws(ws, file, inner)
        }
    })
}

/// Replaces the associated type projections `<A as Iterator>::Item` / `A::Item` / `Self::Item` in a written type with the element type when known
#[cfg(test)]
fn norm_proj(t: &str) -> String {
    norm_proj_with(t, &|_, _| None)
}

fn norm_proj_with(t: &str, more: &dyn Fn(&str, bool) -> Option<String>) -> String {
    let mut s = t.to_string();
    for _ in 0..6 {
        // (`::Item` (the element) and `::IntoIter` (IntoIterator: itself for an iterator, IntoIter<element> for a collection))
        // (`T::Target` (the Deref target) is replaced separately)
        if let Some(ti) = s.find("::Target") {
            let b = s.as_bytes();
            let mut j = ti;
            let mut d = 0;
            while j > 0 {
                let c = b[j - 1];
                if c == b'>' && !(j >= 2 && b[j - 2] == b'-') {
                    d += 1;
                } else if c == b'<' {
                    if d == 0 {
                        break;
                    }
                    d -= 1;
                } else if d == 0 && (c == b',' || c == b'(' || c == b' ' || c == b'&') {
                    break;
                }
                j -= 1;
            }
            let inner = s[j..ti].to_string();
            let tgt = match type_head(&inner).as_str() {
                "String" => Some("str".to_string()),
                "Vec" => targs(&inner).first().map(|e| format!("[{e}]")),
                "Box" | "Rc" | "Arc" => targs(&inner).first().cloned(),
                "PathBuf" => Some("Path".to_string()),
                "OsString" => Some("OsStr".to_string()),
                _ => more(&inner, true).filter(|_| false),
            };
            match tgt {
                Some(x) => {
                    s = format!("{}{}{}", &s[..j], x, &s[ti + 8..]);
                    continue;
                }
                None => break,
            }
        }
        let (i, plen, into) = match (s.find("::Item"), s.find("::IntoIter")) {
            (Some(a), Some(b)) if b < a => (b, 10, true),
            (Some(a), _) => (a, 6, false),
            (None, Some(b)) => (b, 10, true),
            (None, None) => break,
        };
        let b = s.as_bytes();
        // (the type before `::Item`: `<A as Tr>`, or up to a separator at depth 0 (`<` / `,` / `(` / whitespace))
        let (a, inner) = if i >= 1 && b[i - 1] == b'>' && s[..i].contains(" as ") {
            let mut d = 0;
            let mut j = i;
            while j > 0 {
                j -= 1;
                match b[j] {
                    b'>' if j >= 1 && b[j - 1] == b'-' => {}
                    b'>' => d += 1,
                    b'<' => {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inner = &s[j + 1..i - 1];
            (j, inner.split(" as ").next().unwrap_or("").to_string())
        } else {
            let mut d = 0;
            let mut j = i;
            while j > 0 {
                let c = b[j - 1];
                if c == b'>' && j >= 2 && b[j - 2] == b'-' {
                    // (the `>` of `->` is not a bracket)
                } else if c == b'>' || c == b']' {
                    // (does not cut at the whitespace inside an array `[T; N]`)
                    d += 1;
                } else if c == b'<' || c == b'[' {
                    if d == 0 {
                        break;
                    }
                    d -= 1;
                } else if d == 0 && (c == b',' || c == b'(' || c == b' ' || c == b'&') {
                    break;
                }
                j -= 1;
            }
            (j, s[j..i].to_string())
        };
        // (an array value `[T; N]` (its IntoIterator impl cannot be looked up with `[T; N]` as the owner): the element is T, the iterator is `IntoIter<T>`)
        let arr = |x: &str| x.trim().starts_with('[').then(|| elem_type(x.trim())).flatten();
        let rep = if into {
            if is_iter_head(&type_head(&inner)) { Some(inner.clone()) } else { iter_item(&inner).or_else(|| arr(&inner)).map(|e| format!("IntoIter<{e}>")).or_else(|| more(&inner, true)) }
        } else {
            iter_item(&inner).or_else(|| arr(&inner)).or_else(|| more(&inner, false))
        };
        match rep {
            Some(item) => s = format!("{}{}{}", &s[..a], item, &s[i + plen..]),
            None => break,
        }
    }
    s
}

/// Fits the type shape pat (containing the type parameter names vars) to the actual type act, putting the values of vars into out (`Option<U>` and `Option<char>` → U = char)
fn unify(pat: &str, act: &str, vars: &[String], out: &mut Vec<(String, String)>) {
    let p = pat.trim();
    let a = act.trim();
    if vars.iter().any(|v| v == p) {
        if !out.iter().any(|(v, _)| v == p) {
            out.push((p.to_string(), a.to_string()));
        }
        return;
    }
    if let (Some(p2), Some(a2)) = (p.strip_prefix('&'), a.strip_prefix('&')) {
        return unify(p2, a2, vars, out);
    }
    if type_head(p) == type_head(a) {
        for (x, y) in targs(p).iter().zip(targs(a).iter()) {
            unify(x, y, vars, out);
        }
    }
}

/// The list of the parameter types of a closure (token s is `|` or `move`) (`|x, (a, b)|` → `&D, (u8, char)`)
fn closure_args(ws: &[&FileIndex], file: usize, s: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let s = if f.tok(s) == "move" { s + 1 } else { s };
    let mut e = s + 1;
    while e < f.toks.len() && !f.sym(e, b'|') {
        e += 1;
    }
    // (for each parameter: for a name its type, for a tuple pattern the element types listed)
    fn pat_ty(ws: &[&FileIndex], file: usize, a: usize, b: usize, depth: u32) -> Option<String> {
        let f = ws[file];
        let mut a = a;
        while a < b && (f.sym(a, b'&') || f.tok(a) == "mut") {
            a += 1;
        }
        if f.sym(a, b'(') {
            let c = close_of(f, a);
            let mut parts = vec![];
            let mut st = a + 1;
            let mut d = 0;
            for q in a + 1..=c {
                if f.sym(q, b'(') {
                    d += 1;
                } else if f.sym(q, b')') && q != c {
                    d -= 1;
                }
                if (d == 0 && f.sym(q, b',')) || q == c {
                    if st < q {
                        parts.push(pat_ty(ws, file, st, q, depth)?);
                    }
                    st = q + 1;
                }
            }
            return Some(format!("({})", parts.join(", ")));
        }
        if f.ident(a) && f.tok(a) != "_" {
            if f.sym(a + 1, b':') {
                return Some(f.text[f.toks.get(a + 2)?.1 as usize..f.toks.get(b - 1)?.2 as usize].trim().to_string());
            }
            return closure_param_type(ws, file, a, depth);
        }
        None
    }
    let mut out = vec![];
    let mut st = s + 1;
    let mut d = 0;
    for q in s + 1..=e {
        if f.sym(q, b'(') {
            d += 1;
        } else if f.sym(q, b')') {
            d -= 1;
        }
        if (d == 0 && f.sym(q, b',')) || q == e {
            if st < q {
                out.push(pat_ty(ws, file, st, q, depth).unwrap_or_else(|| "_".to_string()));
            }
            st = q + 1;
        }
    }
    Some(out.join(", "))
}

/// The return type of a closure (token s is `|` or `move`) (`-> T`, or the type of the body expression)
fn closure_ret(ws: &[&FileIndex], file: usize, s: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    let mut j = if f.tok(s) == "move" { s + 1 } else { s };
    if !f.sym(j, b'|') {
        return None;
    }
    j += 1;
    while j < f.toks.len() && !f.sym(j, b'|') {
        j += 1;
    }
    let b = j + 1;
    if f.sym(b, b'-') && f.sym(b + 1, b'>') {
        let mut e = b + 2;
        while e < f.toks.len() && !f.sym(e, b'{') {
            e += 1;
        }
        return Some(f.text[f.toks.get(b + 2)?.1 as usize..f.toks.get(e)?.1 as usize].trim().to_string());
    }
    if f.sym(b, b'{') {
        let c = close_of(f, b);
        if c <= b + 1 || f.sym(c - 1, b';') {
            return Some("()".to_string());
        }
        return expr_type(ws, file, c - 1, depth + 1);
    }
    // (the body is `if .. { } else { }` / `match x { }` / `unsafe { }`: the type of the `}` at the end of the whole expression)
    if f.ident(b) && matches!(f.tok(b), "if" | "match" | "unsafe" | "loop") {
        let mut j = b;
        let end = loop {
            // (a struct expression in the condition can only be written inside brackets, so the first `{` at depth 0 is the body)
            let mut q = j + 1;
            let mut d = 0;
            while q < f.toks.len() && !(d == 0 && f.sym(q, b'{')) {
                if f.sym(q, b'(') || f.sym(q, b'[') {
                    d += 1;
                } else if f.sym(q, b')') || f.sym(q, b']') {
                    d -= 1;
                } else if f.sym(q, b';') {
                    return None;
                }
                q += 1;
            }
            let c = close_of(f, q);
            if f.tok(b) == "if" && f.ident(c + 1) && f.tok(c + 1) == "else" {
                if f.ident(c + 2) && f.tok(c + 2) == "if" {
                    j = c + 2;
                    continue;
                }
                break close_of(f, c + 2);
            }
            break c;
        };
        return expr_type(ws, file, end, depth + 1);
    }
    let e = expr_end(f, b, false)?;
    let t = expr_type(ws, file, e, depth + 1)?;
    // (`&x.name` / `&mut v`: a leading reference)
    Some(if f.sym(b, b'&') && !f.sym(b + 1, b'&') {
        if f.tok(b + 1) == "mut" { format!("&mut {t}") } else { format!("&{t}") }
    } else {
        t
    })
}

/// The type of `recv.name(args)` (m is the token of name, receiver type o): the type parameters of the method's return type are decided from the return values of the closures in the arguments
/// (`map(|x| x.n)` → Map<Iter<..>, impl FnMut() -> u8>, `opt.map(|x| ..)` → Option<R>). Associated type projections are replaced too
fn method_call_type(ws: &[&FileIndex], file: usize, o: &str, name: &str, m: usize, depth: u32) -> Option<String> {
    method_call_type_at(ws, file, o, name, m + 1, None, depth)
}

/// The body of method_call_type: op is the token of the arguments' `(`, turbo is the type of a turbofish `::<T>`
fn method_call_type_at(ws: &[&FileIndex], file: usize, o: &str, name: &str, op: usize, turbo: Option<&str>, depth: u32) -> Option<String> {
    // (iterators whose element type is known (`Split` / `RSplit` / slice's `Iter` ..): the Iterator trait's declaration (`Option<Self::Item>`) first.
    // Their impls are inside core macros and not visible, or an impl of another type with the same name (slice's `RSplit`) would be picked up)
    let from_trait = || -> Option<(String, Vec<(u8, String, String, String)>)> {
        if !is_iter_head(&type_head(o)) || iter_item(o).is_none() {
            return None;
        }
        let (_, d) = named(ws, file, name).into_iter().find(|(_, d)| d.kind == SymKind::Method && matches!(d.owner.as_str(), "Iterator" | "DoubleEndedIterator" | "ExactSizeIterator") && !d.container.starts_with("impl"))?;
        Some((def_subst(d, o, &d.ty), d.fnret.clone()))
    };
    let (ret, d) = from_trait().or_else(|| expand(ws, file, o).iter().find_map(|t| member_def(ws, file, t, name, &[SymKind::Method]).map(|(r, d)| (r, d.fnret.clone()))))?;
    let mut ret = ret;
    // (`sum()` / `product()` without a turbofish: usually the element type (with the reference removed))
    if turbo.is_none() && matches!(name, "sum" | "product") && ret.len() == 1 {
        if let Some(item) = iter_item_ws(ws, file, o) {
            ret = strip_ref(&item).to_string();
        }
    }
    // (a turbofish `collect::<Vec<_>>()` / `sum::<u32>()` / `parse::<u8>()`: into the first type parameter (1 uppercase letter) in the return type.
    // `_` is the receiver's element type)
    if let Some(tb) = turbo {
        let tb = match iter_item(o) {
            Some(item) if tb.contains('_') => tb.replace('_', &item),
            _ => tb.to_string(),
        };
        if let Some(g) = ret.split(|c: char| !(c.is_alphanumeric() || c == '_')).find(|w| w.len() == 1 && w.starts_with(|c: char| c.is_ascii_uppercase())) {
            let g = g.to_string();
            ret = subst(&ret, &[g], &[tb]);
        }
    }
    // (the `I::Output` of `get` / `get_mut`: the element of the sequence (the sequence if the argument is a range). Before the argument types are applied)
    if ret.contains("I::Output") {
        let f = ws[file];
        let range = f.sym(op, b'(') && (op + 1..close_of(f, op)).any(|q| f.sym(q, b'.') && f.sym(q + 1, b'.'));
        if let Some(e) = index_type(o, range) {
            ret = ret.replace("I::Output", &e);
        }
    }
    ret = apply_call_args(ws, file, ret, &d, op, depth);
    Some(strip_lifetimes(&norm_proj_ws(ws, file, &ret)))
}

/// Decides the type parameters of the return type ret of a call (the arguments' `(` is op) from the arguments: the return values of closures (`map(|x| ..)`), and the types of
/// the argument expressions for parameters whose type is a type parameter itself (`Mutex::new(x)` / `zip(other)`)
fn apply_call_args(ws: &[&FileIndex], file: usize, mut ret: String, d: &[(u8, String, String, String)], op: usize, depth: u32) -> String {
    let f = ws[file];
    if !d.is_empty() && f.sym(op, b'(') {
        // (the first token of each argument: split at commas at depth 0)
        let close = close_of(f, op);
        let mut starts = vec![op + 1];
        let mut dd = 0;
        for q in op + 1..close {
            if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                dd += 1;
            } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                dd -= 1;
            } else if dd == 0 && f.sym(q, b',') {
                starts.push(q + 1);
            }
        }
        for (idx, fty, pat, refm) in d {
            let Some(&s) = starts.get(*idx as usize) else { continue };
            // (a parameter whose type is a type parameter itself `zip(other)`: the type of the argument expression. For an argument `&mut x` to a `&mut T` parameter, the type of x)
            if pat == "=" {
                let by_ref = refm == "&";
                let (s2, strip) = if by_ref && f.sym(s, b'&') { (if f.tok(s + 1) == "mut" { s + 2 } else { s + 1 }, false) } else { (s, by_ref) };
                if let Some(at) = expr_end(f, s2, false).filter(|&e| e < close).and_then(|e| expr_type(ws, file, e, depth + 1)) {
                    let at = if strip { strip_ref(&at).to_string() } else { at };
                    ret = subst(&ret, &[fty.clone()], &[at]);
                }
                continue;
            }
            let closure = f.sym(s, b'|') || f.tok(s) == "move";
            // (a fn name passed `and_then(str_lit)` / `map(Path::new)`: the return type of that fn)
            let mut e = s;
            while f.ident(e) && f.before_path(e) {
                e += 3;
            }
            let fn_path = !closure && f.ident(e) && (f.sym(e + 1, b',') || e + 1 == close);
            let r = if closure {
                closure_ret(ws, file, s, depth + 1)
            } else if fn_path {
                def_at(ws, file, f.toks[e].1 as usize).first().and_then(|l| ws[l.file].defs.iter().find(|d| d.start == l.start && matches!(d.kind, SymKind::Function | SymKind::Method))).map(|d| d.ty.clone()).filter(|t| !t.is_empty())
            } else {
                None
            };
            let Some(r) = r else { continue };
            // (type parameters in pat: 1 letter, or a short name starting with an uppercase letter)
            let vars: Vec<String> = pat.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty() && w.starts_with(|c: char| c.is_ascii_uppercase()) && w.len() <= 2).map(|w| w.to_string()).collect();
            let mut b = vec![];
            unify(pat, &r, &vars, &mut b);
            let (ps, vs): (Vec<String>, Vec<String>) = b.into_iter().unzip();
            // (the closure's type parameter F first: replaced all at once so as not to replace another F in the return type (`parse`'s type parameter))
            let (mut ps, mut vs) = (ps, vs);
            if !fty.is_empty() {
                let args = closure_args(ws, file, s, depth + 1).unwrap_or_default();
                ps.push(fty.clone());
                vs.push(format!("impl FnMut({args}) -> {r}"));
            }
            ret = subst(&ret, &ps, &vs);
        }
    }
    ret
}

/// Removes lifetime names from references (`&'a str` → `&str`. `'static` is kept: same as rust-analyzer's display)
fn strip_lifetimes(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut it = t.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if c == '&' && t[i + 1..].starts_with('\'') && !t[i + 1..].starts_with("'static") {
            out.push('&');
            // (skips `'a `)
            while let Some(&(_, d)) = it.peek() {
                it.next();
                if d == ' ' {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Decides the type t of binding b (one with unknown parts, like `Vec<_>`) from later uses in the same fn: `v.push(x)` /
/// `v.insert(k, x)` (the Vec's element), and `v = EXPR;` (the type of the assigned expression)
fn refine_by_use(ws: &[&FileIndex], file: usize, b: usize, t: &str, depth: u32) -> Option<String> {
    let f = ws[file];
    if depth > 16 {
        return None;
    }
    let name = f.tok(b);
    let fe = f.enclosing_fn(f.toks[b].1 as usize)?.item_end;
    for &q in f.idents.get(name)? {
        let q = q as usize;
        if q <= b || (f.toks[q].1 as usize) >= fe || f.local(q, name) != Some(b) {
            continue;
        }
        // (`v.push(x)`)
        if f.sym(q + 1, b'.') && f.ident(q + 2) && f.tok(q + 2) == "push" && f.sym(q + 3, b'(') && type_head(t) == "Vec" {
            let c = close_of(f, q + 3);
            if c > q + 4 {
                if let Some(et) = expr_type(ws, file, c - 1, depth + 1) {
                    return Some(format!("Vec<{et}>"));
                }
            }
        }
        // (whether the type is known: contains neither `_` nor a 1-letter name with no type definition (a leftover type parameter `B`))
        let known = |x: &String| {
            !x.contains('_')
                && !x.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w.len() == 1 && w.starts_with(|c: char| c.is_ascii_uppercase()) && !named(ws, file, w).iter().any(|(_, d)| matches!(d.kind, SymKind::Struct | SymKind::Enum | SymKind::TypeAlias | SymKind::Union)))
        };
        // (`v = EXPR;`)
        if f.sym(q + 1, b'=') && !f.sym(q + 2, b'=') && !(q >= 1 && (f.sym(q - 1, b'.') || f.ident(q - 1) && f.tok(q - 1) == "let")) {
            if let Some(e) = expr_end(f, q + 2, false) {
                if let Some(et) = expr_type(ws, file, e, depth + 1).filter(known) {
                    return Some(et);
                }
            }
        }
        // (`f(.., &mut v, ..)` / `x.m(.., &mut v)`: the parameter type of the called fn (a workspace fn))
        if q >= 2 && f.sym(q - 2, b'&') && f.tok(q - 1) == "mut" && (f.sym(q + 1, b',') || f.sym(q + 1, b')')) {
            if let Some(pt) = arg_param_type(ws, file, q - 2) {
                let pt = pt.trim_start_matches("&mut ").trim().to_string();
                if type_head(&pt) == type_head(t) && known(&pt) {
                    return Some(pt);
                }
            }
        }
    }
    None
}

/// The parameter type of the called fn that receives a call argument (first token a) (from the signature of a workspace fn; self is not counted)
fn arg_param_type(ws: &[&FileIndex], file: usize, a: usize) -> Option<String> {
    let f = ws[file];
    // (the enclosing `(`, and which argument it is)
    let mut j = a;
    let mut d = 0;
    let mut idx = 0;
    loop {
        j = j.checked_sub(1)?;
        if f.sym(j, b')') || f.sym(j, b']') || f.sym(j, b'}') {
            d += 1;
        } else if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'{') {
            if d == 0 {
                break;
            }
            d -= 1;
        } else if d == 0 && f.sym(j, b',') {
            idx += 1;
        } else if d == 0 && f.sym(j, b';') {
            return None;
        }
    }
    if !f.sym(j, b'(') || j == 0 || !f.ident(j - 1) {
        return None;
    }
    let l = *def_at(ws, file, f.toks[j - 1].1 as usize).first()?;
    let w = ws[l.file];
    if w.slim {
        return None;
    }
    let d = w.defs.iter().find(|d| d.start == l.start && matches!(d.kind, SymKind::Function | SymKind::Method))?;
    let sig = signature(w.text.get(d.item_start..d.item_end)?, d.kind);
    let o = sig.find('(')?;
    let ps = tuple_items(&format!("({})", &sig[o + 1..sig.rfind(')')?]))?;
    let ps: Vec<&String> = ps.iter().filter(|p| !p.trim_start_matches('&').trim_start_matches("mut ").trim_start().starts_with("self")).collect();
    let p = ps.get(idx)?;
    Some(p.split_once(':')?.1.trim().to_string())
}

/// One step of the path within a pattern (applied from the outside in)
#[derive(Debug, Clone)]
enum PatStep {
    /// The i-th of a tuple `(.., x, ..)`
    Tuple(usize),
    /// The i-th of a variant / tuple struct `Q::V(.., x, ..)` (the token of V)
    Variant(usize, usize),
    /// Field f of a struct pattern `S { f: x }` (the token of S)
    Field(String, usize),
    /// An element of a slice pattern `[a, b, ..]`
    Slice,
}

/// The type at the place of the pattern containing binding b, with the path from the outside to b applied. The places of patterns:
/// `let PAT (: T)? = EXPR` / `if let` / `while let` / `for PAT in EXPR` / a match arm `PAT (if ..)? =>`.
/// Bindings without a path (at the head of the pattern) and closure / fn parameters are not handled (handled elsewhere)
fn pattern_type(ws: &[&FileIndex], file: usize, b: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    if depth > 24 {
        return None;
    }
    // (goes backwards and collects the pattern steps enclosing b (from the inside out))
    let mut steps: Vec<PatStep> = vec![];
    let mut j = b;
    let mut idx = 0usize;
    let mut d = 0i32;
    // (the f of `f: x` in a struct pattern)
    let mut field: Option<String> = if b >= 2 && f.sym(b - 1, b':') && !f.sym(b - 2, b':') && f.ident(b - 2) {
        Some(f.tok(b - 2).to_string())
    } else if f.sym(b + 1, b',') || f.sym(b + 1, b'}') {
        Some(f.tok(b).to_string())
    } else {
        None
    };
    let root;
    let mut mroot = 0;
    // (if inside a match arm pattern, its start: does not go back into the previous arm's body (`x == y,`))
    let arm_start = {
        let mut q = b + 1;
        let mut d = 0i32;
        let mut found = None;
        while q < f.toks.len() && q < b + 256 {
            if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                d += 1;
            } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                d -= 1;
                if d < -8 {
                    break;
                }
            } else if f.sym(q, b';') {
                break;
            } else if f.sym(q, b'=') && f.sym(q + 1, b'>') {
                let (pa, pe) = f.arm_pat(q);
                if pa <= b && b < pe {
                    found = Some(pa);
                }
                break;
            }
            q += 1;
        }
        found
    };
    loop {
        if j == 0 {
            return None;
        }
        if arm_start.is_some_and(|pa| j <= pa) {
            root = 'a';
            break;
        }
        j -= 1;
        if f.toks[j].0 != 1 && !f.ident(j) {
            continue;
        }
        if f.sym(j, b')') || f.sym(j, b']') || f.sym(j, b'}') {
            if d == 0 && f.sym(j, b'}') {
                // (if not the closing bracket of a struct pattern `S { .. }`, the end of the previous arm's body: the start of the arm)
                let o = f.open_of(j);
                let pat = o.is_some_and(|o| o >= 1 && f.ident(o - 1) && f.tok(o - 1).starts_with(|c: char| c.is_ascii_uppercase()));
                if !pat {
                    root = 'a';
                    break;
                }
            }
            d += 1;
            continue;
        }
        if f.sym(j, b'(') || f.sym(j, b'[') || f.sym(j, b'{') {
            if d > 0 {
                d -= 1;
                continue;
            }
            if f.sym(j, b'(') {
                if j >= 1 && f.ident(j - 1) && f.tok(j - 1).starts_with(|c: char| c.is_ascii_uppercase()) {
                    steps.push(PatStep::Variant(j - 1, idx));
                } else if j >= 1 && f.ident(j - 1) && matches!(f.tok(j - 1), "let" | "for" | "mut" | "ref") || j >= 1 && (f.sym(j - 1, b'&') || f.sym(j - 1, b',') || f.sym(j - 1, b'(') || f.sym(j - 1, b'|') || f.sym(j - 1, b'[') || f.sym(j - 1, b'{') || f.sym(j - 1, b'>')) {
                    steps.push(PatStep::Tuple(idx));
                } else if j >= 2 && f.sym(j - 1, b'!') && f.tok(j - 2) == "matches" && idx >= 1 {
                    // (the pattern of `matches!(EXPR, PAT)`: the matched expression is the first argument)
                    mroot = j;
                    root = 'm';
                    break;
                } else {
                    // (the brackets of a call: not a pattern)
                    return None;
                }
            } else if f.sym(j, b'[') {
                steps.push(PatStep::Slice);
            } else {
                // (a struct pattern `S { .. }`, or the block at the start of the arm)
                if j >= 1 && f.ident(j - 1) && f.tok(j - 1).starts_with(|c: char| c.is_ascii_uppercase()) && f.tok(j - 1) != "Self" {
                    steps.push(PatStep::Field(field.take()?, j - 1));
                } else {
                    root = 'a';
                    break;
                }
            }
            idx = 0;
            // (the field name of the outer step: the `f:` before the head (name / path) of this step)
            let mut h = if f.sym(j, b'[') || (j >= 1 && !f.ident(j - 1)) { j } else { j - 1 };
            while h >= 3 && f.after_path(h) {
                h -= 3;
            }
            field = (h >= 2 && f.sym(h - 1, b':') && !f.sym(h - 2, b':') && f.ident(h - 2)).then(|| f.tok(h - 2).to_string());
            continue;
        }
        if d > 0 {
            continue;
        }
        if f.sym(j, b',') {
            // (the arm's pattern is just a binding `.., s if .. =>`: the previous arm's body ends with `,` without brackets)
            if steps.is_empty() && (f.sym(b + 1, b'=') && f.sym(b + 2, b'>') || f.ident(b + 1) && f.tok(b + 1) == "if") {
                root = 'a';
                break;
            }
            idx += 1;
            continue;
        }
        if f.sym(j, b'>') && j >= 1 && f.sym(j - 1, b'=') {
            root = 'a';
            break;
        }
        if f.sym(j, b'|') {
            // (closure parameters are handled elsewhere. An or-pattern `A(x) | B(x)` continues)
            if f.closure_start(j) {
                return None;
            }
            continue;
        }
        if f.sym(j, b';') || f.sym(j, b'=') {
            return None;
        }
        if f.ident(j) {
            match f.tok(j) {
                "let" => {
                    root = 'l';
                    break;
                }
                "for" => {
                    root = 'f';
                    break;
                }
                "match" | "if" | "while" | "fn" | "return" | "in" => return None,
                _ => {}
            }
        }
    }
    if steps.is_empty() {
        // (the match arm's pattern is just a binding `s if .. =>` / `n =>`: the type of the matched expression)
        if root == 'a' && f.sym(b + 1, b'=') && f.sym(b + 2, b'>') || root == 'a' && f.ident(b + 1) && f.tok(b + 1) == "if" {
            return scrutinee_type(ws, file, b, depth + 1);
        }
        return None;
    }
    steps.reverse();
    // (the type of the root)
    let rt = match root {
        'f' => {
            let (in_tok, _) = for_pattern_in(f, b)?;
            let e = expr_end(f, in_tok + 1, true)?;
            let t = match range_type(ws, file, in_tok + 1, e, depth) {
                Some(t) => t,
                None => expr_type(ws, file, e, depth + 1)?,
            };
            let t = if f.sym(in_tok + 1, b'&') && !t.starts_with('&') { if f.tok(in_tok + 2) == "mut" { format!("&mut {t}") } else { format!("&{t}") } } else { t };
            if is_iter_head(&type_head(&t)) { iter_item_ws(ws, file, &t) } else { iter_item(&t).or_else(|| elem_type(&t)).or_else(|| iter_item_ws(ws, file, &t)) }?
        }
        'l' => {
            // (after the pattern: a type annotation `: T` or `= EXPR`)
            let mut q = b + 1;
            let mut dd = steps.len() as i32;
            while q < f.toks.len() && dd > 0 {
                if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
                    dd += 1;
                } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
                    dd -= 1;
                } else if f.sym(q, b';') {
                    return None;
                }
                q += 1;
            }
            if f.sym(q, b':') && !f.sym(q + 1, b':') {
                let mut e = q + 1;
                while e < f.toks.len() && !(f.sym(e, b'=') && !f.sym(e + 1, b'=')) && !f.sym(e, b';') {
                    e += 1;
                }
                f.text[f.toks.get(q + 1)?.1 as usize..f.toks.get(e - 1)?.2 as usize].trim().to_string()
            } else if f.sym(q, b'=') && !f.sym(q + 1, b'=') {
                let e = expr_end(f, q + 1, true)?;
                expr_type(ws, file, e, depth + 1)?
            } else {
                return None;
            }
        }
        'm' => {
            let e = expr_end(f, mroot + 1, true)?;
            expr_type(ws, file, e, depth + 1)?
        }
        _ => scrutinee_type(ws, file, b, depth + 1)?,
    };
    let mut ty = rt;
    for st in &steps {
        let cur = expand(ws, file, strip_ref(&ty)).into_iter().last().unwrap_or_else(|| strip_ref(&ty).to_string());
        ty = match st {
            PatStep::Tuple(i) => tuple_items(strip_ref(&cur))?.get(*i)?.clone(),
            PatStep::Slice => elem_type(&cur).or_else(|| strip_ref(&cur).strip_prefix('[').and_then(|r| r.split([';', ']']).next()).map(|x| x.trim().to_string()))?,
            // (the owner of the fields of a struct-like variant `E::W { x }` is the variant W)
            PatStep::Field(n, st) => member_type(ws, file, &cur, n, &[SymKind::Field]).or_else(|| {
                let w = f.tok(*st);
                named(ws, file, n).into_iter().find(|(_, d)| d.kind == SymKind::Field && d.owner == w).map(|(_, d)| d.ty.clone())
            })?,
            PatStep::Variant(vt, i) => {
                let v = f.tok(*vt);
                match v {
                    "Some" | "Ok" => targs(&cur).into_iter().next()?,
                    "Err" => targs(&cur).into_iter().nth(1)?,
                    _ => variant_field(ws, file, &cur, *vt, *i)?,
                }
            }
        };
        if ty == "_" {
            return None;
        }
    }
    Some(ty)
}

/// The type of the i-th field of a variant / tuple struct pattern `Q::V(..)` (the token of V is vt). The type arguments of the matched type cur are applied to
/// the enum's type parameters
fn variant_field(ws: &[&FileIndex], file: usize, cur: &str, vt: usize, i: usize) -> Option<String> {
    let f = ws[file];
    let v = f.tok(vt);
    let head = type_head(cur);
    let q = if f.after_path(vt) && vt >= 3 { Some(f.tok(vt - 3).to_string()) } else { None };
    let q = match q.as_deref() {
        Some("Self") => f.enclosing_owner(f.toks[vt].1 as usize),
        _ => q,
    };
    let kr = f.krate;
    // (the module of the enum reached by following the qualifier `Ty::` through use: when an enum with the same name is in another crate)
    let qmod: Option<Vec<String>> = if f.after_path(vt) && vt >= 3 && f.tok(vt - 3) != "Self" {
        def_at(ws, file, f.toks[vt - 3].1 as usize).first().and_then(|l| ws[l.file].defs.iter().find(|d| d.start == l.start && d.kind == SymKind::Enum).map(|d| d.module.to_vec()))
    } else {
        None
    };
    // (the PTy of a qualifier alias `use parse::Ty as PTy` by its original name)
    let q = q.map(|q| f.use_alias(&q).map(|s| s.to_string()).unwrap_or(q));
    // (a variant (whose owner is the matched type or the qualifier), or a tuple struct `P(a, b)`)
    let (di, d) = named(ws, file, v)
        .into_iter()
        .filter(|(_, d)| (d.kind == SymKind::Variant && (d.owner == head || q.as_deref() == Some(d.owner.as_str()))) || (d.kind == SymKind::Struct && d.owner.is_empty()))
        .min_by_key(|(k, d)| (d.owner != head && q.as_deref() != Some(d.owner.as_str()), qmod.as_ref().is_some_and(|m| d.module[..] != m[..]), *k != file, ws[*k].krate != kr))?;
    let text = &ws[di].text;
    let inner = match d.fnret.iter().find(|x| x.0 == 255) {
        Some(x) => x.2.as_str(),
        None => {
            let vtxt = text.get(d.item_start..d.item_end)?;
            let a = vtxt.find('(')?;
            &vtxt[a + 1..vtxt.rfind(')')?]
        }
    };
    let tys = targs(&format!("X<{inner}>"));
    let ty = tys.get(i)?.trim_start_matches("pub ").trim().to_string();
    let params = &d.params;
    if !params.is_empty() && type_head(cur) == (if d.kind == SymKind::Variant { d.owner.as_str() } else { d.name.as_str() }) {
        return Some(subst(&ty, params, &targs(cur)));
    }
    Some(ty)
}

/// Only the `in` token of for_pattern (the pattern depth may be anything)
fn for_pattern_in(f: &FileIndex, b: usize) -> Option<(usize, ())> {
    let mut q = b + 1;
    let mut d = 0i32;
    while q < f.toks.len() {
        if f.sym(q, b'(') || f.sym(q, b'[') || f.sym(q, b'{') {
            if f.sym(q, b'{') && d <= 0 {
                return None;
            }
            d += 1;
        } else if f.sym(q, b')') || f.sym(q, b']') || f.sym(q, b'}') {
            d -= 1;
        } else if d <= 0 && f.ident(q) && f.tok(q) == "in" {
            return Some((q, ()));
        } else if f.sym(q, b';') {
            return None;
        }
        q += 1;
    }
    None
}

/// If binding b is in a `Some(..)` / `Ok(..)` / `Err(..)` pattern (in a tuple inside it) followed by `= EXPR`:
/// (Some / Ok / Err, the token of `=`, the list of indices within the tuple)
fn wrap_pattern(f: &FileIndex, b: usize) -> Option<(&'static str, usize, Vec<usize>)> {
    let mut j = b;
    let mut idx = 0;
    let mut path = vec![];
    let mut d = 0;
    let wrap = loop {
        j = j.checked_sub(1)?;
        if f.sym(j, b')') {
            d += 1;
        } else if f.sym(j, b'(') {
            if d == 0 {
                if j >= 1 && f.ident(j - 1) {
                    break match f.tok(j - 1) {
                        "Some" => "Some",
                        "Ok" => "Ok",
                        "Err" => "Err",
                        _ => return None,
                    };
                }
                path.push(idx);
                idx = 0;
            } else {
                d -= 1;
            }
        } else if f.sym(j, b',') && d == 0 {
            idx += 1;
        } else if !(f.ident(j) || f.sym(j, b'&') || f.sym(j, b',')) || matches!(f.tok(j), "let" | "in" | "if" | "match") {
            return None;
        }
    };
    if idx != 0 {
        return None;
    }
    path.reverse();
    // (forwards: the `=` after the wrapper's `)`)
    let mut q = b + 1;
    let mut d = path.len() as i32 + 1;
    while q < f.toks.len() {
        if f.sym(q, b'(') {
            d += 1;
        } else if f.sym(q, b')') {
            d -= 1;
            if d == 0 {
                return (f.sym(q + 1, b'=') && !f.sym(q + 2, b'=') && !f.sym(q + 2, b'>')).then_some((wrap, q + 1, path));
            }
        } else if f.sym(q, b';') || f.sym(q, b'{') {
            return None;
        }
        q += 1;
    }
    None
}

/// If binding b is in a tuple pattern of `let (..) =`: (the token after the pattern (`=` or `:`), the list of indices within the tuple)
fn let_tuple_pattern(f: &FileIndex, b: usize) -> Option<(usize, Vec<usize>)> {
    let mut j = b;
    let mut idx = 0;
    let mut path = vec![];
    let mut d = 0;
    loop {
        j = j.checked_sub(1)?;
        if f.ident(j) && f.tok(j) == "let" {
            break;
        }
        if f.sym(j, b')') {
            d += 1;
        } else if f.sym(j, b'(') {
            if d == 0 {
                // (`Err(e)` / `T(a, b)` patterns are not tuples)
                if j >= 1 && f.ident(j - 1) && f.tok(j - 1) != "let" && f.tok(j - 1) != "mut" {
                    return None;
                }
                path.push(idx);
                idx = 0;
            } else {
                d -= 1;
            }
        } else if f.sym(j, b',') && d == 0 {
            idx += 1;
        } else if !(f.ident(j) || f.sym(j, b'&')) || matches!(f.tok(j), "in" | "for" | "if" | "while" | "match" | "else") {
            return None;
        }
    }
    // (`let (..)`, not `let mut (..)`: the pattern starts with `(`)
    if idx != 0 || path.is_empty() || (j >= 1 && f.ident(j - 1) && matches!(f.tok(j - 1), "if" | "while")) {
        return None;
    }
    path.reverse();
    let mut q = b + 1;
    let mut d = path.len() as i32;
    while q < f.toks.len() {
        if f.sym(q, b'(') {
            d += 1;
        } else if f.sym(q, b')') {
            d -= 1;
            if d == 0 {
                return Some((q + 1, path));
            }
        } else if f.sym(q, b';') || f.sym(q, b'{') {
            return None;
        }
        q += 1;
    }
    None
}

/// If binding b is in the PAT of `for PAT in`: (the token of `in`, the list of indices within the tuple)
fn for_pattern(f: &FileIndex, b: usize) -> Option<(usize, Vec<usize>)> {
    // (backwards: up to `for`. Counts commas and tuple brackets)
    let mut j = b;
    let mut idx = 0;
    let mut path = vec![];
    let mut d = 0;
    loop {
        j = j.checked_sub(1)?;
        if f.ident(j) && f.tok(j) == "for" {
            break;
        }
        if f.sym(j, b')') {
            d += 1;
        } else if f.sym(j, b'(') {
            if d == 0 {
                path.push(idx);
                idx = 0;
            } else {
                d -= 1;
            }
        } else if f.sym(j, b',') && d == 0 {
            idx += 1;
        } else if !(f.ident(j) || f.sym(j, b'&') || f.sym(j, b',')) || matches!(f.tok(j), "in" | "let") {
            return None;
        }
    }
    if idx != 0 {
        return None;
    }
    path.reverse();
    // (forwards: `in`. b is inside tuple brackets (at the depth of path))
    let mut q = b + 1;
    let mut d = path.len() as i32;
    while q < f.toks.len() {
        if f.sym(q, b'(') {
            d += 1;
        } else if f.sym(q, b')') {
            d -= 1;
        } else if d == 0 && f.ident(q) && f.tok(q) == "in" {
            return Some((q, path));
        } else if d < 0 || f.sym(q, b'{') || f.sym(q, b';') {
            return None;
        }
        q += 1;
    }
    None
}

/// The type of closure parameter b (the a / b of `recv.m(|a, b| ..)`): the element type of an iteration (`Iter<T>` / `Vec<T>` / `[T]` / `Option<T>` ..).
/// The parameter of `filter` / `find` / `position` ... is a reference to the element, for `fold` the 2nd is the element, for `map_err` / `unwrap_or_else` the E of the Result
fn closure_param_type(ws: &[&FileIndex], file: usize, b: usize, depth: u32) -> Option<String> {
    let f = ws[file];
    if depth > 24 {
        return None;
    }
    // (the `|` before b: the start of the parameter list. The number of commas in between is the parameter index)
    // (if inside a tuple pattern `|(i, x)|`, also the index within that tuple: path = [parameter index, index within the tuple..])
    let mut j = b;
    let mut idx = 0;
    let mut path: Vec<usize> = vec![];
    let mut d = 0i32;
    loop {
        j = j.checked_sub(1)?;
        if f.sym(j, b'|') {
            break;
        }
        if f.sym(j, b')') {
            d += 1;
        } else if f.sym(j, b'(') {
            if d == 0 {
                path.push(idx);
                idx = 0;
            } else {
                d -= 1;
            }
        } else if f.sym(j, b',') && d == 0 {
            idx += 1;
        } else if !(f.ident(j) || f.sym(j, b'&') || f.sym(j, b',')) {
            return None;
        }
    }
    path.push(idx);
    path.reverse();
    let idx = path[0];
    if !f.closure_start(j) || (j >= 1 && f.sym(j - 1, b'|')) {
        return None;
    }
    // (the m of `recv.m(`, and the end of the receiver expression. The closure may be a later argument: `fold(0, |acc, x| ..)`)
    let mut open = if j >= 1 && f.ident(j - 1) && f.tok(j - 1) == "move" { j.checked_sub(2)? } else { j.checked_sub(1)? };
    let mut d = 0;
    while !(d == 0 && f.sym(open, b'(')) {
        if f.sym(open, b')') || f.sym(open, b']') || f.sym(open, b'}') {
            d += 1;
        } else if f.sym(open, b'(') || f.sym(open, b'[') || f.sym(open, b'{') {
            if d == 0 {
                return None;
            }
            d -= 1;
        } else if d == 0 && f.sym(open, b';') {
            return None;
        }
        open = open.checked_sub(1)?;
    }
    if open < 3 || !f.ident(open - 1) || !f.sym(open - 2, b'.') {
        return None;
    }
    let m = f.tok(open - 1);
    let recv = expr_type(ws, file, open - 3, depth + 1)?;
    // (from the parameter shape of the bound `F: FnOnce(&T) -> R` in the method signature: `cell.with(|m| ..)` / `opt.map(|x| ..)` / `iter.fold(0, |a, x| ..)`)
    if let Some(t) = closure_param_from_sig(ws, file, &recv, m, open, j, idx, &path, depth) {
        return Some(t);
    }
    let recv = expand(ws, file, &recv).into_iter().last().unwrap_or(recv);
    let h = type_head(&recv);
    let a = targs(&recv);
    let elem = match h.as_str() {
        "Result" => match m {
            "map_err" | "unwrap_or_else" | "or_else" => a.get(1).cloned(),
            _ => a.first().cloned(),
        },
        "HashMap" | "BTreeMap" | "FxHashMap" => Some(format!("(&{}, &{})", a.first()?, a.get(1)?)),
        _ if is_iter_head(&h) => iter_item(&recv),
        _ => elem_type(&recv).or_else(|| iter_item_ws(ws, file, &recv)),
    }?;
    let by_ref = matches!(m, "filter" | "find" | "position" | "skip_while" | "take_while" | "inspect" | "retain" | "sort_by_key" | "max_by_key" | "min_by_key" | "is_some_and" | "is_ok_and" | "any" | "all" | "dedup_by_key" | "binary_search_by_key" | "partition")
        && !matches!(m, "any" | "all" | "position" | "partition" | "is_some_and" | "is_ok_and");
    let ty = match (m, idx) {
        ("fold" | "try_fold" | "scan", 0) => return None,
        ("fold" | "try_fold", 1) => elem,
        ("sort_by" | "max_by" | "min_by" | "dedup_by", _) => format!("&{elem}"),
        (_, 0) => if by_ref { format!("&{elem}") } else { elem },
        _ => return None,
    };
    // (inside a tuple pattern: the element at that index. The reference of `&(a, b)` is removed)
    let mut ty = ty;
    for &i in &path[1..] {
        let inner = strip_ref(&ty).to_string();
        ty = tuple_items(&inner)?.get(i)?.clone();
    }
    Some(ty)
}

/// The type of the literal ending at token k (`"a"` → &str, `'c'` → char, `b"x"` → &[u8], `b'x'` → u8, `1u8` → u8, `true` → bool).
/// Only when the whole expression is the literal (the 1 of `x + 1` is not the type of the expression). The type of a number without a suffix (`0` / `1.5`) is decided by its use
/// (rust-analyzer makes `let mut acc = 0; acc += x_i64` i64), so no answer.
/// (in the tokens a number is symbols of 1 char each, and the suffix is the identifier that follows)
fn literal_type(f: &FileIndex, k: usize) -> Option<String> {
    let b = f.text.as_bytes();
    let digit = |j: usize| f.toks[j].0 == 1 && b[f.toks[j].1 as usize].is_ascii_digit();
    let adj = |j: usize| j >= 1 && f.toks[j - 1].2 == f.toks[j].1;
    // (whether what precedes the expression starting at s is a place where an expression can start)
    let whole = |s: usize| -> bool {
        if s == 0 {
            return true;
        }
        let p = s - 1;
        if f.ident(p) {
            return matches!(f.tok(p), "return" | "break" | "in");
        }
        if f.sym(p, b'=') {
            return !(p >= 1 && f.toks[p - 1].2 == f.toks[p].1 && f.toks[p - 1].0 == 1 && b"=!<>+-*/%&|^".contains(&b[f.toks[p - 1].1 as usize]));
        }
        if f.sym(p, b'>') {
            return p >= 1 && f.sym(p - 1, b'=') && f.toks[p - 1].2 == f.toks[p].1;
        }
        f.toks[p].0 == 1 && matches!(b[f.toks[p].1 as usize], b'(' | b',' | b'{' | b'[' | b';' | b'|')
    };
    if f.toks[k].0 == 2 {
        let s = f.tok(k);
        if !whole(k) {
            return None;
        }
        return Some(
            if s.starts_with("b'") {
                "u8"
            } else if s.starts_with('b') {
                "&[u8]"
            } else if s.starts_with('\'') {
                "char"
            } else {
                "&str"
            }
            .to_string(),
        );
    }
    if !f.ident(k) {
        return None;
    }
    let w = f.tok(k);
    if w == "true" || w == "false" {
        return whole(k).then(|| "bool".to_string());
    }
    // (a suffix `1u8` / `2.0f32` / `1_000usize`: the start of the digits / `.` / `_` continuing backwards is the start of the expression)
    // (`1_000usize` is `1` and `_000usize`)
    let suf = w.trim_start_matches(|c: char| c == '_' || c.is_ascii_digit());
    if PRIMS.contains(&suf) && adj(k) && digit(k - 1) {
        let mut j = k - 1;
        while j >= 1 && adj(j) && (digit(j - 1) || f.sym(j - 1, b'.')) {
            j -= 1;
        }
        return whole(j).then(|| suf.to_string());
    }
    None
}

/// Hover of the string / char literal containing off (`&'static str` and the value)
fn literal_hover(f: &FileIndex, off: usize) -> Option<String> {
    let i = f.toks.partition_point(|t| (t.2 as usize) <= off);
    let t = *f.toks.get(i)?;
    if t.0 != 2 || (t.1 as usize) > off {
        return None;
    }
    let s = &f.text[t.1 as usize..t.2 as usize];
    let (byte, s) = match s.strip_prefix('b') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (ty, body) = if let Some(r) = s.strip_prefix('r') {
        // (a raw string `r#"..."#`: the inside as it is)
        let h = r.len() - r.trim_start_matches('#').len();
        let inner = r.get(h + 1..r.len().checked_sub(h + 1)?)?;
        (if byte { format!("&'static [u8; {}]", inner.len()) } else { "&'static str".to_string() }, inner.to_string())
    } else if let Some(r) = s.strip_prefix('"') {
        let v = unescape(r.strip_suffix('"').unwrap_or(r));
        (if byte { format!("&'static [u8; {}]", v.len()) } else { "&'static str".to_string() }, v)
    } else if let Some(r) = s.strip_prefix('\'') {
        (if byte { "u8" } else { "char" }.to_string(), unescape(r.strip_suffix('\'').unwrap_or(r)))
    } else {
        return None;
    };
    Some(format!("```rust\n{ty}\n```\n---\n\nvalue of literal: ` {body} `"))
}

/// Undoes the escapes in a literal (`\n` / `\t` / `\\` / `\"` / `\'` / `\0` / `\u{..}` / `\x..` / line continuation)
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('x') => {
                let h: String = it.by_ref().take(2).collect();
                if let Some(ch) = u8::from_str_radix(&h, 16).ok().map(char::from) {
                    out.push(ch);
                }
            }
            Some('u') => {
                let h: String = it.by_ref().skip_while(|&c| c == '{').take_while(|&c| c != '}').collect();
                if let Some(ch) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            // (a line continuation `\` + newline: also skips the whitespace at the start of the next line)
            Some('\n') => {
                while it.peek().is_some_and(|c| c.is_whitespace()) {
                    it.next();
                }
            }
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// If local binding t is a closure (`let f = |a: T, b| -> R { .. }` / `move |..|`), its signature `impl Fn(a: T, b) -> R`
/// (if the return type is not written, the type of the body expression)
fn closure_sig(ws: &[&FileIndex], t: Loc) -> Option<String> {
    let f = ws[t.file];
    if f.slim {
        return None;
    }
    let b = f.toks.partition_point(|x| (x.1 as usize) < t.start);
    if !(f.sym(b + 1, b'=') && !f.sym(b + 2, b'=')) {
        return None;
    }
    let mut c = b + 2;
    if f.tok(c) == "move" {
        c += 1;
    }
    if !f.sym(c, b'|') {
        return None;
    }
    let mut e = c + 1;
    while e < f.toks.len() && !f.sym(e, b'|') {
        e += 1;
    }
    if e >= f.toks.len() {
        return None;
    }
    let params = if e > c + 1 { f.text[f.toks[c].2 as usize..f.toks[e].1 as usize].split_whitespace().collect::<Vec<_>>().join(" ") } else { String::new() };
    let ret = if f.sym(e + 1, b'-') && f.sym(e + 2, b'>') {
        let mut j = e + 3;
        while j < f.toks.len() && !f.sym(j, b'{') {
            j += 1;
        }
        let r = f.text.get(f.toks.get(e + 3)?.1 as usize..f.toks.get(j)?.1 as usize)?.trim();
        format!(" -> {}", r.split_whitespace().collect::<Vec<_>>().join(" "))
    } else {
        expr_end(f, e + 1, false).and_then(|x| expr_type(ws, t.file, x, 1)).map(|r| format!(" -> {r}")).unwrap_or_default()
    };
    Some(format!("impl Fn({params}){ret}"))
}

/// (for inspection) The type of the expression ending at the token at off (the LSP's `zustc/typeAt`)
pub fn type_at(ws: &[&FileIndex], file: usize, off: usize) -> Option<String> {
    clear_memo();
    let f = ws[file];
    let k = f.toks.partition_point(|t| (t.2 as usize) <= off).min(f.toks.len().checked_sub(1)?);
    let k = if (f.toks[k].1 as usize) > off { k.checked_sub(1)? } else { k };
    // (at the position of a binding itself (pattern, parameter, `let x`): the type of that binding. Otherwise the type of the expression)
    if f.ident(k) && f.local(k, f.tok(k)).is_none() {
        let at = f.toks[k].1 as usize;
        if def_at(ws, file, at).first().is_some_and(|l| l.file == file && l.start == at && !f.defs.iter().any(|d| d.start == at)) {
            if let Some(t) = binding_type(ws, file, k, 0) {
                return Some(t);
            }
        }
    }
    expr_type(ws, file, k, 0)
}
