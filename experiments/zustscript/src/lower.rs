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
    Err((pos, format!("{what} is not in the @wasm subset (numbers, bools, arrays of them, let / if / match, map / filter / fold)")))
}

pub struct Lower<'a> {
    info: &'a Info,
    fns: HashMap<&'a str, &'a Item>,
    subst: HashMap<String, Ty>,
    /// locals that hold a borrowed slice (a parameter, or a `let` of one): returned with `.to_vec()`
    slices: HashSet<String>,
    queue: Vec<(String, Vec<Ty>)>,
    done: HashSet<String>,
}

/// Lowers the wanted `@wasm` instances (those the JS layer calls, and the public ones) and every instance they
/// call. Returns the ones that need glue, and the Rust of the others.
pub fn lower(items: &[Item], info: &Info, wanted: &[(String, Vec<Ty>)]) -> Result<(Vec<WFn>, String), Vec<(Pos, String)>> {
    let fns = items
        .iter()
        .filter_map(|it| match it {
            Item::Fn { name, wasm: true, .. } => Some((name.as_str(), it)),
            _ => None,
        })
        .collect();
    let mut l = Lower { info, fns, subst: HashMap::new(), slices: HashSet::new(), queue: vec![], done: HashSet::new() };
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
                    match boundary(&args, &ret, *pos) {
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
    if errors.is_empty() { Ok((out, internal)) } else { Err(errors) }
}

/// The boundary types of a lowered function's signature (what its glue converts)
fn boundary(args: &[(String, Ty)], ret: &Ty, pos: Pos) -> LResult<(Vec<(String, WTy)>, WTy)> {
    let w = |t: &Ty, arg: bool| -> LResult<WTy> {
        Ok(match t {
            Ty::I32 => WTy::I32,
            Ty::U32 => WTy::U32,
            Ty::I64 => WTy::I64,
            Ty::F64 => WTy::F64,
            Ty::Bool => WTy::Bool,
            Ty::Unit => WTy::Unit,
            Ty::Array(e) if scalar(e).and_then(Elem::parse).is_some() => {
                let el = scalar(e).and_then(Elem::parse).unwrap();
                if arg { WTy::Slice(el) } else { WTy::Vec(el) }
            }
            _ => return Err((pos, format!("{} cannot cross from JS to a @wasm function yet (numbers, bools, arrays of numbers)", show(t)))),
        })
    };
    let a = args.iter().map(|(n, t)| Ok((n.clone(), w(t, true)?))).collect::<LResult<Vec<_>>>()?;
    Ok((a, w(ret, false)?))
}

impl<'a> Lower<'a> {
    fn want(&mut self, name: &str, targs: Vec<Ty>) -> String {
        let rs = rs_name(name, &targs);
        if self.done.insert(rs.clone()) {
            self.queue.push((name.to_string(), targs));
        }
        rs
    }

    fn rty(&self, t: &Ty, pos: Pos, param: bool) -> LResult<String> {
        if let Some(s) = scalar(t) {
            return Ok(s.into());
        }
        match t {
            Ty::Array(e) => match scalar(e) {
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
            if matches!(t, Ty::Array(_)) {
                self.slices.insert(a.clone());
            }
        }
        let rt = self.rty(&ret, *pos, false)?;
        let b = self.ret_expr(body, matches!(ret, Ty::Array(_)))?;
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
            Lit::Float(f) => format!("{f}f64"),
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
            let ps = match p {
                Pat::Wild => "_".to_string(),
                Pat::Bind(n) => local(n),
                Pat::Lit(Lit::Int(n)) => n.to_string(),
                Pat::Lit(Lit::Bool(b)) => b.to_string(),
                _ => return not_in_subset(body.pos, "this pattern"),
            };
            let b = if array { self.ret_expr(body, true)? } else { self.expr(body)? };
            let _ = writeln!(s, "{ps} => {b},");
        }
        s.push('}');
        Ok(s)
    }

    fn expr(&mut self, e: &Expr) -> LResult<String> {
        let pos = e.pos;
        Ok(match &e.kind {
            EK::Lit(l) => self.lit(l, e.id, pos)?,
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
                        t => format!("{}{}", if c == "one" { 1 } else { 0 }, scalar(&t).unwrap_or("")),
                    });
                }
                let EK::Var(n) = &f.kind else { return not_in_subset(pos, "calling this") };
                if let Some((a, b)) = self.range_args(e)? {
                    return Ok(format!("({a}..{b}).collect::<Vec<i32>>()"));
                }
                match self.info.callee.get(&f.id) {
                    Some(Callee::Fn) if self.fns.contains_key(n.as_str()) => {
                        let targs: Vec<Ty> = self.info.inst.get(&f.id).map(|t| t.iter().map(|t| subst_ty(t, &self.subst)).collect()).unwrap_or_default();
                        let ptys: Vec<bool> = self.info.fns[n].args.iter().map(|t| matches!(t, Ty::Array(_))).collect();
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
                        Ty::F64 => format!("({a} {op} {b})"),
                        _ => return not_in_subset(pos, "string concatenation"),
                    },
                }
            }
            EK::Un("!", x) => format!("!({})", self.expr(x)?),
            EK::Un(_, x) => match self.num(e.id) {
                Ty::F64 => format!("-({})", self.expr(x)?),
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
            EK::Field(..) | EK::Record(..) => return not_in_subset(pos, "a record"),
            EK::Tuple(_) => return not_in_subset(pos, "a tuple"),
            EK::Path(..) => return not_in_subset(pos, "an enum"),
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

    fn block(&mut self, e: &Expr) -> LResult<String> {
        let s = self.expr(e)?;
        Ok(if s.starts_with('{') { s } else { format!("{{ {s} }}") })
    }
}
