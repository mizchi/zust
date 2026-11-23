//! The JS layer's type checker: signatures of top-level items are annotated, everything inside a body is inferred
//! by unification (HM without let-polymorphism; generic top-level functions are instantiated per use). The wasm
//! layer is not checked here: its `wasm fn` bodies are Rust, checked by zustc. Only their signatures enter this
//! layer, mapped to JS-layer types (`f64` → Float, `&[f64]` → Array<Float>, `&str` → String, ...).
//!
//! Numbers: i32 / u32 / i64 / f64 are distinct types and nothing converts implicitly (`x as f64`). An integer
//! literal takes the integer type its context asks for (i32 when nothing does); `1.5` is f64.
//!
//! Effects: an enum whose variants are written `Op(Args) -> R` is an effect. `perform Op(..)` has type R; which
//! effects a function performs (and has not handled) is inferred over the call graph ([`Checker::effects`]).
//!
//! What the emitter needs from the checker is kept in side tables keyed by expression id ([`Info`]).

use crate::ast::*;
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    I32,
    U32,
    I64,
    /// (JS: a number rounded with Math.fround after every operation, which is exactly f32 arithmetic)
    F32,
    F64,
    /// `F64Array` / `F32Array` / `I32Array`: a numeric array backed by a JS typed array (the element type inside)
    Typed(Box<Ty>),
    Str,
    Bool,
    Unit,
    Jsx,
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    Fn(Vec<Ty>, Box<Ty>),
    Named(String, Vec<Ty>),
    /// a generic parameter of the item being checked (rigid)
    Param(String),
    Var(u32),
}

/// What a name in an expression refers to
#[derive(Clone, Debug)]
pub enum Callee {
    Local,
    Fn,
    /// (the JS expression to call)
    Extern(String),
    Wasm,
    /// a variant constructor (enum name)
    Variant(String),
    /// the built-in `range(start, end)`: the i32s from start to end (exclusive)
    Range,
    /// `F64Array::zeros(n)` / `F64Array::from(xs)` (the element type, the constructor)
    TypedCtor(Ty, String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Builtin {
    Len,
    Map,
    Filter,
    Push,
    Concat,
    Join,
    Any,
    All,
    Fold,
    ToUpper,
    Trim,
    Contains,
    Split,
    ToString,
    Floor,
    Round,
    Sqrt,
    Abs,
    Max,
    Min,
    ToArray,
}

pub struct RecordDef {
    pub params: Vec<String>,
    pub fields: Vec<(String, Ty)>,
}

pub struct EnumDef {
    pub params: Vec<String>,
    pub variants: Vec<(String, Vec<Ty>)>,
    /// the result type of each operation, for an effect
    pub ops: HashMap<String, Ty>,
}

impl EnumDef {
    pub fn is_effect(&self) -> bool {
        !self.ops.is_empty()
    }
    /// No variant carries data: the enum is emitted as string literals
    pub fn unit_only(&self) -> bool {
        self.variants.iter().all(|v| v.1.is_empty())
    }
}

pub struct FnSig {
    pub generics: Vec<String>,
    /// numeric bounds of generic parameters (`T: Num`): the function is monomorphized
    pub bounds: HashMap<String, String>,
    pub args: Vec<Ty>,
    pub ret: Ty,
}

#[derive(Default)]
pub struct Info {
    pub callee: HashMap<Id, Callee>,
    pub builtin: HashMap<Id, Builtin>,
    /// the numeric type of each arithmetic operator, unary minus and integer literal (decided once the function is
    /// inferred); a cast's (from, to)
    pub num: HashMap<Id, Ty>,
    pub cast: HashMap<Id, (Ty, Ty)>,
    /// the effects each function performs and does not handle (empty: an ordinary function)
    pub effects: HashMap<String, BTreeSet<String>>,
    /// the type arguments at each use of a monomorphized (numeric-bound) function, in its generics' order
    pub inst: HashMap<Id, Vec<Ty>>,
    /// array literals that build a typed array (the element type): `let w: F32Array = [..]`
    pub typed_lit: HashMap<Id, Ty>,
    pub records: HashMap<String, RecordDef>,
    pub enums: HashMap<String, EnumDef>,
    /// variant name → enum name (variant names are unique in a file)
    pub variant_of: HashMap<String, String>,
    pub fns: HashMap<String, FnSig>,
    pub externs: HashMap<String, (FnSig, Option<String>, String)>,
    pub wasm: HashMap<String, (Vec<(String, WTy)>, WTy)>,
    /// effects whose handlers all resume in tail position (see [`direct_effects`]): in JS they are plain calls to the
    /// handler's arm, not generators
    pub direct: BTreeSet<String>,
    /// the type of each expression of a `@wasm` / `@simd` function (the lowering to Rust decides by it what is Copy,
    /// what is borrowed and what is moved). Not recorded for the JS layer, which does not need it
    pub ty: HashMap<Id, Ty>,
    /// `==` / `!=` that compare values with parts (arrays, records, enums with data, tuples): by content, in JS with
    /// `$eq`, in Rust with `derive(PartialEq)`. Values are immutable, so equal contents are equal values
    pub deep_eq: HashMap<Id, Ty>,
}

pub struct Checker {
    pub info: Info,
    subst: Vec<Option<Ty>>,
    scopes: Vec<HashMap<String, Ty>>,
    pub errors: Vec<(Pos, String)>,
    /// (not errors: the guidelines, e.g. i64 in the JS layer)
    pub warnings: Vec<(Pos, String)>,
    /// the function being checked runs as wasm (`@wasm` / `@simd`)
    in_wasm: bool,
    /// (checked when the function is done, once inference has decided the types)
    deferred: Vec<(Pos, Id, &'static str, Ty)>,
    generics: Vec<String>,
    /// type variables that only integer types may fill (from integer literals)
    intv: HashSet<u32>,
    /// (id, type) to resolve into `Info::num` when the function is done
    nums: Vec<(Id, Ty)>,
    casts: Vec<(Id, Pos, Ty, Ty)>,
    /// the numeric bounds of the function being checked
    bounds: HashMap<String, String>,
    insts: Vec<(Id, Vec<Ty>)>,
    /// type variables that only float types may fill (from float literals)
    floatv: HashSet<u32>,
    typed_lits: Vec<(Id, Ty)>,
    /// (id, type) of the expressions of a `@wasm` function, resolved into `Info::ty` when the function is done
    tys: Vec<(Id, Ty)>,
    /// [`Checker::eqable`]'s finished answers
    eq_memo: std::cell::RefCell<HashMap<Ty, bool>>,
    eq_low: std::cell::Cell<usize>,
    eq_pending: std::cell::RefCell<(Vec<Ty>, HashMap<Ty, usize>)>,
    bound_checks: Vec<(Pos, Ty, String)>,
    /// mutable locals: (scope index, name)
    muts: HashSet<(usize, String)>,
    /// the scope count where each enclosing closure starts (a mutable local below it cannot be captured)
    closure_bases: Vec<usize>,
    /// how many loops enclose the expression (in the current closure)
    loops: u32,
}

pub const BOUNDS: &[&str] = &["Num", "Integer", "Float"];

/// The numeric types a bound admits
pub fn bound_types(b: &str) -> Vec<Ty> {
    match b {
        "Integer" => vec![Ty::I32, Ty::U32, Ty::I64],
        "Float" => vec![Ty::F32, Ty::F64],
        _ => vec![Ty::I32, Ty::U32, Ty::I64, Ty::F32, Ty::F64],
    }
}

pub fn wty_to_ty(w: &WTy) -> Ty {
    match w {
        WTy::I32 => Ty::I32,
        WTy::U32 => Ty::U32,
        WTy::I64 => Ty::I64,
        WTy::F32 => Ty::F32,
        WTy::F64 => Ty::F64,
        WTy::Bool => Ty::Bool,
        WTy::Unit => Ty::Unit,
        WTy::Slice(e) | WTy::Vec(e) => Ty::Array(Box::new(elem_ty(*e))),
        WTy::TVec(e) => Ty::Typed(Box::new(elem_ty(*e))),
        WTy::Str | WTy::String => Ty::Str,
        WTy::Callback(a, r, _) => Ty::Fn(a.iter().map(wty_to_ty).collect(), Box::new(wty_to_ty(r))),
        // (only lowered `@wasm fn`s have these, and their types come from their zustscript signature)
        WTy::Value(_) => Ty::Unit,
    }
}

pub fn is_int(t: &Ty) -> bool {
    matches!(t, Ty::I32 | Ty::U32 | Ty::I64)
}

/// The number of nodes of `t`, counted up to `cap`
fn ty_size(t: &Ty, cap: usize) -> usize {
    let mut n = 1;
    let kids: Vec<&Ty> = match t {
        Ty::Array(a) | Ty::Typed(a) => vec![a],
        Ty::Tuple(ts) | Ty::Named(_, ts) => ts.iter().collect(),
        Ty::Fn(a, r) => a.iter().chain(std::iter::once(&**r)).collect(),
        _ => vec![],
    };
    for k in kids {
        if n >= cap {
            break;
        }
        n += ty_size(k, cap - n);
    }
    n
}

/// The size `subst_named(t, m)` would have (a parameter counts as the size of its argument, `sizes`), up to `cap`,
/// without building it
fn subst_size(t: &Ty, sizes: &HashMap<String, usize>, cap: usize) -> usize {
    if let Ty::Param(p) = t {
        if let Some(&n) = sizes.get(p) {
            return n.min(cap);
        }
    }
    let mut n = 1;
    let kids: Vec<&Ty> = match t {
        Ty::Array(a) | Ty::Typed(a) => vec![a],
        Ty::Tuple(ts) | Ty::Named(_, ts) => ts.iter().collect(),
        Ty::Fn(a, r) => a.iter().chain(std::iter::once(&**r)).collect(),
        _ => vec![],
    };
    for k in kids {
        if n >= cap {
            break;
        }
        n += subst_size(k, sizes, cap - n);
    }
    n
}

/// `t` with the generic parameters of a record or enum replaced
pub(crate) fn subst_named(t: &Ty, m: &HashMap<String, Ty>) -> Ty {
    match t {
        Ty::Param(p) => m.get(p).cloned().unwrap_or_else(|| t.clone()),
        Ty::Array(a) => Ty::Array(Box::new(subst_named(a, m))),
        Ty::Typed(a) => Ty::Typed(Box::new(subst_named(a, m))),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| subst_named(t, m)).collect()),
        Ty::Named(n, a) => Ty::Named(n.clone(), a.iter().map(|t| subst_named(t, m)).collect()),
        Ty::Fn(a, r) => Ty::Fn(a.iter().map(|t| subst_named(t, m)).collect(), Box::new(subst_named(r, m))),
        t => t.clone(),
    }
}

pub fn is_num(t: &Ty) -> bool {
    matches!(t, Ty::I32 | Ty::U32 | Ty::I64 | Ty::F32 | Ty::F64)
}

pub fn elem_ty(e: Elem) -> Ty {
    match e {
        Elem::I32 => Ty::I32,
        Elem::U32 => Ty::U32,
        Elem::I64 => Ty::I64,
        Elem::F32 => Ty::F32,
        Elem::F64 => Ty::F64,
    }
}

/// The typed-array types: (name, element type)
pub const TYPED: &[(&str, Ty)] = &[("F64Array", Ty::F64), ("F32Array", Ty::F32), ("I32Array", Ty::I32)];

pub fn typed_name(el: &Ty) -> &'static str {
    TYPED.iter().find(|t| t.1 == *el).map_or("F64Array", |t| t.0)
}

impl Checker {
    pub fn new() -> Checker {
        Checker {
            info: Info::default(),
            subst: vec![],
            scopes: vec![],
            errors: vec![],
            warnings: vec![],
            in_wasm: false,
            deferred: vec![],
            generics: vec![],
            intv: HashSet::new(),
            nums: vec![],
            casts: vec![],
            bounds: HashMap::new(),
            insts: vec![],
            floatv: HashSet::new(),
            typed_lits: vec![],
            tys: vec![],
            eq_memo: Default::default(),
            eq_low: std::cell::Cell::new(usize::MAX),
            eq_pending: Default::default(),
            bound_checks: vec![],
            muts: HashSet::new(),
            closure_bases: vec![],
            loops: 0,
        }
    }

    fn err(&mut self, pos: Pos, m: impl Into<String>) {
        self.errors.push((pos, m.into()));
    }

    fn fresh(&mut self) -> Ty {
        self.subst.push(None);
        Ty::Var(self.subst.len() as u32 - 1)
    }

    /// The type of a float literal: a variable that only f32 / f64 may fill (f64 when nothing decides)
    fn fresh_float(&mut self) -> Ty {
        let t = self.fresh();
        if let Ty::Var(v) = t {
            self.floatv.insert(v);
        }
        t
    }

    /// The type of an integer literal: a variable that only i32 / u32 / i64 may fill
    fn fresh_int(&mut self) -> Ty {
        let t = self.fresh();
        if let Ty::Var(v) = t {
            self.intv.insert(v);
        }
        t
    }

    pub fn resolve(&self, t: &Ty) -> Ty {
        match t {
            Ty::Var(v) => match &self.subst[*v as usize] {
                Some(t) => self.resolve(t),
                None => t.clone(),
            },
            Ty::Array(a) => Ty::Array(Box::new(self.resolve(a))),
            Ty::Typed(a) => Ty::Typed(Box::new(self.resolve(a))),
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| self.resolve(t)).collect()),
            Ty::Fn(a, r) => Ty::Fn(a.iter().map(|t| self.resolve(t)).collect(), Box::new(self.resolve(r))),
            Ty::Named(n, a) => Ty::Named(n.clone(), a.iter().map(|t| self.resolve(t)).collect()),
            o => o.clone(),
        }
    }

    fn occurs(&self, v: u32, t: &Ty) -> bool {
        match self.resolve(t) {
            Ty::Var(w) => v == w,
            Ty::Array(a) => self.occurs(v, &a),
            Ty::Tuple(ts) | Ty::Named(_, ts) => ts.iter().any(|t| self.occurs(v, t)),
            Ty::Fn(a, r) => a.iter().any(|t| self.occurs(v, t)) || self.occurs(v, &r),
            _ => false,
        }
    }

    fn unify_inner(&mut self, a: &Ty, b: &Ty) -> bool {
        let (a, b) = (self.shallow(a), self.shallow(b));
        match (&a, &b) {
            (Ty::Var(x), Ty::Var(y)) if x == y => true,
            (Ty::Var(x), t) | (t, Ty::Var(x)) => {
                if self.occurs(*x, t) {
                    return false;
                }
                if self.floatv.contains(x) {
                    match t {
                        Ty::Var(y) if self.intv.contains(y) => return false,
                        Ty::Var(y) => {
                            self.floatv.insert(*y);
                        }
                        Ty::F32 | Ty::F64 => {}
                        Ty::Param(p) if matches!(self.bounds.get(p).map(|s| s.as_str()), Some("Num" | "Float")) => {}
                        _ => return false,
                    }
                }
                if self.intv.contains(x) {
                    match t {
                        Ty::Var(y) if self.floatv.contains(y) => return false,
                        Ty::Var(y) => {
                            self.intv.insert(*y);
                        }
                        t if is_int(t) => {}
                        Ty::Param(p) if matches!(self.bounds.get(p).map(|s| s.as_str()), Some("Num" | "Integer")) => {}
                        _ => return false,
                    }
                }
                self.subst[*x as usize] = Some(t.clone());
                true
            }
            (Ty::Array(x), Ty::Array(y)) | (Ty::Typed(x), Ty::Typed(y)) => self.unify_inner(x, y),
            (Ty::Tuple(xs), Ty::Tuple(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| self.unify_inner(x, y)),
            (Ty::Named(n, xs), Ty::Named(m, ys)) => n == m && xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| self.unify_inner(x, y)),
            (Ty::Fn(xa, xr), Ty::Fn(ya, yr)) => {
                xa.len() == ya.len() && xa.iter().zip(ya).all(|(x, y)| self.unify_inner(x, y)) && self.unify_inner(xr, yr)
            }
            _ => a == b,
        }
    }

    fn shallow(&self, t: &Ty) -> Ty {
        match t {
            Ty::Var(v) => match &self.subst[*v as usize] {
                Some(t) => self.shallow(t),
                None => t.clone(),
            },
            o => o.clone(),
        }
    }

    /// Unifies `got` with `want`; on a mismatch reports "expected want, found got"
    fn unify(&mut self, pos: Pos, got: &Ty, want: &Ty) {
        if !self.unify_inner(got, want) {
            let (g, w) = (self.show(got), self.show(want));
            self.err(pos, format!("type mismatch: expected {w}, found {g}"));
        }
    }

    pub fn show(&self, t: &Ty) -> String {
        match self.resolve(t) {
            Ty::I32 => "i32".into(),
            Ty::U32 => "u32".into(),
            Ty::I64 => "i64".into(),
            Ty::F32 => "f32".into(),
            Ty::F64 => "f64".into(),
            Ty::Typed(e) => typed_name(&e).into(),
            Ty::Str => "String".into(),
            Ty::Bool => "bool".into(),
            Ty::Unit => "()".into(),
            Ty::Jsx => "Jsx".into(),
            Ty::Array(a) => format!("Array<{}>", self.show(&a)),
            Ty::Tuple(ts) => format!("({})", ts.iter().map(|t| self.show(t)).collect::<Vec<_>>().join(", ")),
            Ty::Fn(a, r) => format!("fn({}) -> {}", a.iter().map(|t| self.show(t)).collect::<Vec<_>>().join(", "), self.show(&r)),
            Ty::Named(n, a) if a.is_empty() => n,
            Ty::Named(n, a) => format!("{n}<{}>", a.iter().map(|t| self.show(t)).collect::<Vec<_>>().join(", ")),
            Ty::Param(p) => p,
            Ty::Var(v) if self.intv.contains(&v) => "{integer}".into(),
            Ty::Var(v) if self.floatv.contains(&v) => "{float}".into(),
            Ty::Var(v) => format!("?{v}"),
        }
    }

    /// A written type → Ty (`params`: the generic parameters in scope)
    fn ty_of(&mut self, t: &TyS, params: &[String]) -> Ty {
        match t {
            TyS::Unit => Ty::Unit,
            TyS::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| self.ty_of(t, params)).collect()),
            TyS::Fn(a, r) => Ty::Fn(a.iter().map(|t| self.ty_of(t, params)).collect(), Box::new(self.ty_of(r, params))),
            TyS::Name(n, args, pos) => {
                let args: Vec<Ty> = args.iter().map(|t| self.ty_of(t, params)).collect();
                let arity = |c: &mut Checker, want: usize| {
                    if args.len() != want {
                        c.err(*pos, format!("`{n}` takes {want} type argument(s), found {}", args.len()));
                    }
                };
                match n.as_str() {
                    "i32" => Ty::I32,
                    "u32" => Ty::U32,
                    "i64" => Ty::I64,
                    "f32" => Ty::F32,
                    "f64" => Ty::F64,
                    "F64Array" => Ty::Typed(Box::new(Ty::F64)),
                    "F32Array" => Ty::Typed(Box::new(Ty::F32)),
                    "I32Array" => Ty::Typed(Box::new(Ty::I32)),
                    "String" => Ty::Str,
                    "bool" => Ty::Bool,
                    "Jsx" => Ty::Jsx,
                    "Array" => {
                        arity(self, 1);
                        Ty::Array(Box::new(args.into_iter().next().unwrap_or(Ty::Unit)))
                    }
                    _ if params.contains(n) => Ty::Param(n.clone()),
                    _ => {
                        let want = if let Some(r) = self.info.records.get(n) {
                            r.params.len()
                        } else if let Some(e) = self.info.enums.get(n) {
                            e.params.len()
                        } else {
                            self.err(*pos, format!("unknown type `{n}`"));
                            return Ty::Unit;
                        };
                        arity(self, want);
                        Ty::Named(n.clone(), args)
                    }
                }
            }
        }
    }

    fn subst_params(t: &Ty, m: &HashMap<String, Ty>) -> Ty {
        match t {
            Ty::Param(p) => m.get(p).cloned().unwrap_or(t.clone()),
            Ty::Array(a) => Ty::Array(Box::new(Self::subst_params(a, m))),
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| Self::subst_params(t, m)).collect()),
            Ty::Fn(a, r) => Ty::Fn(a.iter().map(|t| Self::subst_params(t, m)).collect(), Box::new(Self::subst_params(r, m))),
            Ty::Named(n, a) => Ty::Named(n.clone(), a.iter().map(|t| Self::subst_params(t, m)).collect()),
            o => o.clone(),
        }
    }

    fn instantiate(&mut self, generics: &[String], ts: &[Ty]) -> Vec<Ty> {
        let m: HashMap<String, Ty> = generics.iter().map(|g| (g.clone(), self.fresh())).collect();
        ts.iter().map(|t| Self::subst_params(t, &m)).collect()
    }

    // ------------------------------------------------------------ items

    pub fn check_file(&mut self, items: &[Item]) {
        // types first (records and enums may refer to each other)
        for it in items {
            match it {
                Item::Record { name, params, pos, .. } => {
                    if self.info.records.contains_key(name) || self.info.enums.contains_key(name) {
                        self.err(*pos, format!("the type `{name}` is defined twice"));
                    }
                    self.info.records.insert(name.clone(), RecordDef { params: params.clone(), fields: vec![] });
                }
                Item::Enum { name, params, .. } => {
                    self.info.enums.insert(name.clone(), EnumDef { params: params.clone(), variants: vec![], ops: HashMap::new() });
                }
                _ => {}
            }
        }
        let mut names: HashMap<String, Pos> = HashMap::new();
        for it in items {
            match it {
                Item::Record { name, params, fields, .. } => {
                    let fs = fields.iter().map(|(f, t)| (f.clone(), self.ty_of(t, params))).collect();
                    self.info.records.get_mut(name).unwrap().fields = fs;
                }
                Item::Enum { name, params, variants, pos } => {
                    let mut vs = vec![];
                    let mut ops = HashMap::new();
                    for (v, ts, ret) in variants {
                        if let Some(prev) = self.info.variant_of.insert(v.clone(), name.clone()) {
                            self.err(*pos, format!("variant `{v}` is already a variant of `{prev}` (variant names are unique in a file)"));
                        }
                        vs.push((v.clone(), ts.iter().map(|t| self.ty_of(t, params)).collect()));
                        if let Some(r) = ret {
                            let r = self.ty_of(r, params);
                            ops.insert(v.clone(), r);
                        }
                    }
                    if !ops.is_empty() && (ops.len() != vs.len() || !params.is_empty()) {
                        self.err(*pos, format!("effect `{name}`: every variant is an operation `Op(..) -> R`, and an effect has no type parameters"));
                    }
                    let e = self.info.enums.get_mut(name).unwrap();
                    e.variants = vs;
                    e.ops = ops;
                }
                Item::Fn { name, generics, bounds, args, ret, pos, .. } => {
                    if let Some(p) = names.insert(name.clone(), *pos) {
                        self.err(*pos, format!("`{name}` is already defined at {p}"));
                    }
                    for (_, b) in bounds {
                        if !BOUNDS.contains(&b.as_str()) {
                            self.err(*pos, format!("unknown bound `{b}` (the numeric bounds are Num, Integer, Float)"));
                        }
                    }
                    let a = args.iter().map(|(_, t)| self.ty_of(t, generics)).collect();
                    let r = ret.as_ref().map(|t| self.ty_of(t, generics)).unwrap_or(Ty::Unit);
                    let bounds = bounds.iter().cloned().collect();
                    self.info.fns.insert(name.clone(), FnSig { generics: generics.clone(), bounds, args: a, ret: r });
                }
                Item::Extern { name, module, generics, args, ret, js, pos } => {
                    if let Some(p) = names.insert(name.clone(), *pos) {
                        self.err(*pos, format!("`{name}` is already defined at {p}"));
                    }
                    let a = args.iter().map(|t| self.ty_of(t, generics)).collect();
                    let r = self.ty_of(ret, generics);
                    self.info.externs.insert(name.clone(), (FnSig { generics: generics.clone(), bounds: HashMap::new(), args: a, ret: r }, module.clone(), js.clone()));
                }
                Item::WasmFn { name, args, ret, pos, .. } => {
                    if let Some(p) = names.insert(name.clone(), *pos) {
                        self.err(*pos, format!("`{name}` is already defined at {p}"));
                    }
                    self.info.wasm.insert(name.clone(), (args.clone(), ret.clone()));
                }
                Item::WasmRaw(_) | Item::ImportType { .. } => {}
            }
        }
        let mut errs = vec![];
        infer_effects(items, &mut self.info, &mut errs);
        self.errors.extend(errs);
        for it in items {
            if let Item::Fn { generics, args, body, pos, name, wasm, simd, .. } = it {
                self.in_wasm = *wasm || *simd;
                self.generics = generics.clone();
                self.scopes = vec![HashMap::new()];
                self.muts.clear();
                let sig = &self.info.fns[name];
                self.bounds = sig.bounds.clone();
                let (at, ret) = (sig.args.clone(), sig.ret.clone());
                for ((a, _), t) in args.iter().zip(at) {
                    self.scopes[0].insert(a.clone(), t);
                }
                let got = self.infer(body, Some(&ret));
                self.unify(body_pos(body, *pos), &got, &ret);
                self.finish_fn();
            }
        }
    }

    /// Can values of `t` be compared with `==` (by content): not functions, JSX or effects
    /// (`seen`: the instantiated types being checked, so a recursive type ends; a type that grows without repeating
    /// is refused past a depth, or once it is large, before its fields are substituted. Finished answers are kept in
    /// `eq_memo`, so a type shared by many fields is checked once)
    fn eqable(&self, t: &Ty, seen: &mut Vec<Ty>) -> bool {
        if let Some(r) = self.eq_memo.borrow().get(t) {
            return *r;
        }
        // (`eq_low`: the shallowest type still being checked that a cycle below reached. false is final. true
        // assumes the outer types it reached are comparable: it waits in `eq_pending`, with the depth it relies on,
        // until the outermost of them (the root of its cycle) finishes, and is kept when that one is true too, as
        // in Tarjan's algorithm. Another path to a waiting type reuses it and relies on the same depth)
        if let Some(&at) = self.eq_pending.borrow().1.get(t) {
            self.eq_low.set(self.eq_low.get().min(at));
            return true;
        }
        let depth = seen.len();
        let outer = self.eq_low.replace(usize::MAX);
        let mark = self.eq_pending.borrow().0.len();
        let r = self.eqable_inner(t, seen);
        let low = self.eq_low.get();
        let mut pending = self.eq_pending.borrow_mut();
        let (order, at) = &mut *pending;
        if !r {
            for d in order.drain(mark..) {
                at.remove(&d);
            }
            self.eq_memo.borrow_mut().insert(t.clone(), false);
        } else if low >= depth {
            let mut memo = self.eq_memo.borrow_mut();
            for d in order.drain(mark..) {
                at.remove(&d);
                memo.insert(d, true);
            }
            memo.insert(t.clone(), true);
        } else {
            // (what waits below this type relies on it, so on what it relies on)
            for d in &order[mark..] {
                let a = at.get_mut(d).unwrap();
                *a = (*a).min(low);
            }
            order.push(t.clone());
            at.insert(t.clone(), low);
        }
        drop(pending);
        self.eq_low.set(outer.min(low));
        r
    }

    fn eqable_inner(&self, t: &Ty, seen: &mut Vec<Ty>) -> bool {
        match t {
            // (a generic parameter compares only with a numeric bound: unbounded, it could be a function)
            Ty::Str | Ty::Bool | Ty::Unit => true,
            t if self.numeric(t) => true,
            Ty::Array(e) | Ty::Typed(e) => self.eqable(e, seen),
            Ty::Tuple(ts) => ts.iter().all(|t| self.eqable(t, seen)),
            Ty::Named(n, targs) => {
                if let Some(at) = seen.iter().position(|s| s == t) {
                    self.eq_low.set(self.eq_low.get().min(at));
                    return true;
                }
                if seen.len() > 64 || ty_size(t, 4096) >= 4096 {
                    return false;
                }
                let (params, fields): (Vec<String>, Vec<Ty>) = if let Some(r) = self.info.records.get(n) {
                    (r.params.clone(), r.fields.iter().map(|f| f.1.clone()).collect())
                } else if let Some(e) = self.info.enums.get(n).filter(|e| !e.is_effect()) {
                    (e.params.clone(), e.variants.iter().flat_map(|v| v.1.iter().cloned()).collect())
                } else {
                    return false;
                };
                // (a field that would be large once substituted is refused before it is built)
                let sizes: HashMap<String, usize> = params.iter().cloned().zip(targs.iter().map(|a| ty_size(a, 4096))).collect();
                if fields.iter().any(|f| subst_size(f, &sizes, 4096) >= 4096) {
                    return false;
                }
                let m: HashMap<String, Ty> = params.into_iter().zip(targs.iter().cloned()).collect();
                seen.push(t.clone());
                let ok = fields.iter().all(|f| self.eqable(&subst_named(f, &m), seen));
                seen.pop();
                ok
            }
            _ => false,
        }
    }

    fn finish_fn(&mut self) {
        // (comparability depends on this function's bounds: no answer from another function's)
        self.eq_memo.borrow_mut().clear();
        *self.eq_pending.borrow_mut() = Default::default();
        // integer literals nothing constrained are i32
        let mut intv: Vec<u32> = self.intv.iter().copied().collect();
        intv.sort();
        for v in intv {
            if let Ty::Var(w) = self.shallow(&Ty::Var(v)) {
                self.subst[w as usize] = Some(Ty::I32);
            }
        }
        // float literals: f64
        let mut floatv: Vec<u32> = self.floatv.iter().copied().collect();
        floatv.sort();
        for v in floatv {
            if let Ty::Var(w) = self.shallow(&Ty::Var(v)) {
                self.subst[w as usize] = Some(Ty::F64);
            }
        }
        for (id, t) in std::mem::take(&mut self.typed_lits) {
            let r = self.resolve(&t);
            self.info.typed_lit.insert(id, r);
        }
        for (id, t) in std::mem::take(&mut self.nums) {
            let r = self.resolve(&t);
            self.info.num.insert(id, r);
        }
        for (id, t) in std::mem::take(&mut self.tys) {
            let r = self.resolve(&t);
            self.info.ty.insert(id, r);
        }
        for (id, ts) in std::mem::take(&mut self.insts) {
            let r = ts.iter().map(|t| self.resolve(t)).collect();
            self.info.inst.insert(id, r);
        }
        for (pos, t, b) in std::mem::take(&mut self.bound_checks) {
            let r = self.resolve(&t);
            let ok = match &r {
                Ty::Param(p) => match (b.as_str(), self.bounds.get(p).map(|s| s.as_str())) {
                    (_, None) => false,
                    ("Num", Some(_)) => true,
                    (x, Some(y)) => x == y,
                },
                t => bound_types(&b).contains(t),
            };
            if matches!(r, Ty::Var(_)) {
                self.err(pos, "cannot infer the numeric type here; add an annotation");
            } else if !ok {
                let s = self.show(&r);
                self.err(pos, format!("{s} does not satisfy the bound `{b}`"));
            }
        }
        for (id, pos, from, to) in std::mem::take(&mut self.casts) {
            let (f, t) = (self.resolve(&from), self.resolve(&to));
            if !self.numeric(&f) || !self.numeric(&t) {
                let (a, b) = (self.show(&f), self.show(&t));
                self.err(pos, format!("`as` converts between i32, u32, i64 and f64, not {a} to {b}"));
            }
            self.info.cast.insert(id, (f, t));
        }
        let mut warned_i64 = false;
        for (pos, _id, what, t) in std::mem::take(&mut self.deferred) {
            let r = self.resolve(&t);
            // the guideline: in the JS layer an i64 is a BigInt (slow, allocating); i32 is the integer to use there
            if r == Ty::I64 && matches!(what, "num" | "add" | "div") && !self.in_wasm && !warned_i64 {
                warned_i64 = true;
                self.warnings.push((pos, "i64 arithmetic in the JS layer runs on BigInt (slow, allocates): use i32, or move this to a @wasm fn".into()));
            }
            let ok = match what {
                "num" | "div" => self.numeric(&r),
                "add" | "ord" => self.numeric(&r) || r == Ty::Str,
                "eq" => {
                    let primitive = match &r {
                        Ty::Str | Ty::Bool | Ty::Unit => true,
                        t if self.numeric(t) => true,
                        Ty::Named(n, _) => self.info.enums.get(n).is_some_and(|e| e.unit_only() && !e.is_effect()),
                        _ => false,
                    };
                    let ok = primitive || self.eqable(&r, &mut vec![]);
                    if !primitive && ok {
                        self.info.deep_eq.insert(_id, r.clone());
                    }
                    ok
                }
                "child" => match &r {
                    Ty::Jsx | Ty::Str => true,
                    t if self.numeric(t) => true,
                    Ty::Array(a) => matches!(**a, Ty::Jsx | Ty::Str),
                    _ => false,
                },
                _ => true,
            };
            if matches!(r, Ty::Var(_)) {
                self.err(pos, "cannot infer this type; add an annotation");
            } else if !ok {
                let s = self.show(&r);
                let m = match what {
                    "num" | "div" => format!("expected a number (i32, u32, i64, f64), found {s}"),
                    "add" => format!("`+` takes numbers or String, found {s}"),
                    "ord" => format!("cannot compare {s} with `<`"),
                    "eq" => format!("`==` compares values (numbers, strings, bools, and arrays, records, enums and tuples of them, by content), not {s}"),
                    _ => format!("a JSX child is Jsx, String, a number or an Array of Jsx / String, found {s}"),
                };
                self.err(pos, m);
            }
        }
    }

    /// A numeric type, or a generic parameter with a numeric bound
    fn numeric(&self, t: &Ty) -> bool {
        is_num(t) || matches!(t, Ty::Param(p) if self.bounds.contains_key(p))
    }

    fn lookup_idx(&self, n: &str) -> Option<(usize, Ty)> {
        self.scopes.iter().enumerate().rev().find_map(|(k, s)| s.get(n).map(|t| (k, t.clone())))
    }

    /// Leaves a scope: its locals' mutability goes with it (the index is reused by the next scope)
    fn pop_scope(&mut self) {
        let k = self.scopes.len() - 1;
        self.muts.retain(|(i, _)| *i != k);
        self.scopes.pop();
    }

    /// A use of local `n` (found in scope `idx`): a mutable local cannot be captured by a closure
    fn check_capture(&mut self, pos: Pos, idx: usize, n: &str) {
        if self.muts.contains(&(idx, n.to_string())) && self.closure_bases.last().is_some_and(|b| idx < *b) {
            self.err(pos, format!("a closure cannot capture the mutable local `{n}` (mutation stays local: copy it into a `let` first)"));
        }
    }

    // ------------------------------------------------------------ patterns

    fn check_pat(&mut self, p: &Pat, t: &Ty, pos: Pos, binds: &mut Vec<(String, Ty)>) {
        match p {
            Pat::Wild => {}
            Pat::Bind(n) => binds.push((n.clone(), t.clone())),
            Pat::Lit(l) => {
                let lt = self.lit_ty(l);
                self.unify(pos, &lt, t);
            }
            Pat::Tuple(ps) => {
                let ts: Vec<Ty> = ps.iter().map(|_| self.fresh()).collect();
                self.unify(pos, t, &Ty::Tuple(ts.clone()));
                for (p, t) in ps.iter().zip(&ts) {
                    self.check_pat(p, t, pos, binds);
                }
            }
            Pat::Variant(en, v, subs, vpos) => {
                let Some(e) = self.info.variant_of.get(v).cloned() else {
                    return self.err(*vpos, format!("unknown variant `{v}`"));
                };
                if let Some(en) = en
                    && *en != e
                {
                    return self.err(*vpos, format!("`{v}` is a variant of `{e}`, not `{en}`"));
                }
                let def = &self.info.enums[&e];
                let params = def.params.clone();
                let fields = def.variants.iter().find(|x| x.0 == *v).unwrap().1.clone();
                let mut all = fields.clone();
                all.extend(params.iter().map(|p| Ty::Param(p.clone())));
                let inst = self.instantiate(&params, &all);
                let (ftys, targs) = inst.split_at(fields.len());
                self.unify(*vpos, t, &Ty::Named(e.clone(), targs.to_vec()));
                if subs.len() != ftys.len() {
                    return self.err(*vpos, format!("`{v}` has {} field(s), the pattern has {}", ftys.len(), subs.len()));
                }
                for (s, ft) in subs.iter().zip(ftys) {
                    self.check_pat(s, ft, *vpos, binds);
                }
            }
        }
    }

    fn exhaustive(&mut self, pos: Pos, t: &Ty, pats: &[&Pat]) {
        let irrefutable = |p: &Pat| matches!(p, Pat::Wild | Pat::Bind(_));
        if pats.iter().any(|p| irrefutable(p)) {
            return;
        }
        match self.resolve(t) {
            Ty::Named(n, _) if self.info.enums.contains_key(&n) => {
                let missing: Vec<String> = self.info.enums[&n]
                    .variants
                    .iter()
                    .filter(|(v, _)| {
                        !pats.iter().any(|p| matches!(p, Pat::Variant(_, pv, subs, _) if pv == v && subs.iter().all(irrefutable)))
                    })
                    .map(|(v, _)| v.clone())
                    .collect();
                if !missing.is_empty() {
                    self.err(pos, format!("non-exhaustive match: `{}` not covered", missing.join("`, `")));
                }
            }
            Ty::Bool => {
                for b in [true, false] {
                    if !pats.iter().any(|p| matches!(p, Pat::Lit(Lit::Bool(x)) if *x == b)) {
                        self.err(pos, format!("non-exhaustive match: `{b}` not covered"));
                    }
                }
            }
            Ty::Tuple(_) if pats.iter().any(|p| matches!(p, Pat::Tuple(ps) if ps.iter().all(irrefutable))) => {}
            _ => self.err(pos, "non-exhaustive match: add a `_` arm"),
        }
    }

    // ------------------------------------------------------------ expressions

    fn infer(&mut self, e: &Expr, expect: Option<&Ty>) -> Ty {
        let t = self.infer_kind(e, expect);
        if self.in_wasm {
            self.tys.push((e.id, t.clone()));
        }
        t
    }

    fn infer_kind(&mut self, e: &Expr, expect: Option<&Ty>) -> Ty {
        let pos = e.pos;
        match &e.kind {
            EK::Lit(l) => {
                let t = self.lit_ty(l);
                if matches!(l, Lit::Int(_) | Lit::Float(_)) {
                    self.nums.push((e.id, t.clone()));
                }
                t
            }
            EK::Var(n) => {
                if let Some((idx, t)) = self.lookup_idx(n) {
                    self.check_capture(pos, idx, n);
                    self.info.callee.insert(e.id, Callee::Local);
                    return t;
                }
                if let Some(s) = self.info.fns.get(n) {
                    let (g, mut ts, bounds) = (s.generics.clone(), s.args.clone(), s.bounds.clone());
                    ts.push(s.ret.clone());
                    // (the generic parameters themselves go last, to learn what they were instantiated to)
                    let nts = ts.len();
                    ts.extend(g.iter().map(|p| Ty::Param(p.clone())));
                    let mut inst = self.instantiate(&g, &ts);
                    let targs = inst.split_off(nts);
                    let r = inst.pop().unwrap();
                    if !bounds.is_empty() {
                        for (p, t) in g.iter().zip(&targs) {
                            if let Some(b) = bounds.get(p) {
                                self.bound_checks.push((pos, t.clone(), b.clone()));
                            }
                        }
                        self.insts.push((e.id, targs));
                    }
                    self.info.callee.insert(e.id, Callee::Fn);
                    return Ty::Fn(inst, Box::new(r));
                }
                if let Some((s, _, js)) = self.info.externs.get(n) {
                    let (g, mut ts, js) = (s.generics.clone(), s.args.clone(), js.clone());
                    ts.push(s.ret.clone());
                    let mut inst = self.instantiate(&g, &ts);
                    let r = inst.pop().unwrap();
                    self.info.callee.insert(e.id, Callee::Extern(js));
                    return Ty::Fn(inst, Box::new(r));
                }
                if let Some((args, ret)) = self.info.wasm.get(n) {
                    let t = Ty::Fn(args.iter().map(|a| wty_to_ty(&a.1)).collect(), Box::new(wty_to_ty(ret)));
                    self.info.callee.insert(e.id, Callee::Wasm);
                    return t;
                }
                if self.info.variant_of.contains_key(n) {
                    return self.variant(e.id, pos, None, n);
                }
                if n == "range" {
                    self.info.callee.insert(e.id, Callee::Range);
                    return Ty::Fn(vec![Ty::I32, Ty::I32], Box::new(Ty::Array(Box::new(Ty::I32))));
                }
                self.err(pos, format!("unknown name `{n}`"));
                self.fresh()
            }
            EK::Path(tn, c) if c == "zero" || c == "one" => {
                // `T::zero()` / `i64::one()` (num-traits' constants)
                let t = match tn.as_str() {
                    "i32" => Ty::I32,
                    "u32" => Ty::U32,
                    "i64" => Ty::I64,
                    "f32" => Ty::F32,
                    "f64" => Ty::F64,
                    p if self.bounds.contains_key(p) => Ty::Param(p.to_string()),
                    _ => {
                        self.err(pos, format!("`{tn}::{c}` needs a numeric type or a parameter with a numeric bound"));
                        Ty::F64
                    }
                };
                self.nums.push((e.id, t.clone()));
                Ty::Fn(vec![], Box::new(t))
            }
            EK::Path(tn, c) if TYPED.iter().any(|t| t.0 == tn) => {
                let el = TYPED.iter().find(|t| t.0 == tn).unwrap().1.clone();
                let typed = Ty::Typed(Box::new(el.clone()));
                self.info.callee.insert(e.id, Callee::TypedCtor(el.clone(), c.clone()));
                match c.as_str() {
                    "zeros" => Ty::Fn(vec![Ty::I32], Box::new(typed)),
                    "from" => Ty::Fn(vec![Ty::Array(Box::new(el))], Box::new(typed)),
                    _ => {
                        self.err(pos, format!("`{tn}` has `{tn}::zeros(n)` and `{tn}::from(xs)` (or write a literal where the type says {tn})"));
                        self.fresh()
                    }
                }
            }
            EK::Path(en, v) => self.variant(e.id, pos, Some(en), v),
            EK::Call(f, args) => {
                let ft = self.infer(f, None);
                let mut ft = self.resolve(&ft);
                // (a callee whose type is not known yet: a function of these arguments)
                if let Ty::Var(_) = ft {
                    let fnt = Ty::Fn(args.iter().map(|_| self.fresh()).collect(), Box::new(self.fresh()));
                    self.unify(f.pos, &ft, &fnt);
                    ft = self.resolve(&fnt);
                }
                let Ty::Fn(params, ret) = ft else {
                    if !matches!(ft, Ty::Var(_)) {
                        let s = self.show(&ft);
                        self.err(pos, format!("{s} is not a function"));
                    }
                    for a in args {
                        self.infer(a, None);
                    }
                    return self.fresh();
                };
                if params.len() != args.len() {
                    self.err(pos, format!("expected {} argument(s), found {}", params.len(), args.len()));
                }
                for (a, p) in args.iter().zip(&params) {
                    let t = self.infer(a, Some(p));
                    self.unify(a.pos, &t, p);
                }
                *ret
            }
            EK::Method(recv, name, args) => {
                let rt = self.infer(recv, None);
                let mut rt = self.resolve(&rt);
                // (a float literal's other methods: f64, the default, unless something decided)
                if let Ty::Var(v) = rt
                    && self.floatv.contains(&v)
                    && name != "to_string"
                {
                    self.unify(pos, &rt, &Ty::F64);
                    rt = Ty::F64;
                }
                // `props.onPick(x)`: a call of a record's function-typed field
                if let Ty::Named(n, targs) = &rt
                    && let Some(r) = self.info.records.get(n)
                    && let Some((_, ft)) = r.fields.iter().find(|f| f.0 == *name)
                {
                    let m = r.params.iter().cloned().zip(targs.iter().cloned()).collect();
                    if let Ty::Fn(params, ret) = Self::subst_params(ft, &m) {
                        if params.len() != args.len() {
                            self.err(pos, format!("`{name}` takes {} argument(s), found {}", params.len(), args.len()));
                        }
                        for (a, p) in args.iter().zip(&params) {
                            let t = self.infer(a, Some(p));
                            self.unify(a.pos, &t, p);
                        }
                        return *ret;
                    }
                }
                let Some((params, ret, b)) = self.method_sig(&rt, name) else {
                    if matches!(rt, Ty::Var(_)) {
                        self.err(pos, format!("cannot infer the type of the receiver of `.{name}()`; add an annotation"));
                    } else {
                        let s = self.show(&rt);
                        self.err(pos, format!("no method `{name}` on {s}"));
                    }
                    for a in args {
                        self.infer(a, None);
                    }
                    return self.fresh();
                };
                self.info.builtin.insert(e.id, b);
                // (the float methods' JS depends on f32 / f64: Math.fround)
                if matches!(b, Builtin::Sqrt | Builtin::Max | Builtin::Min | Builtin::Abs | Builtin::Floor | Builtin::Round) {
                    self.nums.push((e.id, rt.clone()));
                }
                if params.len() != args.len() {
                    self.err(pos, format!("`.{name}()` takes {} argument(s), found {}", params.len(), args.len()));
                }
                // `range(0, n).map(..)` where the type says F32Array / ..: it builds that typed array
                let mut ret = ret;
                if b == Builtin::Map
                    && matches!(&recv.kind, EK::Call(f, _) if matches!(self.info.callee.get(&f.id), Some(Callee::Range)))
                    && let Some(Ty::Typed(el)) = expect.map(|t| self.resolve(t))
                {
                    if let Ty::Array(u) = &ret {
                        self.unify(pos, u, &el);
                    }
                    self.typed_lits.push((e.id, (*el).clone()));
                    ret = Ty::Typed(el);
                }
                for (a, p) in args.iter().zip(&params) {
                    let t = self.infer(a, Some(p));
                    self.unify(a.pos, &t, p);
                }
                ret
            }
            EK::Field(x, f) => {
                let t = self.infer(x, None);
                match self.resolve(&t) {
                    Ty::Named(n, targs) if self.info.records.contains_key(&n) => {
                        let r = &self.info.records[&n];
                        let Some((_, ft)) = r.fields.iter().find(|x| x.0 == *f) else {
                            self.err(pos, format!("`{n}` has no field `{f}`"));
                            return self.fresh();
                        };
                        let m = r.params.iter().cloned().zip(targs).collect();
                        Self::subst_params(ft, &m)
                    }
                    Ty::Tuple(ts) if f.parse::<usize>().is_ok_and(|k| k < ts.len()) => ts[f.parse::<usize>().unwrap()].clone(),
                    Ty::Var(_) => {
                        self.err(pos, format!("cannot infer the type of the value of `.{f}`; add an annotation"));
                        self.fresh()
                    }
                    o => {
                        let s = self.show(&o);
                        self.err(pos, format!("{s} has no field `{f}`"));
                        self.fresh()
                    }
                }
            }
            EK::Bin(op, l, r) => {
                let lt = self.infer(l, None);
                let rt = self.infer(r, Some(&lt));
                match *op {
                    "&&" | "||" => {
                        self.unify(l.pos, &lt, &Ty::Bool);
                        self.unify(r.pos, &rt, &Ty::Bool);
                        Ty::Bool
                    }
                    "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                        self.unify(r.pos, &rt, &lt);
                        self.deferred.push((pos, e.id, if op.len() == 2 && op.ends_with('=') && *op != "<=" && *op != ">=" { "eq" } else { "ord" }, lt));
                        Ty::Bool
                    }
                    _ => {
                        self.unify(r.pos, &rt, &lt);
                        let k = match *op {
                            "+" => "add",
                            "/" | "%" => "div",
                            _ => "num",
                        };
                        self.deferred.push((pos, e.id, k, lt.clone()));
                        self.nums.push((e.id, lt.clone()));
                        lt
                    }
                }
            }
            EK::Un(op, x) => {
                let t = self.infer(x, None);
                if *op == "!" {
                    self.unify(x.pos, &t, &Ty::Bool);
                } else {
                    self.deferred.push((pos, e.id, "num", t.clone()));
                    self.nums.push((e.id, t.clone()));
                }
                t
            }
            EK::Assign(n, x) => {
                let Some((idx, t)) = self.lookup_idx(n) else {
                    self.err(pos, format!("unknown name `{n}`"));
                    return Ty::Unit;
                };
                if !self.muts.contains(&(idx, n.clone())) {
                    self.err(pos, format!("`{n}` is not mutable (declare it `let mut {n} = ..`)"));
                }
                self.check_capture(pos, idx, n);
                let xt = self.infer(x, Some(&t));
                self.unify(x.pos, &xt, &t);
                Ty::Unit
            }
            EK::While(c, body) => {
                let ct = self.infer(c, None);
                self.unify(c.pos, &ct, &Ty::Bool);
                self.loops += 1;
                let bt = self.infer(body, Some(&Ty::Unit));
                self.loops -= 1;
                self.unify(body.pos, &bt, &Ty::Unit);
                Ty::Unit
            }
            EK::For(x, it, body) => {
                let itt = self.infer(it, None);
                let el = self.fresh();
                let itt = self.resolve(&itt);
                if let Ty::Typed(e) = &itt {
                    self.unify(it.pos, e, &el);
                } else {
                    self.unify(it.pos, &itt, &Ty::Array(Box::new(el.clone())));
                }
                self.scopes.push([(x.clone(), el)].into_iter().collect());
                self.loops += 1;
                let bt = self.infer(body, Some(&Ty::Unit));
                self.loops -= 1;
                self.pop_scope();
                self.unify(body.pos, &bt, &Ty::Unit);
                Ty::Unit
            }
            EK::Break | EK::Continue => {
                if self.loops == 0 {
                    self.err(pos, "`break` / `continue` outside a loop");
                }
                // (it does not return: any type)
                self.fresh()
            }
            EK::Index(x, i) => {
                let xt = self.infer(x, None);
                let it = self.infer(i, Some(&Ty::I32));
                self.unify(i.pos, &it, &Ty::I32);
                let el = self.fresh();
                match self.resolve(&xt) {
                    Ty::Typed(e) => *e,
                    _ => {
                        self.unify(x.pos, &xt, &Ty::Array(Box::new(el.clone())));
                        el
                    }
                }
            }
            EK::Cast(x, ty) => {
                let g = self.generics.clone();
                let to = self.ty_of(ty, &g);
                let from = self.infer(x, None);
                self.casts.push((e.id, pos, from, to.clone()));
                to
            }
            EK::Perform(en, v, args) => {
                let Some(e) = self.info.variant_of.get(v).cloned() else {
                    self.err(pos, format!("unknown effect operation `{v}`"));
                    return self.fresh();
                };
                if let Some(en) = en
                    && *en != e
                {
                    self.err(pos, format!("`{v}` is an operation of `{e}`, not `{en}`"));
                }
                let def = &self.info.enums[&e];
                let Some(ret) = def.ops.get(v).cloned() else {
                    self.err(pos, format!("`{v}` is a variant of `{e}`, which is not an effect (declare its variants as `{v}(..) -> R`)"));
                    return self.fresh();
                };
                let ps = def.variants.iter().find(|x| x.0 == *v).unwrap().1.clone();
                if ps.len() != args.len() {
                    self.err(pos, format!("`{v}` takes {} argument(s), found {}", ps.len(), args.len()));
                }
                for (a, p) in args.iter().zip(&ps) {
                    let t = self.infer(a, Some(p));
                    self.unify(a.pos, &t, p);
                }
                ret
            }
            EK::Handle(body, arms) => {
                let out = match expect {
                    Some(t) => t.clone(),
                    None => self.fresh(),
                };
                let bt = self.infer(body, Some(&out));
                self.unify(body.pos, &bt, &out);
                let mut covered: HashMap<String, Vec<String>> = HashMap::new();
                for a in arms {
                    let Some(en) = self.info.variant_of.get(&a.variant).cloned() else {
                        self.err(a.pos, format!("unknown effect operation `{}`", a.variant));
                        continue;
                    };
                    let def = &self.info.enums[&en];
                    let Some(ret) = def.ops.get(&a.variant).cloned() else {
                        self.err(a.pos, format!("`{}` is not an effect operation", a.variant));
                        continue;
                    };
                    let ps = def.variants.iter().find(|x| x.0 == a.variant).unwrap().1.clone();
                    covered.entry(en).or_default().push(a.variant.clone());
                    if ps.len() != a.pats.len() {
                        self.err(a.pos, format!("`{}` has {} argument(s), the pattern has {}", a.variant, ps.len(), a.pats.len()));
                    }
                    let mut binds = vec![];
                    for (p, t) in a.pats.iter().zip(&ps) {
                        if !irrefutable(p) {
                            self.err(a.pos, "a handler's arm binds the operation's arguments (names or `_`): match on them in the arm's body");
                        }
                        self.check_pat(p, t, a.pos, &mut binds);
                    }
                    binds.push((a.k.clone(), Ty::Fn(vec![ret], Box::new(out.clone()))));
                    self.scopes.push(binds.into_iter().collect());
                    let t = self.infer(&a.body, Some(&out));
                    self.unify(a.body.pos, &t, &out);
                    self.pop_scope();
                }
                for (en, vs) in covered {
                    let missing: Vec<String> = self.info.enums[&en].ops.keys().filter(|o| !vs.contains(o)).cloned().collect();
                    if !missing.is_empty() {
                        self.err(pos, format!("this handler handles `{en}` but not `{}` (a handler takes all of an effect's operations)", missing.join("`, `")));
                    }
                }
                out
            }
            EK::If(c, th, el) => {
                let ct = self.infer(c, None);
                self.unify(c.pos, &ct, &Ty::Bool);
                let tt = self.infer(th, expect);
                match el {
                    Some(el) => {
                        let et = self.infer(el, Some(&tt));
                        self.unify(el.pos, &et, &tt);
                        tt
                    }
                    None => {
                        self.unify(th.pos, &tt, &Ty::Unit);
                        Ty::Unit
                    }
                }
            }
            EK::Match(s, arms) => {
                let st = self.infer(s, None);
                let out = match expect {
                    Some(t) => t.clone(),
                    None => self.fresh(),
                };
                for (p, body) in arms {
                    let mut binds = vec![];
                    self.check_pat(p, &st, body.pos, &mut binds);
                    self.scopes.push(binds.into_iter().collect());
                    let bt = self.infer(body, Some(&out));
                    self.unify(body.pos, &bt, &out);
                    self.pop_scope();
                }
                let pats: Vec<&Pat> = arms.iter().map(|a| &a.0).collect();
                self.exhaustive(pos, &st, &pats);
                out
            }
            EK::Block(stmts, tail) => {
                self.scopes.push(HashMap::new());
                for s in stmts {
                    match s {
                        Stmt::Let(p, ann, x) => {
                            let at = ann.as_ref().map(|t| {
                                let g = self.generics.clone();
                                self.ty_of(t, &g)
                            });
                            let t = self.infer(x, at.as_ref());
                            if let Some(a) = &at {
                                self.unify(x.pos, &t, a);
                            }
                            if !irrefutable(p) {
                                self.err(x.pos, "a `let` pattern must always match (use `match`)");
                            }
                            let mut binds = vec![];
                            self.check_pat(p, &t, x.pos, &mut binds);
                            let k = self.scopes.len() - 1;
                            for (n, t) in binds {
                                self.muts.remove(&(k, n.clone()));
                                self.scopes[k].insert(n, t);
                            }
                        }
                        Stmt::LetMut(n, ann, x) => {
                            let at = ann.as_ref().map(|t| {
                                let g = self.generics.clone();
                                self.ty_of(t, &g)
                            });
                            let t = self.infer(x, at.as_ref());
                            if let Some(a) = &at {
                                self.unify(x.pos, &t, a);
                            }
                            let k = self.scopes.len() - 1;
                            self.muts.insert((k, n.clone()));
                            self.scopes[k].insert(n.clone(), t);
                        }
                        Stmt::Expr(x) => {
                            self.infer(x, None);
                        }
                    }
                }
                let t = match tail {
                    Some(x) => self.infer(x, expect),
                    None => Ty::Unit,
                };
                self.pop_scope();
                t
            }
            EK::Closure(ps, body) => {
                let expected = expect.map(|t| self.resolve(t));
                let mut pts = vec![];
                let mut sc = HashMap::new();
                for (k, (n, ann)) in ps.iter().enumerate() {
                    let t = match ann {
                        Some(a) => {
                            let g = self.generics.clone();
                            self.ty_of(a, &g)
                        }
                        None => match &expected {
                            Some(Ty::Fn(a, _)) if a.len() == ps.len() => a[k].clone(),
                            _ => self.fresh(),
                        },
                    };
                    sc.insert(n.clone(), t.clone());
                    pts.push(t);
                }
                let rexp = match &expected {
                    Some(Ty::Fn(_, r)) => Some((**r).clone()),
                    _ => None,
                };
                self.closure_bases.push(self.scopes.len());
                let loops = std::mem::take(&mut self.loops);
                self.scopes.push(sc);
                let rt = self.infer(body, rexp.as_ref());
                self.pop_scope();
                self.loops = loops;
                self.closure_bases.pop();
                Ty::Fn(pts, Box::new(rt))
            }
            EK::Record(n, fields, base) => {
                let Some(r) = self.info.records.get(n) else {
                    self.err(pos, format!("unknown record type `{n}`"));
                    return self.fresh();
                };
                let params = r.params.clone();
                let mut all: Vec<Ty> = r.fields.iter().map(|f| f.1.clone()).collect();
                let names: Vec<String> = r.fields.iter().map(|f| f.0.clone()).collect();
                all.extend(params.iter().map(|p| Ty::Param(p.clone())));
                let inst = self.instantiate(&params, &all);
                let (ftys, targs) = inst.split_at(names.len());
                let rt = Ty::Named(n.clone(), targs.to_vec());
                for (f, x) in fields {
                    let Some(k) = names.iter().position(|m| m == f) else {
                        self.err(x.pos, format!("`{n}` has no field `{f}`"));
                        continue;
                    };
                    let t = self.infer(x, Some(&ftys[k]));
                    self.unify(x.pos, &t, &ftys[k]);
                }
                match base {
                    Some(b) => {
                        let bt = self.infer(b, Some(&rt));
                        self.unify(b.pos, &bt, &rt);
                    }
                    None => {
                        let missing: Vec<&String> = names.iter().filter(|m| !fields.iter().any(|f| f.0 == **m)).collect();
                        if !missing.is_empty() {
                            let m = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("`, `");
                            self.err(pos, format!("missing field(s) `{m}` of `{n}`"));
                        }
                    }
                }
                rt
            }
            EK::Array(xs) => {
                // a literal where the type says F64Array / F32Array / I32Array builds that typed array
                if let Some(Ty::Typed(el)) = expect.map(|t| self.resolve(t)) {
                    for x in xs {
                        let t = self.infer(x, Some(&el));
                        self.unify(x.pos, &t, &el);
                    }
                    self.typed_lits.push((e.id, (*el).clone()));
                    return Ty::Typed(el);
                }
                let et = match expect.map(|t| self.resolve(t)) {
                    Some(Ty::Array(t)) => *t,
                    _ => self.fresh(),
                };
                for x in xs {
                    let t = self.infer(x, Some(&et));
                    self.unify(x.pos, &t, &et);
                }
                Ty::Array(Box::new(et))
            }
            EK::Tuple(xs) => Ty::Tuple(xs.iter().map(|x| self.infer(x, None)).collect()),
            EK::Jsx(j) => {
                self.jsx(j);
                Ty::Jsx
            }
        }
    }

    fn lit_ty(&mut self, l: &Lit) -> Ty {
        match l {
            Lit::Int(_) => self.fresh_int(),
            Lit::Float(_) => self.fresh_float(),
            Lit::Str(_) => Ty::Str,
            Lit::Bool(_) => Ty::Bool,
            Lit::Unit => Ty::Unit,
        }
    }

    fn variant(&mut self, id: Id, pos: Pos, en: Option<&String>, v: &str) -> Ty {
        let Some(e) = self.info.variant_of.get(v).cloned() else {
            self.err(pos, format!("unknown variant `{v}`"));
            return self.fresh();
        };
        if let Some(en) = en
            && *en != e
        {
            self.err(pos, format!("`{v}` is a variant of `{e}`, not `{en}`"));
        }
        self.info.callee.insert(id, Callee::Variant(e.clone()));
        let def = &self.info.enums[&e];
        let params = def.params.clone();
        let mut all = def.variants.iter().find(|x| x.0 == v).unwrap().1.clone();
        let nf = all.len();
        all.extend(params.iter().map(|p| Ty::Param(p.clone())));
        let inst = self.instantiate(&params, &all);
        let rt = Ty::Named(e, inst[nf..].to_vec());
        if nf == 0 { rt } else { Ty::Fn(inst[..nf].to_vec(), Box::new(rt)) }
    }

    fn jsx(&mut self, j: &Jsx) {
        let component = j.tag.chars().next().is_some_and(|c| c.is_uppercase());
        if component {
            match self.info.fns.get(&j.tag).map(|s| (s.args.clone(), s.ret.clone(), s.generics.is_empty())) {
                Some((args, ret, true)) if args.len() <= 1 && ret == Ty::Jsx => {
                    let fields = match args.first() {
                        Some(Ty::Named(n, _)) if self.info.records.contains_key(n) => self.info.records[n].fields.clone(),
                        None => vec![],
                        _ => {
                            self.err(j.pos, format!("component `{}` must take one record of props", j.tag));
                            vec![]
                        }
                    };
                    for (a, x) in &j.attrs {
                        if a == "key" {
                            let t = self.infer(x, None);
                            self.deferred.push((x.pos, x.id, "ord", t));
                            continue;
                        }
                        match fields.iter().find(|f| f.0 == *a) {
                            Some((_, ft)) => {
                                let t = self.infer(x, Some(ft));
                                self.unify(x.pos, &t, ft);
                            }
                            None => {
                                self.err(x.pos, format!("component `{}` has no prop `{a}`", j.tag));
                                self.infer(x, None);
                            }
                        }
                    }
                    for (f, _) in &fields {
                        if f != "children" && !j.attrs.iter().any(|a| a.0 == *f) {
                            self.err(j.pos, format!("missing prop `{f}` of <{}>", j.tag));
                        }
                    }
                    if !j.children.is_empty() && !fields.iter().any(|f| f.0 == "children") {
                        self.err(j.pos, format!("<{}> takes no children (its props have no `children: Jsx`)", j.tag));
                    }
                }
                Some(_) => self.err(j.pos, format!("`{}` is not a component: a component is `fn {}(props: Record) -> Jsx`", j.tag, j.tag)),
                None => self.err(j.pos, format!("unknown component `{}`", j.tag)),
            }
        } else {
            for (_, x) in &j.attrs {
                self.infer(x, None);
            }
        }
        for c in &j.children {
            if let JsxChild::Expr(x) = c {
                let t = self.infer(x, None);
                self.deferred.push((x.pos, x.id, "child", t));
            }
        }
    }

    fn method_sig(&mut self, recv: &Ty, name: &str) -> Option<(Vec<Ty>, Ty, Builtin)> {
        use Builtin as B;
        let f = |a: Vec<Ty>, r: Ty| Ty::Fn(a, Box::new(r));
        Some(match (recv, name) {
            (Ty::Array(_), "len") | (Ty::Str, "len") | (Ty::Typed(_), "len") => (vec![], Ty::I32, B::Len),
            // a typed array's map keeps its type (the element type: Float64Array.map stores into a Float64Array)
            (Ty::Typed(t), "map") => (vec![f(vec![(**t).clone()], (**t).clone())], recv.clone(), B::Map),
            (Ty::Typed(t), "filter") => (vec![f(vec![(**t).clone()], Ty::Bool)], recv.clone(), B::Filter),
            (Ty::Typed(t), "any") => (vec![f(vec![(**t).clone()], Ty::Bool)], Ty::Bool, B::Any),
            (Ty::Typed(t), "all") => (vec![f(vec![(**t).clone()], Ty::Bool)], Ty::Bool, B::All),
            (Ty::Typed(t), "fold") => {
                let u = self.fresh();
                (vec![u.clone(), f(vec![u.clone(), (**t).clone()], u.clone())], u, B::Fold)
            }
            (Ty::Typed(t), "to_array") => (vec![], Ty::Array(t.clone()), B::ToArray),
            (Ty::Array(t), "map") => {
                let u = self.fresh();
                (vec![f(vec![(**t).clone()], u.clone())], Ty::Array(Box::new(u)), B::Map)
            }
            (Ty::Array(t), "filter") => (vec![f(vec![(**t).clone()], Ty::Bool)], recv.clone(), B::Filter),
            (Ty::Array(t), "any") => (vec![f(vec![(**t).clone()], Ty::Bool)], Ty::Bool, B::Any),
            (Ty::Array(t), "all") => (vec![f(vec![(**t).clone()], Ty::Bool)], Ty::Bool, B::All),
            (Ty::Array(t), "push") => (vec![(**t).clone()], recv.clone(), B::Push),
            (Ty::Array(_), "concat") => (vec![recv.clone()], recv.clone(), B::Concat),
            (Ty::Array(t), "join") if **t == Ty::Str => (vec![Ty::Str], Ty::Str, B::Join),
            (Ty::Array(t), "fold") => {
                let u = self.fresh();
                (vec![u.clone(), f(vec![u.clone(), (**t).clone()], u.clone())], u, B::Fold)
            }
            (Ty::Str, "to_upper") => (vec![], Ty::Str, B::ToUpper),
            (Ty::Str, "trim") => (vec![], Ty::Str, B::Trim),
            (Ty::Str, "contains") => (vec![Ty::Str], Ty::Bool, B::Contains),
            (Ty::Str, "split") => (vec![Ty::Str], Ty::Array(Box::new(Ty::Str)), B::Split),
            (Ty::I32 | Ty::U32 | Ty::I64 | Ty::F32 | Ty::F64 | Ty::Bool, "to_string") => (vec![], Ty::Str, B::ToString),
            (Ty::Param(p), "to_string") if self.bounds.contains_key(p) => (vec![], Ty::Str, B::ToString),
            // (an integer literal whose type is not decided yet: every integer type has it)
            (Ty::Var(v), "to_string") if self.intv.contains(v) => (vec![], Ty::Str, B::ToString),
            (Ty::Var(v), "to_string") if self.floatv.contains(v) => (vec![], Ty::Str, B::ToString),
            (Ty::F32 | Ty::F64, "floor") => (vec![], recv.clone(), B::Floor),
            (Ty::F32 | Ty::F64, "round") => (vec![], recv.clone(), B::Round),
            (Ty::F32 | Ty::F64, "sqrt") => (vec![], recv.clone(), B::Sqrt),
            (Ty::F32 | Ty::F64, "abs") => (vec![], recv.clone(), B::Abs),
            // (JS's Math.max / Math.min: NaN if either is NaN, and +0 above -0)
            (Ty::F32 | Ty::F64, "max") => (vec![recv.clone()], recv.clone(), B::Max),
            (Ty::F32 | Ty::F64, "min") => (vec![recv.clone()], recv.clone(), B::Min),
            _ => return None,
        })
    }
}


fn irrefutable(p: &Pat) -> bool {
    match p {
        Pat::Wild | Pat::Bind(_) => true,
        Pat::Tuple(ps) => ps.iter().all(irrefutable),
        _ => false,
    }
}

fn body_pos(body: &Expr, item: Pos) -> Pos {
    match &body.kind {
        EK::Block(_, Some(t)) => t.pos,
        _ => item,
    }
}

// ---------------------------------------------------------------- effect inference

/// Which effects each function performs and does not handle: a fixpoint over the call graph (names only, so it
/// runs before type inference). Effects do not travel through values: a closure whose body leaves an effect
/// unhandled, an effectful function used as a value, or a continuation called inside a closure is an error (the
/// prototype's restriction: with generators, a `yield` cannot cross a plain JS function).
pub fn infer_effects(items: &[Item], info: &mut Info, errors: &mut Vec<(Pos, String)>) {
    let fns: Vec<(&String, &Expr)> = items
        .iter()
        .filter_map(|it| match it {
            Item::Fn { name, body, .. } => Some((name, body)),
            _ => None,
        })
        .collect();
    for (n, _) in &fns {
        info.effects.insert((*n).clone(), BTreeSet::new());
    }
    loop {
        let mut changed = false;
        for (n, body) in &fns {
            let mut w = EffWalk { info, errors: vec![], conts: vec![], in_closure: 0 };
            let eff = w.expr(body);
            if eff != info.effects[*n] {
                info.effects.insert((*n).clone(), eff);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    info.direct = direct_effects(items, info);
    // (once more, for the errors, with the final sets)
    for it in items {
        if let Item::Fn { body, ret, pos, name, .. } = it {
            let mut w = EffWalk { info, errors: vec![], conts: vec![], in_closure: 0 };
            w.expr(body);
            errors.extend(w.errors);
            if matches!(ret, Some(TyS::Name(r, _, _)) if r == "Jsx") && !info.effects[name].is_empty() {
                let e: Vec<&String> = info.effects[name].iter().collect();
                errors.push((*pos, format!("component `{name}` performs {e:?}: handle it inside (rendering cannot be suspended)")));
            }
        }
    }
}

struct EffWalk<'a> {
    info: &'a Info,
    errors: Vec<(Pos, String)>,
    conts: Vec<String>,
    in_closure: u32,
}

impl EffWalk<'_> {
    fn expr(&mut self, e: &Expr) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        match &e.kind {
            EK::Perform(_, v, args) => {
                if let Some(en) = self.info.variant_of.get(v) {
                    out.insert(en.clone());
                }
                for a in args {
                    out.extend(self.expr(a));
                }
            }
            EK::Call(f, args) => {
                match &f.kind {
                    EK::Var(n) if self.info.effects.contains_key(n) => out.extend(self.info.effects[n].iter().cloned()),
                    EK::Var(n) if self.conts.contains(n) && self.in_closure > 0 => {
                        self.errors.push((f.pos, format!("the continuation `{n}` is called inside a closure: call it in the handler's arm itself")));
                    }
                    EK::Var(n) if self.conts.contains(n) => {}
                    _ => out.extend(self.expr(f)),
                }
                for a in args {
                    out.extend(self.expr(a));
                }
            }
            EK::Var(n) => {
                if self.info.effects.get(n).is_some_and(|s| !s.is_empty()) {
                    self.errors.push((e.pos, format!("`{n}` performs effects and cannot be used as a value (call it, or handle its effects in a wrapper)")));
                }
                if self.conts.contains(n) {
                    self.errors.push((e.pos, format!("the continuation `{n}` can only be called")));
                }
            }
            EK::Handle(body, arms) => {
                let mut handled = BTreeSet::new();
                for a in arms {
                    if let Some(en) = self.info.variant_of.get(&a.variant) {
                        handled.insert(en.clone());
                    }
                }
                let b = self.expr(body);
                out.extend(b.difference(&handled).cloned());
                for a in arms {
                    self.conts.push(a.k.clone());
                    out.extend(self.expr(&a.body));
                    self.conts.pop();
                }
            }
            EK::Closure(_, body) => {
                self.in_closure += 1;
                let b = self.expr(body);
                self.in_closure -= 1;
                if !b.is_empty() {
                    let v: Vec<&String> = b.iter().collect();
                    self.errors.push((e.pos, format!("this closure performs {v:?}: an effect cannot leave a closure (handle it inside)")));
                }
            }
            EK::Lit(_) | EK::Path(..) => {}
            EK::Method(r, _, args) => {
                out.extend(self.expr(r));
                for a in args {
                    out.extend(self.expr(a));
                }
            }
            EK::Field(x, _) | EK::Un(_, x) | EK::Cast(x, _) => out.extend(self.expr(x)),
            EK::Index(x, i) | EK::While(x, i) | EK::For(_, x, i) => {
                out.extend(self.expr(x));
                out.extend(self.expr(i));
            }
            EK::Assign(_, x) => out.extend(self.expr(x)),
            EK::Break | EK::Continue => {}
            EK::Bin(_, l, r) => {
                out.extend(self.expr(l));
                out.extend(self.expr(r));
            }
            EK::If(c, t, el) => {
                out.extend(self.expr(c));
                out.extend(self.expr(t));
                if let Some(x) = el {
                    out.extend(self.expr(x));
                }
            }
            EK::Match(s, arms) => {
                out.extend(self.expr(s));
                for (_, b) in arms {
                    out.extend(self.expr(b));
                }
            }
            EK::Block(stmts, tail) => {
                for s in stmts {
                    match s {
                        Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) | Stmt::Expr(x) => out.extend(self.expr(x)),
                    }
                }
                if let Some(t) = tail {
                    out.extend(self.expr(t));
                }
            }
            EK::Record(_, fs, base) => {
                for (_, x) in fs {
                    out.extend(self.expr(x));
                }
                if let Some(b) = base {
                    out.extend(self.expr(b));
                }
            }
            EK::Array(xs) | EK::Tuple(xs) => {
                for x in xs {
                    out.extend(self.expr(x));
                }
            }
            EK::Jsx(j) => {
                for (_, x) in &j.attrs {
                    out.extend(self.expr(x));
                }
                for c in &j.children {
                    if let JsxChild::Expr(x) = c {
                        out.extend(self.expr(x));
                    }
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------- direct (tail-resumptive) effects

/// The effects that can run as plain calls in JS. An effect is direct when:
/// - every `handle` of it is tail-resumptive (each arm calls `k` once, in tail position, and nowhere else),
/// - it does not leave a `pub` function (TS callers drive the generator of such a function),
/// - every `handle` of it handles only direct effects, and lets only direct effects through its body and its arms
///   (a plain call cannot carry a generator's `yield`).
/// The last rule is a fixpoint: removing an effect can make another one non-direct.
pub fn direct_effects(items: &[Item], info: &Info) -> BTreeSet<String> {
    struct H {
        handled: BTreeSet<String>,
        through: BTreeSet<String>,
        tail: bool,
    }
    let mut hs: Vec<H> = vec![];
    // `outer`: the continuations of the arms around `e`. A handler whose body or arms call one of them suspends
    // through that arm, so it cannot be a plain call
    fn collect(e: &Expr, info: &Info, hs: &mut Vec<H>, outer: &mut Vec<String>) {
        if let EK::Handle(body, arms) = &e.kind {
            let mut w = EffWalk { info, errors: vec![], conts: vec![], in_closure: 0 };
            let handled: BTreeSet<String> = arms.iter().filter_map(|a| info.variant_of.get(&a.variant).cloned()).collect();
            let mut through: BTreeSet<String> = w.expr(body).difference(&handled).cloned().collect();
            for a in arms {
                w.conts.push(a.k.clone());
                through.extend(w.expr(&a.body));
                w.conts.pop();
            }
            let suspends = outer.iter().any(|k| mentions(body, k) || arms.iter().any(|a| mentions(&a.body, k)));
            let tail = !suspends && arms.iter().all(|a| tail_resumes(&a.body, &a.k));
            hs.push(H { handled, through, tail });
            collect(body, info, hs, outer);
            for a in arms {
                outer.push(a.k.clone());
                collect(&a.body, info, hs, outer);
                outer.pop();
            }
            return;
        }
        for c in subexprs(e) {
            collect(c, info, hs, outer);
        }
    }
    let mut escaping = BTreeSet::new();
    for it in items {
        if let Item::Fn { name, body, public, .. } = it {
            collect(body, info, &mut hs, &mut vec![]);
            if *public {
                escaping.extend(info.effects[name].iter().cloned());
            }
        }
    }
    let mut direct: BTreeSet<String> = hs.iter().flat_map(|h| h.handled.iter().cloned()).collect();
    direct.retain(|e| !escaping.contains(e));
    for h in &hs {
        if !h.tail {
            direct.retain(|e| !h.handled.contains(e));
        }
    }
    loop {
        let mut changed = false;
        for h in &hs {
            if h.handled.iter().any(|e| direct.contains(e)) && (!h.handled.is_subset(&direct) || !h.through.is_subset(&direct)) {
                direct.retain(|e| !h.handled.contains(e));
                changed = true;
            }
        }
        if !changed {
            return direct;
        }
    }
}

/// Every tail of `e` is a call `k(..)` whose arguments do not mention `k`, and `k` appears nowhere else
fn tail_resumes(e: &Expr, k: &str) -> bool {
    match &e.kind {
        EK::Call(f, args) if matches!(&f.kind, EK::Var(n) if n == k) => !args.iter().any(|a| mentions(a, k)),
        EK::Block(stmts, Some(t)) => {
            stmts.iter().all(|s| match s {
                Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) | Stmt::Expr(x) => !mentions(x, k),
            }) && tail_resumes(t, k)
        }
        EK::If(c, t, Some(el)) => !mentions(c, k) && tail_resumes(t, k) && tail_resumes(el, k),
        EK::Match(sc, arms) => !mentions(sc, k) && arms.iter().all(|(_, b)| tail_resumes(b, k)),
        _ => false,
    }
}

fn mentions(e: &Expr, k: &str) -> bool {
    matches!(&e.kind, EK::Var(n) if n == k) || subexprs(e).into_iter().any(|c| mentions(c, k))
}

/// The direct sub-expressions of `e`
pub fn subexprs(e: &Expr) -> Vec<&Expr> {
    let mut v: Vec<&Expr> = vec![];
    match &e.kind {
        EK::Lit(_) | EK::Var(_) | EK::Path(..) | EK::Break | EK::Continue => {}
        EK::Call(f, args) => {
            v.push(f);
            v.extend(args);
        }
        EK::Method(r, _, args) => {
            v.push(r);
            v.extend(args);
        }
        EK::Field(x, _) | EK::Un(_, x) | EK::Cast(x, _) | EK::Assign(_, x) => v.push(x),
        EK::Bin(_, l, r) | EK::Index(l, r) | EK::While(l, r) | EK::For(_, l, r) => {
            v.push(l);
            v.push(r);
        }
        EK::If(c, t, el) => {
            v.push(c);
            v.push(t);
            if let Some(x) = el {
                v.push(x);
            }
        }
        EK::Match(sc, arms) => {
            v.push(sc);
            v.extend(arms.iter().map(|a| &a.1));
        }
        EK::Block(stmts, tail) => {
            for s in stmts {
                match s {
                    Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) | Stmt::Expr(x) => v.push(x),
                }
            }
            if let Some(t) = tail {
                v.push(t);
            }
        }
        EK::Closure(_, b) => v.push(b),
        EK::Record(_, fs, base) => {
            v.extend(fs.iter().map(|f| &f.1));
            if let Some(b) = base {
                v.push(b);
            }
        }
        EK::Array(xs) | EK::Tuple(xs) => v.extend(xs),
        EK::Jsx(j) => {
            v.extend(j.attrs.iter().map(|a| &a.1));
            for c in &j.children {
                if let JsxChild::Expr(x) = c {
                    v.push(x);
                }
            }
        }
        EK::Perform(_, _, args) => v.extend(args),
        EK::Handle(b, arms) => {
            v.push(b);
            v.extend(arms.iter().map(|a| &a.body));
        }
    }
    v
}
