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
    /// `Map::new()` / `Map::from(pairs)` / `Set::new()` / `Set::from(xs)` (a Set's, the constructor)
    Coll(bool, String),
    /// `json_encode(x)` (false) / `json_decode(s)` (true): the type is checked, the value converted by it
    Json(bool),
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
    /// a method of `Option` / `Result` (`map`, `and_then`, `unwrap_or`, ..; see [`Checker::method_sig`])
    Opt(&'static str),
    /// `xs.try_map(f)`: `f`'s `Result` (true) / `Option` (false), stopping at the first `Err` / `None`
    TryMap(bool),
    /// the rest of the standard library, by name (`get`, `sort_by`, `starts_with`, `pow`, ..; see
    /// [`Checker::std_sig`]). The receiver's type tells an array's from a string's or a number's
    Std(On, &'static str),
}

/// What a [`Builtin::Std`] method is on
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum On {
    Array,
    Str,
    Float,
    Int,
    Map,
    Set,
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
    /// read from a TS union: its values are TS's (see tsimport.rs)
    pub ts: Option<crate::ast::TsEnum>,
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

/// An `extern type`: a JS value, its TS name, and the members declared for it
pub struct ExtType {
    pub params: Vec<String>,
    /// (name, type, JS name)
    pub props: Vec<(String, Ty, String)>,
    /// (name, arguments, result, JS name)
    pub methods: Vec<(String, Vec<Ty>, Ty, String)>,
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
    /// the type `json_encode` / `json_decode` converts, at its callee's id
    pub json: HashMap<Id, Ty>,
    /// the functions of an `impl` whose first parameter is `self` (`Point$len`): called as methods
    pub self_methods: HashSet<String>,
    /// a method call's function and its instantiated type (`p.len()` is `Point$len(p)`)
    pub method_fn: HashMap<Id, (String, Ty)>,
    pub cast: HashMap<Id, (Ty, Ty)>,
    /// the effects each function performs and does not handle (empty: an ordinary function)
    pub effects: HashMap<String, BTreeSet<String>>,
    /// the type arguments at each use of a monomorphized (numeric-bound) function, in its generics' order
    pub inst: HashMap<Id, Vec<Ty>>,
    /// every generic function's instantiation at a call (the Rust output writes an instance for each, bounded or
    /// not; the JS one only for a bound)
    pub rust_inst: HashMap<Id, Vec<Ty>>,
    /// array literals that build a typed array (the element type): `let w: F32Array = [..]`
    pub typed_lit: HashMap<Id, Ty>,
    pub records: HashMap<String, RecordDef>,
    pub enums: HashMap<String, EnumDef>,
    /// variant name → enum name (variant names are unique in a file)
    pub variant_of: HashMap<String, String>,
    pub fns: HashMap<String, FnSig>,
    pub ext_types: HashMap<String, ExtType>,
    /// the externs that are another zustscript module's functions (no conversion at the call)
    pub zsc_externs: HashSet<String>,
    /// each property read and method call of an extern type: (the type, the member)
    pub member: HashMap<Id, (String, String)>,
    pub externs: HashMap<String, (FnSig, Option<String>, String)>,
    pub wasm: HashMap<String, (Vec<(String, WTy)>, WTy)>,
    /// effects whose handlers all resume in tail position (see [`direct_effects`]): in JS they are plain calls to the
    /// handler's arm, not generators
    pub direct: BTreeSet<String>,
    /// Expression types for Rust lowering, including ordinary helpers reached from Wasm.
    pub ty: HashMap<Id, Ty>,
    /// each `?`: the enum of its value (`Option` / `Result`)
    pub tries: HashMap<Id, String>,
    /// `==` / `!=` that compare values with parts (arrays, records, enums with data, tuples): by content, in JS with
    /// `$eq`, in Rust with `derive(PartialEq)`. Values are immutable, so equal contents are equal values
    pub deep_eq: HashMap<Id, Ty>,
}

pub fn js_target_errors(items: &[Item]) -> Vec<(Pos, String)> {
    items.iter().filter_map(|item| match item {
        Item::Fn { name, wasm: true, pos, .. } => Some((*pos, format!("`{name}` is @wasm and requires the Rust backend; remove @wasm to compile it as JavaScript"))),
        _ => None,
    }).collect()
}

pub struct Checker {
    pub info: Info,
    subst: Vec<Option<Ty>>,
    scopes: Vec<HashMap<String, Ty>>,
    pub errors: Vec<(Pos, String)>,
    /// (not errors: the guidelines, e.g. i64 in the JS layer)
    pub warnings: Vec<(Pos, String)>,
    /// Suppress JS-only arithmetic warnings when checking a Rust or SIMD function.
    in_wasm: bool,
    /// (in an `async fn` or an `async` closure: `await` is allowed)
    in_async: bool,
    /// (the closure about to be checked is an `async` one)
    next_closure_async: bool,
    record_types: bool,
    /// (checked when the function is done, once inference has decided the types)
    deferred: Vec<(Pos, Id, &'static str, Ty)>,
    generics: Vec<String>,
    /// type variables that only integer types may fill (from integer literals)
    intv: HashSet<u32>,
    /// (id, type) to resolve into `Info::num` when the function is done
    nums: Vec<(Id, Ty)>,
    jsons: Vec<(Id, Ty)>,
    method_fns: Vec<(Id, String, Ty)>,
    rust_insts: Vec<(Id, Vec<Ty>)>,
    /// the callee of the call being checked (a built-in such as `range` or `Map::new` is only called)
    callee_id: Option<Id>,
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
    /// the answers of `uncovered` in one match, by its types and its rows (the patterns' addresses): `true | false`
    /// in many fields gives the same remaining rows on both sides
    cover_memo: std::cell::RefCell<HashMap<(Vec<Ty>, Vec<Vec<usize>>), bool>>,
    json_memo: std::cell::RefCell<HashMap<Ty, bool>>,
    /// the keys of written `Map` / `Set` types, with the bounds where they are written: checked once every enum's
    /// variants are known
    key_checks: Vec<(Pos, Ty, HashMap<String, String>)>,
    eq_low: std::cell::Cell<usize>,
    eq_pending: std::cell::RefCell<(Vec<Ty>, HashMap<Ty, usize>)>,
    bound_checks: Vec<(Pos, Ty, String)>,
    /// mutable locals: (scope index, name)
    muts: HashSet<(usize, String)>,
    /// the scope count where each enclosing closure starts (a mutable local below it cannot be captured)
    closure_bases: Vec<usize>,
    /// how many loops enclose the expression (in the current closure)
    loops: u32,
    /// the result type of each enclosing function and closure, innermost last: what a `?` returns from
    /// (and the result the context expects, which is not yet the result: an async closure's body may give the
    /// Promise itself)
    rets: Vec<(Ty, Option<Ty>)>,
    /// (in a `handle`'s body or arm, not in a closure there: a `?` would return from a generator of its own)
    in_handle: bool,
    /// (where a `?` cannot be written: the right of `&&` / `||`, a `while` condition. The error)
    try_ban: Option<&'static str>,
    /// (a `?` decided the result of the closure being checked)
    closure_try: bool,
}

pub const BOUNDS: &[&str] = &["Num", "Integer", "Float", "Eq", "Ord", "Hash"];

/// A numeric bound (its parameter has arithmetic, `T::zero()`, `to_string`)
pub fn num_bound(b: &str) -> bool {
    matches!(b, "Num" | "Integer" | "Float")
}

/// Whether a parameter bounded by `have` satisfies the bound `want`: every bound gives `==`; a numeric one gives `<`;
/// an integer one can be a key (`Hash`: a `Map`'s key or a `Set`'s element)
pub fn implies(have: &str, want: &str) -> bool {
    have == want
        || want == "Eq"
        || (want == "Num" && num_bound(have))
        || (want == "Ord" && num_bound(have))
        || (want == "Hash" && have == "Integer")
}

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
        WTy::Value(_) | WTy::Wire(..) => Ty::Unit,
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
/// `Promise<t>`
pub(crate) fn promise(t: Ty) -> Ty {
    Ty::Named("Promise".into(), vec![t])
}

/// The `T` of a `Promise<T>`
pub(crate) fn promised(t: &Ty) -> Option<Ty> {
    match t {
        Ty::Named(n, a) if n == "Promise" && a.len() == 1 => Some(a[0].clone()),
        _ => None,
    }
}

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
            in_async: false,
            next_closure_async: false,
            record_types: false,
            deferred: vec![],
            generics: vec![],
            intv: HashSet::new(),
            nums: vec![],
            jsons: vec![],
            method_fns: vec![],
            rust_insts: vec![],
            callee_id: None,
            casts: vec![],
            bounds: HashMap::new(),
            insts: vec![],
            floatv: HashSet::new(),
            typed_lits: vec![],
            tys: vec![],
            eq_memo: Default::default(),
            cover_memo: Default::default(),
            json_memo: Default::default(),
            key_checks: vec![],
            eq_low: std::cell::Cell::new(usize::MAX),
            eq_pending: Default::default(),
            bound_checks: vec![],
            muts: HashSet::new(),
            closure_bases: vec![],
            loops: 0,
            rets: vec![],
            in_handle: false,
            try_ban: None,
            closure_try: false,
        }
    }

    fn err(&mut self, pos: Pos, m: impl Into<String>) {
        self.errors.push((pos, m.into()));
    }

    /// What JS gives back, as an extern's result: a `Result` catches what it throws, as a `String` (the message) or
    /// a value of an extern type (the thrown value); a `Promise` of a `Result` catches its rejection too
    fn extern_result(&mut self, t: &Ty, pos: Pos) {
        let t = promised(t).unwrap_or(t.clone());
        if let Ty::Named(n, a) = &t
            && n == "Result"
            && a.len() == 2
            && !matches!(&a[1], Ty::Str)
            && !matches!(&a[1], Ty::Named(e, _) if self.info.ext_types.contains_key(e))
        {
            let s = self.show(&a[1]);
            self.err(pos, format!("an extern's `Result` catches what JS throws: its error is a `String` (the message) or an extern type (the value thrown), not {s}"));
        }
    }

    /// A function cannot be named like a variant of the prelude (`Ok(x)` would call it, wherever it is written)
    fn prelude_name(&mut self, name: &str, pos: Pos) {
        if let Some(p) = PRELUDE.iter().find(|p| p.2.iter().any(|v| v.0 == name)) {
            self.err(pos, format!("`{name}` is a variant of the prelude's `{}`: a function cannot have this name", p.0));
        }
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
                    // (JS's: what an `async fn` gives, and what `await` takes)
                    "Promise" => {
                        arity(self, 1);
                        promise(args.into_iter().next().unwrap_or(Ty::Unit))
                    }
                    _ if params.contains(n) => Ty::Param(n.clone()),
                    _ => {
                        let want = if let Some(r) = self.info.records.get(n) {
                            r.params.len()
                        } else if let Some(e) = self.info.enums.get(n) {
                            e.params.len()
                        } else if let Some(x) = self.info.ext_types.get(n) {
                            x.params.len()
                        } else if n == "Map" || n == "Set" {
                            // (the built-in collections, unless the file names a type so. The key is checked with the
                            // bounds of the function the type is written in)
                            arity(self, if n == "Map" { 2 } else { 1 });
                            if let Some(k) = args.first() {
                                self.key_checks.push((*pos, k.clone(), self.bounds.clone()));
                            }
                            return Ty::Named(n.clone(), args);
                        } else if let Some((_, _, vs)) = PRELUDE.iter().find(|p| p.0 == n) {
                            let vs: Vec<&str> = vs.iter().map(|v| v.0).collect();
                            self.err(*pos, format!("unknown type `{n}`: the prelude's `{n}` is left out of this file, because another enum has a variant named like one of its own ({}). Rename that variant", vs.join(", ")));
                            return Ty::Unit;
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

    pub fn check_file(&mut self, items: &[Item], rust_output: bool) {
        self.record_types = rust_output || items.iter().any(|it| matches!(it, Item::Fn { wasm: true, .. } | Item::Fn { simd: true, .. }));
        // types first (records and enums may refer to each other)
        for it in items {
            // (`Promise` is the built-in type of `async` / `await`: no record or enum takes its name)
            if let Item::Record { name, pos, .. } | Item::Enum { name, pos, .. } = it {
                if name == "Promise" {
                    self.err(*pos, "`Promise` is the built-in type of `async` / `await`: name this type otherwise");
                }
                if name == "Map" || name == "Set" {
                    self.err(*pos, format!("`{name}` is the built-in collection: name this type otherwise"));
                }
            }
            // (`Option` / `Result` are the prelude's: a file written before the prelude may declare them the same way)
            match it {
                Item::Record { name, pos, .. } if PRELUDE.iter().any(|p| p.0 == name) => {
                    self.err(*pos, format!("`{name}` is the prelude's type: remove this declaration, or name the type otherwise"));
                }
                Item::Enum { name, params, variants, pos, .. } if PRELUDE.iter().any(|p| p.0 == name) && !prelude_shaped(name, params, variants) => {
                    self.err(*pos, format!("`{name}` is the prelude's type: remove this declaration, or name the type otherwise"));
                }
                _ => {}
            }
            match it {
                Item::Record { name, params, pos, .. } => {
                    if self.info.records.contains_key(name) || self.info.enums.contains_key(name) || self.info.ext_types.contains_key(name) {
                        self.err(*pos, format!("the type `{name}` is defined twice"));
                    }
                    self.info.records.insert(name.clone(), RecordDef { params: params.clone(), fields: vec![] });
                }
                Item::Enum { name, params, pos, .. } => {
                    if self.info.records.contains_key(name) || self.info.enums.contains_key(name) || self.info.ext_types.contains_key(name) {
                        self.err(*pos, format!("the type `{name}` is defined twice"));
                    }
                    self.info.enums.insert(name.clone(), EnumDef { params: params.clone(), variants: vec![], ops: HashMap::new(), ts: None });
                }
                Item::ExternType { name, params, pos, .. } => {
                    if self.info.records.contains_key(name) || self.info.enums.contains_key(name) || self.info.ext_types.contains_key(name) || PRELUDE.iter().any(|p| p.0 == name) || name == "Promise" {
                        self.err(*pos, format!("the type `{name}` is defined twice"));
                    }
                    self.info.ext_types.insert(name.clone(), ExtType { params: params.clone(), props: vec![], methods: vec![] });
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
                Item::Enum { name, params, variants, pos, from } => {
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
                    e.ts = from.as_ref().and_then(|f| f.repr.clone());
                }
                Item::Fn { name, generics, bounds, args, ret, pos, is_async, wasm, simd, public, .. } => {
                    if name.contains('$') && args.first().is_some_and(|a| a.0 == "self") {
                        self.info.self_methods.insert(name.clone());
                    }
                    // (a public generic function is written for every type its bounds admit: the numeric ones are
                    // few, the others are not)
                    if *public && let Some((g, b)) = bounds.iter().find(|(_, b)| !num_bound(b)) {
                        self.err(*pos, format!("`{name}` is exported with the bound `{g}: {b}`: export a function for each type it is used with (`export fn uniq_names(xs: Array<String>) -> Array<String> {{ uniq(xs) }}`)"));
                    }
                    if *is_async && (*wasm || *simd) {
                        self.err(*pos, format!("`{name}` cannot be both async and @wasm / @simd (a Promise does not cross into wasm)"));
                    }
                    if let Some(p) = names.insert(name.clone(), *pos) {
                        self.err(*pos, format!("`{}` is already defined at {p}", name.replace('$', "::")));
                    }
                    self.prelude_name(name, *pos);
                    for (_, b) in bounds {
                        if !BOUNDS.contains(&b.as_str()) {
                            self.err(*pos, format!("unknown bound `{b}` (the bounds are Num, Integer, Float, Eq, Ord, Hash)"));
                        }
                    }
                    // (the bounds a `Map<K, V>` in the signature needs)
                    self.bounds = bounds.iter().cloned().collect();
                    let a = args.iter().map(|(_, t)| self.ty_of(t, generics)).collect();
                    let r = ret.as_ref().map(|t| self.ty_of(t, generics)).unwrap_or(Ty::Unit);
                    self.bounds.clear();
                    // (an `async fn`'s callers get a Promise of its result; JS adopts a Promise its body gives, so
                    // the result is written as the value)
                    if *is_async && promised(&r).is_some() {
                        self.err(*pos, format!("`{name}` is async: write its result as the value (`-> T`), callers get `Promise<T>`"));
                    }
                    let r = if *is_async { promise(r) } else { r };
                    let bounds = bounds.iter().cloned().collect();
                    self.info.fns.insert(name.clone(), FnSig { generics: generics.clone(), bounds, args: a, ret: r });
                }
                Item::Extern { name, module, generics, args, ret, js, pos, zsc } => {
                    if let Some(p) = names.insert(name.clone(), *pos) {
                        self.err(*pos, format!("`{}` is already defined at {p}", name.replace('$', "::")));
                    }
                    self.prelude_name(name, *pos);
                    let a = args.iter().map(|t| self.ty_of(t, generics)).collect();
                    let r = self.ty_of(ret, generics);
                    if *zsc {
                        self.info.zsc_externs.insert(name.clone());
                    } else {
                        self.extern_result(&r, *pos);
                    }
                    self.info.externs.insert(name.clone(), (FnSig { generics: generics.clone(), bounds: HashMap::new(), args: a, ret: r }, module.clone(), js.clone()));
                }
                Item::WasmFn { name, args, ret, pos, .. } => {
                    if let Some(p) = names.insert(name.clone(), *pos) {
                        self.err(*pos, format!("`{}` is already defined at {p}", name.replace('$', "::")));
                    }
                    self.info.wasm.insert(name.clone(), (args.clone(), ret.clone()));
                }
                Item::ExternType { name, params, members, .. } => {
                    let (mut props, mut methods) = (vec![], vec![]);
                    for m in members {
                        match m {
                            Member::Prop { name: n, ty, js, pos } => {
                                let t = self.ty_of(ty, params);
                                if props.iter().any(|p: &(String, Ty, String)| p.0 == *n) || methods.iter().any(|p: &(String, Vec<Ty>, Ty, String)| p.0 == *n) {
                                    self.err(*pos, format!("`{name}` declares `{n}` twice"));
                                }
                                self.extern_result(&t, *pos);
                                props.push((n.clone(), t, js.clone()));
                            }
                            Member::Method { name: n, args, ret, js, pos } => {
                                let a: Vec<Ty> = args.iter().map(|t| self.ty_of(t, params)).collect();
                                let r = self.ty_of(ret, params);
                                if props.iter().any(|p| p.0 == *n) || methods.iter().any(|p| p.0 == *n) {
                                    self.err(*pos, format!("`{name}` declares `{n}` twice"));
                                }
                                self.extern_result(&r, *pos);
                                methods.push((n.clone(), a, r, js.clone()));
                            }
                        }
                    }
                    let x = self.info.ext_types.get_mut(name).unwrap();
                    x.props = props;
                    x.methods = methods;
                }
                Item::WasmRaw(_) | Item::ImportType { .. } => {}
            }
        }
        let mut errs = vec![];
        infer_effects(items, &mut self.info, &mut errs);
        self.errors.extend(errs);
        // (an effect's operations run on generators, which an async function is not)
        for it in items {
            if let Item::Fn { name, is_async: true, pos, .. } = it {
                if let Some(es) = self.info.effects.get(name).filter(|e| !e.is_empty()) {
                    let es: Vec<&String> = es.iter().collect();
                    self.errors.push((*pos, format!("the async fn `{name}` performs {es:?}: handle the effects inside, in a function that is not async")));
                }
            }
        }
        for it in items {
            if let Item::Fn { generics, args, body, pos, name, wasm, simd, is_async, .. } = it {
                self.in_wasm = rust_output || *wasm || *simd;
                self.in_async = *is_async;
                self.generics = generics.clone();
                self.scopes = vec![HashMap::new()];
                self.muts.clear();
                let sig = &self.info.fns[name];
                self.bounds = sig.bounds.clone();
                let (at, ret) = (sig.args.clone(), sig.ret.clone());
                // (the body of an `async fn` gives the Promise's value)
                let ret = if *is_async { promised(&ret).unwrap_or(ret) } else { ret };
                for ((a, _), t) in args.iter().zip(at) {
                    self.scopes[0].insert(a.clone(), t);
                }
                self.rets = vec![(ret.clone(), None)];
                self.in_handle = false;
                self.try_ban = None;
                let got = self.infer(body, Some(&ret));
                self.unify(body_pos(body, *pos), &got, &ret);
                self.finish_fn();
            }
        }
        // (every enum's variants are known now, whatever the order of the items)
        for (pos, k, bounds) in std::mem::take(&mut self.key_checks) {
            let saved = std::mem::replace(&mut self.bounds, bounds);
            if !self.key_type(&k) {
                let s = self.show(&k);
                let hint = if let Ty::Param(p) = &k { format!(": add the bound `{p}: Hash`") } else { String::new() };
                self.err(pos, format!("a Map's key or a Set's element is an integer, a String, a bool or an enum without data, not {s}{hint}"));
            }
            self.bounds = saved;
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
            // (every bound gives `==`)
            Ty::Param(p) => self.bounds.contains_key(p),
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
        for (id, t) in std::mem::take(&mut self.jsons) {
            let r = self.resolve(&t);
            self.info.json.insert(id, r);
        }
        for (id, ts) in std::mem::take(&mut self.rust_insts) {
            let r = ts.iter().map(|t| self.resolve(t)).collect();
            self.info.rust_inst.insert(id, r);
        }
        for (id, f, t) in std::mem::take(&mut self.method_fns) {
            let r = self.resolve(&t);
            self.info.method_fn.insert(id, (f, r));
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
                Ty::Param(p) => self.bounds.get(p).is_some_and(|have| implies(have, &b)),
                t if num_bound(&b) => bound_types(&b).contains(t),
                t if b == "Eq" => self.eqable(t, &mut vec![]),
                t if b == "Ord" => self.numeric(t) || *t == Ty::Str,
                t => self.key_type(t),
            };
            if matches!(r, Ty::Var(_)) {
                self.err(pos, if num_bound(&b) { "cannot infer the numeric type here; add an annotation" } else { "cannot infer the type here; add an annotation" });
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
                "add" => self.numeric(&r) || r == Ty::Str,
                "ord" => self.numeric(&r) || r == Ty::Str || matches!(&r, Ty::Param(p) if self.bounds.get(p).is_some_and(|b| implies(b, "Ord"))),
                "eq" => {
                    let primitive = match &r {
                        Ty::Str | Ty::Bool | Ty::Unit => true,
                        t if self.numeric(t) => true,
                        // (a union read from TS with a tag field is objects, `{ kind: "on" }`: compared by content)
                        Ty::Named(n, _) => self.info.enums.get(n).is_some_and(|e| e.unit_only() && !e.is_effect() && e.ts.as_ref().is_none_or(|t| t.key.is_none())),
                        _ => false,
                    };
                    let ok = primitive || self.eqable(&r, &mut vec![]);
                    if !primitive && ok {
                        self.info.deep_eq.insert(_id, r.clone());
                    }
                    ok
                }
                "hole" => r == Ty::Str || r == Ty::Bool || self.numeric(&r),
                "key" => self.key_type(&r),
                "signed" => matches!(r, Ty::I32 | Ty::I64),
                "json" => self.jsonable(&r, &mut vec![]),
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
                    "ord" => match &r {
                        Ty::Param(p) => format!("cannot compare {s} with `<`: add the bound `{p}: Ord` (numbers and strings)"),
                        _ => format!("cannot compare {s} with `<`"),
                    },
                    "hole" => format!("a `${{..}}` of a template string takes a String, a number or a bool, not {s}"),
                    "json" => format!("JSON holds numbers (not i64: JSON.parse rounds past 2^53), strings, bools, and arrays, records, enums, Option (not an Option of an Option: both `None`s would be `null`), Map and Set of them, not {s}"),
                    "signed" => format!("`abs` is a method of i32 and i64, not {s}"),
                    "key" => match &r {
                        Ty::Param(p) => format!("a Map's key or a Set's element is an integer, a String, a bool or an enum without data, not {s}: add the bound `{p}: Hash`"),
                        _ => format!("a Map's key or a Set's element is an integer, a String, a bool or an enum without data, not {s}"),
                    },
                    "eq" => match &r {
                        Ty::Param(p) => format!("`==` compares values (numbers, strings, bools, and arrays, records, enums and tuples of them, by content), not {s}: add the bound `{p}: Eq`"),
                        _ => format!("`==` compares values (numbers, strings, bools, and arrays, records, enums and tuples of them, by content), not {s}"),
                    },
                    _ => format!("a JSX child is Jsx, String, a number or an Array of Jsx / String, found {s}"),
                };
                self.err(pos, m);
            }
        }
    }

    /// A numeric type, or a generic parameter with a numeric bound
    fn numeric(&self, t: &Ty) -> bool {
        is_num(t) || matches!(t, Ty::Param(p) if self.bounds.get(p).is_some_and(|b| num_bound(b)))
    }

    /// A type `json_encode` / `json_decode` can convert (see js.rs `json_fn`)
    /// (an answer per type within one check: the check is a conjunction, so an answer that assumed an outer type
    /// on `seen` holds whenever the whole answer is true. A type shared by many fields is checked once)
    fn jsonable(&self, t: &Ty, seen: &mut Vec<Ty>) -> bool {
        if seen.is_empty() {
            self.json_memo.borrow_mut().clear();
        }
        if let Some(&r) = self.json_memo.borrow().get(t) {
            return r;
        }
        let r = self.jsonable_inner(t, seen);
        self.json_memo.borrow_mut().insert(t.clone(), r);
        r
    }

    fn jsonable_inner(&self, t: &Ty, seen: &mut Vec<Ty>) -> bool {
        match t {
            Ty::I32 | Ty::U32 | Ty::F32 | Ty::F64 | Ty::Str | Ty::Bool => true,
            Ty::Array(e) => self.jsonable(e, seen),
            Ty::Tuple(ts) => ts.iter().all(|t| self.jsonable(t, seen)),
            Ty::Named(n, a) if (n == "Map" && a.len() == 2 || n == "Set" && a.len() == 1) && self.builtin_coll(n) => a.iter().all(|t| self.jsonable(t, seen)),
            // (`None` and `Some(None)` would both be `null`)
            Ty::Named(n, a) if n == "Option" && a.len() == 1 && matches!(&a[0], Ty::Named(m, b) if m == "Option" && b.len() == 1) => false,
            Ty::Named(n, targs) => {
                if seen.contains(t) {
                    return true;
                }
                if ty_size(t, 4096) >= 4096 {
                    return false;
                }
                let (params, fields): (Vec<String>, Vec<Ty>) = if let Some(r) = self.info.records.get(n) {
                    (r.params.clone(), r.fields.iter().map(|f| f.1.clone()).collect())
                } else if let Some(e) = self.info.enums.get(n).filter(|e| !e.is_effect()) {
                    (e.params.clone(), e.variants.iter().flat_map(|v| v.1.iter().cloned()).collect())
                } else {
                    return false;
                };
                // (a type that grows without repeating is refused before its fields are substituted, as for `==`)
                let sizes: HashMap<String, usize> = params.iter().cloned().zip(targs.iter().map(|a| ty_size(a, 4096))).collect();
                if seen.len() > 64 || fields.iter().any(|f| subst_size(f, &sizes, 4096) >= 4096) {
                    return false;
                }
                let m: HashMap<String, Ty> = params.into_iter().zip(targs.iter().cloned()).collect();
                seen.push(t.clone());
                let ok = fields.iter().all(|f| self.jsonable(&subst_named(f, &m), seen));
                seen.pop();
                ok
            }
            _ => false,
        }
    }

    /// `Map` / `Set` are the built-in collections, unless the file declares a type of that name
    fn builtin_coll(&self, n: &str) -> bool {
        !self.info.records.contains_key(n) && !self.info.enums.contains_key(n) && !self.info.ext_types.contains_key(n)
    }

    /// A type that can be a `Map`'s key or a `Set`'s element (`Hash`): integers, strings, bools and enums without
    /// data. They are the same key in JS's `Map` (by value) and in Rust's `HashMap`; floats are not (NaN)
    pub fn key_type(&self, t: &Ty) -> bool {
        match t {
            Ty::I32 | Ty::U32 | Ty::I64 | Ty::Str | Ty::Bool => true,
            Ty::Param(p) => self.bounds.get(p).is_some_and(|b| implies(b, "Hash")),
            Ty::Named(n, _) => self.info.enums.get(n).is_some_and(|e| e.unit_only() && !e.is_effect() && e.ts.as_ref().is_none_or(|t| t.key.is_none())),
            _ => false,
        }
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
            Pat::Or(alts) => {
                // (each alternative binds the same names, of the same types)
                let mut first: Option<Vec<(String, Ty)>> = None;
                for a in alts {
                    let mut b = vec![];
                    self.check_pat(a, t, pos, &mut b);
                    match &first {
                        None => first = Some(b),
                        Some(f) => {
                            let names = |v: &[(String, Ty)]| v.iter().map(|x| x.0.clone()).collect::<std::collections::BTreeSet<_>>();
                            if names(f) != names(&b) {
                                let (x, y) = (names(f).into_iter().collect::<Vec<_>>().join(", "), names(&b).into_iter().collect::<Vec<_>>().join(", "));
                                self.err(pos, format!("each pattern of an `|` binds the same names: one binds ({x}), another ({y})"));
                            }
                            for (n, ty) in &b {
                                if let Some((_, ft)) = f.iter().find(|x| x.0 == *n) {
                                    let ft = ft.clone();
                                    self.unify(pos, ty, &ft);
                                }
                            }
                        }
                    }
                }
                binds.extend(first.unwrap_or_default());
            }
            Pat::Record(n, fs, rest, rpos) => {
                let Some(r) = self.info.records.get(n) else {
                    return self.err(*rpos, format!("`{n}` is not a record type"));
                };
                let params = r.params.clone();
                let fields = r.fields.clone();
                let mut all: Vec<Ty> = fields.iter().map(|f| f.1.clone()).collect();
                all.extend(params.iter().map(|p| Ty::Param(p.clone())));
                let inst = self.instantiate(&params, &all);
                let (ftys, targs) = inst.split_at(fields.len());
                self.unify(*rpos, t, &Ty::Named(n.clone(), targs.to_vec()));
                for (k, (f, p)) in fs.iter().enumerate() {
                    if fs[..k].iter().any(|x| x.0 == *f) {
                        self.err(*rpos, format!("the field `{f}` is named twice in the pattern"));
                    }
                    match fields.iter().position(|x| x.0 == *f) {
                        Some(k) => {
                            let ft = ftys[k].clone();
                            self.check_pat(p, &ft, *rpos, binds);
                        }
                        None => self.err(*rpos, format!("`{n}` has no field `{f}`")),
                    }
                }
                let missing: Vec<&str> = fields.iter().filter(|x| !fs.iter().any(|f| f.0 == x.0)).map(|x| x.0.as_str()).collect();
                if !rest && !missing.is_empty() {
                    self.err(*rpos, format!("the pattern leaves out the field(s) {} of `{n}`: name them, or end with `..`", missing.join(", ")));
                }
            }
        }
    }

    fn exhaustive(&mut self, pos: Pos, t: &Ty, pats: &[&Pat]) {
        let irrefutable = |p: &Pat| matches!(p, Pat::Wild | Pat::Bind(_));
        if pats.iter().any(|p| irrefutable(p)) {
            return;
        }
        // (nested patterns: `A(Some(n))` and `A(None)` cover `A`, `(None, _)` and `(_, None)` the rest of a tuple's)
        let rows: Vec<Vec<&Pat>> = pats.iter().map(|p| vec![*p]).collect();
        self.cover_memo.borrow_mut().clear();
        let gap = self.uncovered(std::slice::from_ref(t), &rows);
        self.cover_memo.borrow_mut().clear();
        if !gap {
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

    /// Whether some value of the types `tys` matches none of the rows (each a pattern per type): the usefulness
    /// of a row of wildcards, splitting on the constructors of the first column (enum variants, `true` / `false`,
    /// a tuple). Numbers and strings have no finite set of them: only the rows with a wildcard there cover them.
    fn uncovered(&self, tys: &[Ty], rows: &[Vec<&Pat>]) -> bool {
        let key = (tys.to_vec(), rows.iter().map(|r| r.iter().map(|p| *p as *const Pat as usize).collect()).collect::<Vec<Vec<usize>>>());
        if let Some(&r) = self.cover_memo.borrow().get(&key) {
            return r;
        }
        let r = self.uncovered_rows(tys, rows);
        self.cover_memo.borrow_mut().insert(key, r);
        r
    }

    fn uncovered_rows(&self, tys: &[Ty], rows: &[Vec<&Pat>]) -> bool {
        let Some((t, rest)) = tys.split_first() else { return rows.is_empty() };
        // (`A | B` in the first column: a row for each)
        if rows.iter().any(|r| matches!(r[0], Pat::Or(_))) {
            let mut out: Vec<Vec<&Pat>> = vec![];
            for r in rows {
                match r[0] {
                    Pat::Or(alts) => out.extend(alts.iter().map(|a| std::iter::once(a).chain(r[1..].iter().copied()).collect())),
                    _ => out.push(r.clone()),
                }
            }
            return self.uncovered(tys, &out);
        }
        let wild = |p: &Pat| matches!(p, Pat::Wild | Pat::Bind(_));
        // (a record's fields, in their order: a record pattern is a tuple of them)
        let rec_fields: Vec<String> = match self.resolve(t) {
            Ty::Named(n, _) => self.info.records.get(&n).map(|r| r.fields.iter().map(|f| f.0.clone()).collect()).unwrap_or_default(),
            _ => vec![],
        };
        // (each constructor: its name and the types of its fields)
        let ctors: Option<Vec<(Option<String>, Vec<Ty>)>> = match self.resolve(t) {
            Ty::Named(n, targs) if self.info.enums.contains_key(&n) => {
                let def = &self.info.enums[&n];
                let m: HashMap<String, Ty> = def.params.iter().cloned().zip(targs.iter().cloned()).collect();
                Some(def.variants.iter().map(|(v, fs)| (Some(v.clone()), fs.iter().map(|f| Self::subst_params(f, &m)).collect())).collect())
            }
            Ty::Bool => Some(vec![(Some("true".into()), vec![]), (Some("false".into()), vec![])]),
            Ty::Tuple(ts) => Some(vec![(None, ts)]),
            Ty::Named(n, targs) if self.info.records.contains_key(&n) => {
                let r = &self.info.records[&n];
                let m: HashMap<String, Ty> = r.params.iter().cloned().zip(targs.iter().cloned()).collect();
                Some(vec![(None, r.fields.iter().map(|f| Self::subst_params(&f.1, &m)).collect())])
            }
            _ => None,
        };
        // (no constructor written in this column: only the rows with a wildcard there match, whatever the value. This
        // also ends the walk into a recursive type's fields)
        let Some(ctors) = ctors.filter(|_| rows.iter().any(|r| !wild(r[0]))) else {
            let rows: Vec<Vec<&Pat>> = rows.iter().filter(|r| wild(r[0])).map(|r| r[1..].to_vec()).collect();
            return self.uncovered(rest, &rows);
        };
        ctors.iter().any(|(c, fields)| {
            let rows: Vec<Vec<&Pat>> = rows
                .iter()
                .filter_map(|r| {
                    let subs: Vec<&Pat> = match (r[0], c) {
                        (p, _) if wild(p) => vec![&Pat::Wild; fields.len()],
                        (Pat::Variant(_, v, subs, _), Some(c)) if v == c && subs.len() == fields.len() => subs.iter().collect(),
                        (Pat::Lit(Lit::Bool(b)), Some(c)) if b.to_string() == *c => vec![],
                        (Pat::Tuple(ps), None) if ps.len() == fields.len() => ps.iter().collect(),
                        (Pat::Record(_, fs, _, _), None) => rec_fields.iter().map(|f| fs.iter().find(|x| x.0 == *f).map_or(&Pat::Wild, |x| &x.1)).collect(),
                        _ => return None,
                    };
                    Some(subs.into_iter().chain(r[1..].iter().copied()).collect())
                })
                .collect();
            let tys: Vec<Ty> = fields.iter().cloned().chain(rest.iter().cloned()).collect();
            self.uncovered(&tys, &rows)
        })
    }

    // ------------------------------------------------------------ expressions

    fn infer(&mut self, e: &Expr, expect: Option<&Ty>) -> Ty {
        let t = self.infer_kind(e, expect);
        if self.record_types {
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
                if self.info.fns.contains_key(n) {
                    return self.fn_value(e.id, pos, n);
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
                if (n == "json_encode" || n == "json_decode" || n == "range") && self.callee_id != Some(e.id) {
                    self.err(pos, format!("`{n}` is built in: call it (`{n}(..)`), or wrap it in a closure to pass it"));
                }
                if n == "json_encode" || n == "json_decode" {
                    let dec = n == "json_decode";
                    let t = self.fresh();
                    self.info.callee.insert(e.id, Callee::Json(dec));
                    self.jsons.push((e.id, t.clone()));
                    self.deferred.push((pos, e.id, "json", t.clone()));
                    return if dec {
                        Ty::Fn(vec![Ty::Str], Box::new(Ty::Named("Result".into(), vec![t, Ty::Str])))
                    } else {
                        Ty::Fn(vec![t], Box::new(Ty::Str))
                    };
                }
                if n == "range" {
                    self.info.callee.insert(e.id, Callee::Range);
                    return Ty::Fn(vec![Ty::I32, Ty::I32], Box::new(Ty::Array(Box::new(Ty::I32))));
                }
                self.err(pos, format!("unknown name `{n}`"));
                self.fresh()
            }
            // `Point::origin()`: a function of an `impl` (one without `self`, or one with it, called as a function)
            EK::Path(tn, c) if self.info.fns.contains_key(&format!("{tn}${c}")) => self.fn_value(e.id, pos, &format!("{tn}${c}")),
            EK::Path(tn, c) if c == "zero" || c == "one" => {
                // `T::zero()` / `i64::one()` (num-traits' constants)
                let t = match tn.as_str() {
                    "i32" => Ty::I32,
                    "u32" => Ty::U32,
                    "i64" => Ty::I64,
                    "f32" => Ty::F32,
                    "f64" => Ty::F64,
                    p if self.bounds.get(p).is_some_and(|b| num_bound(b)) => Ty::Param(p.to_string()),
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
            EK::Path(tn, c) if (tn == "Map" || tn == "Set") && self.builtin_coll(tn) => {
                if self.callee_id != Some(e.id) {
                    self.err(pos, format!("`{tn}::{c}` is built in: call it, or wrap it in a closure to pass it"));
                }
                let set = tn == "Set";
                let k = self.fresh();
                let t = if set { Ty::Named("Set".into(), vec![k.clone()]) } else { Ty::Named("Map".into(), vec![k.clone(), self.fresh()]) };
                self.deferred.push((pos, e.id, "key", k.clone()));
                self.info.callee.insert(e.id, Callee::Coll(set, c.clone()));
                match c.as_str() {
                    "new" => Ty::Fn(vec![], Box::new(t)),
                    "from" => {
                        let Ty::Named(_, a) = &t else { unreachable!() };
                        let el = if set { a[0].clone() } else { Ty::Tuple(a.clone()) };
                        Ty::Fn(vec![Ty::Array(Box::new(el))], Box::new(t))
                    }
                    _ => {
                        self.err(pos, format!("`{tn}` has `{tn}::new()` and `{tn}::from({})`", if set { "xs" } else { "pairs" }));
                        self.fresh()
                    }
                }
            }
            EK::Path(en, v) => self.variant(e.id, pos, Some(en), v),
            EK::Call(f, args) => {
                self.callee_id = Some(f.id);
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
                // a method of an extern type: JS's own
                if let Ty::Named(n, targs) = &rt
                    && let Some(x) = self.info.ext_types.get(n)
                {
                    let m: HashMap<String, Ty> = x.params.iter().cloned().zip(targs.iter().cloned()).collect();
                    let Some((_, params, ret, _)) = x.methods.iter().find(|m| m.0 == *name) else {
                        let what = if x.props.iter().any(|p| p.0 == *name) { format!("`{name}` is a property of `{n}`: read it with `.{name}`") } else { format!("`{n}` has no method `{name}` (declare it: `extern type {n} {{ fn {name}(..) -> R; }}`)") };
                        self.err(pos, what);
                        for a in args {
                            self.infer(a, None);
                        }
                        return self.fresh();
                    };
                    let (params, ret): (Vec<Ty>, Ty) = (params.iter().map(|t| Self::subst_params(t, &m)).collect(), Self::subst_params(ret, &m));
                    self.info.member.insert(e.id, (n.clone(), name.clone()));
                    if params.len() != args.len() {
                        self.err(pos, format!("`.{name}()` takes {} argument(s), found {}", params.len(), args.len()));
                    }
                    for (a, p) in args.iter().zip(&params) {
                        let t = self.infer(a, Some(p));
                        self.unify(a.pos, &t, p);
                    }
                    return ret;
                }
                // `p.len()`: a method of an `impl` of the receiver's type (its `self` is the receiver). Called as the
                // function `Point$len(p)` once checked (ast::desugar_methods)
                if let Ty::Named(n, _) = &rt
                    && let fname = format!("{n}${name}")
                    && self.info.self_methods.contains(&fname)
                {
                    if self.info.effects.get(&fname).is_some_and(|e| !e.is_empty()) {
                        self.err(pos, format!("`{name}` performs effects: a method cannot yet (call it as `{n}::{name}(x, ..)`)"));
                    }
                    let ft = self.fn_value(e.id, pos, &fname);
                    let Ty::Fn(params, ret) = ft.clone() else { unreachable!() };
                    if let Some(p0) = params.first() {
                        self.unify(recv.pos, &rt, p0);
                    }
                    if params.len() != args.len() + 1 {
                        self.err(pos, format!("`.{name}()` takes {} argument(s), found {}", params.len().saturating_sub(1), args.len()));
                    }
                    for (a, p) in args.iter().zip(params.iter().skip(1)) {
                        let t = self.infer(a, Some(p));
                        self.unify(a.pos, &t, p);
                    }
                    self.method_fns.push((e.id, fname, ft));
                    return *ret;
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
                match (b, self.resolve(&rt)) {
                    // (a number's: its JS depends on the number's type)
                    (Builtin::Std(On::Int | On::Float, _), t @ (Ty::I32 | Ty::U32 | Ty::I64 | Ty::F32 | Ty::F64 | Ty::Var(_))) => self.nums.push((e.id, t)),
                    // (by `==`, as the operator compares: by content)
                    (Builtin::Std(On::Array, "includes" | "index_of"), Ty::Array(t)) => self.deferred.push((pos, e.id, "eq", *t)),
                    (Builtin::Std(On::Array, "sort"), Ty::Array(t)) => self.deferred.push((pos, e.id, "ord", *t)),
                    _ => {}
                }
                // (an integer literal's `abs`: its type may be decided u32 later)
                match (b, self.resolve(&rt)) {
                    (Builtin::Std(On::Int, "abs"), t @ Ty::Var(_)) => self.deferred.push((pos, e.id, "signed", t)),
                    _ => {}
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
                // (the expected type decides type arguments the arguments do not: `ok_or(NotFound { .. })?` in a
                // function that returns `Result<_, FormError>`)
                if let (Builtin::Opt(_), Some(x)) = (b, expect) {
                    let x = x.clone();
                    self.unify_inner(&ret, &x);
                }
                for (a, p) in args.iter().zip(&params) {
                    let t = self.infer(a, Some(p));
                    self.unify(a.pos, &t, p);
                }
                if let (Builtin::TryMap(_), Some(Ty::Fn(_, r))) = (b, params.first()) {
                    match self.resolve(r) {
                        Ty::Named(n, a) if n == "Result" && a.len() == 2 => {
                            let want = Ty::Named(n, vec![Ty::Array(Box::new(a[0].clone())), a[1].clone()]);
                            self.unify(pos, &ret, &want);
                        }
                        Ty::Named(n, a) if n == "Option" && a.len() == 1 => {
                            self.info.builtin.insert(e.id, Builtin::TryMap(false));
                            let want = Ty::Named(n, vec![Ty::Array(Box::new(a[0].clone()))]);
                            self.unify(pos, &ret, &want);
                        }
                        o => {
                            let s = self.show(&o);
                            self.err(pos, format!("`.try_map(f)` takes a function that gives a `Result` or an `Option`, found one that gives {s}"));
                        }
                    }
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
                    Ty::Named(n, targs) if self.info.ext_types.contains_key(&n) => {
                        let x = &self.info.ext_types[&n];
                        let Some((_, t, _)) = x.props.iter().find(|p| p.0 == *f) else {
                            let what = if x.methods.iter().any(|p| p.0 == *f) { format!("`{f}` is a method of `{n}`: call it with `.{f}(..)`") } else { format!("`{n}` has no property `{f}` (declare it: `extern type {n} {{ {f}: T }}`)") };
                            self.err(pos, what);
                            return self.fresh();
                        };
                        let m: HashMap<String, Ty> = x.params.iter().cloned().zip(targs).collect();
                        let t = Self::subst_params(t, &m);
                        self.info.member.insert(e.id, (n.clone(), f.clone()));
                        t
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
                let saved = self.try_ban;
                if matches!(*op, "&&" | "||") {
                    self.try_ban = Some("`?` cannot be used on the right of `&&` / `||` (it may not run): write it as an `if` (`if ok { check(d)? } else { false }`)");
                }
                let rt = self.infer(r, Some(&lt));
                self.try_ban = saved;
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
                let saved = self.try_ban.replace("`?` cannot be used in a `while` condition (it runs again each time): test it in the body (`while true { if !more()? { break; } .. }`)");
                let ct = self.infer(c, None);
                self.try_ban = saved;
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
            EK::Loop(body) => {
                self.loops += 1;
                let bt = self.infer(body, Some(&Ty::Unit));
                self.loops -= 1;
                self.unify(body.pos, &bt, &Ty::Unit);
                // (left only by a `break`: without one it gives no value, like a `return`)
                if breaks(body) { Ty::Unit } else { self.fresh() }
            }
            EK::Return(x) => {
                if self.in_handle {
                    self.err(pos, "`return` cannot be used inside `handle` yet (the body and the arms are functions of their own in JS): bind the result with `let r = handle ..;` and decide after it, or move the body into a function");
                } else if self.try_ban.is_some() {
                    self.err(pos, "`return` cannot be used on the right of `&&` / `||` or in a `while` condition: write it as an `if` in a statement of its own");
                }
                match self.rets.last().cloned() {
                    Some((want, hint)) => {
                        // (the closure's expected result, if any, guides the value; its result is what it unifies with)
                        let hint = match (self.resolve(&want), hint) {
                            (Ty::Var(_), Some(h)) => h,
                            _ => want.clone(),
                        };
                        let (vt, vpos) = match x {
                            Some(x) => (self.infer(x, Some(&hint)), x.pos),
                            None => (Ty::Unit, pos),
                        };
                        if !self.unify_inner(&vt, &want) {
                            let (g, w) = (self.show(&vt), self.show(&want));
                            let whose = if self.closure_bases.is_empty() { "this function" } else { "the closure (a `return` in a closure returns from it)" };
                            self.err(vpos, format!("`return` gives {g}, but {whose} returns {w}"));
                        }
                    }
                    None => self.err(pos, "`return` outside a function"),
                }
                // (it does not continue: any type)
                self.fresh()
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
                // (a handler's body and arms run on generators or plain functions in JS: not async)
                let was_async = std::mem::replace(&mut self.in_async, false);
                let was_handle = std::mem::replace(&mut self.in_handle, true);
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
                self.in_async = was_async;
                self.in_handle = was_handle;
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
                for a in arms {
                    let mut binds = vec![];
                    self.check_pat(&a.pat, &st, a.body.pos, &mut binds);
                    self.scopes.push(binds.into_iter().collect());
                    if let Some(g) = &a.guard {
                        // (JS takes the names from the first alternative that matches; Rust tries the next one when
                        // the guard fails: an `|` that binds names has no guard yet)
                        if has_or(&a.pat) && !crate::own::pat_names(&a.pat).is_empty() {
                            self.err(g.pos, "a guard on an `|` pattern that binds names: write an arm for each alternative");
                        }
                        let gt = self.infer(g, Some(&Ty::Bool));
                        self.unify(g.pos, &gt, &Ty::Bool);
                    }
                    let bt = self.infer(&a.body, Some(&out));
                    self.unify(a.body.pos, &bt, &out);
                    self.pop_scope();
                }
                // (an arm with a guard may not match: it covers nothing)
                let pats: Vec<&Pat> = arms.iter().filter(|a| a.guard.is_none()).map(|a| &a.pat).collect();
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
                            } else if has_or(p) {
                                self.err(x.pos, "an `|` pattern in a `let`: use `match`");
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
                    // (`{ ..; return x; }` does not end: any type, as a `return` is)
                    None if stmts.last().is_some_and(|s| matches!(s, Stmt::Expr(x) if diverges(x))) => self.fresh(),
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
                // (a closure is not async unless it is written `async (..) =>`: `await` does not reach into it)
                let was_async = std::mem::replace(&mut self.in_async, std::mem::take(&mut self.next_closure_async));
                let is_async = self.in_async;
                // (a `?` inside returns from the closure: from a plain function in JS, wherever the closure is)
                let saved = (std::mem::replace(&mut self.in_handle, false), self.try_ban.take(), std::mem::replace(&mut self.closure_try, false));
                let rv = self.fresh();
                self.rets.push((rv.clone(), rexp.clone()));
                self.scopes.push(sc);
                let rt = self.infer(body, rexp.as_ref());
                // (an async closure's body may give a Promise, which JS adopts: its value is the result)
                let rt = if is_async { promised(&self.resolve(&rt)).unwrap_or(rt) } else { rt };
                if !self.unify_inner(&rt, &rv) {
                    let (g, w) = (self.show(&rt), self.show(&rv));
                    if self.closure_try {
                        // (`Result<_, String>`: the part no one decided)
                        let w = w.split('?').enumerate().map(|(k, p)| if k == 0 { p.to_string() } else { format!("_{}", p.trim_start_matches(|c: char| c.is_ascii_digit())) }).collect::<String>();
                        self.err(body_pos(body, body.pos), format!("a `?` here returns from the closure, so the closure returns `{w}`, but its body gives {g} (to stop at the first error of an array's elements, use `.try_map((x) => ..)`)"));
                    } else {
                        self.err(body_pos(body, body.pos), format!("type mismatch: expected {w}, found {g}"));
                    }
                }
                self.pop_scope();
                self.rets.pop();
                (self.in_handle, self.try_ban, self.closure_try) = saved;
                self.in_async = was_async;
                self.loops = loops;
                self.closure_bases.pop();
                Ty::Fn(pts, Box::new(rv))
            }
            EK::Try(x) => self.try_(e.id, pos, x),
            EK::Await(x) => {
                if !self.in_async {
                    self.err(pos, "`await` is only allowed in an `async fn` or an `async (..) =>` closure");
                }
                let v = self.fresh();
                let pt = promise(v.clone());
                let t = self.infer(x, Some(&pt));
                self.unify(x.pos, &t, &pt);
                v
            }
            EK::Async(c) => {
                // (the closure inside is checked against the expected function with the Promise taken off its result)
                // (the parameters' types are kept even while the expected result is still unknown)
                let inner = match expect.map(|t| self.resolve(t)) {
                    Some(Ty::Fn(a, r)) => {
                        let r = promised(&self.resolve(&r)).unwrap_or_else(|| self.fresh());
                        Some(Ty::Fn(a, Box::new(r)))
                    }
                    _ => None,
                };
                self.next_closure_async = true;
                match self.infer(c, inner.as_ref()) {
                    // (a body that gives a Promise: JS adopts it, so the closure gives that Promise, not one of it)
                    Ty::Fn(a, r) => {
                        let r = self.resolve(&r);
                        let r = if promised(&r).is_some() { r } else { promise(r) };
                        Ty::Fn(a, Box::new(r))
                    }
                    o => o,
                }
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
            // (a hole is a string, or what has `to_string`: a number or a bool, written as `to_string` writes it.
            // Checked when the function's types are known: a closure's parameter may be decided by a later argument)
            EK::Interp(_, holes) => {
                for h in holes {
                    let t = self.infer(h, None);
                    self.deferred.push((h.pos, h.id, "hole", t));
                }
                Ty::Str
            }
            EK::Jsx(j) => {
                self.jsx(j);
                Ty::Jsx
            }
        }
    }

    /// `x?`: the value inside the `Result` / `Option` `x`; its `Err` / `None` is returned as it is, so the
    /// function's result must be a `Result` with the same error type (no conversion), or an `Option`
    /// A top-level function as a value at `id`: its type, instantiated (the numeric and other bounds checked)
    fn fn_value(&mut self, id: Id, pos: Pos, n: &str) -> Ty {
        let s = &self.info.fns[n];
        let (g, mut ts, bounds) = (s.generics.clone(), s.args.clone(), s.bounds.clone());
        ts.push(s.ret.clone());
        // (the generic parameters themselves go last, to learn what they were instantiated to)
        let nts = ts.len();
        ts.extend(g.iter().map(|p| Ty::Param(p.clone())));
        let mut inst = self.instantiate(&g, &ts);
        let targs = inst.split_off(nts);
        let r = inst.pop().unwrap();
        if !g.is_empty() && self.record_types {
            self.rust_insts.push((id, targs.clone()));
        }
        if !bounds.is_empty() {
            for (p, t) in g.iter().zip(&targs) {
                if let Some(b) = bounds.get(p) {
                    self.bound_checks.push((pos, t.clone(), b.clone()));
                }
            }
            self.insts.push((id, targs));
        }
        self.info.callee.insert(id, Callee::Fn);
        Ty::Fn(inst, Box::new(r))
    }

    fn try_(&mut self, id: Id, pos: Pos, x: &Expr) -> Ty {
        if self.in_handle {
            self.err(pos, "`?` cannot be used inside `handle` yet (the body and the arms are functions of their own in JS): bind the result with `let r = handle ..;` and use `r?` after it, or move the body into a function that returns `Result`");
        }
        if let Some(m) = self.try_ban {
            self.err(pos, m);
        }
        let r = self.rets.last().cloned().map(|(t, x)| {
            let t = self.resolve(&t);
            match (&t, x.map(|x| self.resolve(&x))) {
                (Ty::Var(_), Some(x @ Ty::Named(..))) => x,
                _ => t,
            }
        });
        // (what `x` is expected to be: the result's kind, with its error type)
        let shape = match &r {
            Some(Ty::Named(n, a)) if n == "Result" && a.len() == 2 => Some(Ty::Named(n.clone(), vec![self.fresh(), a[1].clone()])),
            Some(Ty::Named(n, a)) if n == "Option" && a.len() == 1 => Some(Ty::Named(n.clone(), vec![self.fresh()])),
            _ => None,
        };
        let t = self.infer(x, shape.as_ref());
        let t = self.resolve(&t);
        let (kind, inner) = match &t {
            Ty::Named(n, a) if (n == "Result" && a.len() == 2) || (n == "Option" && a.len() == 1) => (n.clone(), a[0].clone()),
            Ty::Var(_) => {
                self.err(pos, "the value before `?` must be known to be an `Option` or a `Result` here: add an annotation");
                return self.fresh();
            }
            o => {
                let s = self.show(o);
                self.err(pos, format!("`?` takes an `Option` or a `Result`, found {s}"));
                return self.fresh();
            }
        };
        self.info.tries.insert(id, kind.clone());
        let closure = !self.closure_bases.is_empty();
        let whose = if closure { "the closure (a `?` in a closure returns from it, not from the function around it)" } else { "this function" };
        match &r {
            // (a closure whose result is not known yet: the `?` makes it this kind)
            Some(rv @ Ty::Var(_)) => {
                let want = match &t {
                    Ty::Named(_, a) if kind == "Result" => Ty::Named(kind.clone(), vec![self.fresh(), a[1].clone()]),
                    _ => Ty::Named(kind.clone(), vec![self.fresh()]),
                };
                let rv = rv.clone();
                self.unify(pos, &want, &rv);
                self.closure_try = true;
            }
            Some(Ty::Named(n, a)) if *n == kind && n == "Result" && a.len() == 2 => {
                let Ty::Named(_, ta) = &t else { unreachable!() };
                if !self.unify_inner(&ta[1], &a[1]) {
                    let (e, w) = (self.show(&ta[1]), self.show(&Ty::Named(n.clone(), a.clone())));
                    self.err(pos, format!("`?` passes on a `{e}`, but {whose} returns `{w}` (no conversion is done: convert it first, e.g. `.map_err((e) => ..)?`)"));
                }
            }
            Some(Ty::Named(n, a)) if *n == kind && a.len() == if n == "Result" { 2 } else { 1 } => {}
            Some(Ty::Named(n, a)) if n == "Result" && a.len() == 2 => {
                self.err(pos, format!("`?` on an `Option` in {whose}, which returns `Result`: turn it into a `Result` with `.ok_or(e)?`"));
            }
            Some(Ty::Named(n, a)) if n == "Option" && a.len() == 1 => {
                self.err(pos, format!("`?` on a `Result` in {whose}, which returns `Option`: drop the error with `.ok()?`"));
            }
            Some(o) => {
                let s = self.show(o);
                let hint = if closure { " (to stop at the first error of an array's elements, use `.try_map((x) => ..)`)" } else { "" };
                self.err(pos, format!("`?` can be used only where the result is an `Option` or a `Result`: {whose} returns {s}{hint}"));
            }
            None => self.err(pos, "`?` outside a function"),
        }
        inner
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
            // (a component may be imported from another module: an `extern` of its JS)
            let sig = self.info.fns.get(&j.tag).or_else(|| self.info.externs.get(&j.tag).map(|e| &e.0));
            match sig.map(|s| (s.args.clone(), s.ret.clone(), s.generics.is_empty())) {
                Some((args, ret, true)) if args.len() <= 1 && ret == Ty::Jsx => {
                    let fields = match args.first() {
                        // (a generic record of props: its fields at the component's type arguments)
                        Some(Ty::Named(n, targs)) if self.info.records.contains_key(n) => {
                            let r = &self.info.records[n];
                            let m: HashMap<String, Ty> = r.params.iter().cloned().zip(targs.iter().cloned()).collect();
                            r.fields.iter().map(|(f, t)| (f.clone(), Self::subst_params(t, &m))).collect()
                        }
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
            (Ty::Param(p), "to_string") if self.bounds.get(p).is_some_and(|b| num_bound(b)) => (vec![], Ty::Str, B::ToString),
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
            // (the prelude's, named as Rust names them)
            (Ty::Named(n, a), m) if n == "Option" && a.len() == 1 => {
                let t = a[0].clone();
                let opt = |t: Ty| Ty::Named("Option".into(), vec![t]);
                let res = |t: Ty, e: Ty| Ty::Named("Result".into(), vec![t, e]);
                let (u, e) = (self.fresh(), self.fresh());
                match m {
                    "map" => (vec![f(vec![t], u.clone())], opt(u), B::Opt("map")),
                    "and_then" => (vec![f(vec![t], opt(u.clone()))], opt(u), B::Opt("and_then")),
                    "filter" => (vec![f(vec![t.clone()], Ty::Bool)], opt(t), B::Opt("filter")),
                    "unwrap_or" => (vec![t.clone()], t, B::Opt("unwrap_or")),
                    "unwrap_or_else" => (vec![f(vec![], t.clone())], t, B::Opt("unwrap_or_else")),
                    "ok_or" => (vec![e.clone()], res(t, e), B::Opt("ok_or")),
                    "ok_or_else" => (vec![f(vec![], e.clone())], res(t, e), B::Opt("ok_or_else")),
                    "is_some" => (vec![], Ty::Bool, B::Opt("is_some")),
                    "is_none" => (vec![], Ty::Bool, B::Opt("is_none")),
                    _ => return None,
                }
            }
            (Ty::Named(n, a), m) if n == "Result" && a.len() == 2 => {
                let (t, e) = (a[0].clone(), a[1].clone());
                let res = |t: Ty, e: Ty| Ty::Named("Result".into(), vec![t, e]);
                let u = self.fresh();
                match m {
                    "map" => (vec![f(vec![t], u.clone())], res(u, e), B::Opt("map")),
                    "and_then" => (vec![f(vec![t], res(u.clone(), e.clone()))], res(u, e), B::Opt("and_then")),
                    "map_err" => (vec![f(vec![e], u.clone())], res(t, u), B::Opt("map_err")),
                    "unwrap_or" => (vec![t.clone()], t, B::Opt("unwrap_or")),
                    "unwrap_or_else" => (vec![f(vec![e], t.clone())], t, B::Opt("unwrap_or_else")),
                    "ok" => (vec![], Ty::Named("Option".into(), vec![t]), B::Opt("ok")),
                    "is_ok" => (vec![], Ty::Bool, B::Opt("is_ok")),
                    "is_err" => (vec![], Ty::Bool, B::Opt("is_err")),
                    _ => return None,
                }
            }
            // (whether it is a Result's or an Option's is decided by `f`'s result, after the arguments)
            (Ty::Array(t), "try_map") => {
                let r = self.fresh();
                (vec![f(vec![(**t).clone()], r)], self.fresh(), B::TryMap(true))
            }
            _ => return self.std_sig(recv, name),
        })
    }

    /// The standard library's methods beyond the first ones: arrays, strings, numbers. Each means the same in JS
    /// and in `@wasm` (indices and lengths of strings in UTF-16 code units, as JS's)
    fn std_sig(&mut self, recv: &Ty, name: &str) -> Option<(Vec<Ty>, Ty, Builtin)> {
        use Builtin as B;
        let f = |a: Vec<Ty>, r: Ty| Ty::Fn(a, Box::new(r));
        let opt = |t: Ty| Ty::Named("Option".into(), vec![t]);
        let intv = matches!(recv, Ty::Var(v) if self.intv.contains(v));
        let floatv = matches!(recv, Ty::Var(v) if self.floatv.contains(v));
        let r = recv.clone();
        const ARRAY: &[&str] = &["get", "first", "last", "is_empty", "slice", "reverse", "includes", "index_of", "find", "find_index", "flat_map", "sort", "sort_by"];
        const STRING: &[&str] = &["is_empty", "to_lower", "starts_with", "ends_with", "index_of", "slice", "replace", "repeat", "trim_start", "trim_end", "chars", "to_i32", "to_f64"];
        const FLOAT: &[&str] = &["ceil", "trunc", "pow", "exp", "ln", "log10", "log2", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "is_nan"];
        const INT: &[&str] = &["abs", "min", "max", "pow"];
        let (on, name, sig) = match recv {
            Ty::Array(t) => {
                let name = *ARRAY.iter().find(|n| **n == name)?;
                let t = (**t).clone();
                let sig = match name {
                    "get" => (vec![Ty::I32], opt(t)),
                    "first" | "last" => (vec![], opt(t)),
                    "is_empty" => (vec![], Ty::Bool),
                    "slice" => (vec![Ty::I32, Ty::I32], r),
                    "reverse" | "sort" => (vec![], r),
                    "includes" => (vec![t], Ty::Bool),
                    "index_of" => (vec![t], opt(Ty::I32)),
                    "find" => (vec![f(vec![t.clone()], Ty::Bool)], opt(t)),
                    "find_index" => (vec![f(vec![t], Ty::Bool)], opt(Ty::I32)),
                    "flat_map" => {
                        let u = Ty::Array(Box::new(self.fresh()));
                        (vec![f(vec![t], u.clone())], u)
                    }
                    _ => (vec![f(vec![t.clone(), t], Ty::I32)], r),
                };
                (On::Array, name, sig)
            }
            Ty::Str => {
                let name = *STRING.iter().find(|n| **n == name)?;
                let sig = match name {
                    "is_empty" => (vec![], Ty::Bool),
                    "starts_with" | "ends_with" => (vec![Ty::Str], Ty::Bool),
                    "index_of" => (vec![Ty::Str], opt(Ty::I32)),
                    "slice" => (vec![Ty::I32, Ty::I32], Ty::Str),
                    "replace" => (vec![Ty::Str, Ty::Str], Ty::Str),
                    "repeat" => (vec![Ty::I32], Ty::Str),
                    "chars" => (vec![], Ty::Array(Box::new(Ty::Str))),
                    "to_i32" => (vec![], opt(Ty::I32)),
                    "to_f64" => (vec![], opt(Ty::F64)),
                    _ => (vec![], Ty::Str),
                };
                (On::Str, name, sig)
            }
            Ty::F32 | Ty::F64 => {
                let name = *FLOAT.iter().find(|n| **n == name)?;
                let sig = match name {
                    "pow" | "atan2" => (vec![r.clone()], r),
                    "is_nan" => (vec![], Ty::Bool),
                    _ => (vec![], r),
                };
                (On::Float, name, sig)
            }
            _ if floatv => {
                let name = *FLOAT.iter().find(|n| **n == name)?;
                (On::Float, name, if name == "is_nan" { (vec![], Ty::Bool) } else if name == "pow" || name == "atan2" { (vec![r.clone()], r) } else { (vec![], r) })
            }
            Ty::I32 | Ty::U32 | Ty::I64 => {
                let name = *INT.iter().find(|n| **n == name)?;
                if name == "abs" && *recv == Ty::U32 {
                    return None;
                }
                (On::Int, name, if name == "pow" { (vec![Ty::I32], r) } else if name == "abs" { (vec![], r) } else { (vec![r.clone()], r) })
            }
            Ty::Named(n, a) if n == "Map" && a.len() == 2 && self.builtin_coll("Map") => {
                const MAP: &[&str] = &["len", "is_empty", "get", "contains_key", "insert", "remove", "keys", "values", "entries"];
                let name = *MAP.iter().find(|n| **n == name)?;
                let (k, v) = (a[0].clone(), a[1].clone());
                let sig = match name {
                    "len" => (vec![], Ty::I32),
                    "is_empty" => (vec![], Ty::Bool),
                    "get" => (vec![k], opt(v)),
                    "contains_key" => (vec![k], Ty::Bool),
                    "insert" => (vec![k, v], r),
                    "remove" => (vec![k], r),
                    "keys" => (vec![], Ty::Array(Box::new(k))),
                    "values" => (vec![], Ty::Array(Box::new(v))),
                    _ => (vec![], Ty::Array(Box::new(Ty::Tuple(vec![k, v])))),
                };
                (On::Map, name, sig)
            }
            Ty::Named(n, a) if n == "Set" && a.len() == 1 && self.builtin_coll("Set") => {
                const SET: &[&str] = &["len", "is_empty", "contains", "insert", "remove", "values"];
                let name = *SET.iter().find(|n| **n == name)?;
                let t = a[0].clone();
                let sig = match name {
                    "len" => (vec![], Ty::I32),
                    "is_empty" => (vec![], Ty::Bool),
                    "contains" => (vec![t], Ty::Bool),
                    "insert" | "remove" => (vec![t], r),
                    _ => (vec![], Ty::Array(Box::new(t))),
                };
                (On::Set, name, sig)
            }
            _ if intv => {
                let name = *INT.iter().find(|n| **n == name)?;
                (On::Int, name, if name == "pow" { (vec![Ty::I32], r) } else if name == "abs" { (vec![], r) } else { (vec![r.clone()], r) })
            }
            _ => return None,
        };
        Some((sig.0, sig.1, B::Std(on, name)))
    }
}


/// An `|` somewhere in the pattern
fn has_or(p: &Pat) -> bool {
    match p {
        Pat::Or(_) => true,
        Pat::Tuple(ps) | Pat::Variant(_, _, ps, _) => ps.iter().any(has_or),
        Pat::Record(_, fs, _, _) => fs.iter().any(|f| has_or(&f.1)),
        Pat::Wild | Pat::Bind(_) | Pat::Lit(_) => false,
    }
}

fn irrefutable(p: &Pat) -> bool {
    match p {
        Pat::Wild | Pat::Bind(_) => true,
        Pat::Tuple(ps) => ps.iter().all(irrefutable),
        Pat::Record(_, fs, _, _) => fs.iter().all(|f| irrefutable(&f.1)),
        Pat::Or(alts) => alts.iter().any(irrefutable),
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
            EK::Await(x) | EK::Async(x) | EK::Try(x) => out.extend(self.expr(x)),
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
                    // (`P::f(p)`: the function `P$f` of an `impl`)
                    EK::Path(tn, c) if self.info.effects.contains_key(&format!("{tn}${c}")) => out.extend(self.info.effects[&format!("{tn}${c}")].iter().cloned()),
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
            EK::Path(tn, c) => {
                if self.info.effects.get(&format!("{tn}${c}")).is_some_and(|s| !s.is_empty()) {
                    self.errors.push((e.pos, format!("`{tn}::{c}` performs effects and cannot be used as a value (call it, or handle its effects in a wrapper)")));
                }
            }
            EK::Lit(_) => {}
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
            EK::Assign(_, x) | EK::Loop(x) | EK::Return(Some(x)) => out.extend(self.expr(x)),
            EK::Break | EK::Continue | EK::Return(None) => {}
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
                for a in arms {
                    if let Some(g) = &a.guard {
                        out.extend(self.expr(g));
                    }
                    out.extend(self.expr(&a.body));
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
            EK::Array(xs) | EK::Tuple(xs) | EK::Interp(_, xs) => {
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
/// - it does not leave an exported function (TS callers drive the generator of such a function),
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
        EK::Match(sc, arms) => !mentions(sc, k) && arms.iter().all(|a| a.guard.as_ref().is_none_or(|g| !mentions(g, k)) && tail_resumes(&a.body, k)),
        _ => false,
    }
}

fn mentions(e: &Expr, k: &str) -> bool {
    matches!(&e.kind, EK::Var(n) if n == k) || subexprs(e).into_iter().any(|c| mentions(c, k))
}

/// `e` never gives a value: it always leaves by `return`, `break` or `continue`, or loops for ever
pub fn diverges(e: &Expr) -> bool {
    match &e.kind {
        EK::Return(_) | EK::Break | EK::Continue => true,
        EK::Loop(b) => !breaks(b),
        EK::Block(stmts, tail) => match tail {
            Some(t) => diverges(t),
            None => stmts.last().is_some_and(|s| matches!(s, Stmt::Expr(x) if diverges(x))),
        },
        EK::If(_, t, Some(el)) => diverges(t) && diverges(el),
        EK::Match(_, arms) => !arms.is_empty() && arms.iter().all(|a| diverges(&a.body)),
        _ => false,
    }
}

/// A `break` in `body` that leaves the loop `body` belongs to (not one of its own loops, not in a closure)
pub fn breaks(body: &Expr) -> bool {
    match &body.kind {
        EK::Break => true,
        // (a `for`'s iterable is evaluated outside its loop: a `break` there leaves this one)
        EK::For(_, it, _) => breaks(it),
        EK::While(..) | EK::Loop(_) | EK::Closure(..) | EK::Async(_) => false,
        _ => subexprs(body).into_iter().any(breaks),
    }
}

/// The direct sub-expressions of `e`
pub fn subexprs(e: &Expr) -> Vec<&Expr> {
    let mut v: Vec<&Expr> = vec![];
    match &e.kind {
        EK::Lit(_) | EK::Var(_) | EK::Path(..) | EK::Break | EK::Continue | EK::Return(None) => {}
        EK::Await(x) | EK::Async(x) | EK::Try(x) | EK::Loop(x) | EK::Return(Some(x)) => v.push(x),
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
            for a in arms {
                v.extend(a.guard.as_ref());
                v.push(&a.body);
            }
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
        EK::Array(xs) | EK::Tuple(xs) | EK::Interp(_, xs) => v.extend(xs),
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
