//! Rust lowering for standalone modules and the Wasm backend. The generated code preserves zustscript numeric
//! semantics across targets: integer operations wrap (`wrapping_*`), `/` and `%`
//! fail on zero; `as` saturates from f64, as Rust's `as` does.
//!
//! The subset (anything else is an error naming the construct):
//! - types: i32 u32 i64 f64 bool, arrays, and records and enums of them (nested, generic); an array or a record /
//!   enum that holds one is not Copy (see "Ownership" below);
//! - `let`, `if`, `match`, loops, arithmetic, comparisons, `as`, `T::zero()` / `T::one()`;
//! - calls of ordinary Rust-compatible functions, `@wasm` functions and of `wasm fn`s (Rust), recursion;
//! - on arrays: `len` `map` `filter` `fold` `any` `all` `push` `concat` and array literals; a closure only as
//!   the argument of one of those (it becomes a Rust closure, the call an iterator chain);
//! - f64's `sqrt` `abs` `floor` `round`.
//!
//! A numeric-bound function (`T: Num`) is lowered once per instance, as in the JS output (`sum$f64` → Rust
//! `zs_sum__f64`). Only the instances the JS layer calls get a wrapper and glue; the ones only other lowered
//! functions call are plain Rust functions.
//!
//! # Ownership (what Rust's lifetime inference would do, inferred instead of written)
//!
//! zustscript values are immutable, so sharing is always safe; the lowering's job is to say it in Rust with as few
//! copies as possible, without the user writing `&`, `'a` or `.clone()`:
//!
//! - **Parameter modes.** A parameter that is not Copy is borrowed (`&[T]`, `&ZsX`) unless the body consumes it
//!   whole (returns it, stores it, passes it to an owned parameter, pushes onto it), then owned (`Vec<T>`, `ZsX`).
//! - **Borrowed results.** A result that is always (a part of) borrowed parameters is returned as a borrow with a
//!   lifetime: `fn pick<'a>(c: bool, a: &'a ZsP, b: &'a ZsP) -> &'a [f64]`. The caller keeps borrowing, and copies
//!   only if it then consumes the result.
//! - Both are decided together, for all the functions at once, as a fixed point over the call graph: start with every
//!   parameter borrowed and every result borrowed, lower everything, and make owned what the lowering found it had to
//!   copy (a consumed parameter, a result that is not a borrow of parameters); repeat until nothing changes. Each
//!   change only goes one way, so this ends, after a few rounds of a linear pass.
//! - **Moves.** Inside a body, a value the function owns is moved at its last use ([`crate::own::liveness`]) and
//!   cloned elsewhere. A move at the last use makes `acc = acc.push(x)` and `xs.fold([], (a, x) => a.push(x))`
//!   push in place (amortized O(1), not a copy of the array), and `map` / `filter` / `for` over a dead array take
//!   its elements instead of cloning them.
//! - **Local borrows.** `let pts = p.pts` with `p` borrowed is `let pts = &p.pts[..]`; matching or iterating a value
//!   that stays alive binds by reference (`match *s { ZsS::Poly(ref p, n) => .. }`).
//!
//! zustc's borrow checker checks the Rust again, so a mistake here is a compile error, never unsafety.

use crate::ast::*;
use crate::check::{Builtin, Callee, Info, On, Ty};
use crate::js::inst_name;
use crate::own::{self, Live};
use crate::wasm::WFn;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::rc::Rc;

const RUST_KW: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "priv", "pub", "ref", "return", "self", "Self",
    "static", "struct", "super", "trait", "true", "try", "type", "typeof", "union", "unsafe", "unsized", "use", "virtual",
    "where", "while", "yield",
];

/// A source name in Rust: keywords get a `_`, and names in the generated `__zs` namespace are moved out of it, so
/// no source name can meet a temporary the lowering makes. A name of the form `u..u__zs..` (any number of `u`s) gets
/// one more `u`, so the mapping stays injective (`__zs_x` -> `u__zs_x`, `u__zs_x` -> `uu__zs_x`). Likewise a keyword
/// followed by any number of `_`s gets one more (`async` -> `async_`, `async_` -> `async__`)
pub(crate) fn local(n: &str) -> String {
    if RUST_KW.contains(&n.trim_end_matches('_')) {
        format!("{n}_")
    } else if n.trim_start_matches('u').starts_with("__zs") {
        format!("u{n}")
    } else {
        n.to_string()
    }
}

/// The Rust name of an instance: `zs_sum__f64`
fn rs_name(name: &str, targs: &[Ty]) -> String {
    format!("zs_{}", inst_name(name, targs).replace('$', "__"))
}

fn subst_ty(t: &Ty, m: &HashMap<String, Ty>) -> Ty {
    match t {
        Ty::Param(p) => m.get(p).cloned().unwrap_or(t.clone()),
        Ty::Array(a) => Ty::Array(Box::new(subst_ty(a, m))),
        Ty::Typed(a) => Ty::Typed(Box::new(subst_ty(a, m))),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| subst_ty(t, m)).collect()),
        Ty::Named(n, a) => Ty::Named(n.clone(), a.iter().map(|t| subst_ty(t, m)).collect()),
        Ty::Fn(a, r) => Ty::Fn(a.iter().map(|t| subst_ty(t, m)).collect(), Box::new(subst_ty(r, m))),
        o => o.clone(),
    }
}

fn scalar(t: &Ty) -> Option<&'static str> {
    Some(match t {
        Ty::I32 => "i32",
        Ty::U32 => "u32",
        Ty::I64 => "i64",
        Ty::F32 => "f32",
        Ty::F64 => "f64",
        Ty::Bool => "bool",
        Ty::Unit => "()",
        _ => return None,
    })
}

fn is_array(t: &Ty) -> bool {
    matches!(t, Ty::Array(_) | Ty::Typed(_))
}

/// A type whose borrow is a slice of it (`&[T]`, `&str`: `&x[..]`)
fn sliced(t: &Ty) -> bool {
    is_array(t) || *t == Ty::Str
}

/// The string helpers (JS's meaning, in Rust): (name, the helpers it needs, text). zustc's std has no
/// `encode_utf16` and no `memmem`, so the UTF-16 length and order and the substring search are written here
const STR_HELPERS: &[(&str, &[&str], &str)] = &[
    (
        "zs_push_int",
        &[],
        "/// An integer in decimal, appended: `to_string` and the holes of a template string without `core::fmt`\n/// (`format!` and `to_string` pull its formatting machinery into the wasm: about 10 KB)\nfn zs_push_int(s: &mut String, v: i64) {\n    if v < 0 {\n        s.push('-');\n    }\n    let mut u = v.unsigned_abs();\n    let mut buf = [0u8; 20];\n    let mut i = buf.len();\n    loop {\n        i -= 1;\n        buf[i] = b'0' + (u % 10) as u8;\n        u /= 10;\n        if u == 0 {\n            break;\n        }\n    }\n    while i < buf.len() {\n        s.push(buf[i] as char);\n        i += 1;\n    }\n}",
    ),
    (
        "zs_len16",
        &[],
        "/// JS's `length`: UTF-16 code units\nfn zs_len16(s: &str) -> i32 {\n    let mut n = 0i32;\n    for c in s.chars() {\n        n += if (c as u32) > 0xffff { 2 } else { 1 };\n    }\n    n\n}",
    ),
    (
        "zs_units16",
        &[],
        "fn zs_units16(c: char) -> (u32, u32) {\n    let v = c as u32;\n    if v > 0xffff {\n        let w = v - 0x10000;\n        (0xd800 + (w >> 10), 0xdc00 + (w & 0x3ff))\n    } else {\n        (v, 0x10000)\n    }\n}",
    ),
    (
        "zs_cmp16",
        &["zs_units16"],
        "/// JS's string order: by UTF-16 code units (not UTF-8 bytes: they differ above U+FFFF)\nfn zs_cmp16(a: &str, b: &str) -> i32 {\n    let mut x = a.chars();\n    let mut y = b.chars();\n    loop {\n        match (x.next(), y.next()) {\n            (None, None) => return 0,\n            (None, Some(_)) => return -1,\n            (Some(_), None) => return 1,\n            (Some(p), Some(q)) => {\n                if p == q {\n                    continue;\n                }\n                let (p1, p2) = zs_units16(p);\n                let (q1, q2) = zs_units16(q);\n                if p1 != q1 {\n                    return if p1 < q1 { -1 } else { 1 };\n                }\n                return if p2 < q2 { -1 } else { 1 };\n            }\n        }\n    }\n}",
    ),
    (
        "zs_ws",
        &[],
        "/// JS's white space and line terminators (what `trim` removes)\nfn zs_ws(c: char) -> bool {\n    matches!(c, '\\t' | '\\u{b}' | '\\u{c}' | ' ' | '\\u{a0}' | '\\u{feff}' | '\\n' | '\\r' | '\\u{2028}' | '\\u{2029}' | '\\u{1680}' | '\\u{2000}'..='\\u{200a}' | '\\u{202f}' | '\\u{205f}' | '\\u{3000}')\n}",
    ),
    (
        "zs_trim",
        &["zs_ws"],
        "fn zs_trim(s: &str) -> &str {\n    let mut start = s.len();\n    let mut end = 0usize;\n    for (i, c) in s.char_indices() {\n        if !zs_ws(c) {\n            if start == s.len() {\n                start = i;\n            }\n            end = i + c.len_utf8();\n        }\n    }\n    if start >= end { \"\" } else { &s[start..end] }\n}",
    ),
    (
        "zs_contains",
        &[],
        "fn zs_contains(a: &str, b: &str) -> bool {\n    let (x, y) = (a.as_bytes(), b.as_bytes());\n    if y.len() > x.len() {\n        return false;\n    }\n    let mut i = 0usize;\n    while i + y.len() <= x.len() {\n        let mut j = 0usize;\n        while j < y.len() && x[i + j] == y[j] {\n            j += 1;\n        }\n        if j == y.len() {\n            return true;\n        }\n        i += 1;\n    }\n    false\n}",
    ),
    (
        "zs_split",
        &[],
        "/// JS's `split`: an empty separator splits into characters\nfn zs_split(s: &str, sep: &str) -> Vec<String> {\n    if sep.is_empty() {\n        return s.chars().map(|c| c.to_string()).collect::<Vec<_>>();\n    }\n    s.split(sep).map(|x| x.to_string()).collect::<Vec<_>>()\n}",
    ),
];

/// The standard library's string helpers (JS's meaning: UTF-16 indices, JS's white space); see [`STR_HELPERS`]
const STD_STR_HELPERS: &[(&str, &[&str], &str)] = &[
    (
        "zs_trim_start",
        &["zs_ws"],
        "fn zs_trim_start(s: &str) -> &str {\n    for (i, c) in s.char_indices() {\n        if !zs_ws(c) {\n            return &s[i..];\n        }\n    }\n    &s[s.len()..]\n}",
    ),
    (
        "zs_trim_end",
        &["zs_ws"],
        "fn zs_trim_end(s: &str) -> &str {\n    let mut end = 0usize;\n    for (i, c) in s.char_indices() {\n        if !zs_ws(c) {\n            end = i + c.len_utf8();\n        }\n    }\n    &s[..end]\n}",
    ),
    (
        "zs_index_of16",
        &[],
        "/// JS's `indexOf`: the first match's index in UTF-16 code units, or -1\nfn zs_index_of16(a: &str, b: &str) -> i32 {\n    let (x, y) = (a.as_bytes(), b.as_bytes());\n    let mut i = 0usize;\n    while i + y.len() <= x.len() {\n        let mut j = 0usize;\n        while j < y.len() && x[i + j] == y[j] {\n            j += 1;\n        }\n        if j == y.len() {\n            let mut n = 0i32;\n            for c in a[..i].chars() {\n                n += if (c as u32) > 0xffff { 2 } else { 1 };\n            }\n            return n;\n        }\n        i += 1;\n    }\n    -1\n}",
    ),
    (
        "zs_map",
        &[],
        "/// JS's Map: entries in insertion order (a key set again keeps its place), found through a HashMap\n#[derive(Clone)]\nstruct ZsMap<K, V> {\n    ks: Vec<K>,\n    vs: Vec<V>,\n    idx: std::collections::HashMap<K, usize>,\n}\n\nimpl<K: PartialEq, V: PartialEq> PartialEq for ZsMap<K, V> {\n    fn eq(&self, o: &Self) -> bool {\n        self.ks == o.ks && self.vs == o.vs\n    }\n}\n\nimpl<K: Clone + Eq + std::hash::Hash, V: Clone> ZsMap<K, V> {\n    fn zs_new() -> Self {\n        ZsMap { ks: Vec::new(), vs: Vec::new(), idx: std::collections::HashMap::new() }\n    }\n    fn zs_insert(&mut self, k: K, v: V) {\n        match self.idx.get(&k) {\n            Some(&i) => {\n                self.vs[i] = v;\n            }\n            None => {\n                self.idx.insert(k.clone(), self.ks.len());\n                self.ks.push(k);\n                self.vs.push(v);\n            }\n        }\n    }\n    fn zs_remove(&mut self, k: &K) {\n        if let Some(i) = self.idx.remove(k) {\n            self.ks.remove(i);\n            self.vs.remove(i);\n            let mut j = i;\n            while j < self.ks.len() {\n                self.idx.insert(self.ks[j].clone(), j);\n                j += 1;\n            }\n        }\n    }\n    fn zs_find(&self, k: &K) -> Option<&V> {\n        match self.idx.get(k) {\n            Some(&i) => Some(&self.vs[i]),\n            None => None,\n        }\n    }\n}",
    ),
    (
        "zs_set",
        &[],
        "/// JS's Set: elements in insertion order, found through a HashMap\n#[derive(Clone)]\nstruct ZsSet<T> {\n    xs: Vec<T>,\n    idx: std::collections::HashMap<T, usize>,\n}\n\nimpl<T: PartialEq> PartialEq for ZsSet<T> {\n    fn eq(&self, o: &Self) -> bool {\n        self.xs == o.xs\n    }\n}\n\nimpl<T: Clone + Eq + std::hash::Hash> ZsSet<T> {\n    fn zs_new() -> Self {\n        ZsSet { xs: Vec::new(), idx: std::collections::HashMap::new() }\n    }\n    fn zs_insert(&mut self, x: T) {\n        if !self.idx.contains_key(&x) {\n            self.idx.insert(x.clone(), self.xs.len());\n            self.xs.push(x);\n        }\n    }\n    fn zs_remove(&mut self, x: &T) {\n        if let Some(i) = self.idx.remove(x) {\n            self.xs.remove(i);\n            let mut j = i;\n            while j < self.xs.len() {\n                self.idx.insert(self.xs[j].clone(), j);\n                j += 1;\n            }\n        }\n    }\n    fn zs_has(&self, x: &T) -> bool {\n        self.idx.contains_key(x)\n    }\n}",
    ),
    (
        "zs_to_f64",
        &[],
        "unsafe extern \"C\" {\n    fn zs_host_to_f64(p: *const u8, n: usize, out: *mut f64) -> i32;\n}\n\n/// A float read as the JS layer reads it (by the same function: the glue's): zustc's `parse::<f64>` needs `strtod`\nfn zs_to_f64(s: &str, out: &mut f64) -> bool {\n    unsafe { zs_host_to_f64(s.as_ptr(), s.len(), out as *mut f64) != 0 }\n}",
    ),
    (
        "zs_sort_by",
        &[],
        "/// A sort by a comparator that gives a negative, zero or positive i32, as JS's does (both are stable)\nfn zs_sort_by<T, F: Fn(&T, &T) -> i32>(v: &mut Vec<T>, f: F) {\n    v.sort_by(|a, b| f(a, b).cmp(&0));\n}",
    ),
    (
        "zs_position",
        &[],
        "/// The index of the first element `==` to `x` (by content), or -1\nfn zs_position<T: PartialEq>(xs: &[T], x: &T) -> i32 {\n    let mut i = 0usize;\n    while i < xs.len() {\n        if xs[i] == *x {\n            return i as i32;\n        }\n        i += 1;\n    }\n    -1\n}",
    ),
    (
        "zs_range",
        &[],
        "/// JS's `slice` bounds: a negative one counts from the end; both are clamped to the length\nfn zs_range(len: i32, a: i32, b: i32) -> (usize, usize) {\n    let f = |x: i32| if x < 0 { if len + x < 0 { 0 } else { len + x } } else if x > len { len } else { x };\n    let (a, b) = (f(a), f(b));\n    (a as usize, if b < a { a as usize } else { b as usize })\n}",
    ),
    (
        "zs_slice16",
        &["zs_range", "zs_len16"],
        "/// JS's `slice` of a string, in UTF-16 code units (half of a surrogate pair at either end is U+FFFD here)\nfn zs_slice16(s: &str, a: i32, b: i32) -> String {\n    let (a, b) = zs_range(zs_len16(s), a, b);\n    let mut out = String::new();\n    let mut k = 0usize;\n    for c in s.chars() {\n        let w = if (c as u32) > 0xffff { 2 } else { 1 };\n        if k >= a && k + w <= b {\n            out.push(c);\n        } else if k + w > a && k < b {\n            out.push('\\u{fffd}');\n        }\n        k += w;\n    }\n    out\n}",
    ),
];

/// A type as zustscript writes it (for messages)
fn show(t: &Ty) -> String {
    match t {
        Ty::Str => "String".into(),
        Ty::Jsx => "Jsx".into(),
        Ty::Array(e) => format!("Array<{}>", show(e)),
        Ty::Tuple(ts) => format!("({})", ts.iter().map(show).collect::<Vec<_>>().join(", ")),
        Ty::Fn(a, r) => format!("fn({}) -> {}", a.iter().map(show).collect::<Vec<_>>().join(", "), show(r)),
        Ty::Named(n, a) if a.is_empty() => n.clone(),
        Ty::Named(n, a) => format!("{n}<{}>", a.iter().map(show).collect::<Vec<_>>().join(", ")),
        Ty::Param(p) => p.clone(),
        t => scalar(t).unwrap_or("?").to_string(),
    }
}

type LResult<X> = Result<X, (Pos, String)>;

fn not_in_subset<X>(pos: Pos, what: &str) -> LResult<X> {
    Err((pos, format!("{what} is not supported by Rust lowering (numbers, bools, arrays, records and enums of them, let / let mut / if / match / loops, map / filter / fold)")))
}

/// Where the value of a lowered expression lives, for an expression whose type is not Copy (a Copy one is always
/// [`Form::Val`])
#[derive(Clone, Debug)]
enum Form {
    /// an owned temporary
    Val,
    /// a place in a local the function owns (`xs`, `p.pts`, `ps[i]`); `movable`: it can be moved from here (the
    /// local's last use, no index on the way); `root`: that local
    Own { movable: bool, root: String },
    /// reached through a borrow: a reference already (`is_ref`: `&[T]` / `&ZsX`), or a place behind one (`p.pts`
    /// where `p: &ZsP`). `roots`: the borrowed parameters it comes from (None: a local or a temporary, so it lives only
    /// as long as the statement); `param`: it is that parameter itself, whole
    Brw { is_ref: bool, roots: Option<BTreeSet<String>>, param: Option<String> },
}

/// What a local that is not Copy holds
#[derive(Clone, Debug)]
enum VarK {
    Owned,
    /// a reference (`&[T]` / `&ZsX`) to (a part of) `roots` (None: of a local); `param`: the parameter itself
    Borrow { roots: Option<BTreeSet<String>>, param: bool },
}

/// The inferred signatures (see the module comment): for each instance (Rust name), which parameters are owned, and
/// whether the result is a borrow. Missing entries are the optimistic start (borrowed, borrowed)
#[derive(Clone, Default, PartialEq, Debug)]
struct Modes {
    owned: HashSet<(String, usize)>,
    owned_result: HashSet<String>,
    /// for a borrowed result, the parameters (by index) it borrows from. Optimistic like the rest: missing (not
    /// lowered yet) is none, and it only grows (a result that comes to borrow from something else is owned)
    region: HashMap<String, BTreeSet<usize>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Rust,
    Wasm { scalar_simd: bool },
}

pub struct Lower<'a> {
    target: Target,
    info: &'a Info,
    fns: HashMap<&'a str, &'a Item>,
    subst: HashMap<String, Ty>,
    /// the non-Copy locals in scope
    vars: HashMap<String, VarK>,
    /// owned locals a live `let` borrows from (to the end of its block): never moved while frozen
    frozen: Vec<String>,
    /// the locals the rest of the block being lowered assigns (a borrow of one of them cannot be kept)
    later_assigns: HashSet<String>,
    live: Rc<Live>,
    lives: HashMap<String, Rc<Live>>,
    /// the inferred signatures so far (only ever made more owned)
    modes: Modes,
    /// the instances the one being lowered calls
    calls: HashSet<String>,
    /// [`own::is_copy`] of the types met (it is asked for at every expression)
    copies: std::cell::RefCell<HashMap<Ty, bool>>,
    /// the instance being lowered, its parameters, and the parameters its borrowed result borrows from
    cur: String,
    params: Vec<String>,
    region: BTreeSet<String>,
    queue: Vec<(String, Vec<Ty>)>,
    done: HashSet<String>,
    /// the records and enums the lowered code uses (their Rust definitions are written once, generic as declared)
    used: BTreeSet<String>,
    /// where each record and enum is declared (for errors in their definitions)
    type_pos: HashMap<String, Pos>,
    /// the helper functions used (`zs_max_f32` ..)
    helpers: BTreeSet<String>,
    /// the result type of the function being lowered (None inside a closure: a `return` there is not lowered)
    ret: Option<Ty>,
    /// `m = m.insert(k, ..)` ([`own::assigned_change`]): this `insert` / `remove` takes `m` itself, after its arguments
    move_recv: Option<Id>,
    /// the string helpers used ([`STR_HELPERS`])
    str_helpers: BTreeSet<&'static str>,
}

/// The Rust name of a record or enum: `ZsPoint`
fn rs_type(n: &str) -> String {
    format!("Zs{n}")
}

/// Emit a Rust module with native signatures and any embedded Rust definitions.
pub fn rust_module(items: &[Item], info: &Info) -> Result<String, Vec<(Pos, String)>> {
    let (_, mut rust) = lower(items, info, &[], Target::Rust)?;
    for item in items {
        match item {
            Item::WasmFn { text, public, .. } => {
                if *public {
                    rust.push_str("pub ");
                }
                rust.push_str(text);
            }
            Item::WasmRaw(text) => rust.push_str(text),
            _ => continue,
        }
        rust.push_str("\n\n");
    }
    Ok(rust)
}

/// Only `wanted` entry points need JS boundary adapters. Standalone Rust modules use an empty list.
pub fn lower(items: &[Item], info: &Info, wanted: &[(String, Vec<Ty>)], target: Target) -> Result<(Vec<WFn>, String), Vec<(Pos, String)>> {
    let fns: HashMap<&str, &Item> = items
        .iter()
        .filter_map(|it| match it {
            Item::Fn { name, simd, .. } if !*simd || !matches!(target, Target::Wasm { scalar_simd: false }) => Some((name.as_str(), it)),
            _ => None,
        })
        .collect();
    let mut type_pos = HashMap::new();
    for it in items {
        if let Item::Record { name, pos, .. } | Item::Enum { name, pos, .. } = it {
            type_pos.insert(name.clone(), *pos);
        }
    }
    let mut l = Lower {
        target,
        info,
        fns,
        subst: HashMap::new(),
        vars: HashMap::new(),
        frozen: vec![],
        later_assigns: HashSet::new(),
        live: Rc::default(),
        lives: HashMap::new(),
        modes: Modes::default(),
        calls: HashSet::new(),
        copies: Default::default(),
        cur: String::new(),
        params: vec![],
        region: BTreeSet::new(),
        queue: vec![],
        done: HashSet::new(),
        used: Default::default(),
        type_pos,
        helpers: Default::default(),
        ret: None,
        move_recv: None,
        str_helpers: Default::default(),
    };
    // Even a private @wasm definition that has no callers must be valid Rust.
    // Only instances reached from JS need a boundary adapter.
    for item in items {
        if let Item::Fn { name, generics, bounds, wasm, .. } = item
            && (target == Target::Rust || *wasm)
        {
            for types in crate::js::all_instances(generics, bounds) {
                l.want(name, types);
            }
        }
    }
    l.all(wanted)
}

impl Lower<'_> {
    /// The fixed point of the signatures, by a worklist: an instance is lowered again when its own signature changed
    /// (what it found while being lowered) or a callee's did. Each change makes a parameter or a result owned, or
    /// grows a result's region, and is never undone, so this ends; an instance is lowered at most (1 + the changes of
    /// itself and its callees) times, and fewer as the changes are batched (see `again`)
    fn all(&mut self, wanted: &[(String, Vec<Ty>)]) -> Result<(Vec<WFn>, String), Vec<(Pos, String)>> {
        for (n, t) in wanted {
            self.want(n, t.clone());
        }
        let mut order = vec![];
        let mut results: HashMap<String, (String, Vec<Ty>, LResult<(String, Vec<(String, Ty)>, Ty)>)> = HashMap::new();
        let mut callers: HashMap<String, HashSet<String>> = HashMap::new();
        // (instances never lowered come first; the ones to lower again wait until there are none, so a caller of
        // many callees that each change is lowered again once for all of them, not once for each)
        let mut fresh: Vec<(String, Vec<Ty>)> = std::mem::take(&mut self.queue);
        let mut again: Vec<(String, Vec<Ty>)> = vec![];
        let mut queued: HashSet<String> = fresh.iter().map(|(n, t)| rs_name(n, t)).collect();
        while let Some((name, targs)) = fresh.pop().or_else(|| again.pop()) {
            let rs = rs_name(&name, &targs);
            queued.remove(&rs);
            if !results.contains_key(&rs) {
                order.push(rs.clone());
            }
            let before = self.sig_of(&name, &rs);
            self.calls.clear();
            let r = self.function(&name, &targs);
            for c in std::mem::take(&mut self.calls) {
                callers.entry(c).or_default().insert(rs.clone());
            }
            results.insert(rs.clone(), (name.clone(), targs.clone(), r));
            for (n, t) in std::mem::take(&mut self.queue) {
                if queued.insert(rs_name(&n, &t)) {
                    fresh.push((n, t));
                }
            }
            if self.sig_of(&name, &rs) != before {
                for c in std::iter::once(&rs).chain(callers.get(&rs).into_iter().flatten()) {
                    if queued.insert(c.clone()) {
                        let (n, t, _) = &results[c];
                        again.push((n.clone(), t.clone()));
                    }
                }
            }
        }
        let wanted_names: HashSet<String> = wanted.iter().map(|(n, t)| rs_name(n, t)).collect();
        let mut errors = vec![];
        let mut out = vec![];
        let mut internal = String::new();
        for rs in &order {
            let (name, targs, r) = results.remove(rs).unwrap();
            match r {
                Ok((text, args, ret)) => {
                    if wanted_names.contains(rs) {
                        let Item::Fn { public, pos, .. } = self.fns[name.as_str()] else { unreachable!() };
                        match self.boundary(&args, &ret, *pos) {
                            Ok((wargs, wret)) => {
                                // (the glue passes arrays as slices and takes an owned result: an instance whose
                                // inferred signature differs gets an adapter)
                                let (text, entry) = self.adapter(rs, text, &args, &ret)?;
                                out.push(WFn { js: inst_name(&name, &targs), rs: entry, args: wargs, ret: wret, text, public: *public })
                            }
                            Err(e) => errors.push(e),
                        }
                    } else {
                        internal.push_str(&text);
                        internal.push_str("\n\n");
                    }
                }
                Err(e) => errors.push(e),
            }
        }
        // the records and enums used (a definition can use more of them: to a fixed point)
        let mut written = HashSet::new();
        let mut defs = String::new();
        loop {
            let todo: Vec<String> = self.used.iter().filter(|n| !written.contains(*n)).cloned().collect();
            if todo.is_empty() {
                break;
            }
            for n in todo {
                written.insert(n.clone());
                match self.type_def(&n) {
                    Ok(d) => defs.push_str(&d),
                    Err(e) => errors.push(e),
                }
            }
        }
        for h in &self.helpers {
            // zs_max_f32 / zs_min_f64 ..
            let (op, t) = h.trim_start_matches("zs_").split_once('_').unwrap();
            let (pick, zero) = if op == "max" { ("a > b", "a.is_sign_positive()") } else { ("a < b", "a.is_sign_negative()") };
            let _ = write!(
                defs,
                "fn {h}(a: {t}, b: {t}) -> {t} {{\n    if a.is_nan() || b.is_nan() {{ {t}::NAN }} else if a == b {{ if {zero} {{ a }} else {{ b }} }} else if {pick} {{ a }} else {{ b }}\n}}\n\n"
            );
        }
        // (with the helpers they call)
        let all = || STR_HELPERS.iter().chain(STD_STR_HELPERS);
        let mut need = self.str_helpers.clone();
        loop {
            let more: Vec<&'static str> = all().filter(|h| need.contains(h.0)).flat_map(|h| h.1.iter().copied()).filter(|d| !need.contains(*d)).collect();
            if more.is_empty() {
                break;
            }
            need.extend(more);
        }
        for (name, _, text) in all() {
            if need.contains(name) {
                defs.push_str(text);
                defs.push_str("\n\n");
            }
        }
        if errors.is_empty() { Ok((out, defs + &internal)) } else { Err(errors) }
    }

    /// An instance's inferred signature (its owned parameters, whether its result is owned)
    fn sig_of(&self, name: &str, rs: &str) -> (usize, bool, Option<BTreeSet<usize>>) {
        let k = (0..self.info.fns.get(name).map_or(0, |s| s.args.len())).filter(|i| self.modes.owned.contains(&(rs.to_string(), *i))).count();
        (k, self.modes.owned_result.contains(rs), self.modes.region.get(rs).cloned())
    }

    /// For an instance the glue calls: (its text, the name the glue calls). The glue passes an array as a slice and
    /// takes an owned result; if the inferred signature owns an array parameter or borrows the result, a small
    /// adapter in front converts
    fn adapter(&mut self, rs: &str, text: String, args: &[(String, Ty)], ret: &Ty) -> Result<(String, String), Vec<(Pos, String)>> {
        let owned: Vec<bool> = (0..args.len()).map(|i| self.modes.owned.contains(&(rs.to_string(), i))).collect();
        let ret_ref = !self.copy(ret) && !self.modes.owned_result.contains(rs);
        // (the glue hands a value that is not Copy by reference: one the instance owns is cloned here)
        if !owned.iter().zip(args).any(|(o, (_, t))| *o && !self.copy(t)) && !ret_ref {
            return Ok((text, rs.to_string()));
        }
        let pos = Pos::default();
        let mut ps = vec![];
        let mut call = vec![];
        for ((n, t), o) in args.iter().zip(&owned) {
            ps.push(format!("{}: {}", local(n), self.rty(t, pos, Some(None)).map_err(|e| vec![e])?));
            call.push(if *o && !self.copy(t) { self.clone_of(&local(n), t) } else { local(n) });
        }
        let rt = self.rty(ret, pos, None).map_err(|e| vec![e])?;
        let mut body = format!("{rs}({})", call.join(", "));
        if ret_ref {
            body = self.clone_of(&body, ret);
        }
        // (named in the generated `__zs` namespace: a source function's Rust name is `zs_..`, so none can meet it)
        let entry = format!("__zs_js_{rs}");
        Ok((format!("{text}\n\nfn {entry}({}) -> {rt} {{\n    {body}\n}}", ps.join(", ")), entry))
    }

    /// The Rust definition of record or enum `n` (Copy, and passed by value like a number, when all its fields are)
    fn type_def(&mut self, n: &str) -> LResult<String> {
        let pos = self.type_pos.get(n).copied().unwrap_or_default();
        let generic = Ty::Named(n.to_string(), {
            let ps = self.info.records.get(n).map(|r| &r.params).unwrap_or_else(|| &self.info.enums[n].params);
            ps.iter().map(|p| Ty::Param(p.clone())).collect()
        });
        let vis = if self.target == Target::Rust { "pub " } else { "" };
        // (an enum without data can be a Map's key or a Set's element: Eq and Hash too)
        let key = self.info.enums.get(n).is_some_and(|e| e.unit_only());
        let mut s = String::from(if key {
            "#[derive(Clone, Copy, PartialEq, Eq, Hash)]\n"
        } else if own::is_copy(&generic, self.info) {
            "#[derive(Clone, Copy, PartialEq)]\n"
        } else {
            "#[derive(Clone, PartialEq)]\n"
        });
        if let Some(r) = self.info.records.get(n) {
            let g = if r.params.is_empty() { String::new() } else { format!("<{}>", r.params.join(", ")) };
            let _ = writeln!(s, "{vis}struct {}{g} {{", rs_type(n));
            for (f, t) in r.fields.clone() {
                let _ = writeln!(s, "    {vis}{}: {},", local(&f), self.field_ty(&t, pos)?);
            }
            s.push_str("}\n\n");
        } else {
            let e = &self.info.enums[n];
            let g = if e.params.is_empty() { String::new() } else { format!("<{}>", e.params.join(", ")) };
            let _ = writeln!(s, "{vis}enum {}{g} {{", rs_type(n));
            for (v, fs) in e.variants.clone() {
                if fs.is_empty() {
                    let _ = writeln!(s, "    {v},");
                } else {
                    let ts = fs.iter().map(|t| self.field_ty(t, pos)).collect::<LResult<Vec<_>>>()?;
                    let _ = writeln!(s, "    {v}({}),", ts.join(", "));
                }
            }
            s.push_str("}\n\n");
        }
        Ok(s)
    }

    /// A field's type: owned (`Vec<T>` for an array)
    fn field_ty(&mut self, t: &Ty, pos: Pos) -> LResult<String> {
        match t {
            Ty::Param(p) => Ok(p.clone()),
            t => self.rty(t, pos, None),
        }
    }

    /// The shape a record or enum crosses the boundary in
    fn vty(&mut self, t: &Ty, pos: Pos) -> LResult<VTy> {
        let sc = |w: WTy| Ok(VTy::Scalar(w));
        match t {
            Ty::I32 => sc(WTy::I32),
            Ty::U32 => sc(WTy::U32),
            Ty::I64 => sc(WTy::I64),
            Ty::F32 => sc(WTy::F32),
            Ty::F64 => sc(WTy::F64),
            Ty::Bool => sc(WTy::Bool),
            Ty::Named(n, targs) => {
                let rt = self.rty(t, pos, None)?;
                if let Some(r) = self.info.records.get(n) {
                    let m: HashMap<String, Ty> = r.params.iter().cloned().zip(targs.iter().cloned()).collect();
                    let fields: Vec<(String, Ty)> = r.fields.iter().map(|(f, ft)| (f.clone(), subst_ty(ft, &m))).collect();
                    let fs = fields.iter().map(|(f, ft)| Ok((f.clone(), self.vty(ft, pos)?))).collect::<LResult<Vec<_>>>()?;
                    Ok(VTy::Record(rt, fs))
                } else {
                    let e = &self.info.enums[n];
                    // (a union read from TS has TS's shape in JS, not the `{ TAG, _0 }` the glue reads)
                    if e.ts.is_some() {
                        return Err((pos, format!("{n} is a union read from TS: it does not cross to wasm yet (use it in the JS layer)")));
                    }
                    let m: HashMap<String, Ty> = e.params.iter().cloned().zip(targs.iter().cloned()).collect();
                    let string_enum = e.unit_only() && !e.is_effect();
                    let vs: Vec<(String, Vec<Ty>)> = e.variants.iter().map(|(v, fs)| (v.clone(), fs.iter().map(|t| subst_ty(t, &m)).collect())).collect();
                    let vs = vs
                        .iter()
                        .map(|(v, fs)| Ok((v.clone(), fs.iter().map(|t| self.vty(t, pos)).collect::<LResult<Vec<_>>>()?)))
                        .collect::<LResult<Vec<_>>>()?;
                    Ok(VTy::Enum(rt, string_enum, vs))
                }
            }
            _ => Err((pos, format!("{} cannot be inside a record or enum that crosses to wasm (keep it inside the wasm side)", show(t)))),
        }
    }

    /// The schema a value crosses in as bytes: numbers, bools, strings, arrays and records of them (the stable part
    /// of the format; enums, typed arrays and functions do not cross this way yet)
    fn ws(&mut self, t: &Ty, pos: Pos) -> LResult<WS> {
        self.ws_in(t, pos, &mut vec![])
    }

    /// (`open`: the records being expanded: one that holds itself, through an array, has no fixed schema)
    fn ws_in(&mut self, t: &Ty, pos: Pos, open: &mut Vec<Ty>) -> LResult<WS> {
        Ok(match t {
            Ty::I32 => WS::Scalar(WTy::I32),
            Ty::U32 => WS::Scalar(WTy::U32),
            Ty::I64 => WS::Scalar(WTy::I64),
            Ty::F32 => WS::Scalar(WTy::F32),
            Ty::F64 => WS::Scalar(WTy::F64),
            Ty::Bool => WS::Scalar(WTy::Bool),
            Ty::Str => WS::Str,
            Ty::Array(e) => WS::Arr(Box::new(self.ws_in(e, pos, open)?)),
            // (or a type whose arguments grow on each expansion, `Grow<T> { More(Array<Grow<Array<T>>>) }`: finite
            // nesting like `Wrap<Wrap<i32>>` stays well under the bound)
            Ty::Named(n, _) if open.contains(t) || open.iter().filter(|o| matches!(o, Ty::Named(m, _) if m == n)).count() >= 16 => {
                return Err((pos, format!("{n} holds itself: a recursive record or enum cannot cross from JS to a @wasm function yet")));
            }
            Ty::Named(n, targs) if self.info.records.contains_key(n) => {
                let r = &self.info.records[n];
                let m: HashMap<String, Ty> = r.params.iter().cloned().zip(targs.iter().cloned()).collect();
                let fields: Vec<(String, Ty)> = r.fields.iter().map(|(f, ft)| (f.clone(), subst_ty(ft, &m))).collect();
                let rt = self.rty(t, pos, None)?;
                open.push(t.clone());
                let fs = fields.iter().map(|(f, ft)| Ok((f.clone(), self.ws_in(ft, pos, open)?))).collect::<LResult<Vec<_>>>();
                open.pop();
                WS::Rec(rt, fs?)
            }
            Ty::Named(n, targs) if self.info.enums.get(n).is_some_and(|e| !e.is_effect()) => {
                let e = &self.info.enums[n];
                let m: HashMap<String, Ty> = e.params.iter().cloned().zip(targs.iter().cloned()).collect();
                let vs: Vec<(String, Vec<Ty>)> = e.variants.iter().map(|(v, fs)| (v.clone(), fs.iter().map(|t| subst_ty(t, &m)).collect())).collect();
                let ts = e.ts.clone();
                // (the JS shape: TS's own for a union read from TS, else zustscript's `{ TAG, _0 }` or a string)
                let string = e.unit_only() && ts.as_ref().is_none_or(|t| t.key.is_none());
                let key = if string { None } else { Some(ts.as_ref().and_then(|t| t.key.clone()).unwrap_or_else(|| "TAG".into())) };
                let rt = self.rty(t, pos, None)?;
                open.push(t.clone());
                let mut variants = vec![];
                let mut err = None;
                for (vi, (v, fs)) in vs.iter().enumerate() {
                    let tag = ts.as_ref().map_or_else(|| v.clone(), |t| t.tags[vi].clone());
                    let mut fields = vec![];
                    for (k, ft) in fs.iter().enumerate() {
                        let name = ts.as_ref().map_or_else(|| format!("_{k}"), |t| t.fields[vi][k].clone());
                        match self.ws_in(ft, pos, open) {
                            Ok(w) => fields.push((name, w)),
                            Err(x) => err = err.or(Some(x)),
                        }
                    }
                    variants.push((v.clone(), tag, fields));
                }
                open.pop();
                if let Some(x) = err {
                    return Err(x);
                }
                WS::Enum { rt, key, variants }
            }
            _ => {
                let what = if let Ty::Typed(_) = t { "a typed array".to_string() } else { show(t) };
                return Err((pos, format!("{what} cannot cross from JS to a @wasm function inside a record, an enum or an array yet (numbers, bools, strings, arrays, records and enums of them)")));
            }
        })
    }

    /// The boundary types of a lowered function's signature (what its glue converts)
    fn boundary(&mut self, args: &[(String, Ty)], ret: &Ty, pos: Pos) -> LResult<(Vec<(String, WTy)>, WTy)> {
        let mut a = vec![];
        for (n, t) in args {
            a.push((n.clone(), self.wty(t, true, pos)?));
        }
        Ok((a, self.wty(ret, false, pos)?))
    }

    fn wty(&mut self, t: &Ty, arg: bool, pos: Pos) -> LResult<WTy> {
        // (a record or enum of numbers: as its scalar slots; one holding a string or an array, a union read from TS,
        // and an array of anything but numbers: as bytes, through the helpers)
        if let Ty::Named(..) = t {
            let by_ref = !self.copy(t);
            if !by_ref {
                if let Ok(v) = self.vty(t, pos) {
                    return Ok(WTy::Value(Box::new(v)));
                }
            }
            return Ok(WTy::Wire(Box::new(self.ws(t, pos)?), self.rty(t, pos, None)?, by_ref));
        }
        if let Ty::Array(e) = t {
            if scalar(e).and_then(Elem::parse).is_none() {
                return Ok(WTy::Wire(Box::new(self.ws(t, pos)?), self.rty(t, pos, None)?, true));
            }
        }
        Ok(match t {
            Ty::I32 => WTy::I32,
            Ty::U32 => WTy::U32,
            Ty::I64 => WTy::I64,
            Ty::F32 => WTy::F32,
            Ty::F64 => WTy::F64,
            Ty::Typed(e) if scalar(e).and_then(Elem::parse).is_some() => {
                let el = scalar(e).and_then(Elem::parse).unwrap();
                if arg { WTy::Slice(el) } else { WTy::TVec(el) }
            }
            Ty::Bool => WTy::Bool,
            Ty::Unit => WTy::Unit,
            Ty::Str => {
                if arg { WTy::Str } else { WTy::String }
            }
            Ty::Array(e) if scalar(e).and_then(Elem::parse).is_some() => {
                let el = scalar(e).and_then(Elem::parse).unwrap();
                if arg { WTy::Slice(el) } else { WTy::Vec(el) }
            }
            _ => return Err((pos, format!("{} cannot cross from JS to a @wasm function yet (numbers, bools, arrays of numbers, records, enums)", show(t)))),
        })
    }
}

impl<'a> Lower<'a> {
    fn want(&mut self, name: &str, targs: Vec<Ty>) -> String {
        let rs = rs_name(name, &targs);
        if self.done.insert(rs.clone()) {
            self.queue.push((name.to_string(), targs));
        }
        rs
    }

    fn copy(&self, t: &Ty) -> bool {
        match t {
            Ty::Array(_) | Ty::Typed(_) | Ty::Str => false,
            Ty::Named(..) | Ty::Tuple(_) => {
                if let Some(c) = self.copies.borrow().get(t) {
                    return *c;
                }
                let c = own::is_copy(t, self.info);
                self.copies.borrow_mut().insert(t.clone(), c);
                c
            }
            _ => true,
        }
    }

    /// The type of expression `id` (in this instance)
    fn ty(&self, id: Id) -> Ty {
        subst_ty(self.info.ty.get(&id).unwrap_or(&Ty::Unit), &self.subst)
    }

    /// A type in Rust. `borrow`: None for an owned value; Some(lifetime) for a borrow of a type that is not Copy
    /// (`&[T]`, `&'a ZsX`; a Copy type is still by value)
    fn rty(&mut self, t: &Ty, pos: Pos, borrow: Option<Option<&str>>) -> LResult<String> {
        if let Some(s) = scalar(t) {
            return Ok(s.into());
        }
        let r = |lt: Option<&str>| lt.map_or("&".to_string(), |l| format!("&'{l} "));
        match t {
            // (in a generic record's or enum's definition)
            Ty::Param(p) => Ok(p.clone()),
            Ty::Named(n, targs) if self.info.records.contains_key(n) || self.info.enums.get(n).is_some_and(|e| !e.is_effect()) => {
                self.used.insert(n.clone());
                let base = if targs.is_empty() {
                    rs_type(n)
                } else {
                    let a = targs.iter().map(|t| self.rty(t, pos, None)).collect::<LResult<Vec<_>>>()?;
                    format!("{}<{}>", rs_type(n), a.join(", "))
                };
                Ok(match borrow {
                    Some(lt) if !self.copy(t) => format!("{}{base}", r(lt)),
                    _ => base,
                })
            }
            // (the built-in collections: in insertion order, as JS's Map / Set, with a HashMap index)
            Ty::Named(n, targs) if n == "Map" || n == "Set" => {
                self.str_helpers.insert(if n == "Map" { "zs_map" } else { "zs_set" });
                let a = targs.iter().map(|t| self.rty(t, pos, None)).collect::<LResult<Vec<_>>>()?;
                let base = format!("{}<{}>", rs_type(n), a.join(", "));
                Ok(match borrow {
                    Some(lt) => format!("{}{base}", r(lt)),
                    None => base,
                })
            }
            Ty::Array(e) | Ty::Typed(e) => {
                let el = match scalar(e) {
                    Some("()") => return not_in_subset(pos, "an array of ()"),
                    Some(s) => s.to_string(),
                    None => self.rty(e, pos, None)?,
                };
                Ok(match borrow {
                    Some(lt) => format!("{}[{el}]", r(lt)),
                    None => format!("Vec<{el}>"),
                })
            }
            Ty::Str => Ok(match borrow {
                Some(lt) => format!("{}str", r(lt)),
                None => "String".into(),
            }),
            _ => not_in_subset(pos, &format!("the type {}", show(t))),
        }
    }

    /// One instance: (its Rust text, its parameters' and result's types)
    fn function(&mut self, name: &str, targs: &[Ty]) -> LResult<(String, Vec<(String, Ty)>, Ty)> {
        let Item::Fn { args, body, pos, generics, bounds, public, .. } = self.fns[name] else { unreachable!() };
        if !self.info.effects[name].is_empty() {
            return Err((*pos, format!("a function lowered to Rust cannot perform effects ({:?})", self.info.effects[name])));
        }
        for g in generics {
            if !bounds.iter().any(|b| b.0 == *g) {
                return Err((*pos, format!("the generic parameter `{g}` of a function lowered to Rust needs a numeric bound (`{g}: Num`)")));
            }
        }
        self.subst = generics.iter().cloned().zip(targs.iter().cloned()).collect();
        let rs = rs_name(name, targs);
        let sig = &self.info.fns[name];
        let ats: Vec<Ty> = sig.args.iter().map(|t| subst_ty(t, &self.subst)).collect();
        let ret = subst_ty(&sig.ret, &self.subst);
        let params: Vec<String> = args.iter().map(|a| a.0.clone()).collect();
        if !self.lives.contains_key(name) {
            let l = own::liveness(&params, body, &|id| self.copy(&self.ty(id)));
            self.lives.insert(name.to_string(), Rc::new(l));
        }
        self.live = self.lives[name].clone();
        self.cur = rs.clone();
        self.params = params;
        self.vars.clear();
        self.frozen.clear();
        self.region.clear();
        let mut owned = vec![];
        for (i, ((a, _), t)) in args.iter().zip(&ats).enumerate() {
            let o = self.modes.owned.contains(&(rs.clone(), i));
            owned.push(o);
            if !self.copy(t) {
                let k = if o { VarK::Owned } else { VarK::Borrow { roots: Some(BTreeSet::from([a.clone()])), param: true } };
                self.vars.insert(a.clone(), k);
            }
        }
        // (a `?` returns its `Err` / `None`, which is owned: so is every result then)
        if !self.copy(&ret) && crate::js::has_try(body) {
            self.modes.owned_result.insert(rs.clone());
        }
        let ret_ref = !self.copy(&ret) && !self.modes.owned_result.contains(&rs);
        self.ret = Some(ret.clone());
        let b = if self.copy(&ret) {
            self.expr(body)?
        } else if ret_ref {
            let (s, f) = self.ex(body)?;
            match self.borrow_of(&s, &f, &ret) {
                // (empty roots: only while a callee's region is still being found; it grows, and this is redone)
                (s, Some(roots)) if !roots.iter().any(|r| r.starts_with('@')) => {
                    // (which parameters the result borrows from: callers count only those arguments)
                    let idx = self.params.iter().enumerate().filter(|(_, p)| roots.contains(*p)).map(|(i, _)| i).collect();
                    self.modes.region.insert(rs.clone(), idx);
                    self.region = roots;
                    s
                }
                _ => {
                    // (not a borrow of the parameters: from now on it returns owned)
                    self.modes.owned_result.insert(rs.clone());
                    self.owned(s, &f, &ret)
                }
            }
        } else {
            let (s, f) = self.ex(body)?;
            self.owned(s, &f, &ret)
        };
        let b = format!("{{\n{b}\n}}");
        let lt = if ret_ref { Some("a") } else { None };
        let mut ps = vec![];
        for (((a, _), t), o) in args.iter().zip(&ats).zip(&owned) {
            let borrow = if *o { None } else { Some(if self.region.contains(a) { lt } else { None }) };
            ps.push(format!("{}: {}", local(a), self.rty(t, *pos, borrow)?));
        }
        let rt = self.rty(&ret, *pos, if lt.is_some() { Some(lt) } else { None })?;
        let g = if lt.is_some() { "<'a>" } else { "" };
        let vis = if self.target == Target::Rust && *public { "pub " } else { "" };
        let text = format!("{vis}fn {rs}{g}({}) -> {rt} {b}", ps.join(", "));
        let named = args.iter().map(|a| a.0.clone()).zip(ats).collect();
        Ok((text, named, ret))
    }

    // ------------------------------------------------------------ forms

    /// An owned value of expression `s` (form `f`, type `t`): moved where it can be, cloned otherwise
    fn owned(&mut self, s: String, f: &Form, t: &Ty) -> String {
        if self.copy(t) {
            return s;
        }
        match f {
            Form::Val | Form::Own { movable: true, .. } => s,
            Form::Own { movable: false, .. } => self.clone_of(&s, t),
            Form::Brw { param, .. } => {
                // (a borrowed parameter consumed whole: from now on it is taken owned)
                if let Some(p) = param
                    && let Some(i) = self.params.iter().position(|x| x == p)
                {
                    self.modes.owned.insert((self.cur.clone(), i));
                }
                self.clone_of(&s, t)
            }
        }
    }

    fn clone_of(&self, s: &str, t: &Ty) -> String {
        // (a block, `if`, `match` or reference takes the method only inside parentheses)
        let s = if ["if ", "match ", "{", "&", "*"].iter().any(|p| s.starts_with(p)) { format!("({s})") } else { s.to_string() };
        if is_array(t) {
            format!("{s}.to_vec()")
        } else if *t == Ty::Str {
            format!("{s}.to_string()")
        } else {
            format!("{s}.clone()")
        }
    }

    /// A borrow of expression `s` (`&[T]` / `&ZsX`), and the parameters it borrows from (None: a local or temporary)
    fn borrow_of(&self, s: &str, f: &Form, t: &Ty) -> (String, Option<BTreeSet<String>>) {
        let tail = if sliced(t) { "[..]" } else { "" };
        match f {
            Form::Val => (format!("&({s}){tail}"), None),
            // (a borrow of a local: `@name` among the roots, so it can be kept past the statement only while that
            // local is frozen, and never returned)
            Form::Own { root, .. } => (format!("&{s}{tail}"), Some(BTreeSet::from([format!("@{root}")]))),
            Form::Brw { is_ref: true, roots, .. } => (s.to_string(), roots.clone()),
            Form::Brw { is_ref: false, roots, .. } => (format!("&{s}{tail}"), roots.clone()),
        }
    }

    /// An owned value (the default for an operand)
    fn expr(&mut self, e: &Expr) -> LResult<String> {
        let (s, f) = self.ex(e)?;
        let t = self.ty(e.id);
        Ok(self.owned(s, &f, &t))
    }

    /// A borrowed value, for a parameter that borrows (a Copy value is passed as is)
    fn borrowed(&mut self, e: &Expr) -> LResult<(String, Option<BTreeSet<String>>)> {
        let (s, f) = self.ex(e)?;
        let t = self.ty(e.id);
        if self.copy(&t) {
            return Ok((s, Some(BTreeSet::new())));
        }
        Ok(self.borrow_of(&s, &f, &t))
    }

    /// The form of a place `x.f` / `x[i]` reached from `x` (form `f`)
    fn project(f: Form, index: bool) -> Form {
        match f {
            Form::Val if index => Form::Brw { is_ref: false, roots: None, param: None },
            Form::Val => Form::Val,
            Form::Own { movable, root } => Form::Own { movable: movable && !index, root },
            Form::Brw { roots, .. } => Form::Brw { is_ref: false, roots, param: None },
        }
    }

    /// The branches of an `if` / `match` of a type that is not Copy: all borrows of parameters stay a borrow;
    /// otherwise each becomes owned
    fn join(&mut self, arms: Vec<(String, Form)>, t: &Ty) -> (Vec<String>, Form) {
        let mut roots = BTreeSet::new();
        let mut all = true;
        for (s, f) in &arms {
            match self.borrow_of(s, f, t) {
                (_, Some(r)) if matches!(f, Form::Brw { .. }) => roots.extend(r),
                _ => all = false,
            }
        }
        if all && !arms.is_empty() {
            let ss = arms.iter().map(|(s, f)| self.borrow_of(s, f, t).0).collect();
            (ss, Form::Brw { is_ref: true, roots: Some(roots), param: None })
        } else {
            let ss = arms.into_iter().map(|(s, f)| self.owned(s, &f, t)).collect();
            (ss, Form::Val)
        }
    }

    fn scoped<X>(&mut self, f: impl FnOnce(&mut Self) -> X) -> X {
        let saved = self.vars.clone();
        let r = f(self);
        self.vars = saved;
        r
    }

    /// A local bound to a reference that is not of the canonical type (`ref xs` of an array field is `&Vec<T>`):
    /// rebound as `&[T]`, so every borrow of an array is a slice
    fn canon(&mut self, n: &str, t: &Ty) -> String {
        if is_array(t) {
            format!("let {0}: &[{1}] = {0};\n", local(n), self.elem_rty(t))
        } else if *t == Ty::Str {
            format!("let {0}: &str = {0};\n", local(n))
        } else {
            String::new()
        }
    }

    fn elem_rty(&mut self, t: &Ty) -> String {
        let (Ty::Array(e) | Ty::Typed(e)) = t else { return "()".into() };
        self.rty(e, Pos::default(), None).unwrap_or_default()
    }

    // ------------------------------------------------------------ statements and expressions

    fn stmt(&mut self, st: &Stmt) -> LResult<String> {
        match st {
            Stmt::Let(Pat::Bind(n), _, x) => {
                let t = self.ty(x.id);
                if self.copy(&t) {
                    // (the initializer still sees the binding this one shadows)
                    let s = self.expr(x)?;
                    self.vars.remove(n);
                    return Ok(format!("let {} = {s};\n", local(n)));
                }
                let (s, f) = self.ex(x)?;
                let (s, k) = match &f {
                    Form::Val | Form::Own { movable: true, .. } => (s, VarK::Owned),
                    // (a part of the borrowed parameters: borrowed for as long as the function runs)
                    // (a part of the borrowed parameters, or of owned locals the rest of the block does not assign:
                    // borrowed while it lives, and those locals are frozen, never moved, to the end of the block)
                    Form::Brw { roots: Some(r), .. } if !r.iter().any(|x| x.strip_prefix('@').is_some_and(|n| self.later_assigns.contains(n))) => {
                        for x in r.iter().filter_map(|x| x.strip_prefix('@')) {
                            self.frozen.push(x.to_string());
                        }
                        (self.borrow_of(&s, &f, &t).0, VarK::Borrow { roots: Some(r.clone()), param: false })
                    }
                    _ => (self.owned(s, &f, &t), VarK::Owned),
                };
                self.vars.insert(n.clone(), k);
                Ok(format!("let {} = {s};\n", local(n)))
            }
            Stmt::Let(Pat::Wild, _, x) => {
                let (s, f) = self.ex(x)?;
                // (a place is only evaluated: `let _ = &xs[i];` checks the index without moving)
                Ok(if matches!(f, Form::Val) { format!("let _ = {s};\n") } else { format!("let _ = &{s};\n") })
            }
            Stmt::LetMut(n, _, x) => {
                let t = self.ty(x.id);
                let s = self.expr(x)?;
                if self.copy(&t) {
                    self.vars.remove(n);
                } else {
                    self.vars.insert(n.clone(), VarK::Owned);
                }
                Ok(format!("let mut {}: {} = {s};\n", local(n), self.rty(&t, x.pos, None)?))
            }
            Stmt::Let(_, _, x) => not_in_subset(x.pos, "a destructuring `let`"),
            Stmt::Expr(x) => {
                let (s, f) = self.ex(x)?;
                Ok(if matches!(f, Form::Val) { format!("{s};\n") } else { format!("let _ = &{s};\n") })
            }
        }
    }

    fn num(&self, id: Id) -> Ty {
        subst_ty(&self.info.num.get(&id).cloned().unwrap_or(Ty::F64), &self.subst)
    }

    fn lit(&self, l: &Lit, id: Id, pos: Pos) -> LResult<String> {
        Ok(match l {
            Lit::Int(n) => match scalar(&self.num(id)) {
                Some(s) => format!("{n}{s}"),
                None => format!("{n}"),
            },
            Lit::Float(f) => format!("{f}{}", if self.num(id) == Ty::F32 { "f32" } else { "f64" }),
            Lit::Bool(b) => b.to_string(),
            Lit::Unit => "()".into(),
            Lit::Str(_) => return not_in_subset(pos, "a string"),
        })
    }

    /// A closure given to an iterator method: its parameters bound as `kinds` say (None: Copy or owned; Some(roots):
    /// a reference into the iterated array), `pre` is the pattern prefix of each (`&` for filter's `&&T`)
    fn closure(&mut self, f: &Expr, kinds: &[(Option<Option<BTreeSet<String>>>, &str)]) -> LResult<String> {
        let EK::Closure(ps, body) = &f.kind else {
            return not_in_subset(f.pos, "a function value (pass a closure `(x) => ..` here)");
        };
        let ret = self.ret.take();
        let r = self.scoped(|l| {
            let mut pre = String::new();
            let mut names = vec![];
            let Ty::Fn(pts, _) = l.ty(f.id) else { unreachable!() };
            for (((n, _), (k, p)), t) in ps.iter().zip(kinds).zip(&pts) {
                names.push(format!("{p}{}", local(n)));
                match k {
                    _ if l.copy(t) => {
                        l.vars.remove(n);
                    }
                    None => {
                        l.vars.insert(n.clone(), VarK::Owned);
                    }
                    Some(roots) => {
                        l.vars.insert(n.clone(), VarK::Borrow { roots: roots.clone(), param: false });
                        pre.push_str(&l.canon(n, t));
                    }
                }
            }
            let b = l.expr(body)?;
            Ok(if pre.is_empty() { format!("|{}| {b}", names.join(", ")) } else { format!("|{}| {{\n{pre}{b}\n}}", names.join(", ")) })
        });
        self.ret = ret;
        r
    }

    fn match_(&mut self, m: &Expr, sc: &Expr, arms: &[Arm]) -> LResult<(String, Form)> {
        let st = self.ty(sc.id);
        let t = self.ty(m.id);
        let (ss, sf) = self.ex(sc)?;
        // how the patterns bind: by value (Copy, or the scrutinee is owned and dead after), or by reference
        let (head, by_ref, roots) = if self.copy(&st) {
            (ss, false, None)
        } else {
            match &sf {
                Form::Val => (ss, false, None),
                // (the arms assign a local the scrutinee reads: match a value of it, taken if dead, else a copy)
                _ if self.live.clone_root.contains(&m.id) => (self.owned(ss, &sf, &st), false, None),
                Form::Own { movable: true, .. } if self.live.match_move.contains(&m.id) => (ss, false, None),
                Form::Brw { is_ref: true, roots, .. } => (format!("*{ss}"), true, roots.clone()),
                Form::Brw { roots, .. } => (ss, true, roots.clone()),
                Form::Own { .. } => (ss, true, None),
            }
        };
        let mut bodies = vec![];
        let mut pats = vec![];
        for Arm { pat: p, guard, body } in arms {
            let r = self.scoped(|l| -> LResult<(String, String, Form)> {
                let mut pre = String::new();
                let ps = l.pat(p, &st, by_ref, &roots, &mut pre, body.pos)?;
                // (`pat if guard`: Rust's own guard, with the pattern's names)
                let ps = match guard {
                    Some(g) => format!("{ps} if {}", l.expr(g)?),
                    None => ps,
                };
                let (b, f) = l.ex(body)?;
                // (a borrow of what the arm binds cannot leave the arm)
                let names: Vec<String> = own::pat_names(p).into_iter().map(|n| format!("@{n}")).collect();
                let (b, f) = match &f {
                    Form::Brw { roots: Some(r), .. } if r.iter().any(|x| names.contains(x)) => {
                        let t = l.ty(body.id);
                        (l.owned(b, &f, &t), Form::Val)
                    }
                    _ => (b, f),
                };
                Ok((ps, if pre.is_empty() { b } else { format!("{{\n{pre}{b}\n}}") }, f))
            })?;
            pats.push(r.0);
            bodies.push((r.1, r.2));
        }
        let (bs, f) = if self.copy(&t) { (bodies.into_iter().map(|b| b.0).collect(), Form::Val) } else { self.join(bodies, &t) };
        let mut s = format!("match {head} {{\n");
        for (p, b) in pats.iter().zip(bs) {
            let _ = writeln!(s, "{p} => {b},");
        }
        s.push('}');
        Ok((s, f))
    }

    /// A pattern against a value of type `t`. `by_ref`: the scrutinee is a place that stays alive, so the bindings
    /// that are not Copy borrow from it (`ref xs`) instead of moving
    fn pat(&mut self, p: &Pat, t: &Ty, by_ref: bool, roots: &Option<BTreeSet<String>>, pre: &mut String, pos: Pos) -> LResult<String> {
        Ok(match p {
            Pat::Wild => "_".to_string(),
            Pat::Bind(n) => {
                if self.copy(t) {
                    self.vars.remove(n);
                    local(n)
                } else if by_ref {
                    self.vars.insert(n.clone(), VarK::Borrow { roots: roots.clone(), param: false });
                    pre.push_str(&self.canon(n, t));
                    format!("ref {}", local(n))
                } else {
                    self.vars.insert(n.clone(), VarK::Owned);
                    local(n)
                }
            }
            Pat::Lit(Lit::Int(n)) => n.to_string(),
            Pat::Lit(Lit::Bool(b)) => b.to_string(),
            Pat::Variant(_, v, subs, _) => {
                let en = self.info.variant_of[v].clone();
                self.used.insert(en.clone());
                // (the payload's types, in this instance)
                let fts: Vec<Ty> = match t {
                    Ty::Named(_, targs) => {
                        let e = &self.info.enums[&en];
                        let m: HashMap<String, Ty> = e.params.iter().cloned().zip(targs.iter().cloned()).collect();
                        e.variants.iter().find(|x| x.0 == *v).map(|x| x.1.iter().map(|t| subst_ty(t, &m)).collect()).unwrap_or_default()
                    }
                    _ => vec![],
                };
                let mut ps = vec![];
                for (i, p) in subs.iter().enumerate() {
                    let ft = fts.get(i).cloned().unwrap_or(Ty::Unit);
                    ps.push(self.pat(p, &ft, by_ref, roots, pre, pos)?);
                }
                if ps.is_empty() { format!("{}::{v}", rs_type(&en)) } else { format!("{}::{v}({})", rs_type(&en), ps.join(", ")) }
            }
            Pat::Or(alts) => {
                let ps = alts.iter().map(|a| self.pat(a, t, by_ref, roots, pre, pos)).collect::<LResult<Vec<_>>>()?;
                format!("({})", ps.join(" | "))
            }
            Pat::Record(n, fs, _, _) => {
                self.used.insert(n.clone());
                let r = &self.info.records[n];
                let m: HashMap<String, Ty> = match t {
                    Ty::Named(_, targs) => r.params.iter().cloned().zip(targs.iter().cloned()).collect(),
                    _ => HashMap::new(),
                };
                let fts: Vec<(String, Ty)> = r.fields.iter().map(|(f, ft)| (f.clone(), subst_ty(ft, &m))).collect();
                let mut ps = vec![];
                for (f, p) in fs {
                    let ft = fts.iter().find(|x| x.0 == *f).map(|x| x.1.clone()).unwrap_or(Ty::Unit);
                    ps.push(format!("{}: {}", local(f), self.pat(p, &ft, by_ref, roots, pre, pos)?));
                }
                ps.push("..".into());
                format!("{} {{ {} }}", rs_type(n), ps.join(", "))
            }
            _ => return not_in_subset(pos, "this pattern (a tuple, a float or a string)"),
        })
    }

    /// A variant as a value: `ZsShape::Circle(c, r)`
    fn variant(&mut self, en: &str, v: &str, args: Vec<String>, pos: Pos) -> LResult<String> {
        if self.info.enums[en].is_effect() {
            return not_in_subset(pos, "an effect");
        }
        self.used.insert(en.to_string());
        Ok(if args.is_empty() { format!("{}::{v}", rs_type(en)) } else { format!("{}::{v}({})", rs_type(en), args.join(", ")) })
    }

    /// The arguments of a call of a lowered instance: each borrowed or owned as the callee's inferred signature says
    fn call_args(&mut self, rs: &str, args: &[Expr], ptys: &[Ty]) -> LResult<(Vec<String>, Vec<Option<BTreeSet<String>>>)> {
        let borrow: Vec<bool> = args.iter().zip(ptys).enumerate().map(|(i, (_, t))| !self.copy(t) && !self.modes.owned.contains(&(rs.to_string(), i))).collect();
        self.args_as(args, &borrow, ptys)
    }

    /// Arguments, each borrowed or owned as `borrow` says. A borrowed argument that reads a local a later argument
    /// assigns (`pair(xs, { xs = [2]; .. })`) is made a value first, so it keeps the old one and no borrow is held
    /// across the assignment
    fn args_as(&mut self, args: &[Expr], borrow: &[bool], ptys: &[Ty]) -> LResult<(Vec<String>, Vec<Option<BTreeSet<String>>>)> {
        let mut later = vec![HashSet::new(); args.len()];
        let any = args.iter().skip(1).any(|x| self.live.assigns_in.contains(&x.id));
        for i in (0..args.len().saturating_sub(1)).rev().filter(|_| any) {
            let mut a = later[i + 1].clone();
            own::names(&args[i + 1], &mut HashSet::new(), &mut a);
            later[i] = a;
        }
        let mut out = vec![];
        let mut roots = vec![];
        for (i, x) in args.iter().enumerate() {
            if !borrow[i] {
                out.push(self.expr(x)?);
                roots.push(Some(BTreeSet::new()));
                continue;
            }
            let mut reads = HashSet::new();
            own::names(x, &mut reads, &mut HashSet::new());
            if reads.iter().any(|n| later[i].contains(n)) {
                // (`{ v }`: a value, so a local is moved (or copied) into a temporary, not borrowed in place)
                let v = format!("{{ {} }}", self.expr(x)?);
                out.push(self.borrow_of(&v, &Form::Val, &ptys[i]).0);
                roots.push(None);
            } else {
                let (s, r) = self.borrowed(x)?;
                out.push(s);
                roots.push(r);
            }
        }
        Ok((out, roots))
    }

    /// The method receiver: an expression usable before `.method()` (parenthesized unless a name or a place)
    fn recv(s: String, x: &Expr) -> String {
        if matches!(x.kind, EK::Var(_) | EK::Field(..) | EK::Index(..)) { s } else { format!("({s})") }
    }

    /// An expression and the form of its value
    fn ex(&mut self, e: &Expr) -> LResult<(String, Form)> {
        let pos = e.pos;
        let val = |s: String| Ok((s, Form::Val));
        match &e.kind {
            // (a string literal is a `&'static str`: borrowed from nothing, copied only where it is consumed)
            EK::Lit(Lit::Str(v)) => Ok((format!("{v:?}"), Form::Brw { is_ref: true, roots: Some(BTreeSet::new()), param: None })),
            EK::Lit(l) => val(self.lit(l, e.id, pos)?),
            EK::Var(n) | EK::Path(_, n) if matches!(self.info.callee.get(&e.id), Some(Callee::Variant(_))) => {
                let Some(Callee::Variant(en)) = self.info.callee.get(&e.id).cloned() else { unreachable!() };
                if self.info.enums[&en].variants.iter().any(|x| x.0 == *n && !x.1.is_empty()) {
                    return not_in_subset(pos, "a variant constructor as a function value");
                }
                val(self.variant(&en, n, vec![], pos)?)
            }
            EK::Var(n) => match self.info.callee.get(&e.id) {
                Some(Callee::Local) => {
                    let s = local(n);
                    Ok(match self.vars.get(n) {
                        // (a local borrowed by a live `let` is frozen: never moved while the borrow may be used)
                        Some(VarK::Owned) => (s, Form::Own { movable: self.live.last.contains(&e.id) && !self.frozen.contains(n), root: n.clone() }),
                        Some(VarK::Borrow { roots, param }) => (s, Form::Brw { is_ref: true, roots: roots.clone(), param: param.then(|| n.clone()) }),
                        None => (s, Form::Val),
                    })
                }
                _ => not_in_subset(pos, &format!("`{n}` as a value")),
            },
            EK::Call(f, args) => {
                if let EK::Path(_, c) = &f.kind
                    && (c == "zero" || c == "one")
                {
                    let t = self.num(f.id);
                    return val(match t {
                        Ty::F64 => (if c == "one" { "1.0f64" } else { "0.0f64" }).into(),
                        Ty::F32 => (if c == "one" { "1.0f32" } else { "0.0f32" }).into(),
                        t => format!("{}{}", if c == "one" { 1 } else { 0 }, scalar(&t).unwrap_or("")),
                    });
                }
                if let EK::Var(n) | EK::Path(_, n) = &f.kind
                    && let Some(Callee::Variant(en)) = self.info.callee.get(&f.id).cloned()
                {
                    let a = args.iter().map(|x| self.expr(x)).collect::<LResult<Vec<_>>>()?;
                    return val(self.variant(&en, n, a, pos)?);
                }
                if let Some(Callee::Coll(set, c)) = self.info.callee.get(&f.id).cloned() {
                    let t = self.ty(e.id);
                    let rt = self.rty(&t, pos, None)?;
                    let base = rs_type(if set { "Set" } else { "Map" });
                    return match (set, c.as_str()) {
                        (_, "new") => val(format!("{{ let zs_c: {rt} = {base}::zs_new(); zs_c }}")),
                        (true, _) => {
                            let (xs, _) = self.borrowed(&args[0])?;
                            val(format!("{{ let mut zs_c: {rt} = {base}::zs_new(); for zs_x in ({xs}).iter() {{ zs_c.zs_insert(zs_x.clone()); }} zs_c }}"))
                        }
                        _ => not_in_subset(pos, "`Map::from(pairs)` (tuples): build it with `insert`"),
                    };
                }
                if let Some(Callee::TypedCtor(el, c)) = self.info.callee.get(&f.id).cloned() {
                    let zero = match el {
                        Ty::F32 => "0.0f32",
                        Ty::F64 => "0.0f64",
                        _ => "0i32",
                    };
                    return val(if c == "zeros" {
                        format!("vec![{zero}; ({}) as usize]", self.expr(&args[0])?)
                    } else {
                        format!("({}).to_vec()", self.borrowed(&args[0])?.0)
                    });
                }
                let EK::Var(n) = &f.kind else { return not_in_subset(pos, "calling this") };
                if let Some((a, b)) = self.range_args(e)? {
                    return val(format!("({a}..{b}).collect::<Vec<i32>>()"));
                }
                match self.info.callee.get(&f.id) {
                    Some(Callee::Fn) if self.fns.contains_key(n.as_str()) => {
                        let targs: Vec<Ty> = self.info.inst.get(&f.id).map(|t| t.iter().map(|t| subst_ty(t, &self.subst)).collect()).unwrap_or_default();
                        let Item::Fn { generics, .. } = self.fns[n.as_str()] else { unreachable!() };
                        let m: HashMap<String, Ty> = generics.iter().cloned().zip(targs.iter().cloned()).collect();
                        let ptys: Vec<Ty> = self.info.fns[n].args.iter().map(|t| subst_ty(t, &m)).collect();
                        let ret = subst_ty(&self.info.fns[n].ret, &m);
                        let rs = self.want(n, targs);
                        self.calls.insert(rs.clone());
                        let (a, roots) = self.call_args(&rs, args, &ptys)?;
                        let s = format!("{rs}({})", a.join(", "));
                        if self.copy(&ret) || self.modes.owned_result.contains(&rs) {
                            val(s)
                        } else {
                            // (a borrowed result borrows from the arguments the callee's result borrows from: what they
                            // borrow from)
                            let region = self.modes.region.get(&rs).cloned().unwrap_or_default();
                            let mut all = Some(BTreeSet::new());
                            for (_, r) in roots.into_iter().enumerate().filter(|(i, _)| region.contains(i)) {
                                all = match (all, r) {
                                    (Some(mut a), Some(r)) => {
                                        a.extend(r);
                                        Some(a)
                                    }
                                    _ => None,
                                };
                            }
                            Ok((s, Form::Brw { is_ref: true, roots: all, param: None }))
                        }
                    }
                    Some(Callee::Fn) => Err((pos, format!("`{n}` is a SIMD kernel: use --no-simd to lower this SIMD kernel to scalar Rust"))),
                    Some(Callee::Wasm) => {
                        let (wargs, _) = &self.info.wasm[n];
                        let arrs: Vec<bool> = wargs.iter().map(|(_, w)| matches!(w, WTy::Slice(_))).collect();
                        if wargs.iter().any(|(_, w)| matches!(w, WTy::Str | WTy::Callback(..))) {
                            return not_in_subset(pos, "calling a `wasm fn` that takes a string or a callback");
                        }
                        let ptys: Vec<Ty> = args.iter().map(|x| self.ty(x.id)).collect();
                        let (a, _) = self.args_as(args, &arrs, &ptys)?;
                        val(format!("{n}({})", a.join(", ")))
                    }
                    Some(Callee::Local) => not_in_subset(pos, "calling a function value"),
                    // (an `extern`, or a function imported from another module, which is one: JS, not in this wasm)
                    _ if self.info.externs.get(n).is_some_and(|e| e.1.as_deref().is_some_and(|m| m.ends_with(".js") && (m.starts_with("./") || m.starts_with("../")))) => {
                        not_in_subset(pos, &format!("calling `{n}` (imported from another module, so it is JS here)"))
                    }
                    _ => not_in_subset(pos, &format!("calling `{n}`")),
                }
            }
            EK::Method(r, _, args) => self.method(e, r, args),
            EK::Bin(op, l, r) => {
                let lt = self.ty(l.id);
                if !self.copy(&lt) {
                    // (values that are not Copy: compared through borrows of the same type, `&[T]` / `&str` / `&ZsX`,
                    // by content (derive(PartialEq)), as the JS layer's `$eq`)
                    // (both borrowed, through the arguments' path: a right operand that assigns what the left one reads,
                    // `xs == { xs = [2]; xs }`, gets the left one as a value first. The left string of `+` is taken
                    // owned, so nothing of it is borrowed while the right one runs)
                    let (a, b) = match *op {
                        "+" => (self.expr(l)?, self.borrowed(r)?.0),
                        _ => {
                            let (v, _) = self.args_as(&[(**l).clone(), (**r).clone()], &[true, true], &[lt.clone(), lt.clone()])?;
                            (v[0].clone(), v[1].clone())
                        }
                    };
                    return val(match *op {
                        "==" | "!=" => format!("({a} {op} {b})"),
                        "<" | "<=" | ">" | ">=" if lt == Ty::Str => {
                            self.str_helpers.extend(["zs_cmp16", "zs_units16"]);
                            format!("(zs_cmp16({a}, {b}) {op} 0)")
                        }
                        // (the left string is taken, moved where it can be: `a + b + c` appends in place)
                        "+" if lt == Ty::Str => format!("{{ let mut __zs_s = {a}; __zs_s.push_str({b}); __zs_s }}"),
                        _ => return not_in_subset(pos, &format!("`{op}` on {}", show(&lt))),
                    });
                }
                let (a, b) = (self.expr(l)?, self.expr(r)?);
                val(match *op {
                    "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||" => format!("({a} {op} {b})"),
                    _ => match self.num(e.id) {
                        Ty::I32 | Ty::U32 | Ty::I64 => {
                            let m = match *op {
                                "+" => "wrapping_add",
                                "-" => "wrapping_sub",
                                "*" => "wrapping_mul",
                                "/" => "wrapping_div",
                                _ => "wrapping_rem",
                            };
                            format!("({a}).{m}({b})")
                        }
                        Ty::F64 | Ty::F32 => format!("({a} {op} {b})"),
                        _ => return not_in_subset(pos, "string concatenation"),
                    },
                })
            }
            EK::Un("!", x) => val(format!("!({})", self.expr(x)?)),
            EK::Un(_, x) => val(match self.num(e.id) {
                Ty::F64 | Ty::F32 => format!("-({})", self.expr(x)?),
                _ => format!("({}).wrapping_neg()", self.expr(x)?),
            }),
            EK::Cast(x, _) => {
                let (_, to) = self.info.cast[&e.id].clone();
                let to = subst_ty(&to, &self.subst);
                val(format!("(({}) as {})", self.expr(x)?, scalar(&to).unwrap_or("f64")))
            }
            EK::If(c, t, el) => {
                let c = self.expr(c)?;
                let ty = self.ty(e.id);
                let a = self.scoped(|l| l.ex(t))?;
                let Some(el) = el else {
                    return val(format!("if {c} {}", Self::braced(a.0)));
                };
                let b = self.scoped(|l| l.ex(el))?;
                let (bs, f) = if self.copy(&ty) { (vec![a.0, b.0], Form::Val) } else { self.join(vec![a, b], &ty) };
                Ok((format!("if {c} {} else {}", Self::braced(bs[0].clone()), Self::braced(bs[1].clone())), f))
            }
            EK::Match(sc, arms) => self.match_(e, sc, arms),
            EK::Block(stmts, tail) => self.scoped(|l| {
                let mut s = String::from("{\n");
                let frozen = l.frozen.len();
                let saved = std::mem::take(&mut l.later_assigns);
                for (i, st) in stmts.iter().enumerate() {
                    // (what the rest of the block assigns: a `let` may keep a borrow of a local only if it is not
                    // assigned while the borrow lives. Most blocks assign nothing, which the liveness pass knows)
                    l.later_assigns.clear();
                    if let Stmt::Let(_, _, x) = st
                        && !l.copy(&l.ty(x.id))
                    {
                        let rest = stmts[i + 1..].iter().map(|s| match s {
                            Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) | Stmt::Expr(x) => x,
                        });
                        for x in rest.chain(tail.as_deref()).filter(|x| l.live.assigns_in.contains(&x.id)) {
                            own::names(x, &mut HashSet::new(), &mut l.later_assigns);
                        }
                    }
                    s.push_str(&l.stmt(st)?);
                }
                l.later_assigns = saved;
                // (the locals this block declares: a borrow of one cannot leave it)
                let declared: HashSet<String> = stmts
                    .iter()
                    .filter_map(|st| match st {
                        Stmt::Let(Pat::Bind(n), ..) | Stmt::LetMut(n, ..) => Some(format!("@{n}")),
                        _ => None,
                    })
                    .collect();
                let mut f = Form::Val;
                if let Some(t) = tail {
                    let (ts, tf) = l.ex(t)?;
                    let ty = l.ty(t.id);
                    // (the block's value is its tail's: a borrow of the parameters stays one, made a reference inside
                    // the block (a place there would be moved out by the block); a borrow of a local declared in the
                    // block cannot leave it, so it is copied)
                    let ts = match tf {
                        _ if l.copy(&ty) => ts,
                        Form::Brw { roots: Some(r), param, is_ref } if !r.iter().any(|x| declared.contains(x)) => {
                            let b = if is_ref { ts } else { l.borrow_of(&ts, &Form::Brw { is_ref, roots: None, param: None }, &ty).0 };
                            f = Form::Brw { is_ref: true, roots: Some(r), param };
                            b
                        }
                        other => l.owned(ts, &other, &ty),
                    };
                    let _ = writeln!(s, "{ts}");
                }
                s.push('}');
                l.frozen.truncate(frozen);
                Ok((s, f))
            }),
            EK::Assign(n, x) => {
                // (`m = m.insert(k, f(m.get(k)))`: the arguments first, then `m` changed in place)
                if own::assigned_change(e, self.info).is_some() {
                    self.move_recv = Some(x.id);
                }
                val(format!("{} = {}", local(n), self.expr(x)?))
            }
            EK::While(c, body) => val(format!("while {} {}", self.expr(c)?, self.block(body)?)),
            EK::For(x, it, body) => {
                let el = match self.ty(it.id) {
                    Ty::Array(t) | Ty::Typed(t) => *t,
                    _ => Ty::I32,
                };
                let (iter, kind) = match self.range_args(it)? {
                    Some((a, b)) => (format!("({a})..({b})"), None),
                    None => {
                        let (v, f) = self.ex(it)?;
                        let it_ty = self.ty(it.id);
                        let v = Self::recv(v, it);
                        if self.live.clone_root.contains(&e.id) {
                            // (the body assigns a local the iterable reads: iterate a value of it, taken if dead)
                            (format!("{}.into_iter()", self.owned(v, &f, &it_ty)), None)
                        } else if self.copy(&el) {
                            (format!("{v}.iter().copied()"), None)
                        } else {
                            match f {
                                Form::Val | Form::Own { movable: true, .. } => (format!("{v}.into_iter()"), None),
                                Form::Brw { roots, .. } => (format!("{v}.iter()"), Some(roots)),
                                Form::Own { .. } => (format!("{v}.iter()"), Some(None)),
                            }
                        }
                    }
                };
                self.scoped(|l| {
                    let mut pre = String::new();
                    match kind {
                        _ if l.copy(&el) => {
                            l.vars.remove(x);
                        }
                        None => {
                            l.vars.insert(x.clone(), VarK::Owned);
                        }
                        Some(roots) => {
                            l.vars.insert(x.clone(), VarK::Borrow { roots, param: false });
                            pre = l.canon(x, &el);
                        }
                    }
                    let b = l.block(body)?;
                    let b = if pre.is_empty() { b } else { format!("{{\n{pre}{b}\n}}") };
                    val(format!("for {} in {iter} {b}", local(x)))
                })
            }
            EK::Loop(body) => val(format!("loop {}", self.block(body)?)),
            // (owned, as the function's result is when it has a `return`)
            EK::Return(x) => {
                let Some(ret) = self.ret.clone() else {
                    return not_in_subset(pos, "`return` in a closure");
                };
                match x {
                    None => val("return".into()),
                    Some(x) if self.copy(&ret) => val(format!("return {}", self.expr(x)?)),
                    Some(x) => {
                        let (s, f) = self.ex(x)?;
                        val(format!("return {}", self.owned(s, &f, &ret)))
                    }
                }
            }
            EK::Break => val("break".into()),
            EK::Continue => val("continue".into()),
            EK::Index(x, i) => {
                let (a, f) = self.ex(x)?;
                let a = Self::recv(a, x);
                // (a negative index becomes a huge usize: out of bounds, as in JS)
                let s = format!("{a}[({}) as usize]", self.expr(i)?);
                Ok(if self.copy(&self.ty(e.id)) { (s, Form::Val) } else { (s, Self::project(f, true)) })
            }
            EK::Array(xs) => {
                let v = xs.iter().map(|x| self.expr(x)).collect::<LResult<Vec<_>>>()?;
                if v.is_empty() {
                    // (the element type, for an empty literal)
                    let t = self.ty(e.id);
                    return val(format!("Vec::<{}>::new()", self.elem_rty(&t)));
                }
                // (an element with a block in it is bound first, in order: zustc loses a block's locals and types
                // inside `vec![..]`, mizchi/zust#208)
                if v.iter().any(|s| s.contains('{')) {
                    let lets: String = v.iter().enumerate().map(|(i, s)| format!("let zs_e{i} = {s}; ")).collect();
                    let names: Vec<String> = (0..v.len()).map(|i| format!("zs_e{i}")).collect();
                    return val(format!("{{ {lets}vec![{}] }}", names.join(", ")));
                }
                val(format!("vec![{}]", v.join(", ")))
            }
            EK::Closure(..) => not_in_subset(pos, "a closure outside map / filter / fold / any / all"),
            EK::Field(x, f) => {
                if f.parse::<usize>().is_ok() {
                    return not_in_subset(pos, "a tuple");
                }
                let (s, xf) = self.ex(x)?;
                let s = if matches!(x.kind, EK::Var(_) | EK::Field(..) | EK::Index(..)) { s } else { format!("({s})") };
                let s = format!("{s}.{}", local(f));
                Ok(if self.copy(&self.ty(e.id)) { (s, Form::Val) } else { (s, Self::project(xf, false)) })
            }
            EK::Record(n, fields, base) => {
                self.used.insert(n.clone());
                let mut parts = fields.iter().map(|(f, x)| Ok(format!("{}: {}", local(f), self.expr(x)?))).collect::<LResult<Vec<_>>>()?;
                if let Some(b) = base {
                    parts.push(format!("..{}", self.expr(b)?));
                }
                val(format!("{} {{ {} }}", rs_type(n), parts.join(", ")))
            }
            EK::Tuple(_) => not_in_subset(pos, "a tuple"),
            EK::Await(_) | EK::Async(_) => not_in_subset(pos, "`await` / an async closure (a Promise is JS's)"),
            // (Rust's own `?` would need the std types: a match that returns the `Err` / `None`, rebuilt)
            EK::Try(x) => {
                let en = self.info.tries[&e.id].clone();
                self.used.insert(en.clone());
                let v = self.expr(x)?;
                let t = rs_type(&en);
                val(if en == "Result" {
                    format!("match {v} {{ {t}::Ok(zs_v) => zs_v, {t}::Err(zs_e) => return {t}::Err(zs_e) }}")
                } else {
                    format!("match {v} {{ {t}::Some(zs_v) => zs_v, {t}::None => return {t}::None }}")
                })
            }
            // (appended piece by piece: `format!` would pull `core::fmt` into the wasm (about 10 KB). The holes are
            // borrowed through the arguments' path; an integer is written by `zs_push_int`, as JS writes it. A float
            // is written differently by JS and Rust)
            EK::Interp(parts, holes) => {
                let tys: Vec<Ty> = holes.iter().map(|h| subst_ty(&self.ty(h.id), &self.subst)).collect();
                if let Some(k) = tys.iter().position(|t| matches!(t, Ty::F32 | Ty::F64)) {
                    return not_in_subset(holes[k].pos, "a float in a template string (JS and Rust format floats differently)");
                }
                let (vs, _) = self.args_as(holes, &vec![true; holes.len()], &tys)?;
                let mut o = String::from("{ let mut __zs_t = String::new();");
                for (k, p) in parts.iter().enumerate() {
                    if !p.is_empty() {
                        let _ = write!(o, " __zs_t.push_str({p:?});");
                    }
                    if let (Some(v), Some(t)) = (vs.get(k), tys.get(k)) {
                        match t {
                            Ty::Str => {
                                let _ = write!(o, " __zs_t.push_str({v});");
                            }
                            Ty::Bool => {
                                let _ = write!(o, " __zs_t.push_str(if {v} {{ \"true\" }} else {{ \"false\" }});");
                            }
                            _ => {
                                self.str_helpers.insert("zs_push_int");
                                let _ = write!(o, " zs_push_int(&mut __zs_t, ({v}) as i64);");
                            }
                        }
                    }
                }
                o.push_str(" __zs_t }");
                val(o)
            }
            EK::Path(..) => not_in_subset(pos, "this path"),
            EK::Jsx(_) => not_in_subset(pos, "JSX"),
            EK::Perform(..) | EK::Handle(..) => not_in_subset(pos, "an effect"),
        }
    }

    fn method(&mut self, e: &Expr, r: &Expr, args: &[Expr]) -> LResult<(String, Form)> {
        use Builtin as B;
        let pos = e.pos;
        let Some(b) = self.info.builtin.get(&e.id).copied() else { return not_in_subset(pos, "this method") };
        let rt = self.ty(r.id);
        let el = match &rt {
            Ty::Array(t) | Ty::Typed(t) => (**t).clone(),
            _ => Ty::I32,
        };
        // `range(a, b).fold(..)`: the iterator `a..b` directly, no array. (The receiver is lowered once: a chain of
        // methods would otherwise lower its innermost receiver exponentially many times)
        let range = self.range_args(r)?;
        let (recv, rf) = match &range {
            Some((a, b)) => (format!("({a}..{b}).collect::<Vec<i32>>()"), Form::Val),
            None => {
                let (s, f) = self.ex(r)?;
                (Self::recv(s, r), f)
            }
        };
        // how the elements are visited: copied (Copy), taken (an owned array dead here), or borrowed
        let (it, elems): (String, Option<Option<BTreeSet<String>>>) = match (&range, &rf) {
            (Some((a, b)), _) => (format!("({a}..{b})"), None),
            _ if self.copy(&el) => (format!("{recv}.iter().copied()"), None),
            (_, Form::Val | Form::Own { movable: true, .. }) => (format!("{recv}.into_iter()"), None),
            (_, Form::Brw { roots, .. }) => (format!("{recv}.iter()"), Some(roots.clone())),
            (_, Form::Own { .. }) => (format!("{recv}.iter()"), Some(None)),
        };
        let by_ref = elems.is_some();
        let k = |pre: &'static str| (elems.clone(), pre);
        let val = |s: String| Ok((s, Form::Val));
        // strings: JS's meaning (lengths and order in UTF-16 code units, JS's white space)
        if rt == Ty::Str {
            let (mut sb, mut sroots) = self.borrow_of(&recv, &rf, &rt);
            // (an argument that assigns what the receiver reads, `s.contains({ s = "b"; "a" })`: the receiver is taken
            // as a value first, so no borrow of it is held across the assignment)
            if args.iter().any(|a| self.live.assigns_in.contains(&a.id)) {
                let (mut reads, mut assigns) = (HashSet::new(), HashSet::new());
                own::names(r, &mut reads, &mut HashSet::new());
                for a in args {
                    own::names(a, &mut HashSet::new(), &mut assigns);
                }
                if reads.iter().any(|n| assigns.contains(n)) {
                    let v = format!("{{ {} }}", self.owned(recv.clone(), &rf, &rt));
                    sb = self.borrow_of(&v, &Form::Val, &rt).0;
                    sroots = None;
                }
            }
            return match b {
                B::Len => {
                    self.str_helpers.insert("zs_len16");
                    val(format!("zs_len16({sb})"))
                }
                B::ToUpper => val(format!("{recv}.to_uppercase()")),
                // (a part of the string: a borrow of what the string borrows from)
                B::Trim => {
                    self.str_helpers.extend(["zs_trim", "zs_ws"]);
                    Ok((format!("zs_trim({sb})"), Form::Brw { is_ref: true, roots: sroots, param: None }))
                }
                B::Contains | B::Split => {
                    let (o, _) = self.borrowed(&args[0])?;
                    let h = if b == B::Contains { "zs_contains" } else { "zs_split" };
                    self.str_helpers.insert(h);
                    val(format!("{h}({sb}, {o})"))
                }
                B::Std(_, m) => {
                    let o: Vec<String> = args.iter().map(|a| self.borrowed(a).map(|x| x.0)).collect::<LResult<_>>()?;
                    let (opt, oty) = self.option_type(&match m {
                        "to_f64" => Ty::F64,
                        _ => Ty::I32,
                    });
                    match m {
                        "is_empty" => val(format!("{sb}.is_empty()")),
                        "to_lower" => val(format!("{recv}.to_lowercase()")),
                        "starts_with" | "ends_with" => val(format!("{sb}.{m}({})", o[0])),
                        "index_of" => {
                            self.str_helpers.insert("zs_index_of16");
                            val(format!("{{ let zs_i = zs_index_of16({sb}, {}); let zs_o: {oty} = if zs_i >= 0 {{ {opt}::Some(zs_i) }} else {{ {opt}::None }}; zs_o }}", o[0]))
                        }
                        "slice" => {
                            self.str_helpers.insert("zs_slice16");
                            val(format!("zs_slice16({sb}, {}, {})", o[0], o[1]))
                        }
                        "replace" => val(format!("{sb}.replace({}, {})", o[0], o[1])),
                        "repeat" => val(format!("{sb}.repeat(({}) as usize)", o[0])),
                        // (a part of the string: a borrow of what the string borrows from)
                        "trim_start" | "trim_end" => {
                            let h = if m == "trim_start" { "zs_trim_start" } else { "zs_trim_end" };
                            self.str_helpers.insert(h);
                            Ok((format!("{h}({sb})"), Form::Brw { is_ref: true, roots: sroots, param: None }))
                        }
                        "chars" => val(format!("{sb}.chars().map(|zs_c| zs_c.to_string()).collect::<Vec<String>>()")),
                        "to_f64" if matches!(self.target, Target::Wasm { .. }) => {
                            self.str_helpers.insert("zs_to_f64");
                            val(format!("{{ let mut zs_f = 0.0f64; let zs_o: {oty} = if zs_to_f64({sb}, &mut zs_f) {{ {opt}::Some(zs_f) }} else {{ {opt}::None }}; zs_o }}"))
                        }
                        _ => {
                            let t = if m == "to_i32" { "i32" } else { "f64" };
                            val(format!("{{ let zs_o: {oty} = match {sb}.parse::<{t}>() {{ Ok(zs_v) => {opt}::Some(zs_v), Err(_) => {opt}::None }}; zs_o }}"))
                        }
                    }
                }
                _ => not_in_subset(pos, "this method on a string"),
            };
        }
        match b {
            B::Len => val(format!("({recv}.len() as i32)")),
            B::Join => {
                // (a separator that assigns what the receiver reads: the receiver is taken as a value first)
                let mut recv = recv;
                if self.live.assigns_in.contains(&args[0].id) {
                    let (mut reads, mut assigns) = (HashSet::new(), HashSet::new());
                    own::names(r, &mut reads, &mut HashSet::new());
                    own::names(&args[0], &mut HashSet::new(), &mut assigns);
                    if reads.iter().any(|n| assigns.contains(n)) {
                        recv = format!("({{ {} }})", self.owned(recv, &rf, &rt));
                    }
                }
                let (sep, _) = self.borrowed(&args[0])?;
                val(format!("{recv}.join({sep})"))
            }
            // (JS's and Rust's formats agree for integers and bools; not for floats)
            B::ToString if matches!(rt, Ty::F32 | Ty::F64) => not_in_subset(pos, "`to_string` of a float (JS and Rust format floats differently)"),
            B::ToString if rt == Ty::Bool => val(format!("String::from(if {recv} {{ \"true\" }} else {{ \"false\" }})")),
            B::ToString => {
                self.str_helpers.insert("zs_push_int");
                val(format!("{{ let mut __zs_t = String::new(); zs_push_int(&mut __zs_t, ({recv}) as i64); __zs_t }}"))
            }
            B::Map => val(format!("{it}.map({}).collect::<Vec<_>>()", self.closure(&args[0], &[k("")])?)),
            B::Filter => {
                // (filter's closure gets `&Item`. For copied items and for references `|&x|` binds the item; for
                // items the iterator owns that are not Copy, `|x|` borrows it)
                let param = if elems.is_none() && range.is_none() && !self.copy(&el) { (Some(None), "") } else { k("&") };
                let f = self.closure(&args[0], &[param])?;
                val(if by_ref { format!("{it}.filter({f}).cloned().collect::<Vec<_>>()") } else { format!("{it}.filter({f}).collect::<Vec<_>>()") })
            }
            B::Any => val(format!("{it}.any({})", self.closure(&args[0], &[k("")])?)),
            B::All => val(format!("{it}.all({})", self.closure(&args[0], &[k("")])?)),
            B::Fold => {
                let init = self.expr(&args[0])?;
                val(format!("{it}.fold({init}, {})", self.closure(&args[1], &[(None, ""), k("")])?))
            }
            B::Push => {
                // (in place when the array is owned and dead here: amortized O(1))
                let v = self.owned(recv, &rf, &rt);
                val(format!("{{ let mut __zs_v = {v}; __zs_v.push({}); __zs_v }}", self.expr(&args[0])?))
            }
            B::Concat => {
                let v = self.owned(recv, &rf, &rt);
                let (o, _) = self.borrowed(&args[0])?;
                let ext = if self.copy(&el) { format!("__zs_v.extend_from_slice({o})") } else { format!("__zs_v.extend(({o}).iter().cloned())") };
                val(format!("{{ let mut __zs_v = {v}; {ext}; __zs_v }}"))
            }
            B::Sqrt => val(format!("{recv}.sqrt()")),
            B::Abs => val(format!("{recv}.abs()")),
            B::Max | B::Min => {
                // (JS's Math.max / Math.min: NaN wins, +0 is above -0)
                let t = scalar(&self.num(e.id)).unwrap_or("f64");
                let h = format!("zs_{}_{t}", if b == B::Max { "max" } else { "min" });
                self.helpers.insert(h.clone());
                val(format!("{h}({recv}, {})", self.expr(&args[0])?))
            }
            // (a typed array and an array are both a Vec in Rust: moved when it can be, copied otherwise)
            B::ToArray => val(self.owned(recv, &rf, &rt)),
            B::Floor => val(format!("{recv}.floor()")),
            B::Round => val(format!("{recv}.round()")),
            // (the prelude's Option / Result: the methods that take no function)
            B::Opt(m @ ("is_some" | "is_none" | "is_ok" | "is_err")) => {
                let Ty::Named(en, _) = &rt else { unreachable!() };
                self.used.insert(en.clone());
                let v = match m {
                    "is_some" => "Some",
                    "is_none" => "None",
                    "is_ok" => "Ok",
                    _ => "Err",
                };
                let pat = if v == "None" { format!("{}::None", rs_type(en)) } else { format!("{}::{v}(_)", rs_type(en)) };
                val(format!("match &{recv} {{ {pat} => true, _ => false }}"))
            }
            B::Opt(m @ ("unwrap_or" | "ok" | "ok_or")) => {
                let Ty::Named(en, targs) = rt.clone() else { unreachable!() };
                self.used.insert(en.clone());
                let o = self.owned(recv, &rf, &rt);
                let (opt, res) = (rs_type("Option"), rs_type("Result"));
                let arg = |l: &mut Self, t: &Ty| -> LResult<String> {
                    let (s, f) = l.ex(&args[0])?;
                    Ok(if l.copy(t) { s } else { l.owned(s, &f, t) })
                };
                match (en.as_str(), m) {
                    // (bound with its type: zustc does not know the type of a generic enum's field bound by a
                    // `match` when it is sliced with `[..]`)
                    ("Option", "unwrap_or") => {
                        let d = arg(self, &targs[0])?;
                        let t = self.rty(&targs[0], pos, None)?;
                        val(format!("{{ let zs_u: {t} = match {o} {{ {opt}::Some(zs_x) => zs_x, {opt}::None => {d} }}; zs_u }}"))
                    }
                    (_, "unwrap_or") => {
                        let d = arg(self, &targs[0])?;
                        let t = self.rty(&targs[0], pos, None)?;
                        val(format!("{{ let zs_u: {t} = match {o} {{ {res}::Ok(zs_x) => zs_x, {res}::Err(_) => {d} }}; zs_u }}"))
                    }
                    (_, "ok") => {
                        let (_, oty) = self.option_type(&targs[0]);
                        val(format!("{{ let zs_o: {oty} = match {o} {{ {res}::Ok(zs_x) => {opt}::Some(zs_x), {res}::Err(_) => {opt}::None }}; zs_o }}"))
                    }
                    _ => {
                        let et = self.ty(e.id);
                        let Ty::Named(_, rargs) = &et else { unreachable!() };
                        let ea = arg(self, &rargs[1])?;
                        self.used.insert("Result".into());
                        let rty = self.rty(&et, pos, None)?;
                        val(format!("{{ let zs_o: {rty} = match {o} {{ {opt}::Some(zs_x) => {res}::Ok(zs_x), {opt}::None => {res}::Err({ea}) }}; zs_o }}"))
                    }
                }
            }
            B::Std(on @ (On::Map | On::Set), m) => {
                let opt = |l: &mut Self, t: &Ty| l.option_type(t);
                let Ty::Named(_, targs) = &rt else { unreachable!() };
                let targs = targs.clone();
                let mut owned_args = vec![];
                for (a, t) in args.iter().zip(if on == On::Map && m == "insert" { vec![targs[0].clone(), targs[1].clone()] } else { vec![targs[0].clone()] }) {
                    let (s, f) = self.ex(a)?;
                    owned_args.push(if self.copy(&t) { s } else { self.owned(s, &f, &t) });
                }
                let change = |l: &mut Self, call: String, recv: String| -> String {
                    // (the arguments first when the receiver is the local assigned the result: it is moved, not copied)
                    if l.move_recv == Some(e.id) {
                        let EK::Var(n) = &r.kind else { unreachable!() };
                        let lets: String = owned_args.iter().enumerate().map(|(i, a)| format!("let zs_a{i} = {a}; ")).collect();
                        let call = (0..owned_args.len()).fold(call, |c, i| c.replace(&format!("@{i}"), &format!("zs_a{i}")));
                        format!("{{ {lets}let mut zs_m = {}; zs_m.{call}; zs_m }}", local(n))
                    } else {
                        let call = (0..owned_args.len()).fold(call, |c, i| c.replace(&format!("@{i}"), &owned_args[i]));
                        let v = l.owned(recv, &rf, &rt);
                        format!("{{ let mut zs_m = {v}; zs_m.{call}; zs_m }}")
                    }
                };
                match (on, m) {
                    (On::Map, "len") => val(format!("({recv}.ks.len() as i32)")),
                    (On::Set, "len") => val(format!("({recv}.xs.len() as i32)")),
                    (On::Map, "is_empty") => val(format!("{recv}.ks.is_empty()")),
                    (On::Set, "is_empty") => val(format!("{recv}.xs.is_empty()")),
                    (On::Map, "get") => {
                        let (o, oty) = opt(self, &targs[1]);
                        let c = if self.copy(&targs[1]) { ".copied()" } else { ".cloned()" };
                        val(format!("{{ let zs_o: {oty} = match {recv}.zs_find(&{}){c} {{ Some(zs_x) => {o}::Some(zs_x), None => {o}::None }}; zs_o }}", owned_args[0]))
                    }
                    (On::Map, "contains_key") => val(format!("{recv}.zs_find(&{}).is_some()", owned_args[0])),
                    (On::Set, "contains") => val(format!("{recv}.zs_has(&{})", owned_args[0])),
                    (On::Map, "insert") => val(change(self, "zs_insert(@0, @1)".into(), recv)),
                    (_, "insert") => val(change(self, "zs_insert(@0)".into(), recv)),
                    (_, "remove") => val(change(self, "zs_remove(&@0)".into(), recv)),
                    (On::Map, "keys") => val(format!("{recv}.ks.clone()")),
                    (On::Map, "values") => val(format!("{recv}.vs.clone()")),
                    (On::Set, "values") => val(format!("{recv}.xs.clone()")),
                    _ => not_in_subset(pos, "a Map's `entries()` (a tuple)"),
                }
            }
            B::Std(On::Float, m) => {
                let a: Vec<String> = args.iter().map(|a| self.expr(a)).collect::<LResult<_>>()?;
                val(match m {
                    "pow" => format!("{recv}.powf({})", a[0]),
                    "atan2" => format!("{recv}.atan2({})", a[0]),
                    _ => format!("{recv}.{m}()"),
                })
            }
            B::Std(On::Int, m) => {
                let a: Vec<String> = args.iter().map(|a| self.expr(a)).collect::<LResult<_>>()?;
                val(match m {
                    "abs" => format!("{recv}.wrapping_abs()"),
                    "min" | "max" => format!("{recv}.{m}({})", a[0]),
                    // (the receiver first, then the exponent; a negative one is an error, as in JS)
                    _ => format!("{{ let zs_a = {recv}; let zs_e: i32 = {}; if zs_e < 0 {{ panic!(\"a negative exponent\") }} zs_a.wrapping_pow(zs_e as u32) }}", a[0]),
                })
            }
            B::Std(_, m) => {
                let (opt, oty) = self.option_type(&if matches!(m, "index_of" | "find_index") { Ty::I32 } else { el.clone() });
                let cloned = if self.copy(&el) { ".copied()" } else { ".cloned()" };
                let some = |s: String| format!("{{ let zs_o: {oty} = match {s} {{ Some(zs_x) => {opt}::Some(zs_x), None => {opt}::None }}; zs_o }}");
                match m {
                    // (by index: a slice index is the same in every Rust)
                    "get" => {
                        let i = self.expr(&args[0])?;
                        let x = if self.copy(&el) { "zs_r[zs_i]" } else { "zs_r[zs_i].clone()" };
                        val(format!("{{ let zs_r = &{recv}; let zs_i = ({i}) as usize; let zs_o: {oty} = if zs_i < zs_r.len() {{ {opt}::Some({x}) }} else {{ {opt}::None }}; zs_o }}"))
                    }
                    "first" | "last" => val(some(format!("{recv}.{m}(){cloned}"))),
                    "is_empty" => val(format!("{recv}.is_empty()")),
                    "slice" => {
                        self.str_helpers.insert("zs_range");
                        let (a, b) = (self.expr(&args[0])?, self.expr(&args[1])?);
                        val(format!("{{ let zs_r = &{recv}; let (zs_a, zs_b) = zs_range(zs_r.len() as i32, {a}, {b}); zs_r[zs_a..zs_b].to_vec() }}"))
                    }
                    "reverse" | "sort" | "sort_by" => {
                        let v = self.owned(recv, &rf, &rt);
                        let how = match m {
                            "reverse" => "zs_v.reverse();".to_string(),
                            "sort" => match &el {
                                Ty::Str => {
                                    self.str_helpers.insert("zs_cmp16");
                                    "zs_v.sort_by(|a, b| zs_cmp16(a, b).cmp(&0));".to_string()
                                }
                                // (NaN is equal to every number here, as in JS's comparison)
                                Ty::F32 | Ty::F64 => "zs_v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));".to_string(),
                                _ => "zs_v.sort();".to_string(),
                            },
                            _ => {
                                let p = if self.copy(&el) { (None, "&") } else { (Some(None), "") };
                                let f = self.closure(&args[0], &[p.clone(), p])?;
                                self.str_helpers.insert("zs_sort_by");
                                format!("zs_sort_by(&mut zs_v, {f});")
                            }
                        };
                        val(format!("{{ let mut zs_v = {v}; {how} zs_v }}"))
                    }
                    "includes" | "index_of" => {
                        let (x, xf) = self.ex(&args[0])?;
                        let x = if self.copy(&el) { x } else { self.owned(x, &xf, &el) };
                        self.str_helpers.insert("zs_position");
                        let pos = format!("zs_position(&{recv}, &{x})");
                        val(if m == "includes" {
                            format!("({pos} >= 0)")
                        } else {
                            format!("{{ let zs_i = {pos}; let zs_o: {oty} = if zs_i >= 0 {{ {opt}::Some(zs_i) }} else {{ {opt}::None }}; zs_o }}")
                        })
                    }
                    "find" => {
                        let param = if elems.is_none() && range.is_none() && !self.copy(&el) { (Some(None), "") } else { k("&") };
                        let f = self.closure(&args[0], &[param])?;
                        val(some(if by_ref { format!("{it}.find({f}).cloned()") } else { format!("{it}.find({f})") }))
                    }
                    "find_index" => val(some(format!("{it}.position({}).map(|zs_i| zs_i as i32)", self.closure(&args[0], &[k("")])?))),
                    _ => val(format!("{it}.flat_map({}).collect::<Vec<_>>()", self.closure(&args[0], &[k("")])?)),
                }
            }
            _ => not_in_subset(pos, "this method (strings are not in it)"),
        }
    }

    /// The Rust type of the prelude's `Option` of `t`, marked used (its definition is written once), with its type
    /// argument written out (zustc does not infer it from a `None` branch)
    fn option_type(&mut self, t: &Ty) -> (String, String) {
        self.used.insert("Option".into());
        let a = self.rty(t, Pos::default(), None).unwrap_or_default();
        (rs_type("Option"), format!("{}<{a}>", rs_type("Option")))
    }

    /// `range(a, b)`: its bounds in Rust
    fn range_args(&mut self, e: &Expr) -> LResult<Option<(String, String)>> {
        if let EK::Call(f, args) = &e.kind
            && matches!(self.info.callee.get(&f.id), Some(Callee::Range))
        {
            return Ok(Some((self.expr(&args[0])?, self.expr(&args[1])?)));
        }
        Ok(None)
    }

    fn braced(s: String) -> String {
        if s.starts_with('{') { s } else { format!("{{ {s} }}") }
    }

    fn block(&mut self, e: &Expr) -> LResult<String> {
        let s = self.scoped(|l| l.expr(e))?;
        Ok(Self::braced(s))
    }
}
