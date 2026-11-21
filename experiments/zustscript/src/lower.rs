//! `@wasm fn`: a zustscript function lowered to Rust, which zustc compiles to wasm. The body is checked by the
//! same checker as the JS layer, so the same source can run as JS (`zsc --all-js`) or as wasm, with the same
//! results: the Rust keeps the JS layer's meaning of numbers (i32 / u32 / i64 wrap: `wrapping_*`; `/` and `%`
//! fail on zero; `as` saturates from f64, as Rust's `as` does).
//!
//! The subset (anything else is an error naming the construct):
//! - types: i32 u32 i64 f64 bool, and `Array<T>` of them (a parameter is `&[T]`, a result `Vec<T>`);
//! - `let`, `if`, `match` on numbers and bools, arithmetic, comparisons, `as`, `T::zero()` / `T::one()`;
//! - calls of other `@wasm` functions and of `wasm fn`s (Rust), recursion;
//! - on arrays: `len` `map` `filter` `fold` `any` `all` `push` `concat` and array literals; a closure only as
//!   the argument of one of those (it becomes a Rust closure, the call an iterator chain);
//! - f64's `sqrt` `abs` `floor` `round`.
//!
//! A numeric-bound function (`T: Num`) is lowered once per instance, as in the JS output (`sum$f64` → Rust
//! `zs_sum__f64`). Only the instances the JS layer calls get a wrapper and glue; the ones only other lowered
//! functions call are plain Rust functions.

use crate::ast::*;
use crate::check::{Builtin, Callee, Info, Ty};
use crate::js::inst_name;
use crate::wasm::WFn;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

const RUST_KW: &[&str] = &[
    "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "priv", "pub", "ref", "return", "self", "Self",
    "static", "struct", "super", "trait", "true", "try", "type", "typeof", "union", "unsafe", "unsized", "use", "virtual",
    "where", "while", "yield",
];

fn local(n: &str) -> String {
    if RUST_KW.contains(&n) { format!("{n}_") } else { n.to_string() }
}

/// The Rust name of an instance: `zs_sum__f64`
fn rs_name(name: &str, targs: &[Ty]) -> String {
    format!("zs_{}", inst_name(name, targs).replace('$', "__"))
}

fn subst_ty(t: &Ty, m: &HashMap<String, Ty>) -> Ty {
    match t {
        Ty::Param(p) => m.get(p).cloned().unwrap_or(t.clone()),
        Ty::Array(a) => Ty::Array(Box::new(subst_ty(a, m))),
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
    Err((pos, format!("{what} is not in the @wasm subset (numbers, bools, arrays of numbers, records and enums of them, let / let mut / if / match / loops, map / filter / fold)")))
}

pub struct Lower<'a> {
    info: &'a Info,
    fns: HashMap<&'a str, &'a Item>,
    subst: HashMap<String, Ty>,
    /// locals that hold a borrowed slice (a parameter, or a `let` of one): returned with `.to_vec()`
    slices: HashSet<String>,
    queue: Vec<(String, Vec<Ty>)>,
    done: HashSet<String>,
    /// the records and enums the lowered code uses (their Rust definitions are written once, generic as declared)
    used: std::collections::BTreeSet<String>,
    /// where each record and enum is declared (for errors in their definitions)
    type_pos: HashMap<String, Pos>,
    /// the helper functions used (`zs_max_f32` ..)
    helpers: std::collections::BTreeSet<String>,
}

/// The Rust name of a record or enum: `ZsPoint`
fn rs_type(n: &str) -> String {
    format!("Zs{n}")
}

/// Lowers the wanted `@wasm` instances (those the JS layer calls, and the public ones) and every instance they
/// call. Returns the ones that need glue, and the Rust of the others.
/// (`simd_as_wasm`: `@simd fn`s are lowered too: `zsc --no-simd`)
pub fn lower(items: &[Item], info: &Info, wanted: &[(String, Vec<Ty>)], simd_as_wasm: bool) -> Result<(Vec<WFn>, String), Vec<(Pos, String)>> {
    let fns = items
        .iter()
        .filter_map(|it| match it {
            Item::Fn { name, wasm, simd, .. } if *wasm || (*simd && simd_as_wasm) => Some((name.as_str(), it)),
            _ => None,
        })
        .collect();
    let mut l = Lower { info, fns, subst: HashMap::new(), slices: HashSet::new(), queue: vec![], done: HashSet::new(), used: Default::default(), type_pos: HashMap::new(), helpers: Default::default() };
    for it in items {
        if let Item::Record { name, pos, .. } | Item::Enum { name, pos, .. } = it {
            l.type_pos.insert(name.clone(), *pos);
        }
    }
    let mut errors = vec![];
    let mut out = vec![];
    let wanted_names: HashSet<String> = wanted.iter().map(|(n, t)| rs_name(n, t)).collect();
    let mut internal = String::new();
    for (n, t) in wanted {
        l.want(n, t.clone());
    }
    while let Some((name, targs)) = l.queue.pop() {
        let rs = rs_name(&name, &targs);
        match l.function(&name, &targs) {
            Ok((text, args, ret)) => {
                if wanted_names.contains(&rs) {
                    let Item::Fn { public, pos, .. } = l.fns[name.as_str()] else { unreachable!() };
                    match l.boundary(&args, &ret, *pos) {
                        Ok((wargs, wret)) => out.push(WFn { js: inst_name(&name, &targs), rs, args: wargs, ret: wret, text, public: *public }),
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
        let todo: Vec<String> = l.used.iter().filter(|n| !written.contains(*n)).cloned().collect();
        if todo.is_empty() {
            break;
        }
        for n in todo {
            written.insert(n.clone());
            match l.type_def(&n) {
                Ok(d) => defs.push_str(&d),
                Err(e) => errors.push(e),
            }
        }
    }
    for h in &l.helpers {
        // zs_max_f32 / zs_min_f64 ..
        let (op, t) = h.trim_start_matches("zs_").split_once('_').unwrap();
        let (pick, zero) = if op == "max" { ("a > b", "a.is_sign_positive()") } else { ("a < b", "a.is_sign_negative()") };
        let _ = write!(
            defs,
            "fn {h}(a: {t}, b: {t}) -> {t} {{\n    if a.is_nan() || b.is_nan() {{ {t}::NAN }} else if a == b {{ if {zero} {{ a }} else {{ b }} }} else if {pick} {{ a }} else {{ b }}\n}}\n\n"
        );
    }
    if errors.is_empty() { Ok((out, defs + &internal)) } else { Err(errors) }
}

impl Lower<'_> {
    /// The Rust definition of record or enum `n`: Copy, so that it is passed and returned by value like a number
    fn type_def(&mut self, n: &str) -> LResult<String> {
        let pos = self.type_pos.get(n).copied().unwrap_or_default();
        let mut s = String::from("#[derive(Clone, Copy, PartialEq)]\n");
        if let Some(r) = self.info.records.get(n) {
            let g = if r.params.is_empty() { String::new() } else { format!("<{}>", r.params.join(", ")) };
            let _ = writeln!(s, "struct {}{g} {{", rs_type(n));
            for (f, t) in r.fields.clone() {
                let _ = writeln!(s, "    {}: {},", local(&f), self.field_ty(&t, n, pos)?);
            }
            s.push_str("}\n\n");
        } else {
            let e = &self.info.enums[n];
            let g = if e.params.is_empty() { String::new() } else { format!("<{}>", e.params.join(", ")) };
            let _ = writeln!(s, "enum {}{g} {{", rs_type(n));
            for (v, fs) in e.variants.clone() {
                if fs.is_empty() {
                    let _ = writeln!(s, "    {v},");
                } else {
                    let ts = fs.iter().map(|t| self.field_ty(t, n, pos)).collect::<LResult<Vec<_>>>()?;
                    let _ = writeln!(s, "    {v}({}),", ts.join(", "));
                }
            }
            s.push_str("}\n\n");
        }
        Ok(s)
    }

    /// A field's type: a scalar, a record or enum, or a type parameter (an array would not be Copy)
    fn field_ty(&mut self, t: &Ty, owner: &str, pos: Pos) -> LResult<String> {
        match t {
            Ty::Param(p) => Ok(p.clone()),
            Ty::Array(_) => not_in_subset(pos, &format!("`{owner}` (a record or enum with an array field)")),
            t => self.rty(t, pos, false),
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
                let rt = self.rty(t, pos, false)?;
                if let Some(r) = self.info.records.get(n) {
                    let m: HashMap<String, Ty> = r.params.iter().cloned().zip(targs.iter().cloned()).collect();
                    let fields: Vec<(String, Ty)> = r.fields.iter().map(|(f, ft)| (f.clone(), subst_ty(ft, &m))).collect();
                    let fs = fields.iter().map(|(f, ft)| Ok((f.clone(), self.vty(ft, pos)?))).collect::<LResult<Vec<_>>>()?;
                    Ok(VTy::Record(rt, fs))
                } else {
                    let e = &self.info.enums[n];
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
            _ => Err((pos, format!("{} cannot be inside a record or enum that crosses to wasm", show(t)))),
        }
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
        if let Ty::Named(..) = t {
            return Ok(WTy::Value(Box::new(self.vty(t, pos)?)));
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

    fn rty(&mut self, t: &Ty, pos: Pos, param: bool) -> LResult<String> {
        if let Some(s) = scalar(t) {
            return Ok(s.into());
        }
        match t {
            // (in a generic record's or enum's definition)
            Ty::Param(p) => Ok(p.clone()),
            Ty::Named(n, targs) if self.info.records.contains_key(n) || self.info.enums.get(n).is_some_and(|e| !e.is_effect()) => {
                self.used.insert(n.clone());
                if targs.is_empty() {
                    Ok(rs_type(n))
                } else {
                    let a = targs.iter().map(|t| self.rty(t, pos, false)).collect::<LResult<Vec<_>>>()?;
                    Ok(format!("{}<{}>", rs_type(n), a.join(", ")))
                }
            }
            Ty::Array(e) | Ty::Typed(e) => match scalar(e) {
                Some(s) if s != "()" => Ok(if param { format!("&[{s}]") } else { format!("Vec<{s}>") }),
                _ => not_in_subset(pos, &format!("an array of {}", show(e))),
            },
            _ => not_in_subset(pos, &format!("the type {}", show(t))),
        }
    }

    /// One instance: (its Rust text, its parameters' and result's types)
    fn function(&mut self, name: &str, targs: &[Ty]) -> LResult<(String, Vec<(String, Ty)>, Ty)> {
        let Item::Fn { args, body, pos, generics, bounds, .. } = self.fns[name] else { unreachable!() };
        if !self.info.effects[name].is_empty() {
            return Err((*pos, format!("a @wasm function cannot perform effects ({:?})", self.info.effects[name])));
        }
        for g in generics {
            if !bounds.iter().any(|b| b.0 == *g) {
                return Err((*pos, format!("the generic parameter `{g}` of a @wasm function needs a numeric bound (`{g}: Num`)")));
            }
        }
        self.subst = generics.iter().cloned().zip(targs.iter().cloned()).collect();
        let sig = &self.info.fns[name];
        let ats: Vec<Ty> = sig.args.iter().map(|t| subst_ty(t, &self.subst)).collect();
        let ret = subst_ty(&sig.ret, &self.subst);
        self.slices.clear();
        let mut ps = vec![];
        for ((a, _), t) in args.iter().zip(&ats) {
            ps.push(format!("{}: {}", local(a), self.rty(t, *pos, true)?));
            if matches!(t, Ty::Array(_) | Ty::Typed(_)) {
                self.slices.insert(a.clone());
            }
        }
        let rt = self.rty(&ret, *pos, false)?;
        let b = self.ret_expr(body, matches!(ret, Ty::Array(_) | Ty::Typed(_)))?;
        let text = format!("fn {}({}) -> {rt} {b}", rs_name(name, targs), ps.join(", "));
        let named = args.iter().map(|a| a.0.clone()).zip(ats).collect();
        Ok((text, named, ret))
    }

    /// An expression in a result position: an array result must be owned (`xs.to_vec()` for a borrowed one)
    fn ret_expr(&mut self, e: &Expr, array: bool) -> LResult<String> {
        if !array {
            return self.expr(e);
        }
        match &e.kind {
            EK::Var(n) if self.slices.contains(n) => Ok(format!("{}.to_vec()", local(n))),
            EK::Block(stmts, Some(t)) => {
                let mut s = String::from("{\n");
                for st in stmts {
                    s.push_str(&self.stmt(st)?);
                }
                let _ = writeln!(s, "{}\n}}", self.ret_expr(t, true)?);
                Ok(s)
            }
            EK::If(c, t, Some(el)) => Ok(format!("if {} {} else {}", self.expr(c)?, self.block_ret(t)?, self.block_ret(el)?)),
            EK::Match(sc, arms) => self.match_(sc, arms, true),
            _ => self.expr(e),
        }
    }

    fn block_ret(&mut self, e: &Expr) -> LResult<String> {
        let s = self.ret_expr(e, true)?;
        Ok(if s.starts_with('{') || s.starts_with("if ") { s } else { format!("{{ {s} }}") })
    }

    fn stmt(&mut self, st: &Stmt) -> LResult<String> {
        match st {
            Stmt::Let(Pat::Bind(n), _, x) => {
                if matches!(&x.kind, EK::Var(v) if self.slices.contains(v)) {
                    self.slices.insert(n.clone());
                } else {
                    self.slices.remove(n);
                }
                Ok(format!("let {} = {};\n", local(n), self.expr(x)?))
            }
            Stmt::Let(Pat::Wild, _, x) => Ok(format!("let _ = {};\n", self.expr(x)?)),
            Stmt::LetMut(n, _, x) => {
                self.slices.remove(n);
                Ok(format!("let mut {} = {};\n", local(n), self.owned(x)?))
            }
            Stmt::Let(_, _, x) => not_in_subset(x.pos, "a destructuring `let`"),
            Stmt::Expr(x) => Ok(format!("{};\n", self.expr(x)?)),
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

    /// An argument for a parameter of type `t` (an array is passed borrowed)
    fn arg(&mut self, x: &Expr, array: bool) -> LResult<String> {
        let s = self.expr(x)?;
        Ok(if !array {
            s
        } else if matches!(&x.kind, EK::Var(n) if self.slices.contains(n)) {
            s
        } else if matches!(&x.kind, EK::Var(_)) {
            format!("&{s}[..]")
        } else {
            format!("&({s})[..]")
        })
    }

    fn closure(&mut self, f: &Expr, by_ref: &[bool]) -> LResult<String> {
        let EK::Closure(ps, body) = &f.kind else {
            return not_in_subset(f.pos, "a function value (pass a closure `(x) => ..` here)");
        };
        let ps: Vec<String> = ps.iter().zip(by_ref).map(|((n, _), r)| format!("{}{}", if *r { "&" } else { "" }, local(n))).collect();
        Ok(format!("|{}| {}", ps.join(", "), self.expr(body)?))
    }

    fn match_(&mut self, sc: &Expr, arms: &[(Pat, Expr)], array: bool) -> LResult<String> {
        let mut s = format!("match {} {{\n", self.expr(sc)?);
        for (p, body) in arms {
            let ps = self.pat(p, body.pos)?;
            let b = if array { self.ret_expr(body, true)? } else { self.expr(body)? };
            let _ = writeln!(s, "{ps} => {b},");
        }
        s.push('}');
        Ok(s)
    }

    fn pat(&mut self, p: &Pat, pos: Pos) -> LResult<String> {
        Ok(match p {
            Pat::Wild => "_".to_string(),
            Pat::Bind(n) => local(n),
            Pat::Lit(Lit::Int(n)) => n.to_string(),
            Pat::Lit(Lit::Bool(b)) => b.to_string(),
            Pat::Variant(_, v, subs, _) => {
                let en = &self.info.variant_of[v];
                self.used.insert(en.clone());
                let ps = subs.iter().map(|p| self.pat(p, pos)).collect::<LResult<Vec<_>>>()?;
                if ps.is_empty() { format!("{}::{v}", rs_type(en)) } else { format!("{}::{v}({})", rs_type(en), ps.join(", ")) }
            }
            _ => return not_in_subset(pos, "this pattern (a tuple or a float)"),
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

    fn expr(&mut self, e: &Expr) -> LResult<String> {
        let pos = e.pos;
        Ok(match &e.kind {
            EK::Lit(l) => self.lit(l, e.id, pos)?,
            EK::Var(n) | EK::Path(_, n) if matches!(self.info.callee.get(&e.id), Some(Callee::Variant(_))) => {
                let Some(Callee::Variant(en)) = self.info.callee.get(&e.id).cloned() else { unreachable!() };
                if self.info.enums[&en].variants.iter().any(|x| x.0 == *n && !x.1.is_empty()) {
                    return not_in_subset(pos, "a variant constructor as a function value");
                }
                self.variant(&en, n, vec![], pos)?
            }
            EK::Var(n) => match self.info.callee.get(&e.id) {
                Some(Callee::Local) => local(n),
                _ => return not_in_subset(pos, &format!("`{n}` as a value")),
            },
            EK::Call(f, args) => {
                if let EK::Path(_, c) = &f.kind
                    && (c == "zero" || c == "one")
                {
                    let t = self.num(f.id);
                    return Ok(match t {
                        Ty::F64 => (if c == "one" { "1.0f64" } else { "0.0f64" }).into(),
                        Ty::F32 => (if c == "one" { "1.0f32" } else { "0.0f32" }).into(),
                        t => format!("{}{}", if c == "one" { 1 } else { 0 }, scalar(&t).unwrap_or("")),
                    });
                }
                if let EK::Var(n) | EK::Path(_, n) = &f.kind
                    && let Some(Callee::Variant(en)) = self.info.callee.get(&f.id).cloned()
                {
                    let a = args.iter().map(|x| self.expr(x)).collect::<LResult<Vec<_>>>()?;
                    return self.variant(&en, n, a, pos);
                }
                if let Some(Callee::TypedCtor(el, c)) = self.info.callee.get(&f.id).cloned() {
                    let a = self.expr(&args[0])?;
                    let zero = match el {
                        Ty::F32 => "0.0f32",
                        Ty::F64 => "0.0f64",
                        _ => "0i32",
                    };
                    return Ok(if c == "zeros" { format!("vec![{zero}; ({a}) as usize]") } else { format!("({a}).to_vec()") });
                }
                let EK::Var(n) = &f.kind else { return not_in_subset(pos, "calling this") };
                if let Some((a, b)) = self.range_args(e)? {
                    return Ok(format!("({a}..{b}).collect::<Vec<i32>>()"));
                }
                match self.info.callee.get(&f.id) {
                    Some(Callee::Fn) if self.fns.contains_key(n.as_str()) => {
                        let targs: Vec<Ty> = self.info.inst.get(&f.id).map(|t| t.iter().map(|t| subst_ty(t, &self.subst)).collect()).unwrap_or_default();
                        let ptys: Vec<bool> = self.info.fns[n].args.iter().map(|t| matches!(t, Ty::Array(_) | Ty::Typed(_))).collect();
                        let rs = self.want(n, targs);
                        let a = args.iter().zip(ptys).map(|(x, arr)| self.arg(x, arr)).collect::<LResult<Vec<_>>>()?;
                        format!("{rs}({})", a.join(", "))
                    }
                    Some(Callee::Fn) => return Err((pos, format!("`{n}` is a JS-layer function: a @wasm function calls only @wasm and `wasm fn` functions (mark `{n}` @wasm)"))),
                    Some(Callee::Wasm) => {
                        let (wargs, _) = &self.info.wasm[n];
                        let arrs: Vec<bool> = wargs.iter().map(|(_, w)| matches!(w, WTy::Slice(_))).collect();
                        if wargs.iter().any(|(_, w)| matches!(w, WTy::Str | WTy::Callback(..))) {
                            return not_in_subset(pos, "calling a `wasm fn` that takes a string or a callback");
                        }
                        let a = args.iter().zip(arrs).map(|(x, arr)| self.arg(x, arr)).collect::<LResult<Vec<_>>>()?;
                        format!("{n}({})", a.join(", "))
                    }
                    Some(Callee::Local) => return not_in_subset(pos, "calling a function value"),
                    _ => return not_in_subset(pos, &format!("calling `{n}`")),
                }
            }
            EK::Method(r, _, args) => {
                use Builtin as B;
                let Some(b) = self.info.builtin.get(&e.id).copied() else { return not_in_subset(pos, "this method") };
                // `range(a, b).fold(..)`: the iterator `a..b` directly, no array
                let it = match self.range_args(r)? {
                    Some((a, b)) => format!("({a}..{b})"),
                    None => {
                        let recv = self.expr(r)?;
                        let recv = if matches!(r.kind, EK::Var(_)) { recv } else { format!("({recv})") };
                        format!("{recv}.iter().copied()")
                    }
                };
                let recv = match self.range_args(r)? {
                    Some((a, b)) => format!("({a}..{b}).collect::<Vec<i32>>()"),
                    None => {
                        let recv = self.expr(r)?;
                        if matches!(r.kind, EK::Var(_)) { recv } else { format!("({recv})") }
                    }
                };
                match b {
                    B::Len => format!("({recv}.len() as i32)"),
                    B::Map => format!("{it}.map({}).collect::<Vec<_>>()", self.closure(&args[0], &[false])?),
                    B::Filter => format!("{it}.filter({}).collect::<Vec<_>>()", self.closure(&args[0], &[true])?),
                    B::Any => format!("{it}.any({})", self.closure(&args[0], &[false])?),
                    B::All => format!("{it}.all({})", self.closure(&args[0], &[false])?),
                    B::Fold => {
                        let init = self.expr(&args[0])?;
                        format!("{it}.fold({init}, {})", self.closure(&args[1], &[false, false])?)
                    }
                    B::Push => format!("{{ let mut v = {recv}.to_vec(); v.push({}); v }}", self.expr(&args[0])?),
                    B::Concat => format!("{{ let mut v = {recv}.to_vec(); v.extend_from_slice({}); v }}", self.arg(&args[0], true)?),
                    B::Sqrt => format!("{recv}.sqrt()"),
                    B::Abs => format!("{recv}.abs()"),
                    B::Max | B::Min => {
                        // (JS's Math.max / Math.min: NaN wins, +0 is above -0)
                        let t = scalar(&self.num(e.id)).unwrap_or("f64");
                        let h = format!("zs_{}_{t}", if b == B::Max { "max" } else { "min" });
                        self.helpers.insert(h.clone());
                        format!("{h}({recv}, {})", self.expr(&args[0])?)
                    }
                    B::ToArray => format!("{recv}.to_vec()"),
                    B::Floor => format!("{recv}.floor()"),
                    B::Round => format!("{recv}.round()"),
                    _ => return not_in_subset(pos, "this method (strings are not in it)"),
                }
            }
            EK::Bin(op, l, r) => {
                let (a, b) = (self.expr(l)?, self.expr(r)?);
                match *op {
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
                }
            }
            EK::Un("!", x) => format!("!({})", self.expr(x)?),
            EK::Un(_, x) => match self.num(e.id) {
                Ty::F64 | Ty::F32 => format!("-({})", self.expr(x)?),
                _ => format!("({}).wrapping_neg()", self.expr(x)?),
            },
            EK::Cast(x, _) => {
                let (_, to) = self.info.cast[&e.id].clone();
                let to = subst_ty(&to, &self.subst);
                format!("(({}) as {})", self.expr(x)?, scalar(&to).unwrap_or("f64"))
            }
            EK::If(c, t, el) => {
                let c = self.expr(c)?;
                let t = self.block(t)?;
                match el {
                    Some(x) => format!("if {c} {t} else {}", self.block(x)?),
                    None => format!("if {c} {t}"),
                }
            }
            EK::Match(sc, arms) => self.match_(sc, arms, false)?,
            EK::Block(stmts, tail) => {
                let mut s = String::from("{\n");
                for st in stmts {
                    s.push_str(&self.stmt(st)?);
                }
                if let Some(t) = tail {
                    let _ = writeln!(s, "{}", self.expr(t)?);
                }
                s.push('}');
                s
            }
            EK::Assign(n, x) => format!("{} = {}", local(n), self.owned(x)?),
            EK::While(c, body) => format!("while {} {}", self.expr(c)?, self.block(body)?),
            EK::For(x, it, body) => {
                let iter = match self.range_args(it)? {
                    Some((a, b)) => format!("({a})..({b})"),
                    None => {
                        let v = self.expr(it)?;
                        let v = if matches!(it.kind, EK::Var(_)) { v } else { format!("({v})") };
                        format!("{v}.iter().copied()")
                    }
                };
                format!("for {} in {iter} {}", local(x), self.block(body)?)
            }
            EK::Break => "break".into(),
            EK::Continue => "continue".into(),
            EK::Index(x, i) => {
                let a = self.expr(x)?;
                let a = if matches!(x.kind, EK::Var(_)) { a } else { format!("({a})") };
                // (a negative index becomes a huge usize: out of bounds, as in JS)
                format!("{a}[({}) as usize]", self.expr(i)?)
            }
            EK::Array(xs) => {
                let v = xs.iter().map(|x| self.expr(x)).collect::<LResult<Vec<_>>>()?;
                format!("vec![{}]", v.join(", "))
            }
            EK::Closure(..) => return not_in_subset(pos, "a closure outside map / filter / fold / any / all"),
            EK::Field(x, f) => {
                if f.parse::<usize>().is_ok() {
                    return not_in_subset(pos, "a tuple");
                }
                let s = self.expr(x)?;
                let s = if matches!(x.kind, EK::Var(_) | EK::Field(..)) { s } else { format!("({s})") };
                format!("{s}.{}", local(f))
            }
            EK::Record(n, fields, base) => {
                self.used.insert(n.clone());
                let mut parts = fields.iter().map(|(f, x)| Ok(format!("{}: {}", local(f), self.expr(x)?))).collect::<LResult<Vec<_>>>()?;
                if let Some(b) = base {
                    parts.push(format!("..{}", self.expr(b)?));
                }
                format!("{} {{ {} }}", rs_type(n), parts.join(", "))
            }
            EK::Tuple(_) => return not_in_subset(pos, "a tuple"),
            EK::Path(..) => return not_in_subset(pos, "this path"),
            EK::Jsx(_) => return not_in_subset(pos, "JSX"),
            EK::Perform(..) | EK::Handle(..) => return not_in_subset(pos, "an effect"),
        })
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

    /// A value to store in a mutable local: an array owned (a borrowed one copied)
    fn owned(&mut self, x: &Expr) -> LResult<String> {
        let s = self.expr(x)?;
        Ok(if matches!(&x.kind, EK::Var(v) if self.slices.contains(v)) { format!("{s}.to_vec()") } else { s })
    }

    fn block(&mut self, e: &Expr) -> LResult<String> {
        let s = self.expr(e)?;
        Ok(if s.starts_with('{') { s } else { format!("{{ {s} }}") })
    }
}
