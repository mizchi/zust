//! `@simd fn`: an array kernel vectorized with wasm's v128 (f64x2 / i32x4). zsc writes this wasm module itself:
//! zustc's IR has no vector types, and the prototype does not change zust's crates.
//!
//! A kernel is a function whose body (after `let`s of scalars) is one of
//!
//! ```text
//! xs.map((x) => E)                                    // elementwise: Array<T>
//! range(0, a.len()).map((i) => E)                     // E reads a[i], b[i], ..: Array<T>
//! xs.fold(init, (acc, x) => acc + E)                  // a sum: T
//! range(0, a.len()).fold(init, (s, i) => s + E)       // a dot product and the like: T
//! ```
//!
//! where E is `+ - *` (and `/`, `sqrt`, `abs` for f64), unary `-`, the element(s), scalar parameters and `let`s,
//! and literals. T is f64 (two lanes) or i32 (four lanes); all arrays and scalars of a kernel have that type.
//!
//! Its meaning, against the same function as JS (`zsc --all-js`) or through zustc (`zsc --no-simd`):
//! - `map`, and an i32 `fold` (wrapping adds commute): the same results;
//! - an f64 `fold` sums each lane on its own and adds the lanes at the end: the sum is regrouped, so the last
//!   bits can differ. That is what `@simd` opts into.
//! - an array read past its end is an error before the loop starts (the glue checks the lengths).
//!
//! Memory: the module has no allocator. For each call the glue lays the arguments out from offset 0 (16-byte
//! aligned), grows the memory if they do not fit, and copies the result out: an arena per call.

use crate::ast::*;
use crate::check::{Builtin, Callee, Info, Ty};
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Copy, PartialEq, Debug)]
enum El {
    F64,
    I32,
}

impl El {
    fn lanes(self) -> u32 {
        match self {
            El::F64 => 2,
            El::I32 => 4,
        }
    }
    fn shift(self) -> i32 {
        match self {
            El::F64 => 3,
            El::I32 => 2,
        }
    }
    fn valtype(self) -> u8 {
        match self {
            El::F64 => 0x7C,
            El::I32 => 0x7F,
        }
    }
}

/// An elementwise expression
#[derive(Clone, Debug)]
enum VE {
    /// the element of array parameter k (at the loop's index)
    Elem(usize),
    /// scalar slot k (a parameter or a `let`)
    Scalar(usize),
    Const(f64),
    IConst(i32),
    Bin(&'static str, Box<VE>, Box<VE>),
    Neg(Box<VE>),
    Sqrt(Box<VE>),
    Abs(Box<VE>),
}

#[derive(Clone)]
struct Kernel {
    name: String,
    public: bool,
    el: El,
    /// the parameters: (name, is an array)
    params: Vec<(String, bool)>,
    /// `let`s before the loop: scalar slots computed once (slot index → expression over earlier slots)
    lets: Vec<VE>,
    /// the array whose length is the loop's count
    count: usize,
    /// the arrays read at the loop's index (checked to be long enough)
    read: Vec<usize>,
    /// None: a map; Some(init): a sum starting at init
    fold: Option<VE>,
    body: VE,
}

type SResult<X> = Result<X, (Pos, String)>;

fn no<X>(pos: Pos, what: &str) -> SResult<X> {
    Err((pos, format!("@simd: {what} (a kernel is `xs.map((x) => ..)`, `xs.fold(init, (acc, x) => acc + ..)`, or the same over `range(0, a.len())` reading `a[i]`; arithmetic on one numeric type)")))
}

struct Ctx<'a> {
    info: &'a Info,
    el: El,
    params: Vec<(String, bool)>,
    /// names in scope → scalar slot
    scalars: HashMap<String, usize>,
    nslots: usize,
    /// the closure's element variable → array parameter (`xs.map((x) => ..)`), or the index variable
    lane: Option<(String, Option<usize>)>,
    read: Vec<usize>,
}

impl Ctx<'_> {
    fn array_param(&self, e: &Expr) -> Option<usize> {
        match &e.kind {
            EK::Var(n) => self.params.iter().position(|p| p.0 == *n && p.1),
            _ => None,
        }
    }

    fn ve(&mut self, e: &Expr) -> SResult<VE> {
        let pos = e.pos;
        Ok(match &e.kind {
            EK::Lit(Lit::Int(n)) => match self.el {
                El::I32 => VE::IConst(*n as i32),
                El::F64 => return no(pos, "an integer literal in an f64 kernel"),
            },
            EK::Lit(Lit::Float(f)) => match self.el {
                El::F64 => VE::Const(f.parse().unwrap_or(0.0)),
                El::I32 => return no(pos, "a float literal in an i32 kernel"),
            },
            EK::Var(n) => {
                if let Some((x, Some(k))) = &self.lane
                    && x == n
                {
                    return Ok(VE::Elem(*k));
                }
                match self.scalars.get(n) {
                    Some(s) => VE::Scalar(*s),
                    None => return no(pos, &format!("`{n}` here")),
                }
            }
            EK::Index(a, i) => {
                let Some(k) = self.array_param(a) else { return no(pos, "indexing something other than an array parameter") };
                match (&self.lane, &i.kind) {
                    (Some((x, None)), EK::Var(v)) if x == v => {
                        if !self.read.contains(&k) {
                            self.read.push(k);
                        }
                        VE::Elem(k)
                    }
                    _ => return no(pos, "an index other than the loop's `i`"),
                }
            }
            EK::Bin(op @ ("+" | "-" | "*" | "/"), l, r) => {
                if *op == "/" && self.el == El::I32 {
                    return no(pos, "i32 division (wasm has no vector integer division)");
                }
                VE::Bin(op, Box::new(self.ve(l)?), Box::new(self.ve(r)?))
            }
            EK::Un("-", x) => VE::Neg(Box::new(self.ve(x)?)),
            EK::Method(x, _, _) if self.el == El::F64 => match self.info.builtin.get(&e.id) {
                Some(Builtin::Sqrt) => VE::Sqrt(Box::new(self.ve(x)?)),
                Some(Builtin::Abs) => VE::Abs(Box::new(self.ve(x)?)),
                _ => return no(pos, "this method"),
            },
            _ => return no(pos, "this expression"),
        })
    }
}

fn el_of(t: &Ty) -> Option<El> {
    match t {
        Ty::F64 => Some(El::F64),
        Ty::I32 => Some(El::I32),
        _ => None,
    }
}

/// Recognizes the kernel in a `@simd fn`
fn kernel(it: &Item, info: &Info) -> SResult<Kernel> {
    let Item::Fn { name, public, args, body, pos, generics, .. } = it else { unreachable!() };
    if !generics.is_empty() {
        return no(*pos, "a generic kernel (write one per type)");
    }
    if !info.effects[name].is_empty() {
        return no(*pos, "a kernel with effects");
    }
    let sig = &info.fns[name];
    // the element type: from the first array parameter
    let el = sig
        .args
        .iter()
        .find_map(|t| if let Ty::Array(e) = t { el_of(e) } else { None })
        .ok_or_else(|| (*pos, "@simd: a kernel takes an Array<f64> or an Array<i32>".to_string()))?;
    let want = match el {
        El::F64 => Ty::F64,
        El::I32 => Ty::I32,
    };
    let mut params = vec![];
    for ((a, _), t) in args.iter().zip(&sig.args) {
        match t {
            Ty::Array(e) if **e == want => params.push((a.clone(), true)),
            t if *t == want => params.push((a.clone(), false)),
            t => return Err((*pos, format!("@simd: the parameter `{a}` is {t:?}: a kernel's arrays and scalars are all {want:?}"))),
        }
    }
    let mut cx = Ctx { info, el, params: params.clone(), scalars: HashMap::new(), nslots: 0, lane: None, read: vec![] };
    for (k, (a, arr)) in params.iter().enumerate() {
        if !arr {
            cx.scalars.insert(a.clone(), k);
        }
    }
    cx.nslots = params.len();
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
            Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) | Stmt::Expr(x) => return no(x.pos, "a statement other than `let x = <scalar>`"),
        }
    }
    let EK::Method(recv, _, margs) = &tail.kind else { return no(tail.pos, "the body") };
    let b = info.builtin.get(&tail.id).copied();
    // the loop: over an array parameter, or over range(0, a.len())
    let (count, lane_array) = match &recv.kind {
        EK::Call(f, ra) if matches!(info.callee.get(&f.id), Some(Callee::Range)) => {
            let zero = matches!(ra[0].kind, EK::Lit(Lit::Int(0)));
            let len_of = match &ra[1].kind {
                EK::Method(a, _, _) if info.builtin.get(&ra[1].id) == Some(&Builtin::Len) => cx.array_param(a),
                _ => None,
            };
            match (zero, len_of) {
                (true, Some(k)) => (k, None),
                _ => return no(recv.pos, "a range other than `range(0, a.len())`"),
            }
        }
        _ => match cx.array_param(recv) {
            Some(k) => (k, Some(k)),
            None => return no(recv.pos, "a loop over something other than an array parameter or `range(0, a.len())`"),
        },
    };
    let closure = |k: usize| -> SResult<(&Vec<(String, Option<TyS>)>, &Expr)> {
        match margs.get(k).map(|a| &a.kind) {
            Some(EK::Closure(ps, body)) => Ok((ps, body)),
            _ => no(tail.pos, "the function argument (write a closure `(x) => ..`)"),
        }
    };
    let (fold, body) = match b {
        Some(Builtin::Map) => {
            let (ps, e) = closure(0)?;
            cx.lane = Some((ps[0].0.clone(), lane_array));
            (None, cx.ve(e)?)
        }
        Some(Builtin::Fold) => {
            let init = cx.ve(&margs[0])?;
            let (ps, e) = closure(1)?;
            let acc = ps[0].0.clone();
            cx.lane = Some((ps[1].0.clone(), lane_array));
            // `acc + E` or `E + acc`
            let EK::Bin("+", l, r) = &e.kind else { return no(e.pos, "a fold other than a sum `acc + ..`") };
            let is_acc = |x: &Expr| matches!(&x.kind, EK::Var(n) if *n == acc);
            let term = if is_acc(l) {
                r
            } else if is_acc(r) {
                l
            } else {
                return no(e.pos, "a fold other than a sum `acc + ..`");
            };
            (Some(init), cx.ve(term)?)
        }
        _ => return no(tail.pos, "a method other than `map` / `fold`"),
    };
    if lane_array.is_some() && !cx.read.is_empty() {
        return no(tail.pos, "indexing inside `xs.map` / `xs.fold` (use `range(0, xs.len())`)");
    }
    let mut read = cx.read.clone();
    if !read.contains(&count) {
        read.push(count);
    }
    Ok(Kernel { name: name.clone(), public: *public, el, params, lets, count, read, fold, body })
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

fn simd(o: &mut Vec<u8>, op: u32) {
    o.push(0xFD);
    uleb(o, op as u64);
}

const V128: u8 = 0x7B;

struct FnCode<'a> {
    k: &'a Kernel,
    code: Vec<u8>,
    /// locals: params first (arrays as i32 offsets, then n, then scalars), then these
    i: u32,
    acc: u32,
    vacc: u32,
    /// scalar slot → its local; and the local of its splat
    slot_local: Vec<u32>,
    splat_local: Vec<u32>,
}

impl FnCode<'_> {
    fn get(&mut self, l: u32) {
        self.code.push(0x20);
        uleb(&mut self.code, l as u64);
    }
    fn set(&mut self, l: u32) {
        self.code.push(0x21);
        uleb(&mut self.code, l as u64);
    }
    fn i32c(&mut self, v: i32) {
        self.code.push(0x41);
        sleb(&mut self.code, v as i64);
    }
    /// the address of element i of array parameter k
    fn addr(&mut self, k: usize) {
        self.get(k as u32);
        self.get(self.i);
        self.i32c(self.k.el.shift());
        self.code.push(0x74); // i32.shl
        self.code.push(0x6A); // i32.add
    }
    fn scalar_const(&mut self, v: &VE) {
        match v {
            VE::Const(f) => {
                self.code.push(0x44);
                self.code.extend_from_slice(&f.to_le_bytes());
            }
            VE::IConst(n) => self.i32c(*n),
            _ => unreachable!(),
        }
    }
    /// An expression on one element (the loop's tail, the lets, the fold's init)
    fn scalar(&mut self, v: &VE) {
        let f = self.k.el == El::F64;
        match v {
            VE::Elem(k) => {
                self.addr(*k);
                if f {
                    self.code.extend_from_slice(&[0x2B, 0x03, 0x00]); // f64.load
                } else {
                    self.code.extend_from_slice(&[0x28, 0x02, 0x00]); // i32.load
                }
            }
            VE::Scalar(s) => {
                let l = self.slot_local[*s];
                self.get(l);
            }
            VE::Const(_) | VE::IConst(_) => self.scalar_const(v),
            VE::Bin(op, a, b) => {
                self.scalar(a);
                self.scalar(b);
                self.code.push(match (f, *op) {
                    (true, "+") => 0xA0,
                    (true, "-") => 0xA1,
                    (true, "*") => 0xA2,
                    (true, _) => 0xA3,
                    (false, "+") => 0x6A,
                    (false, "-") => 0x6B,
                    (false, _) => 0x6C,
                });
            }
            VE::Neg(a) => {
                if f {
                    self.scalar(a);
                    self.code.push(0x9A);
                } else {
                    self.i32c(0);
                    self.scalar(a);
                    self.code.push(0x6B);
                }
            }
            VE::Sqrt(a) => {
                self.scalar(a);
                self.code.push(0x9F);
            }
            VE::Abs(a) => {
                self.scalar(a);
                self.code.push(0x99);
            }
        }
    }
    /// The same expression on a vector of lanes
    fn vector(&mut self, v: &VE) {
        let f = self.k.el == El::F64;
        match v {
            VE::Elem(k) => {
                self.addr(*k);
                simd(&mut self.code, 0x00); // v128.load
                self.code.extend_from_slice(&[0x00, 0x00]);
            }
            VE::Scalar(s) => {
                let l = self.splat_local[*s];
                self.get(l);
            }
            VE::Const(_) | VE::IConst(_) => {
                self.scalar_const(v);
                simd(&mut self.code, if f { 0x14 } else { 0x11 }); // splat
            }
            VE::Bin(op, a, b) => {
                self.vector(a);
                self.vector(b);
                let o = match (f, *op) {
                    (true, "+") => 0xF0,
                    (true, "-") => 0xF1,
                    (true, "*") => 0xF2,
                    (true, _) => 0xF3,
                    (false, "+") => 0xAE,
                    (false, "-") => 0xB1,
                    (false, _) => 0xB5,
                };
                simd(&mut self.code, o);
            }
            VE::Neg(a) => {
                self.vector(a);
                simd(&mut self.code, if f { 0xED } else { 0xA1 });
            }
            VE::Sqrt(a) => {
                self.vector(a);
                simd(&mut self.code, 0xEF);
            }
            VE::Abs(a) => {
                self.vector(a);
                simd(&mut self.code, 0xEC);
            }
        }
    }

    /// `block { loop { br_if (i >= end) 1; body; i += step; br 0 } }`
    fn counted(&mut self, end: u32, step: u32, body: &dyn Fn(&mut Self)) {
        self.code.extend_from_slice(&[0x02, 0x40, 0x03, 0x40]);
        self.get(self.i);
        self.get(end);
        self.code.extend_from_slice(&[0x4F, 0x0D, 0x01]); // i32.ge_u; br_if 1
        body(self);
        self.get(self.i);
        self.i32c(step as i32);
        self.code.push(0x6A);
        self.set(self.i);
        self.code.extend_from_slice(&[0x0C, 0x00, 0x0B, 0x0B]); // br 0; end; end
    }
}

/// One kernel's function body (and its wasm signature: params, result)
fn function(k: &Kernel) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let el = k.el;
    let narr = k.params.iter().filter(|p| p.1).count();
    // wasm params: the array parameters' offsets (i32) in order, the out offset (map), n, then the scalars
    let mut ptypes = vec![];
    let mut local_of_param = vec![0u32; k.params.len()];
    let mut idx = 0u32;
    for (j, p) in k.params.iter().enumerate() {
        if p.1 {
            ptypes.push(0x7F);
            local_of_param[j] = idx;
            idx += 1;
        }
    }
    let out = if k.fold.is_none() {
        ptypes.push(0x7F);
        idx += 1;
        Some(idx - 1)
    } else {
        None
    };
    let n = idx;
    ptypes.push(0x7F);
    idx += 1;
    let mut slot_local = vec![0u32; k.params.len() + k.lets.len()];
    for (j, p) in k.params.iter().enumerate() {
        if !p.1 {
            ptypes.push(el.valtype());
            slot_local[j] = idx;
            idx += 1;
        }
    }
    let nparams = idx;
    // locals: i, nv (i32); acc (el); the lets (el); vacc and a splat per slot (v128)
    let i = nparams;
    let nv = nparams + 1;
    let acc = nparams + 2;
    let mut next = nparams + 3;
    for j in 0..k.lets.len() {
        slot_local[k.params.len() + j] = next;
        next += 1;
    }
    let nscal_locals = next - nparams - 2; // acc + lets
    let vacc = next;
    let mut splat_local = vec![0u32; slot_local.len()];
    for (j, s) in splat_local.iter_mut().enumerate() {
        *s = vacc + 1 + j as u32;
    }
    let nvec = 1 + slot_local.len() as u32;
    // (array parameter j's local is its position among the arrays)
    let mut fk = k.clone();
    for b in [&mut fk.body] {
        remap(b, &local_of_param);
    }
    if let Some(f) = &mut fk.fold {
        remap(f, &local_of_param);
    }
    for l in &mut fk.lets {
        remap(l, &local_of_param);
    }
    let mut c = FnCode { k: &fk, code: vec![], i, acc, vacc, slot_local, splat_local };
    // the lets, then a splat of every scalar slot
    for (j, l) in fk.lets.iter().enumerate() {
        c.scalar(l);
        let dst = c.slot_local[fk.params.len() + j];
        c.set(dst);
    }
    for j in 0..c.slot_local.len() {
        let is_array = j < fk.params.len() && fk.params[j].1;
        if is_array {
            continue;
        }
        let (src, dst) = (c.slot_local[j], c.splat_local[j]);
        c.get(src);
        simd(&mut c.code, if el == El::F64 { 0x14 } else { 0x11 });
        c.set(dst);
    }
    // nv = n & -lanes
    c.get(n);
    c.i32c(-(el.lanes() as i32));
    c.code.push(0x71);
    c.set(nv);
    if fk.fold.is_some() {
        // vacc = 0
        if el == El::F64 {
            c.scalar_const(&VE::Const(0.0));
        } else {
            c.i32c(0);
        }
        simd(&mut c.code, if el == El::F64 { 0x14 } else { 0x11 });
        c.set(vacc);
    }
    let body = fk.body.clone();
    let lanes = el.lanes();
    let shift = el.shift();
    let out_vec = |c: &mut FnCode, o: u32| {
        c.get(o);
        c.get(c.i);
        c.i32c(shift);
        c.code.push(0x74);
        c.code.push(0x6A);
    };
    c.counted(nv, lanes, &|c| match out {
        Some(o) => {
            out_vec(c, o);
            c.vector(&body);
            simd(&mut c.code, 0x0B); // v128.store
            c.code.extend_from_slice(&[0x00, 0x00]);
        }
        None => {
            c.get(c.vacc);
            c.vector(&body);
            simd(&mut c.code, if c.k.el == El::F64 { 0xF0 } else { 0xAE });
            c.set(c.vacc);
        }
    });
    if let Some(init) = &fk.fold {
        // acc = init + lane 0 + lane 1 (+ ..): the lanes' sums, then the tail one element at a time
        c.scalar(init);
        for l in 0..lanes {
            c.get(vacc);
            simd(&mut c.code, if el == El::F64 { 0x21 } else { 0x1B }); // extract_lane
            c.code.push(l as u8);
            c.code.push(if el == El::F64 { 0xA0 } else { 0x6A });
        }
        c.set(acc);
    }
    c.counted(n, 1, &|c| match out {
        Some(o) => {
            out_vec(c, o);
            c.scalar(&body);
            if c.k.el == El::F64 {
                c.code.extend_from_slice(&[0x39, 0x03, 0x00]);
            } else {
                c.code.extend_from_slice(&[0x36, 0x02, 0x00]);
            }
        }
        None => {
            c.get(c.acc);
            c.scalar(&body);
            c.code.push(if c.k.el == El::F64 { 0xA0 } else { 0x6A });
            c.set(c.acc);
        }
    });
    if fk.fold.is_some() {
        c.get(acc);
    }
    c.code.push(0x0B);
    let _ = narr;
    // locals declaration: 2 i32, (acc + lets) el, the v128s
    let mut f = vec![];
    uleb(&mut f, 3);
    uleb(&mut f, 2);
    f.push(0x7F);
    uleb(&mut f, nscal_locals as u64);
    f.push(el.valtype());
    uleb(&mut f, nvec as u64);
    f.push(V128);
    f.extend_from_slice(&c.code);
    let results = if k.fold.is_some() { vec![el.valtype()] } else { vec![] };
    (f, ptypes, results)
}

/// Elem(parameter index) → Elem(wasm local of that array's offset)
fn remap(v: &mut VE, local_of_param: &[u32]) {
    match v {
        VE::Elem(k) => *k = local_of_param[*k] as usize,
        VE::Bin(_, a, b) => {
            remap(a, local_of_param);
            remap(b, local_of_param);
        }
        VE::Neg(a) | VE::Sqrt(a) | VE::Abs(a) => remap(a, local_of_param),
        _ => {}
    }
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
    sec(&mut m, 5, &[0x01, 0x00, 0x01]); // one memory, 1 page, no maximum
    let mut ex = vec![];
    uleb(&mut ex, fs.len() as u64 + 1);
    let name = |o: &mut Vec<u8>, s: &str| {
        uleb(o, s.len() as u64);
        o.extend_from_slice(s.as_bytes());
    };
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
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
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
pub fn generate(items: &[Item], info: &Info, names: &[String]) -> Result<Option<Simd>, Vec<(Pos, String)>> {
    let mut ks = vec![];
    let mut errs = vec![];
    for it in items {
        if let Item::Fn { name, simd: true, .. } = it
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
    let mut g = String::from("// ---- @simd kernels: a v128 module written by zsc (no allocator: the glue lays out each call's arrays)\n");
    let _ = writeln!(g, "const $simdBytes = Uint8Array.from(atob({:?}), (c) => c.charCodeAt(0));", base64(&wasm));
    g.push_str("const $simd = (await WebAssembly.instantiate($simdBytes)).instance.exports;\n");
    g.push_str("// (a typed array's elements into a plain array: a loop is about 3 times as fast as Array.from)\nfunction $toArray(v) {\n  const r = new Array(v.length);\n  for (let i = 0; i < v.length; i++) r[i] = v[i];\n  return r;\n}\n");
    g.push_str("// (an arena per call: the arrays from offset 0, 16-byte aligned; the memory grows when they do not fit)\nfunction $simdLayout(bytes) {\n  let at = 0;\n  const offs = bytes.map((b) => { const o = at; at += (b + 15) & ~15; return o; });\n  const have = $simd.memory.buffer.byteLength;\n  if (at > have) $simd.memory.grow(Math.ceil((at - have) / 65536));\n  return offs;\n}\n\n");
    for k in &ks {
        let (arr, sz) = match k.el {
            El::F64 => ("Float64Array", 8),
            El::I32 => ("Int32Array", 4),
        };
        let ps: Vec<&str> = k.params.iter().map(|p| p.0.as_str()).collect();
        let arrays: Vec<&str> = k.params.iter().filter(|p| p.1).map(|p| p.0.as_str()).collect();
        let n = &k.params[k.count].0;
        let _ = writeln!(g, "{}function {}({}) {{", if k.public { "export " } else { "" }, k.name, ps.join(", "));
        let _ = writeln!(g, "  const $n = {n}.length;");
        for r in &k.read {
            let a = &k.params[*r].0;
            if a != n {
                let _ = writeln!(g, "  if ({a}.length < $n) throw new RangeError(\"index out of bounds: the len is \" + {a}.length + \" but the index is \" + {a}.length);");
            }
        }
        let mut sizes: Vec<String> = arrays.iter().map(|a| format!("{a}.length * {sz}")).collect();
        if k.fold.is_none() {
            sizes.push(format!("$n * {sz}"));
        }
        let offs: Vec<String> = (0..sizes.len()).map(|j| format!("$o{j}")).collect();
        let _ = writeln!(g, "  const [{}] = $simdLayout([{}]);", offs.join(", "), sizes.join(", "));
        for (j, a) in arrays.iter().enumerate() {
            let _ = writeln!(g, "  new {arr}($simd.memory.buffer, $o{j}, {a}.length).set({a});");
        }
        let mut cargs: Vec<String> = offs.clone();
        cargs.push("$n".into());
        for p in &k.params {
            if !p.1 {
                cargs.push(p.0.clone());
            }
        }
        if k.fold.is_some() {
            let _ = writeln!(g, "  return $simd.{}({});", k.name, cargs.join(", "));
        } else {
            let _ = writeln!(g, "  $simd.{}({});", k.name, cargs.join(", "));
            let _ = writeln!(g, "  return $toArray(new {arr}($simd.memory.buffer, $o{}, $n));", arrays.len());
        }
        g.push_str("}\n\n");
    }
    Ok(Some(Simd { glue: g, wasm }))
}
