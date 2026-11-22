//! `@simd fn`: array kernels vectorized with wasm's v128 (f64x2, f32x4, i32x4). zsc writes this wasm module itself:
//! zustc's IR has no vector types (issue: a vector IR in zust), and the prototype does not change zust's crates.
//!
//! A kernel's body (after `let`s of scalars) is a loop over an array parameter or over `range(0, N)`:
//!
//! ```text
//! xs.map((x) => E)                                        // elementwise
//! range(0, n).map((i) => E)                               // E reads a[i], b[i + off], ..
//! xs.fold(init, (acc, x) => acc + E)                      // a sum
//! range(0, n).fold(init, (s, i) => s + E)                 // a dot product and the like
//! range(0, m).map((r) => F)                               // F: an expression around one inner fold, which
//!                                                         //   reads rows: w[r * n + i] (a matrix-vector product)
//! ```
//!
//! E is `+ - *` (and, for floats, `/` `sqrt` `abs` `max` `min`), unary `-`, elements, scalars, literals. N is an i32
//! parameter, a literal or `a.len()`; an index is `i`, or `off + i` where `off` is affine in the outer `r` (`r * n
//! + k`: i32 parameters and literals). The arrays are `F64Array` / `F32Array` / `I32Array` or `Array<..>`; a kernel
//! computes in one element type (2, 4 or 4 lanes); i32 parameters may also be counts and offsets.
//!
//! Meaning, against the same function as JS (`zsc --all-js`) or through zustc (`--no-simd`):
//! - `map` and i32 sums: the same results;
//! - a float sum adds each lane on its own and the lanes at the end: regrouped, the last bits can differ;
//! - `@simd(relaxed)` also allows relaxed SIMD: `a * b + c` as one fused multiply-add (`relaxed_madd`: rounded once
//!   or twice, as the engine likes), `max` / `min` as `relaxed_max` / `relaxed_min` (NaN and ±0 as the engine likes);
//! - a read past an array's end is an error before the loop starts: the glue checks every index's range (an
//!   offset is affine in `r`, so its values at r = 0 and r = m - 1 bound it).
//!
//! Memory: no allocator. For each call the glue lays the arrays out from offset 0 (16-byte aligned), grows the
//! memory if they do not fit and copies the result out (a typed array with `.slice()`): an arena per call.

use crate::ast::*;
use crate::check::{Builtin, Callee, Info, Ty};
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Debug)]
enum El {
    F64,
    F32,
    I32,
}

impl El {
    fn lanes(self) -> u32 {
        match self {
            El::F64 => 2,
            _ => 4,
        }
    }
    fn shift(self) -> i32 {
        match self {
            El::F64 => 3,
            _ => 2,
        }
    }
    fn size(self) -> u32 {
        1 << self.shift()
    }
    fn valtype(self) -> u8 {
        match self {
            El::F64 => 0x7C,
            El::F32 => 0x7D,
            El::I32 => 0x7F,
        }
    }
    fn float(self) -> bool {
        self != El::I32
    }
    fn js_array(self) -> &'static str {
        match self {
            El::F64 => "Float64Array",
            El::F32 => "Float32Array",
            El::I32 => "Int32Array",
        }
    }
    fn ty(self) -> Ty {
        match self {
            El::F64 => Ty::F64,
            El::F32 => Ty::F32,
            El::I32 => Ty::I32,
        }
    }
}

/// An i32 index expression (an offset): over the outer `r`, i32 parameters and literals
#[derive(Clone, Debug, PartialEq)]
enum IE {
    Outer,
    /// (parameter index)
    Param(usize),
    Const(i32),
    Add(Box<IE>, Box<IE>),
    Mul(Box<IE>, Box<IE>),
}

impl IE {
    /// its degree in `r` (an offset must be affine: degree <= 1)
    fn degree(&self) -> u32 {
        match self {
            IE::Outer => 1,
            IE::Param(_) | IE::Const(_) => 0,
            IE::Add(a, b) => a.degree().max(b.degree()),
            IE::Mul(a, b) => a.degree() + b.degree(),
        }
    }
    /// as JS, with `r` as given
    fn js(&self, params: &[(String, PK)], r: &str) -> String {
        match self {
            IE::Outer => r.to_string(),
            IE::Param(k) => params[*k].0.clone(),
            IE::Const(c) => c.to_string(),
            IE::Add(a, b) => format!("({} + {})", a.js(params, r), b.js(params, r)),
            IE::Mul(a, b) => format!("({} * {})", a.js(params, r), b.js(params, r)),
        }
    }
}

/// A loop count: `range(0, N)` / an array's length
#[derive(Clone, Debug)]
enum Count {
    Param(usize),
    Len(usize),
    Const(i32),
}

impl Count {
    fn js(&self, params: &[(String, PK)]) -> String {
        match self {
            Count::Param(k) => format!("Math.max({}, 0)", params[*k].0),
            Count::Len(k) => format!("{}.length", params[*k].0),
            Count::Const(c) => (*c).max(0).to_string(),
        }
    }
}

/// An elementwise expression
#[derive(Clone, Debug)]
enum VE {
    /// array parameter k at the inner index (+ an offset)
    Elem(usize, Option<IE>),
    /// array parameter k at an index over `r` only (read once per outer iteration)
    OuterElem(usize, IE),
    /// a scalar: (parameter or `let` index in `slots`)
    Scalar(usize),
    Const(f64),
    IConst(i32),
    Bin(&'static str, Box<VE>, Box<VE>),
    Neg(Box<VE>),
    Sqrt(Box<VE>),
    Abs(Box<VE>),
    Max(Box<VE>, Box<VE>),
    Min(Box<VE>, Box<VE>),
    /// (in an outer map's body) the inner fold's value
    Inner,
}

/// A parameter's kind
#[derive(Clone, Copy, Debug, PartialEq)]
enum PK {
    Array,
    /// a scalar of the kernel's element type
    Scalar,
    /// an i32 count / offset (in a float kernel)
    Index,
}

#[derive(Clone)]
struct Inner {
    count: Count,
    /// None: map; Some(init): sum
    fold: Option<VE>,
    body: VE,
}

#[derive(Clone)]
struct Kernel {
    name: String,
    public: bool,
    relaxed: bool,
    el: El,
    params: Vec<(String, PK)>,
    /// `let`s before the loops: slot index (after the parameters) → expression
    lets: Vec<VE>,
    /// `range(0, M).map((r) => F)`: M and F
    outer: Option<(Count, VE)>,
    inner: Inner,
    /// the result of a map is a typed array (else a plain array)
    typed_result: bool,
    /// the reads, for the glue's bounds checks: (array, inner offset, or an index over `r` only)
    reads: Vec<(usize, Option<IE>, Option<IE>)>,
}

type SResult<X> = Result<X, (Pos, String)>;

fn no<X>(pos: Pos, what: &str) -> SResult<X> {
    Err((
        pos,
        format!(
            "@simd: {what} (a kernel is a map / sum over an array or `range(0, n)`, or `range(0, m).map((r) => ..)` around one inner sum; arithmetic on one numeric type)"
        ),
    ))
}

struct Ctx<'a> {
    info: &'a Info,
    el: El,
    params: Vec<(String, PK)>,
    /// element-typed scalars in scope → slot
    scalars: HashMap<String, usize>,
    nslots: usize,
    /// the outer loop's variable
    outer_var: Option<String>,
    /// in the inner loop: (lane variable, its array) or (index variable, None)
    lane: Option<(String, Option<usize>)>,
    inner: Option<Inner>,
    reads: Vec<(usize, Option<IE>, Option<IE>)>,
}

impl Ctx<'_> {
    fn array_param(&self, e: &Expr) -> Option<usize> {
        match &e.kind {
            EK::Var(n) => self
                .params
                .iter()
                .position(|p| p.0 == *n && p.1 == PK::Array),
            _ => None,
        }
    }

    fn int_param(&self, k: usize) -> bool {
        self.params[k].1 == PK::Index || (self.el == El::I32 && self.params[k].1 == PK::Scalar)
    }

    fn count(&self, e: &Expr) -> SResult<Count> {
        match &e.kind {
            EK::Lit(Lit::Int(n)) => Ok(Count::Const(*n as i32)),
            EK::Var(n) => match self
                .params
                .iter()
                .position(|p| p.0 == *n && p.1 != PK::Array)
            {
                Some(k) if self.int_param(k) => Ok(Count::Param(k)),
                _ => no(
                    e.pos,
                    "a count other than an i32 parameter, a literal or `a.len()`",
                ),
            },
            EK::Method(a, _, _) if self.info.builtin.get(&e.id) == Some(&Builtin::Len) => {
                match self.array_param(a) {
                    Some(k) => Ok(Count::Len(k)),
                    None => no(
                        e.pos,
                        "the length of something other than an array parameter",
                    ),
                }
            }
            _ => no(
                e.pos,
                "a count other than an i32 parameter, a literal or `a.len()`",
            ),
        }
    }

    /// An i32 index expression (no inner index in it)
    fn ie(&self, e: &Expr) -> SResult<IE> {
        Ok(match &e.kind {
            EK::Lit(Lit::Int(n)) => IE::Const(*n as i32),
            EK::Var(n) if self.outer_var.as_deref() == Some(n.as_str()) => IE::Outer,
            EK::Var(n) => match self.params.iter().position(|p| p.0 == *n) {
                Some(k) if self.int_param(k) => IE::Param(k),
                _ => return no(e.pos, &format!("`{n}` in an index")),
            },
            EK::Bin("+", a, b) => IE::Add(Box::new(self.ie(a)?), Box::new(self.ie(b)?)),
            EK::Bin("*", a, b) => IE::Mul(Box::new(self.ie(a)?), Box::new(self.ie(b)?)),
            _ => {
                return no(
                    e.pos,
                    "this index (an index is `i` or `off + i`, `off` affine in `r`)",
                );
            }
        })
    }

    fn is_index_var(&self, e: &Expr) -> bool {
        matches!((&self.lane, &e.kind), (Some((x, None)), EK::Var(v)) if x == v)
    }

    fn ve(&mut self, e: &Expr) -> SResult<VE> {
        let pos = e.pos;
        let float = self.el.float();
        Ok(match &e.kind {
            EK::Lit(Lit::Int(n)) => match self.el {
                El::I32 => VE::IConst(*n as i32),
                _ => return no(pos, "an integer literal in a float kernel"),
            },
            EK::Lit(Lit::Float(f)) if float => VE::Const(f.parse().unwrap_or(0.0)),
            EK::Var(n) => {
                if let Some((x, Some(k))) = &self.lane
                    && x == n
                {
                    return Ok(VE::Elem(*k, None));
                }
                match self.scalars.get(n) {
                    Some(s) => VE::Scalar(*s),
                    None => return no(pos, &format!("`{n}` as a value here")),
                }
            }
            EK::Index(a, i) => {
                let Some(k) = self.array_param(a) else {
                    return no(pos, "indexing something other than an array parameter");
                };
                if self.lane.is_some() {
                    if matches!(self.lane, Some((_, Some(_)))) {
                        return no(
                            pos,
                            "indexing inside `xs.map` / `xs.fold` (loop over `range(0, xs.len())`)",
                        );
                    }
                    // the inner loop: `a[i]`, `a[off + i]`, `a[i + off]`
                    let off = if self.is_index_var(i) {
                        None
                    } else {
                        match &i.kind {
                            EK::Bin("+", l, r) if self.is_index_var(r) => Some(self.ie(l)?),
                            EK::Bin("+", l, r) if self.is_index_var(l) => Some(self.ie(r)?),
                            _ => {
                                return no(
                                    pos,
                                    "an index other than `i` or `off + i` in the inner loop",
                                );
                            }
                        }
                    };
                    if off.as_ref().is_some_and(|o| o.degree() > 1) {
                        return no(pos, "an offset that is not affine in `r`");
                    }
                    self.reads.push((k, off.clone(), None));
                    VE::Elem(k, off)
                } else if self.outer_var.is_some() {
                    let ix = self.ie(i)?;
                    if ix.degree() > 1 {
                        return no(pos, "an index that is not affine in `r`");
                    }
                    self.reads.push((k, None, Some(ix.clone())));
                    VE::OuterElem(k, ix)
                } else {
                    return no(pos, "indexing outside a loop");
                }
            }
            EK::Bin(op @ ("+" | "-" | "*" | "/"), l, r) => {
                if *op == "/" && !float {
                    return no(pos, "i32 division (wasm has no vector integer division)");
                }
                VE::Bin(op, Box::new(self.ve(l)?), Box::new(self.ve(r)?))
            }
            EK::Un("-", x) => VE::Neg(Box::new(self.ve(x)?)),
            EK::Method(..) if matches!(self.info.builtin.get(&e.id), Some(Builtin::Fold)) => {
                self.inner_fold(e)?
            }
            EK::Method(x, _, args) if float => match self.info.builtin.get(&e.id) {
                Some(Builtin::Sqrt) => VE::Sqrt(Box::new(self.ve(x)?)),
                Some(Builtin::Abs) => VE::Abs(Box::new(self.ve(x)?)),
                Some(Builtin::Max) => VE::Max(Box::new(self.ve(x)?), Box::new(self.ve(&args[0])?)),
                Some(Builtin::Min) => VE::Min(Box::new(self.ve(x)?), Box::new(self.ve(&args[0])?)),
                _ => return no(pos, "this method"),
            },
            _ => return no(pos, "this expression"),
        })
    }

    /// The one inner fold of an outer map's body
    fn inner_fold(&mut self, e: &Expr) -> SResult<VE> {
        if self.outer_var.is_none() || self.lane.is_some() || self.inner.is_some() {
            return no(
                e.pos,
                "a fold here (one inner fold, in the body of `range(0, m).map((r) => ..)`)",
            );
        }
        let inner = self.inner_loop(e)?;
        self.inner = Some(inner);
        Ok(VE::Inner)
    }

    /// `xs.map(..)` / `range(0, n).fold(..)` ...
    fn inner_loop(&mut self, e: &Expr) -> SResult<Inner> {
        let EK::Method(recv, _, margs) = &e.kind else {
            return no(e.pos, "the body");
        };
        let b = self.info.builtin.get(&e.id).copied();
        let (count, lane_array) = match &recv.kind {
            EK::Call(f, ra) if matches!(self.info.callee.get(&f.id), Some(Callee::Range)) => {
                if !matches!(ra[0].kind, EK::Lit(Lit::Int(0))) {
                    return no(recv.pos, "a range that does not start at 0");
                }
                (self.count(&ra[1])?, None)
            }
            _ => match self.array_param(recv) {
                Some(k) => (Count::Len(k), Some(k)),
                None => {
                    return no(
                        recv.pos,
                        "a loop over something other than an array parameter or `range(0, n)`",
                    );
                }
            },
        };
        let closure = |k: usize| -> SResult<(&Vec<(String, Option<TyS>)>, &Expr)> {
            match margs.get(k).map(|a| &a.kind) {
                Some(EK::Closure(ps, body)) => Ok((ps, body)),
                _ => no(e.pos, "the function argument (write a closure `(x) => ..`)"),
            }
        };
        let r = match b {
            Some(Builtin::Map) => {
                let (ps, body) = closure(0)?;
                self.lane = Some((ps[0].0.clone(), lane_array));
                let v = self.ve(body)?;
                Inner {
                    count,
                    fold: None,
                    body: v,
                }
            }
            Some(Builtin::Fold) => {
                let init = self.ve(&margs[0])?;
                let (ps, body) = closure(1)?;
                let acc = ps[0].0.clone();
                self.lane = Some((ps[1].0.clone(), lane_array));
                let EK::Bin("+", l, r) = &body.kind else {
                    return no(body.pos, "a fold other than a sum `acc + ..`");
                };
                let is_acc = |x: &Expr| matches!(&x.kind, EK::Var(n) if *n == acc);
                let term = if is_acc(l) {
                    r
                } else if is_acc(r) {
                    l
                } else {
                    return no(body.pos, "a fold other than a sum `acc + ..`");
                };
                let v = self.ve(term)?;
                Inner {
                    count,
                    fold: Some(init),
                    body: v,
                }
            }
            _ => return no(e.pos, "a method other than `map` / `fold`"),
        };
        self.lane = None;
        // (the array a loop runs over is read at every index)
        if let Some(k) = lane_array {
            self.reads.push((k, None, None));
        }
        Ok(r)
    }
}

fn el_of(t: &Ty) -> Option<El> {
    match t {
        Ty::F64 => Some(El::F64),
        Ty::F32 => Some(El::F32),
        Ty::I32 => Some(El::I32),
        _ => None,
    }
}

fn has_fold(e: &Expr, info: &Info) -> bool {
    match &e.kind {
        EK::Method(r, _, a) => {
            info.builtin.get(&e.id) == Some(&Builtin::Fold)
                || has_fold(r, info)
                || a.iter().any(|x| has_fold(x, info))
        }
        EK::Bin(_, l, r) => has_fold(l, info) || has_fold(r, info),
        EK::Un(_, x) => has_fold(x, info),
        _ => false,
    }
}

/// Recognizes the kernel in a `@simd fn`
fn kernel(it: &Item, info: &Info) -> SResult<Kernel> {
    let Item::Fn {
        name,
        public,
        relaxed,
        args,
        body,
        pos,
        generics,
        ..
    } = it
    else {
        unreachable!()
    };
    if !generics.is_empty() {
        return no(*pos, "a generic kernel (write one per type)");
    }
    if !info.effects[name].is_empty() {
        return no(*pos, "a kernel with effects");
    }
    let sig = &info.fns[name];
    let elem = |t: &Ty| match t {
        Ty::Array(e) | Ty::Typed(e) => el_of(e),
        _ => None,
    };
    let el = sig.args.iter().find_map(elem).ok_or_else(|| {
        (
            *pos,
            "@simd: a kernel takes an array of f64, f32 or i32".to_string(),
        )
    })?;
    let want = el.ty();
    let mut params = vec![];
    for ((a, _), t) in args.iter().zip(&sig.args) {
        let k = match t {
            Ty::Array(e) | Ty::Typed(e) if **e == want => PK::Array,
            t if *t == want => PK::Scalar,
            Ty::I32 => PK::Index,
            t => {
                return Err((
                    *pos,
                    format!(
                        "@simd: the parameter `{a}` is {t:?}: a kernel's arrays and scalars are {want:?} (and i32 counts / offsets)"
                    ),
                ));
            }
        };
        params.push((a.clone(), k));
    }
    let typed_result = matches!(sig.ret, Ty::Typed(_));
    let mut cx = Ctx {
        info,
        el,
        params: params.clone(),
        scalars: HashMap::new(),
        nslots: params.len(),
        outer_var: None,
        lane: None,
        inner: None,
        reads: vec![],
    };
    for (k, (a, pk)) in params.iter().enumerate() {
        if *pk == PK::Scalar {
            cx.scalars.insert(a.clone(), k);
        }
    }
    let (stmts, tail) = match &body.kind {
        EK::Block(s, Some(t)) => (s.as_slice(), &**t),
        _ => (&[][..], body),
    };
    let mut lets = vec![];
    for s in stmts {
        match s {
            Stmt::Let(Pat::Bind(n), _, x) => {
                let v = cx.ve(x)?;
                lets.push(v);
                cx.scalars.insert(n.clone(), cx.nslots);
                cx.nslots += 1;
            }
            Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) | Stmt::Expr(x) => {
                return no(x.pos, "a statement other than `let x = <scalar>`");
            }
        }
    }
    // an outer map whose body has an inner fold: `range(0, m).map((r) => .. range(0, n).fold(..) ..)`
    let outer_map = match &tail.kind {
        EK::Method(recv, _, margs)
            if info.builtin.get(&tail.id) == Some(&Builtin::Map)
                && matches!(&recv.kind, EK::Call(f, _) if matches!(info.callee.get(&f.id), Some(Callee::Range)))
                && matches!(margs.first().map(|a| &a.kind), Some(EK::Closure(_, b)) if has_fold(b, info)) =>
        {
            Some((recv, margs))
        }
        _ => None,
    };
    let (outer, inner) = match outer_map {
        Some((recv, margs)) => {
            let EK::Call(_, ra) = &recv.kind else {
                unreachable!()
            };
            if !matches!(ra[0].kind, EK::Lit(Lit::Int(0))) {
                return no(recv.pos, "a range that does not start at 0");
            }
            let m = cx.count(&ra[1])?;
            let EK::Closure(ps, ob) = &margs[0].kind else {
                unreachable!()
            };
            cx.outer_var = Some(ps[0].0.clone());
            let f = cx.ve(ob)?;
            let inner = cx.inner.take().ok_or_else(|| {
                (
                    ob.pos,
                    "@simd: the outer map's body needs one inner fold".to_string(),
                )
            })?;
            if inner.fold.is_none() {
                return no(ob.pos, "a map inside a map");
            }
            (Some((m, f)), inner)
        }
        None => (None, cx.inner_loop(tail)?),
    };
    Ok(Kernel {
        name: name.clone(),
        public: *public,
        relaxed: *relaxed,
        el,
        params,
        lets,
        outer,
        inner,
        typed_result,
        reads: cx.reads,
    })
}

// ---------------------------------------------------------------- wasm encoding

fn uleb(o: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            o.push(b);
            return;
        }
        o.push(b | 0x80);
    }
}

fn sleb(o: &mut Vec<u8>, mut v: i64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0) {
            o.push(b);
            return;
        }
        o.push(b | 0x80);
    }
}

const V128: u8 = 0x7B;

/// Opcodes of an element type: (scalar, vector)
struct Ops {
    load: [u8; 3],
    store: [u8; 3],
    add: (u8, u32),
    sub: (u8, u32),
    mul: (u8, u32),
    div: (u8, u32),
    neg: (u8, u32),
    abs: (u8, u32),
    sqrt: (u8, u32),
    min: (u8, u32),
    max: (u8, u32),
    splat: u32,
    extract: u32,
    /// relaxed SIMD: madd, min, max
    rmadd: u32,
    rmin: u32,
    rmax: u32,
}

fn ops(el: El) -> Ops {
    match el {
        El::F64 => Ops {
            load: [0x2B, 0x03, 0x00],
            store: [0x39, 0x03, 0x00],
            add: (0xA0, 0xF0),
            sub: (0xA1, 0xF1),
            mul: (0xA2, 0xF2),
            div: (0xA3, 0xF3),
            neg: (0x9A, 0xED),
            abs: (0x99, 0xEC),
            sqrt: (0x9F, 0xEF),
            min: (0xA4, 0xF4),
            max: (0xA5, 0xF5),
            splat: 0x14,
            extract: 0x21,
            rmadd: 0x107,
            rmin: 0x10F,
            rmax: 0x110,
        },
        El::F32 => Ops {
            load: [0x2A, 0x02, 0x00],
            store: [0x38, 0x02, 0x00],
            add: (0x92, 0xE4),
            sub: (0x93, 0xE5),
            mul: (0x94, 0xE6),
            div: (0x95, 0xE7),
            neg: (0x8C, 0xE1),
            abs: (0x8B, 0xE0),
            sqrt: (0x91, 0xE3),
            min: (0x96, 0xE8),
            max: (0x97, 0xE9),
            splat: 0x13,
            extract: 0x1F,
            rmadd: 0x105,
            rmin: 0x10D,
            rmax: 0x10E,
        },
        El::I32 => Ops {
            load: [0x28, 0x02, 0x00],
            store: [0x36, 0x02, 0x00],
            add: (0x6A, 0xAE),
            sub: (0x6B, 0xB1),
            mul: (0x6C, 0xB5),
            div: (0, 0),
            neg: (0, 0xA1),
            abs: (0, 0),
            sqrt: (0, 0),
            min: (0, 0),
            max: (0, 0),
            splat: 0x11,
            extract: 0x1B,
            rmadd: 0,
            rmin: 0,
            rmax: 0,
        },
    }
}

struct Code<'a> {
    k: &'a Kernel,
    o: Ops,
    c: Vec<u8>,
    /// the wasm local of each parameter (an array: its offset, and its length in the next local)
    p_local: Vec<u32>,
    /// slot → local (the parameters' scalars, then the lets), and its splat
    slot: Vec<u32>,
    splat: Vec<u32>,
    i: u32,
    r: u32,
    n_inner: u32,
    nv: u32,
    acc: u32,
    vacc: u32,
    /// the offsets of the inner reads (computed per outer iteration)
    offs: Vec<(IE, u32)>,
    out: Option<u32>,
}

impl Code<'_> {
    fn op(&mut self, b: u8) {
        self.c.push(b);
    }
    fn simd(&mut self, op: u32) {
        self.c.push(0xFD);
        uleb(&mut self.c, op as u64);
    }
    fn get(&mut self, l: u32) {
        self.c.push(0x20);
        uleb(&mut self.c, l as u64);
    }
    fn set(&mut self, l: u32) {
        self.c.push(0x21);
        uleb(&mut self.c, l as u64);
    }
    fn i32c(&mut self, v: i32) {
        self.c.push(0x41);
        sleb(&mut self.c, v as i64);
    }
    fn fconst(&mut self, v: f64) {
        match self.k.el {
            El::F64 => {
                self.c.push(0x44);
                self.c.extend_from_slice(&v.to_le_bytes());
            }
            El::F32 => {
                self.c.push(0x43);
                self.c.extend_from_slice(&(v as f32).to_le_bytes());
            }
            El::I32 => self.i32c(v as i32),
        }
    }
    fn ie(&mut self, e: &IE) {
        match e {
            IE::Outer => self.get(self.r),
            IE::Param(k) => {
                let l = self.p_local[*k];
                self.get(l);
            }
            IE::Const(c) => self.i32c(*c),
            IE::Add(a, b) => {
                self.ie(a);
                self.ie(b);
                self.op(0x6A);
            }
            IE::Mul(a, b) => {
                self.ie(a);
                self.ie(b);
                self.op(0x6C);
            }
        }
    }
    /// pushes the address of array k's element at local `index` (+ the offset)
    fn addr(&mut self, k: usize, off: &Option<IE>, index: u32) {
        let base = self.p_local[k];
        self.get(base);
        self.get(index);
        if let Some(o) = off {
            let l = self.offs.iter().find(|x| x.0 == *o).unwrap().1;
            self.get(l);
            self.op(0x6A);
        }
        self.i32c(self.k.el.shift());
        self.op(0x74);
        self.op(0x6A);
    }
    fn scalar(&mut self, v: &VE) {
        match v {
            VE::Elem(k, off) => {
                self.addr(*k, off, self.i);
                let l = self.o.load;
                self.c.extend_from_slice(&l);
            }
            VE::OuterElem(k, ix) => {
                let base = self.p_local[*k];
                self.get(base);
                self.ie(ix);
                self.i32c(self.k.el.shift());
                self.op(0x74);
                self.op(0x6A);
                let l = self.o.load;
                self.c.extend_from_slice(&l);
            }
            VE::Scalar(s) => {
                let l = self.slot[*s];
                self.get(l);
            }
            VE::Const(f) => self.fconst(*f),
            VE::IConst(n) => self.i32c(*n),
            VE::Inner => self.get(self.acc),
            VE::Bin(op, a, b) => {
                self.scalar(a);
                self.scalar(b);
                let o = match *op {
                    "+" => self.o.add.0,
                    "-" => self.o.sub.0,
                    "*" => self.o.mul.0,
                    _ => self.o.div.0,
                };
                self.op(o);
            }
            VE::Neg(a) => {
                if self.k.el.float() {
                    self.scalar(a);
                    self.op(self.o.neg.0);
                } else {
                    self.i32c(0);
                    self.scalar(a);
                    self.op(0x6B);
                }
            }
            VE::Sqrt(a) => {
                self.scalar(a);
                self.op(self.o.sqrt.0);
            }
            VE::Abs(a) => {
                self.scalar(a);
                self.op(self.o.abs.0);
            }
            VE::Max(a, b) | VE::Min(a, b) => {
                self.scalar(a);
                self.scalar(b);
                let o = if matches!(v, VE::Max(..)) {
                    self.o.max.0
                } else {
                    self.o.min.0
                };
                self.op(o);
            }
        }
    }
    fn vector(&mut self, v: &VE) {
        match v {
            VE::Elem(k, off) => {
                self.addr(*k, off, self.i);
                self.simd(0x00); // v128.load
                self.c.extend_from_slice(&[0x00, 0x00]);
            }
            VE::Scalar(s) => {
                let l = self.splat[*s];
                self.get(l);
            }
            VE::Const(_) | VE::IConst(_) | VE::OuterElem(..) | VE::Inner => {
                self.scalar(v);
                self.simd(self.o.splat);
            }
            // relaxed: `a * b + c` as one fused multiply-add
            VE::Bin("+", a, b)
                if self.k.relaxed
                    && self.k.el.float()
                    && (matches!(**a, VE::Bin("*", ..)) || matches!(**b, VE::Bin("*", ..))) =>
            {
                let (m, c) = if matches!(**a, VE::Bin("*", ..)) {
                    (a, b)
                } else {
                    (b, a)
                };
                let VE::Bin(_, x, y) = &**m else {
                    unreachable!()
                };
                self.vector(x);
                self.vector(y);
                self.vector(c);
                self.simd(self.o.rmadd);
            }
            VE::Bin(op, a, b) => {
                self.vector(a);
                self.vector(b);
                let o = match *op {
                    "+" => self.o.add.1,
                    "-" => self.o.sub.1,
                    "*" => self.o.mul.1,
                    _ => self.o.div.1,
                };
                self.simd(o);
            }
            VE::Neg(a) => {
                self.vector(a);
                self.simd(self.o.neg.1);
            }
            VE::Sqrt(a) => {
                self.vector(a);
                self.simd(self.o.sqrt.1);
            }
            VE::Abs(a) => {
                self.vector(a);
                self.simd(self.o.abs.1);
            }
            VE::Max(a, b) | VE::Min(a, b) => {
                self.vector(a);
                self.vector(b);
                let max = matches!(v, VE::Max(..));
                let o = match (self.k.relaxed, max) {
                    (true, true) => self.o.rmax,
                    (true, false) => self.o.rmin,
                    (false, true) => self.o.max.1,
                    (false, false) => self.o.min.1,
                };
                self.simd(o);
            }
        }
    }
    /// `block { loop { br_if (var >= end) 1; body; var += step; br 0 } }`
    fn counted(&mut self, var: u32, end: u32, step: u32, body: &dyn Fn(&mut Self)) {
        self.c.extend_from_slice(&[0x02, 0x40, 0x03, 0x40]);
        self.get(var);
        self.get(end);
        self.c.extend_from_slice(&[0x4E, 0x0D, 0x01]); // i32.ge_s; br_if 1
        body(self);
        self.get(var);
        self.i32c(step as i32);
        self.op(0x6A);
        self.set(var);
        self.c.extend_from_slice(&[0x0C, 0x00, 0x0B, 0x0B]);
    }
    fn count(&mut self, c: &Count) {
        match c {
            Count::Param(k) => {
                let l = self.p_local[*k];
                self.get(l);
            }
            Count::Len(k) => {
                let l = self.p_local[*k] + 1;
                self.get(l);
            }
            Count::Const(n) => self.i32c(*n),
        }
    }
    /// The inner loop: the vector part, then the rest one element at a time. A sum ends in `acc`; a map writes
    /// out[i]
    fn inner(&mut self) {
        let k = self.k;
        let lanes = k.el.lanes();
        self.i32c(0);
        self.set(self.i);
        self.get(self.n_inner);
        self.i32c(-(lanes as i32));
        self.op(0x71);
        self.set(self.nv);
        let body = k.inner.body.clone();
        if let Some(init) = &k.inner.fold {
            self.fconst(0.0);
            self.simd(self.o.splat);
            self.set(self.vacc);
            let relaxed_fma = k.relaxed && k.el.float() && matches!(body, VE::Bin("*", ..));
            self.counted(self.i, self.nv, lanes, &|c| {
                if relaxed_fma {
                    // vacc = madd(x, y, vacc)
                    let VE::Bin(_, x, y) = &body else {
                        unreachable!()
                    };
                    c.vector(x);
                    c.vector(y);
                    c.get(c.vacc);
                    c.simd(c.o.rmadd);
                } else {
                    c.get(c.vacc);
                    c.vector(&body);
                    c.simd(c.o.add.1);
                }
                c.set(c.vacc);
            });
            // acc = init + lane 0 + lane 1 (+ ..)
            self.scalar(init);
            for l in 0..lanes {
                self.get(self.vacc);
                self.simd(self.o.extract);
                self.c.push(l as u8);
                self.op(self.o.add.0);
            }
            self.set(self.acc);
            self.counted(self.i, self.n_inner, 1, &|c| {
                c.get(c.acc);
                c.scalar(&body);
                c.op(c.o.add.0);
                c.set(c.acc);
            });
        } else {
            let out = self.out.unwrap();
            let shift = k.el.shift();
            self.counted(self.i, self.nv, lanes, &|c| {
                c.get(out);
                c.get(c.i);
                c.i32c(shift);
                c.op(0x74);
                c.op(0x6A);
                c.vector(&body);
                c.simd(0x0B); // v128.store
                c.c.extend_from_slice(&[0x00, 0x00]);
            });
            self.counted(self.i, self.n_inner, 1, &|c| {
                c.get(out);
                c.get(c.i);
                c.i32c(shift);
                c.op(0x74);
                c.op(0x6A);
                c.scalar(&body);
                let s = c.o.store;
                c.c.extend_from_slice(&s);
            });
        }
    }
}

/// One kernel: (body, wasm param types, results)
fn function(k: &Kernel) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let el = k.el;
    let mut ptypes = vec![];
    let mut p_local = vec![0u32; k.params.len()];
    let mut idx = 0u32;
    let mut slot = vec![0u32; k.params.len() + k.lets.len()];
    for (j, (_, pk)) in k.params.iter().enumerate() {
        p_local[j] = idx;
        match pk {
            PK::Array => {
                ptypes.extend_from_slice(&[0x7F, 0x7F]);
                idx += 2;
            }
            PK::Scalar => {
                ptypes.push(el.valtype());
                slot[j] = idx;
                idx += 1;
            }
            PK::Index => {
                ptypes.push(0x7F);
                idx += 1;
            }
        }
    }
    let has_out = k.inner.fold.is_none() || k.outer.is_some();
    let out = if has_out {
        ptypes.push(0x7F);
        idx += 1;
        Some(idx - 1)
    } else {
        None
    };
    let nparams = idx;
    // i32 locals: i, r, n, m, nv, the offsets; element locals: acc, the lets; v128: vacc, the splats
    let mut offs: Vec<(IE, u32)> = vec![];
    for (_, off, _) in &k.reads {
        if let Some(o) = off
            && !offs.iter().any(|x| x.0 == *o)
        {
            offs.push((o.clone(), 0));
        }
    }
    let (i, r, n_inner, m_outer, nv) =
        (nparams, nparams + 1, nparams + 2, nparams + 3, nparams + 4);
    let mut next = nparams + 5;
    for o in &mut offs {
        o.1 = next;
        next += 1;
    }
    let n_i32 = next - nparams;
    let acc = next;
    next += 1;
    for j in 0..k.lets.len() {
        slot[k.params.len() + j] = next;
        next += 1;
    }
    let n_el = 1 + k.lets.len() as u32;
    let vacc = next;
    next += 1;
    let mut splat = vec![0u32; slot.len()];
    for s in splat.iter_mut() {
        *s = next;
        next += 1;
    }
    let n_vec = 1 + slot.len() as u32;
    let mut c = Code {
        k,
        o: ops(el),
        c: vec![],
        p_local,
        slot,
        splat,
        i,
        r,
        n_inner,
        nv,
        acc,
        vacc,
        offs,
        out,
    };
    // the lets, then a splat of every scalar
    for (j, l) in k.lets.iter().enumerate() {
        c.scalar(l);
        let d = c.slot[k.params.len() + j];
        c.set(d);
    }
    for j in 0..c.slot.len() {
        if j < k.params.len() && k.params[j].1 != PK::Scalar {
            continue;
        }
        let (s, d) = (c.slot[j], c.splat[j]);
        c.get(s);
        c.simd(c.o.splat);
        c.set(d);
    }
    let inner_count = k.inner.count.clone();
    c.count(&inner_count);
    c.set(n_inner);
    match &k.outer {
        None => {
            for (o, l) in c.offs.clone() {
                c.ie(&o);
                c.set(l);
            }
            c.inner();
            if k.inner.fold.is_some() {
                c.get(acc);
            }
        }
        Some((m, f)) => {
            c.count(m);
            c.set(m_outer);
            c.i32c(0);
            c.set(r);
            let f = f.clone();
            let shift = el.shift();
            c.counted(r, m_outer, 1, &|c| {
                for (o, l) in c.offs.clone() {
                    c.ie(&o);
                    c.set(l);
                }
                c.inner();
                // out[r] = F(the sum)
                c.get(c.out.unwrap());
                c.get(c.r);
                c.i32c(shift);
                c.op(0x74);
                c.op(0x6A);
                c.scalar(&f);
                let s = c.o.store;
                c.c.extend_from_slice(&s);
            });
        }
    }
    c.op(0x0B);
    let mut body = vec![];
    uleb(&mut body, 3);
    uleb(&mut body, n_i32 as u64);
    body.push(0x7F);
    uleb(&mut body, n_el as u64);
    body.push(el.valtype());
    uleb(&mut body, n_vec as u64);
    body.push(V128);
    body.extend_from_slice(&c.c);
    let results = if has_out { vec![] } else { vec![el.valtype()] };
    (body, ptypes, results)
}

fn module(ks: &[Kernel]) -> Vec<u8> {
    let mut m = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00];
    let sec = |m: &mut Vec<u8>, id: u8, body: &[u8]| {
        m.push(id);
        uleb(m, body.len() as u64);
        m.extend_from_slice(body);
    };
    let fs: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = ks.iter().map(function).collect();
    let mut types = vec![];
    uleb(&mut types, fs.len() as u64);
    for (_, p, r) in &fs {
        types.push(0x60);
        uleb(&mut types, p.len() as u64);
        types.extend_from_slice(p);
        uleb(&mut types, r.len() as u64);
        types.extend_from_slice(r);
    }
    sec(&mut m, 1, &types);
    let mut funcs = vec![];
    uleb(&mut funcs, fs.len() as u64);
    for k in 0..fs.len() {
        uleb(&mut funcs, k as u64);
    }
    sec(&mut m, 3, &funcs);
    sec(&mut m, 5, &[0x01, 0x00, 0x01]);
    let name = |o: &mut Vec<u8>, s: &str| {
        uleb(o, s.len() as u64);
        o.extend_from_slice(s.as_bytes());
    };
    let mut ex = vec![];
    uleb(&mut ex, fs.len() as u64 + 1);
    name(&mut ex, "memory");
    ex.extend_from_slice(&[0x02, 0x00]);
    for (k, kn) in ks.iter().enumerate() {
        name(&mut ex, &kn.name);
        ex.push(0x00);
        uleb(&mut ex, k as u64);
    }
    sec(&mut m, 7, &ex);
    let mut code = vec![];
    uleb(&mut code, fs.len() as u64);
    for (f, _, _) in &fs {
        uleb(&mut code, f.len() as u64);
        code.extend_from_slice(f);
    }
    sec(&mut m, 10, &code);
    m
}

fn base64(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

pub struct Simd {
    pub glue: String,
    pub wasm: Vec<u8>,
}

/// The kernels of the `@simd fn`s `names`: the module (embedded in the glue) and the glue
pub fn generate(
    items: &[Item],
    info: &Info,
    names: &[String],
) -> Result<Option<Simd>, Vec<(Pos, String)>> {
    let mut ks = vec![];
    let mut errs = vec![];
    for it in items {
        if let Item::Fn {
            name, simd: true, ..
        } = it
            && names.contains(name)
        {
            match kernel(it, info) {
                Ok(k) => ks.push(k),
                Err(e) => errs.push(e),
            }
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    if ks.is_empty() {
        return Ok(None);
    }
    let wasm = module(&ks);
    let mut g = String::from(
        "// ---- @simd kernels: a v128 module written by zsc (no allocator: the glue lays out each call's arrays)\n",
    );
    let _ = writeln!(
        g,
        "const $simdBytes = Uint8Array.from(atob({:?}), (c) => c.charCodeAt(0));",
        base64(&wasm)
    );
    g.push_str("const $simd = (await WebAssembly.instantiate($simdBytes)).instance.exports;\n");
    g.push_str("function $simdArray(v) {\n  const r = new Array(v.length);\n  for (let i = 0; i < v.length; i++) r[i] = v[i];\n  return r;\n}\n");
    g.push_str("// (an arena per call: the arrays from offset 0, 16-byte aligned; the memory grows when they do not fit)\nfunction $simdLayout(bytes) {\n  let at = 0;\n  const offs = bytes.map((b) => { const o = at; at += (b + 15) & ~15; return o; });\n  const have = $simd.memory.buffer.byteLength;\n  if (at > have) $simd.memory.grow(Math.ceil((at - have) / 65536));\n  return offs;\n}\n");
    g.push_str("function $simdBounds(len, lo, hi) {\n  if (lo < 0 || hi > len) throw new RangeError(\"index out of bounds: the len is \" + len + \" but the index is \" + (lo < 0 ? lo : hi - 1));\n}\n\n");
    for k in &ks {
        let (arr, sz) = (k.el.js_array(), k.el.size());
        let ps: Vec<&str> = k.params.iter().map(|p| p.0.as_str()).collect();
        let narrays = k.params.iter().filter(|p| p.1 == PK::Array).count();
        let _ = writeln!(
            g,
            "{}function {}({}) {{",
            if k.public { "export " } else { "" },
            k.name,
            ps.join(", ")
        );
        let _ = writeln!(g, "  const $n = {};", k.inner.count.js(&k.params));
        if let Some((m, _)) = &k.outer {
            let _ = writeln!(g, "  const $m = {};", m.js(&k.params));
        }
        // the bounds of every read (an offset / index is affine in r: its extremes are at r = 0 and r = m - 1)
        let r_hi = if k.outer.is_some() { "($m - 1)" } else { "0" };
        let mut seen = vec![];
        for rd in &k.reads {
            if seen.contains(&format!("{rd:?}")) {
                continue;
            }
            seen.push(format!("{rd:?}"));
            let (a, off, outer_ix) = rd;
            let an = &k.params[*a].0;
            let guard = if k.outer.is_some() {
                "$n > 0 && $m > 0"
            } else {
                "$n > 0"
            };
            match (off, outer_ix) {
                (None, None) => {
                    let _ = writeln!(g, "  if ({guard}) $simdBounds({an}.length, 0, $n);");
                }
                (Some(o), _) => {
                    let (lo, hi) = (o.js(&k.params, "0"), o.js(&k.params, r_hi));
                    let _ = writeln!(
                        g,
                        "  if ({guard}) $simdBounds({an}.length, Math.min({lo}, {hi}), Math.max({lo}, {hi}) + $n);"
                    );
                }
                (None, Some(ix)) => {
                    let (lo, hi) = (ix.js(&k.params, "0"), ix.js(&k.params, r_hi));
                    let _ = writeln!(
                        g,
                        "  if ($m > 0) $simdBounds({an}.length, Math.min({lo}, {hi}), Math.max({lo}, {hi}) + 1);"
                    );
                }
            }
        }
        let out_len = if k.outer.is_some() { "$m" } else { "$n" };
        let has_out = k.inner.fold.is_none() || k.outer.is_some();
        let mut sizes: Vec<String> = k
            .params
            .iter()
            .filter(|p| p.1 == PK::Array)
            .map(|p| format!("{}.length * {sz}", p.0))
            .collect();
        if has_out {
            sizes.push(format!("{out_len} * {sz}"));
        }
        let offs: Vec<String> = (0..sizes.len()).map(|j| format!("$o{j}")).collect();
        let _ = writeln!(
            g,
            "  const [{}] = $simdLayout([{}]);",
            offs.join(", "),
            sizes.join(", ")
        );
        let mut cargs = vec![];
        let mut ai = 0;
        for (p, pk) in &k.params {
            if *pk == PK::Array {
                let _ = writeln!(
                    g,
                    "  new {arr}($simd.memory.buffer, $o{ai}, {p}.length).set({p});"
                );
                cargs.push(format!("$o{ai}"));
                cargs.push(format!("{p}.length"));
                ai += 1;
            } else {
                cargs.push(p.clone());
            }
        }
        if has_out {
            cargs.push(format!("$o{narrays}"));
            let _ = writeln!(g, "  $simd.{}({});", k.name, cargs.join(", "));
            let view = format!("new {arr}($simd.memory.buffer, $o{narrays}, {out_len})");
            if k.typed_result {
                let _ = writeln!(g, "  return {view}.slice();");
            } else {
                let _ = writeln!(g, "  return $simdArray({view});");
            }
        } else {
            let _ = writeln!(g, "  return $simd.{}({});", k.name, cargs.join(", "));
        }
        g.push_str("}\n\n");
    }
    Ok(Some(Simd { glue: g, wasm }))
}
