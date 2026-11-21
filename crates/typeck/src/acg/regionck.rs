//! Region checking against signatures (issue #1): NLL's check of universal regions.
//!
//! `Ty` carries no regions. Here, in a function that has regions in its signature, each value of interest gets a
//! *region tree*: one region variable per region slot of its type (see `types::RSigSyn`; a reference's region, then
//! its pointee's; a struct's lifetime parameters, then its type arguments'). Lowering relates the trees where values
//! flow (a `let`, an assignment, a call's arguments and result, the returned value) with outlives constraints
//! `a: b`, following variance. The regions of the signature are universal; the function may only assume the
//! relations among them that its declaration and the types of its parameters imply. When the constraints require
//! `'a: 'b` of two universal regions without that, it is "lifetime may not live long enough".
//!
//! NLL's constraints between regions hold at every point (only liveness is per point), and a universal region holds
//! the whole body, so this check needs no control flow: it is reachability in the constraint graph from each
//! universal region (as rustc's `check_universal_regions`).
//!
//! What is not followed (a coercion that changes the shape of a type, a pattern that binds by parts, a closure, a
//! generic function's instance, a type the summaries cannot map) has no tree, and constraints touching it are left
//! out: an error is reported only when the constraints that are known require it.

use super::{Cx, Loc, Sp};
use crate::fxhash::FxMap as HashMap;
use crate::parse::{Expr, Stmt, Ty as PTy};
use crate::program::Program;
use crate::types::{RSigSyn, Ty, R_ELIDED, R_ELIDED_ADT, R_STATIC, R_UNKNOWN};

/// Region variables of a value, aligned with the region slots of its type
pub type RT = Vec<u32>;
/// `'static`
pub const STATIC: u32 = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Var {
    Co,
    Contra,
    Inv,
    Bi,
}

impl Var {
    /// The variance of a position inside a position of variance self
    fn then(self, inner: Var) -> Var {
        match (self, inner) {
            (Var::Bi, _) | (_, Var::Bi) => Var::Bi,
            (Var::Inv, _) | (_, Var::Inv) => Var::Inv,
            (Var::Contra, Var::Contra) => Var::Co,
            (Var::Contra, _) | (_, Var::Contra) => Var::Contra,
            _ => Var::Co,
        }
    }
    /// Two uses of one parameter
    fn join(self, o: Var) -> Var {
        match (self, o) {
            (Var::Bi, x) | (x, Var::Bi) => x,
            (a, b) if a == b => a,
            _ => Var::Inv,
        }
    }
}

#[derive(Default)]
pub struct Rg {
    pub on: bool,
    /// The function may be checked (its own body, not a closure's), though the gate may have left it out
    pub eligible: bool,
    /// The `let` being lowered: where an error is reported for what has no position of its own (a block)
    pub let_sp: Sp,
    /// Region variables made (0 is `'static`)
    pub n: u32,
    /// The universal regions (not `'static`), with their names
    pub univ: Vec<(u32, String)>,
    /// Outlives relations among universal regions the function may assume (a: b)
    pub known: Vec<(u32, u32)>,
    /// Constraints a: b, with where they come from
    pub edges: Vec<(u32, u32, Sp)>,
    pub locals: HashMap<Loc, RT>,
    /// The trees of call results (by the address of the call expression)
    pub calls: HashMap<usize, (Ty, RT)>,
    /// The result of the last call lowered (at the depth of call_fn it was made at)
    pub last_call: Option<(u32, Ty, RT)>,
    /// Nesting of call_fn
    pub depth: u32,
    /// The receiver of the method call being lowered, as passed, and its position
    pub recv: Option<(RT, Sp)>,
    /// The tree of the return type
    pub ret: Option<RT>,
    /// The function's own lifetime parameters (in `RSigSyn::lts` order)
    pub lts: Vec<u32>,
    pub lt_names: Vec<String>,
    /// Variance of a struct / enum definition: its lifetime parameters, then its type parameters
    pub variance: HashMap<u32, Vec<Var>>,
    /// The definitions whose variance is being computed; whether a provisional one was read; the ones computed, in
    /// order (see variance)
    pub var_open: Vec<u32>,
    pub var_cycle: bool,
    pub var_log: Vec<u32>,
    /// The function's own definition (of an instance: the generic one) and an instance's type arguments
    pub base: u32,
    pub inst_args: Vec<Ty>,
    /// Borrows of what the function owns (a local, not through a reference): (region, the place's text, where)
    pub loans: Vec<(u32, String, Sp)>,
    /// ZUSTC_RG_DEBUG: the kinds of values not followed
    pub missed: Vec<&'static str>,
    /// The last scrutinee's tree; the part of it the pattern being bound matches, and the region of the reference
    /// matched through last (bindings in it are by reference)
    pub scrut: Option<(Ty, RT)>,
    pub pat: Option<(Ty, RT)>,
    pub pat_ref: Option<u32>,
    /// A match lowered as another (its or-pattern arms with guards split): the one lowered
    pub alias: HashMap<usize, &'static Expr>,
}

fn kind(e: &Expr) -> &'static str {
    match e {
        Expr::Paren(x) => kind(x),
        Expr::Lit(..) => "lit",
        Expr::Path(..) => "path",
        Expr::Unary(..) => "unary",
        Expr::Binary(..) => "binary",
        Expr::Call(..) => "call",
        Expr::MethodCall(..) => "method",
        Expr::Field(..) => "field",
        Expr::Index(..) => "index",
        Expr::Ref(..) => "ref",
        Expr::Cast(..) => "cast",
        Expr::Block(..) => "block",
        Expr::If(..) => "if",
        Expr::Match(..) => "match",
        Expr::Closure(..) => "closure",
        Expr::Tuple(..) => "tuple",
        Expr::Array(..) => "array",
        Expr::Struct(..) => "struct",
        Expr::Macro(..) => "macro",
        Expr::Try(..) => "try",
        _ => "other",
    }
}

pub fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZUSTC_RG_DEBUG").is_some())
}

/// ZUSTC_NO_REGIONCK=1 turns the check off (read once: it is asked for each function)
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZUSTC_NO_REGIONCK").is_none())
}

fn lts_of(prog: &Program, base: u32) -> usize {
    prog.adt_regions.get(&base).map_or(0, |r| r.lts.len())
}

fn is_closure(prog: &Program, sid: u32) -> bool {
    prog.closure_of_struct.contains_key(&prog.base_of(sid))
}

/// The number of region slots of t (None: not followed)
pub fn count(prog: &Program, t: Ty, depth: u32) -> Option<usize> {
    if depth > 24 {
        return None;
    }
    Some(match t {
        Ty::Ref(_, i) => 1 + count(prog, *i, depth + 1)?,
        Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => count(prog, *i, depth + 1)?,
        Ty::Struct(sid) => {
            if is_closure(prog, sid) {
                return None;
            }
            let mut n = lts_of(prog, prog.base_of(sid));
            for &a in &prog.structs[sid as usize].args {
                n += count(prog, a, depth + 1)?;
            }
            n
        }
        Ty::Dyn(_) | Ty::Fn(_) => 1,
        Ty::Assoc(_) => return None,
        _ => 0,
    })
}

fn recv_sp(r: &Option<Sp>, or: Sp) -> Sp {
    r.unwrap_or(or)
}

fn strip(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(x) => strip(x),
        e => e,
    }
}

/// `x`, `x.f`: the text of a place for messages
fn place_text(e: &Expr) -> Option<String> {
    match strip(e) {
        Expr::Path(None, p) if p.segs.len() == 1 => Some(p.segs[0].name.clone()),
        Expr::Field(x, f, _) => Some(format!("{}.{f}", place_text(x)?)),
        _ => None,
    }
}

/// Reads region codes along a type (see `instantiate`)
struct Codes<'a> {
    codes: Option<&'a [u8]>,
    pos: usize,
    ok: bool,
}

impl Codes<'_> {
    fn next(&mut self) -> u8 {
        match self.codes {
            None => R_ELIDED,
            Some(c) => match c.get(self.pos) {
                Some(&x) => {
                    self.pos += 1;
                    x
                }
                None => {
                    self.ok = false;
                    R_UNKNOWN
                }
            },
        }
    }
    /// A path type's marker for lifetimes not written (`Vec<T>`)
    fn elided_adt(&mut self) -> bool {
        match self.codes {
            None => true,
            Some(c) if c.get(self.pos) == Some(&R_ELIDED_ADT) => {
                self.pos += 1;
                true
            }
            Some(_) => false,
        }
    }
}

impl Rg {
    pub fn var(&mut self) -> u32 {
        self.n += 1;
        self.n
    }

    /// a: b
    pub fn edge(&mut self, a: u32, b: u32, sp: Sp) {
        if a != b && a != STATIC {
            self.edges.push((a, b, sp));
        }
    }

    fn constrain(&mut self, a: u32, b: u32, v: Var, sp: Sp) {
        match v {
            Var::Co => self.edge(a, b, sp),
            Var::Contra => self.edge(b, a, sp),
            Var::Inv => {
                self.edge(a, b, sp);
                self.edge(b, a, sp);
            }
            Var::Bi => {}
        }
    }

    /// A tree of fresh variables for t
    pub fn fresh(&mut self, prog: &Program, t: Ty) -> Option<RT> {
        let n = count(prog, t, 0)?;
        Some((0..n).map(|_| self.var()).collect())
    }

    /// The variance of the lifetime and type parameters of a definition (computed once)
    pub fn variance(&mut self, prog: &Program, base: u32) -> Vec<Var> {
        if let Some(v) = self.variance.get(&base) {
            // (a type whose variance is being computed: what is known of it so far)
            if self.var_open.contains(&base) {
                self.var_cycle = true;
            }
            return v.clone();
        }
        let sd = &prog.structs[base as usize];
        let nl = lts_of(prog, base);
        let ng = sd.generics.len();
        let prelude = prog.struct_item.get(base as usize).is_some_and(|&i| i >= prog.prelude_from);
        // (the prelude's types stand for std's: raw pointers in them are as `NonNull` (covariant); its cells are
        // invariant)
        if prelude {
            let inv = matches!(sd.name.as_str(), "Cell" | "RefCell" | "UnsafeCell" | "Mutex" | "RwLock" | "OnceCell" | "OnceLock" | "AtomicPtr" | "MutexGuard");
            let v = vec![if inv { Var::Inv } else { Var::Co }; nl + ng];
            self.variance.insert(base, v.clone());
            return v;
        }
        // (a recursive type: computed again from what the last round found, until it no longer changes. The
        // variances of the types computed meanwhile from a provisional one are computed again too. Each round can
        // only move a variance up (bivariant, co/contra, invariant), so this ends)
        let ar = prog.adt_regions.get(&base).cloned();
        let fields: Vec<Ty> = sd.fields.iter().map(|f| f.1).chain(sd.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1))).collect();
        let mut cur = vec![Var::Bi; nl + ng];
        self.var_open.push(base);
        let outer_cycle = std::mem::take(&mut self.var_cycle);
        for _ in 0..16 {
            self.variance.insert(base, cur.clone());
            let log0 = self.var_log.len();
            self.var_cycle = false;
            let mut acc = vec![Var::Bi; nl + ng];
            for (i, &ft) in fields.iter().enumerate() {
                let codes = ar.as_ref().map(|r| r.fields.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
                let mut c = Codes { codes, pos: 0, ok: true };
                self.var_walk(prog, ft, &mut c, Var::Co, nl, &mut acc, 0);
            }
            let again = self.var_cycle && acc != cur;
            cur = acc;
            if !again {
                break;
            }
            for b in self.var_log.drain(log0..) {
                self.variance.remove(&b);
            }
        }
        self.var_open.pop();
        // (a cycle through a type still open further out: that one is computed again, and this one with it)
        self.var_cycle |= outer_cycle;
        // (a parameter not used is an error of its own (E0392); as rustc, bivariant)
        self.variance.insert(base, cur.clone());
        self.var_log.push(base);
        cur
    }

    #[allow(clippy::too_many_arguments)]
    fn var_walk(&mut self, prog: &Program, t: Ty, c: &mut Codes, v: Var, nl: usize, acc: &mut Vec<Var>, depth: u32) {
        if depth > 24 {
            return;
        }
        let lt = |acc: &mut Vec<Var>, code: u8, v: Var| {
            if (code as usize) < nl {
                acc[code as usize] = acc[code as usize].join(v);
            }
        };
        match t {
            Ty::Ref(m, i) => {
                let code = c.next();
                lt(acc, code, v);
                self.var_walk(prog, *i, c, if m { v.then(Var::Inv) } else { v }, nl, acc, depth + 1);
            }
            Ty::Ptr(m, i) => self.var_walk(prog, *i, c, if m { v.then(Var::Inv) } else { v }, nl, acc, depth + 1),
            Ty::Box(i) => {
                c.elided_adt();
                self.var_walk(prog, *i, c, v, nl, acc, depth + 1)
            }
            Ty::Array(i, _) | Ty::Slice(i) => self.var_walk(prog, *i, c, v, nl, acc, depth + 1),
            Ty::Struct(sid) => {
                if is_closure(prog, sid) {
                    c.ok = false;
                    return;
                }
                let base = prog.base_of(sid);
                let n2 = lts_of(prog, base);
                let nv = self.variance(prog, base);
                if !prog.is_tuple(sid) {
                    // (the marker of lifetimes not written is read whether or not the type has any)
                    let elided = c.elided_adt();
                    if n2 == 0 || elided {
                    } else {
                        for k in 0..n2 {
                            let code = c.next();
                            lt(acc, code, v.then(nv[k]));
                        }
                    }
                }
                let args = prog.structs[sid as usize].args.clone();
                for (j, a) in args.into_iter().enumerate() {
                    let w = if prog.is_tuple(sid) { Var::Co } else { nv.get(n2 + j).copied().unwrap_or(Var::Inv) };
                    self.var_walk(prog, a, c, v.then(w), nl, acc, depth + 1);
                }
            }
            Ty::Param(g) => {
                let k = nl + g as usize;
                if k < acc.len() {
                    acc[k] = acc[k].join(v);
                }
            }
            Ty::Fn(ft) => {
                c.next();
                let mut none = Codes { codes: Some(&[]), pos: 0, ok: true };
                for &p in &ft.params {
                    // (the regions in a fn pointer's types are not followed, its type parameters are)
                    none.codes = None;
                    self.var_walk(prog, p, &mut none, v.then(Var::Contra), 0, acc, depth + 1);
                }
                self.var_walk(prog, ft.ret, &mut none, v, 0, acc, depth + 1);
            }
            Ty::Dyn(_) => {
                let code = c.next();
                lt(acc, code, v);
            }
            _ => {}
        }
    }

    /// The tree of a type written with region codes: a signature's parameter (generic: its type parameters' trees are
    /// targs) or a field. `lts`: the variables of the lifetimes the codes name; `elided`: makes the variable of an
    /// elided region
    #[allow(clippy::too_many_arguments)]
    fn inst(&mut self, prog: &Program, t: Ty, c: &mut Codes, lts: &[u32], targs: &[Option<RT>], elided: &mut dyn FnMut(&mut Rg) -> u32, out: &mut RT, depth: u32) {
        if depth > 24 || !c.ok {
            c.ok = false;
            return;
        }
        let region = |me: &mut Rg, c: &mut Codes, elided: &mut dyn FnMut(&mut Rg) -> u32| -> u32 {
            match c.next() {
                R_STATIC => STATIC,
                R_ELIDED => elided(me),
                x if (x as usize) < lts.len() => lts[x as usize],
                _ => {
                    c.ok = false;
                    0
                }
            }
        };
        match t {
            Ty::Ref(_, i) => {
                let r = region(self, c, elided);
                out.push(r);
                self.inst(prog, *i, c, lts, targs, elided, out, depth + 1);
            }
            Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) => self.inst(prog, *i, c, lts, targs, elided, out, depth + 1),
            Ty::Box(i) => {
                c.elided_adt();
                self.inst(prog, *i, c, lts, targs, elided, out, depth + 1)
            }
            Ty::Struct(sid) => {
                if is_closure(prog, sid) {
                    c.ok = false;
                    return;
                }
                let base = prog.base_of(sid);
                let n = lts_of(prog, base);
                if !prog.is_tuple(sid) {
                    if n == 0 {
                        c.elided_adt();
                    } else if c.elided_adt() {
                        for _ in 0..n {
                            let r = elided(self);
                            out.push(r);
                        }
                    } else {
                        for _ in 0..n {
                            let r = region(self, c, elided);
                            out.push(r);
                        }
                    }
                }
                for &a in &prog.structs[sid as usize].args {
                    self.inst(prog, a, c, lts, targs, elided, out, depth + 1);
                }
            }
            Ty::Param(g) => match targs.get(g as usize) {
                Some(Some(t)) => out.extend_from_slice(t),
                Some(None) => c.ok = false,
                None => {}
            },
            Ty::Dyn(_) | Ty::Fn(_) => {
                // (a trait object's or fn pointer's regions are not followed: a variable of its own)
                let _ = c.next();
                let v = self.var();
                out.push(v);
            }
            Ty::Assoc(_) => c.ok = false,
            _ => {}
        }
    }

    /// Instantiate t with codes (all of them must be read)
    #[allow(clippy::too_many_arguments)]
    pub fn instantiate(&mut self, prog: &Program, t: Ty, codes: Option<&[u8]>, lts: &[u32], targs: &[Option<RT>], elided: &mut dyn FnMut(&mut Rg) -> u32) -> Option<RT> {
        // (a parameter whose type is not followed)
        if codes.is_some_and(|c| c.contains(&R_UNKNOWN)) {
            return None;
        }
        let mut c = Codes { codes, pos: 0, ok: true };
        let mut out = vec![];
        self.inst(prog, t, &mut c, lts, targs, elided, &mut out, 0);
        (c.ok && codes.is_none_or(|x| c.pos == x.len())).then_some(out)
    }

    /// sub <: sup at type t (the value of sub flows where sup is expected)
    pub fn relate(&mut self, prog: &Program, t: Ty, sub: &[u32], sup: &[u32], sp: Sp) {
        if sub.len() != sup.len() || count(prog, t, 0) != Some(sub.len()) {
            return;
        }
        let mut k = 0;
        self.rel(prog, t, sub, sup, &mut k, Var::Co, sp, 0);
    }

    #[allow(clippy::too_many_arguments)]
    fn rel(&mut self, prog: &Program, t: Ty, sub: &[u32], sup: &[u32], k: &mut usize, v: Var, sp: Sp, depth: u32) {
        if depth > 24 {
            return;
        }
        match t {
            Ty::Ref(m, i) => {
                self.constrain(sub[*k], sup[*k], v, sp);
                *k += 1;
                self.rel(prog, *i, sub, sup, k, if m { v.then(Var::Inv) } else { v }, sp, depth + 1);
            }
            Ty::Ptr(m, i) => self.rel(prog, *i, sub, sup, k, if m { v.then(Var::Inv) } else { v }, sp, depth + 1),
            Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.rel(prog, *i, sub, sup, k, v, sp, depth + 1),
            Ty::Struct(sid) => {
                let base = prog.base_of(sid);
                let n = lts_of(prog, base);
                let nv = self.variance(prog, base);
                for j in 0..n {
                    self.constrain(sub[*k], sup[*k], v.then(nv[j]), sp);
                    *k += 1;
                }
                let tuple = prog.is_tuple(sid);
                for (j, &a) in prog.structs[sid as usize].args.clone().iter().enumerate() {
                    let w = if tuple { Var::Co } else { nv.get(n + j).copied().unwrap_or(Var::Inv) };
                    self.rel(prog, a, sub, sup, k, v.then(w), sp, depth + 1);
                }
            }
            Ty::Dyn(_) => {
                self.constrain(sub[*k], sup[*k], v, sp);
                *k += 1;
            }
            Ty::Fn(_) => *k += 1,
            _ => {}
        }
    }

    /// The trees of a struct's lifetime parameters and type arguments, from the tree of an instance
    pub fn split_adt(&self, prog: &Program, sid: u32, t: &[u32]) -> Option<(Vec<u32>, Vec<Option<RT>>)> {
        let n = lts_of(prog, prog.base_of(sid));
        let own = t.get(..n)?.to_vec();
        let mut k = n;
        let mut args = vec![];
        for &a in &prog.structs[sid as usize].args {
            let c = count(prog, a, 0)?;
            args.push(Some(t.get(k..k + c)?.to_vec()));
            k += c;
        }
        Some((own, args))
    }

    /// The tree of field i (over all variants) of an instance sid whose tree is t
    pub fn field(&mut self, prog: &Program, sid: u32, i: usize, t: &[u32]) -> Option<RT> {
        let base = prog.base_of(sid);
        let bd = &prog.structs[base as usize];
        let ft = bd.fields.iter().map(|f| f.1).chain(bd.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1))).nth(i)?;
        let (own, args) = self.split_adt(prog, sid, t)?;
        let ar = prog.adt_regions.get(&base).cloned();
        let codes = ar.as_ref().map(|r| r.fields.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        self.instantiate(prog, ft, codes, &own, &args, &mut |me| me.var())
    }

    /// What the types of the signature imply among its regions: `&'a T` means every region in T outlives 'a; a
    /// struct's fields likewise (rustc's implied bounds, with the outlives requirements inferred from fields)
    pub fn implied(&mut self, prog: &Program, t: Ty, tree: &[u32], depth: u32) {
        if depth > 4 {
            return;
        }
        let mut k = 0;
        self.imp(prog, t, tree, &mut k, &mut vec![], depth);
    }

    fn imp(&mut self, prog: &Program, t: Ty, tree: &[u32], k: &mut usize, outer: &mut Vec<u32>, depth: u32) {
        if *k > tree.len() || depth > 24 {
            return;
        }
        match t {
            Ty::Ref(_, i) => {
                let Some(&r) = tree.get(*k) else { return };
                for &o in outer.iter() {
                    self.known.push((r, o));
                }
                *k += 1;
                outer.push(r);
                self.imp(prog, *i, tree, k, outer, depth);
                outer.pop();
            }
            Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.imp(prog, *i, tree, k, outer, depth),
            Ty::Struct(sid) => {
                let Some(c) = count(prog, t, 0) else { return };
                let Some(me) = tree.get(*k..*k + c) else { return };
                let me = me.to_vec();
                for &r in &me {
                    for &o in outer.iter() {
                        self.known.push((r, o));
                    }
                }
                *k += c;
                // (the fields: what the definition's types require of its parameters)
                let base = prog.base_of(sid);
                if prog.adt_regions.contains_key(&base) && depth < 3 {
                    let bd = &prog.structs[base as usize];
                    let nf = bd.fields.len() + bd.variants.iter().map(|v| v.fields.len()).sum::<usize>();
                    let fts: Vec<Ty> = prog.structs[sid as usize].fields.iter().map(|f| f.1).chain(prog.structs[sid as usize].variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1))).collect();
                    for i in 0..nf {
                        let n0 = self.n;
                        if let (Some(ft), Some(fr)) = (fts.get(i).copied(), self.field(prog, sid, i, &me)) {
                            // (only the field's regions that are the struct's: fresh ones were made for nothing)
                            if fr.iter().all(|&x| x <= n0) {
                                self.implied(prog, ft, &fr, depth + 1);
                            }
                        }
                    }
                    if let Some(ar) = prog.adt_regions.get(&base).cloned() {
                        let (own, args) = self.split_adt(prog, sid, &me).unwrap_or_default();
                        for &(a, b) in &ar.outlives {
                            if let (Some(&x), Some(&y)) = (own.get(a as usize), own.get(b as usize)) {
                                self.known.push((x, y));
                            }
                        }
                        for (tp, b) in &ar.ty_outlives {
                            let (Some(g), Some(&y)) = (bd.generics.iter().position(|x| x == tp), own.get(*b as usize)) else { continue };
                            if let Some(Some(at)) = args.get(g) {
                                for &x in at {
                                    self.known.push((x, y));
                                }
                            }
                        }
                    }
                }
            }
            Ty::Dyn(_) | Ty::Fn(_) => *k += 1,
            _ => {}
        }
    }

    /// The first universal region that must outlive another the function does not know it outlives
    pub fn check(&self) -> Option<(Sp, String)> {
        if self.edges.is_empty() {
            return None;
        }
        // (the universal regions and 'static, as indices; what each is known to outlive, closed)
        let mut ids: Vec<u32> = vec![STATIC];
        ids.extend(self.univ.iter().map(|u| u.0));
        let ix = |v: u32| ids.iter().position(|&x| x == v);
        let nu = ids.len();
        let mut k = vec![vec![false; nu]; nu];
        for i in 0..nu {
            k[i][i] = true;
            k[0][i] = true;
        }
        for &(a, b) in &self.known {
            if let (Some(i), Some(j)) = (ix(a), ix(b)) {
                k[i][j] = true;
            }
        }
        for m in 0..nu {
            for i in 0..nu {
                if k[i][m] {
                    for j in 0..nu {
                        if k[m][j] {
                            k[i][j] = true;
                        }
                    }
                }
            }
        }
        let n = self.n as usize + 1;
        let mut adj: Vec<Vec<(u32, Sp)>> = vec![vec![]; n];
        for &(a, b, sp) in &self.edges {
            if (a as usize) < n && (b as usize) < n {
                adj[a as usize].push((b, sp));
            }
        }
        let mut errs: Vec<(Sp, String)> = vec![];
        for (ui, &(u, ref uname)) in self.univ.iter().enumerate() {
            let i = ui + 1;
            let mut seen = vec![false; n];
            seen[u as usize] = true;
            let mut st = vec![u];
            while let Some(x) = st.pop() {
                for &(y, sp) in &adj[x as usize] {
                    if seen[y as usize] {
                        continue;
                    }
                    seen[y as usize] = true;
                    if let Some(j) = ix(y) {
                        if !k[i][j] {
                            let other = if j == 0 { "'static".to_string() } else { self.univ[j - 1].1.clone() };
                            errs.push((sp, format!("lifetime may not live long enough (`{uname}` must outlive `{other}`)")));
                        }
                    }
                    st.push(y);
                }
            }
        }
        // (a borrow of what the function owns that must outlive a universal region (or be 'static): it does not
        // live long enough)
        for (r, name, lsp) in &self.loans {
            let mut seen = vec![false; n];
            seen[*r as usize] = true;
            let mut st = vec![*r];
            let mut bad = false;
            while let Some(x) = st.pop() {
                for &(y, _) in &adj[x as usize] {
                    if seen[y as usize] {
                        continue;
                    }
                    seen[y as usize] = true;
                    if ix(y).is_some() {
                        bad = true;
                    }
                    st.push(y);
                }
            }
            if bad {
                errs.push((*lsp, if name.is_empty() { "temporary value dropped while borrowed (it must outlive the function)".to_string() } else { format!("`{name}` does not live long enough (borrowed value must outlive the function)") }));
            }
        }
        errs.sort_by_key(|e| (e.0.line, e.0.col));
        errs.into_iter().next()
    }
}

impl Cx<'_> {
    pub(crate) fn rg_on(&self) -> bool {
        self.rg.on && self.inline_depth == 0
    }

    /// At the start of the body: the universal regions of the signature, the trees of the parameters and of the
    /// return type. Only a function with at least two regions in its signature (or a named one) is checked
    pub(crate) fn rg_begin(&mut self, regions: Option<&RSigSyn>, params: &[(Loc, Ty)], self_elides: bool) {
        let prog = self.prog;
        let rs = regions;
        let codes = |i: usize| rs.map(|r| r.slots.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        let mut rg = std::mem::take(&mut self.rg);
        for l in rs.map(|r| r.lts.as_slice()).unwrap_or(&[]) {
            let v = rg.var();
            rg.univ.push((v, l.clone()));
            rg.lts.push(v);
            rg.lt_names.push(l.clone());
        }
        let lts = rg.lts.clone();
        // (an instance: the generic signature's types, with fresh trees for the type parameters)
        let gsig = &prog.sigs[rg.base as usize];
        let generic = !rg.inst_args.is_empty() && gsig.params.len() == params.len();
        let targs: Vec<Option<RT>> = if generic { rg.inst_args.clone().into_iter().map(|t| rg.fresh(prog, t)).collect() } else { vec![] };
        let mut trees: Vec<Option<RT>> = vec![];
        let mut inputs: Vec<u32> = vec![];
        for (i, &(_, t)) in params.iter().enumerate() {
            let t = if generic { gsig.params[i] } else { t };
            let mut el = |me: &mut Rg| {
                let v = me.var();
                me.univ.push((v, "'_".to_string()));
                v
            };
            let tr = rg.instantiate(prog, t, codes(i), &lts, &targs, &mut el);
            if let Some(tr) = &tr {
                inputs.extend(tr.iter().copied().filter(|&v| v != STATIC && rg.univ.iter().any(|u| u.0 == v)));
            }
            trees.push(tr);
        }
        // (the return type's elided regions: of `&self` (or a receiver type naming `&Self`: `self: Box<&Self>`), else
        // of the one region among the parameters)
        let self_r = if self_elides { trees.first().and_then(|t| t.as_ref()).and_then(|t| t.first().copied()) } else { None };
        inputs.dedup();
        let one = if inputs.len() == 1 { Some(inputs[0]) } else { None };
        let mut bad = false;
        let ret_t = self.ret;
        let mut el = |_: &mut Rg| match self_r.or(one) {
            Some(v) => v,
            None => {
                bad = true;
                STATIC
            }
        };
        let gret = if generic { gsig.ret } else { ret_t };
        let ret = rg.instantiate(prog, gret, codes(params.len()), &lts, &targs, &mut el);
        rg.ret = ret.filter(|_| !bad);
        let nuniv = rg.univ.len();
        rg.on = nuniv >= 2 || (nuniv >= 1 && rs.is_some());
        if rg.on {
            for (i, &(loc, t)) in params.iter().enumerate() {
                if let Some(tr) = trees[i].clone() {
                    rg.implied(prog, t, &tr, 0);
                    rg.locals.insert(loc, tr);
                }
            }
            if let Some(tr) = rg.ret.clone() {
                rg.implied(prog, ret_t, &tr, 0);
            }
            if let Some(r) = rs {
                for &(a, b) in &r.outlives {
                    let m = |c: u8| if c == R_STATIC { Some(STATIC) } else { lts.get(c as usize).copied() };
                    if let (Some(x), Some(y)) = (m(a), m(b)) {
                        rg.known.push((x, y));
                    }
                }
            }
        }
        self.rg = rg;
    }

    /// The tree of a value (None: not followed)
    pub(crate) fn rt_expr(&mut self, e: &Expr) -> Option<(Ty, RT)> {
        let r = self.rt_expr_inner(e);
        if r.is_none() && debug() {
            self.rg.missed.push(kind(e));
        }
        r
    }

    fn rt_expr_inner(&mut self, e: &Expr) -> Option<(Ty, RT)> {
        if let Some(x) = self.rg.calls.get(&(e as *const Expr as usize)) {
            return Some(x.clone());
        }
        match e {
            Expr::Paren(x) => self.rt_expr(x),
            Expr::Lit(s, _) if s.starts_with('"') || s.starts_with("r\"") || s.starts_with("r#") => Some((Ty::Ref(false, crate::types::intern(Ty::Str)), vec![STATIC])),
            // (`&f(..)` / `&mut 4` / `&{ let y = ..; y }`: a temporary the function owns (a `&` of a literal is
            // promoted to a static, and so may be a block of a constant alone))
            Expr::Ref(m, x)
                if matches!(strip(x), Expr::Call(..) | Expr::MethodCall(..))
                    || (*m && matches!(strip(x), Expr::Lit(..)))
                    || matches!(strip(x), Expr::Block(None, false, b) if b.stmts.len() > 1)
                    || (matches!(strip(x), Expr::Ref(..) | Expr::Struct(..) | Expr::Tuple(..)) && self.mentions_local(x)) =>
            {
                let (t, tr) = match strip(x) {
                    Expr::Lit(..) => (Ty::U8, vec![]),
                    Expr::Block(_, _, b) => match b.stmts.last() {
                        Some(Stmt::Expr(tail, false)) => self.rt_expr(tail)?,
                        _ => return None,
                    },
                    x @ (Expr::Ref(..) | Expr::Struct(..) | Expr::Tuple(..)) => self.rt_expr(x)?,
                    x => self.rg.calls.get(&(x as *const Expr as usize)).cloned()?,
                };
                let r = self.rg.var();
                let sp = Some(super::borrowck::pos(e)).filter(|s| s.line != 0).unwrap_or(self.rg.let_sp);
                self.rg.loans.push((r, String::new(), sp));
                let mut out = vec![r];
                out.extend(tr);
                Some((Ty::Ref(*m, crate::types::intern(t)), out))
            }
            Expr::Ref(m, x) => {
                let (t, tr, derefs) = self.rt_place(x)?;
                let r = self.rg.var();
                let sp = super::borrowck::pos(e);
                // (of a place the function owns: the borrow ends with the function at the latest)
                if derefs.is_empty() && !matches!(strip(x), Expr::Call(..) | Expr::MethodCall(..)) {
                    if let Some(n) = place_text(x) {
                        self.rg.loans.push((r, n, sp));
                    }
                }
                // (a reborrow through references: it lives no longer than they do)
                for d in derefs {
                    self.rg.edge(d, r, sp);
                }
                let mut out = vec![r];
                out.extend(tr);
                Some((Ty::Ref(*m, crate::types::intern(t)), out))
            }
            Expr::Call(..) | Expr::MethodCall(..) | Expr::Struct(..) => self.rg.calls.get(&(e as *const Expr as usize)).cloned(),
            // (`e as &'static T`: the cast's type, with the regions written in it; e flows into it)
            Expr::Cast(x, ty) => {
                let (it, itr) = self.rt_expr(x)?;
                let to = self.parse_ty(ty).ok()?;
                if to != it {
                    return None;
                }
                let prog = self.prog;
                let names = self.rg.lt_names.clone();
                let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], any: false };
                let mut codes = vec![];
                s.slots(ty, false, &mut codes);
                let lts = self.rg.lts.clone();
                let tree = self.rg.instantiate(prog, to, Some(&codes), &lts, &[], &mut |me| me.var())?;
                self.rg.relate(prog, to, &itr, &tree, super::borrowck::pos(x));
                Some((to, tree))
            }
            // (a block's value: its tail's, kept while the block's locals were in scope)
            Expr::Block(None, false, b) => match b.stmts.last() {
                Some(Stmt::Expr(t, false)) => self.rt_expr(t),
                _ => None,
            },
            Expr::Path(..) | Expr::Field(..) | Expr::Unary(..) => match self.rt_place(e) {
                Some((t, tr, _)) => Some((t, tr)),
                // (a unit variant: `None`)
                None => self.rg.calls.get(&(e as *const Expr as usize)).cloned(),
            },
            _ => None,
        }
    }

    /// e reads a local (so it is not a constant a `&` would promote to a static)
    fn mentions_local(&self, e: &Expr) -> bool {
        match strip(e) {
            Expr::Path(None, p) => p.segs.len() == 1 && self.lookup(&p.segs[0].name).is_some(),
            Expr::Ref(_, x) | Expr::Unary(_, x, _) | Expr::Field(x, _, _) | Expr::Cast(x, _) => self.mentions_local(x),
            Expr::Tuple(xs) | Expr::Array(xs) => xs.iter().any(|x| self.mentions_local(x)),
            Expr::Struct(_, fs, _) => fs.iter().any(|f| self.mentions_local(&f.1)),
            _ => false,
        }
    }

    /// A function the gate left out begins its check here, where something must be 'static: what it owns must not
    /// be borrowed for that (what came before has no trees and adds no constraint)
    pub(crate) fn rg_late_on(&mut self) {
        if !self.rg.on && self.rg.eligible {
            self.rg.on = true;
        }
    }

    /// The signature requires a region of it to be 'static
    pub(crate) fn needs_static(r: &RSigSyn) -> bool {
        r.slots.iter().flatten().any(|&c| c == R_STATIC) || r.ty_outlives.iter().any(|x| x.1 == R_STATIC) || r.outlives.iter().any(|x| x.1 == R_STATIC)
    }

    /// The tree of a place, and the regions of the references it goes through
    pub(crate) fn rt_place(&mut self, e: &Expr) -> Option<(Ty, RT, Vec<u32>)> {
        match e {
            Expr::Paren(x) => self.rt_place(x),
            Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => {
                let (loc, t) = self.lookup(&p.segs[0].name)?;
                let tr = match self.rg.locals.get(&loc) {
                    Some(tr) => tr.clone(),
                    // (a local whose type has no region (bound before the check began): its tree is empty)
                    None if count(self.prog, t, 0) == Some(0) => vec![],
                    None => return None,
                };
                Some((t, tr, vec![]))
            }
            Expr::Unary(op, x, _) if op == "*" => {
                let (t, tr, mut ds) = self.rt_place(x)?;
                match t {
                    Ty::Ref(_, i) => {
                        ds.push(*tr.first()?);
                        Some((*i, tr[1..].to_vec(), ds))
                    }
                    Ty::Box(i) => Some((*i, tr, ds)),
                    _ => None,
                }
            }
            Expr::Field(x, f, _) => {
                let (mut t, mut tr, mut ds) = self.rt_place(x)?;
                // (auto-deref)
                loop {
                    match t {
                        Ty::Ref(_, i) => {
                            ds.push(*tr.first()?);
                            tr = tr[1..].to_vec();
                            t = *i;
                        }
                        Ty::Box(i) => t = *i,
                        _ => break,
                    }
                }
                let Ty::Struct(sid) = t else { return None };
                let sd = &self.prog.structs[sid as usize];
                if sd.is_enum {
                    return None;
                }
                let i = sd.fields.iter().position(|x| x.0 == *f)?;
                let ft = sd.fields[i].1;
                let prog = self.prog;
                let fr = self.rg.field(prog, sid, i, &tr)?;
                Some((ft, fr, ds))
            }
            Expr::Call(..) | Expr::MethodCall(..) => {
                let (t, tr) = self.rg.calls.get(&(e as *const Expr as usize)).cloned()?;
                Some((t, tr, vec![]))
            }
            _ => None,
        }
    }

    /// `let name[: ann] = init;` (after name is bound)
    pub(crate) fn rg_let(&mut self, name: &str, ann: Option<&PTy>, init: &Expr, init_tree: Option<(Ty, RT)>) {
        if !self.rg_on() {
            return;
        }
        let Some((loc, t)) = self.lookup(name) else { return };
        let prog = self.prog;
        // (an annotation names the function's lifetimes; its elided ones are fresh)
        let tree = match ann {
            Some(a) => {
                let names = self.rg.lt_names.clone();
                let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], any: false };
                let mut codes = vec![];
                s.slots(a, false, &mut codes);
                let lts = self.rg.lts.clone();
                self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var())
            }
            None => self.rg.fresh(prog, t),
        };
        let Some(tree) = tree else { return };
        // (the initializer's tree was taken before the binding: `let y = y;` reads the y it shadows)
        if let Some((it, itr)) = init_tree {
            if count(prog, it, 0) == Some(tree.len()) {
                self.rg.relate(prog, t, &itr, &tree, super::borrowck::pos(init));
            }
        }
        self.rg.locals.insert(loc, tree);
    }

    /// `place = value;`
    pub(crate) fn rg_assign(&mut self, place: &Expr, value: &Expr) {
        if !self.rg_on() {
            return;
        }
        let (Some((t, ptr, _)), Some((_, vtr))) = (self.rt_place(place), self.rt_expr(value)) else { return };
        let prog = self.prog;
        self.rg.relate(prog, t, &vtr, &ptr, super::borrowck::pos(value));
    }

    /// The value e is returned
    pub(crate) fn rg_ret(&mut self, e: &Expr) {
        if !self.rg_on() {
            return;
        }
        match e {
            Expr::Paren(x) => return self.rg_ret(x),
            Expr::Block(None, _, b) => {
                if let Some(Stmt::Expr(t, false)) = b.stmts.last() {
                    self.rg_ret(t);
                }
                return;
            }
            Expr::If(_, b, els) => {
                if let Some(Stmt::Expr(t, false)) = b.stmts.last() {
                    self.rg_ret(t);
                }
                if let Some(x) = els {
                    self.rg_ret(x);
                }
                return;
            }
            Expr::Match(_, arms) => {
                if let Some(&m2) = self.rg.alias.get(&(e as *const Expr as usize)) {
                    return self.rg_ret(m2);
                }
                for a in arms {
                    self.rg_ret(&a.body);
                }
                return;
            }
            _ => {}
        }
        let Some(rt) = self.rg.ret.clone() else { return };
        let Some((_, tr)) = self.rt_expr(e) else { return };
        let (prog, t) = (self.prog, self.ret);
        self.rg.relate(prog, t, &tr, &rt, super::borrowck::pos(e));
    }

    /// A call of id (generic signature sig0, type arguments sol, instance's parameter types params) with these
    /// arguments (the receiver's tree is rg.recv): relate the arguments to the parameters, and keep the result's tree
    pub(crate) fn rg_call(&mut self, id: u32, sol: &[Option<Ty>], args: &[&Expr], params: &[Ty], ret: Ty, recv: Option<(RT, Sp)>, has_recv: bool, sp: Sp) {
        if !self.rg_on() {
            return;
        }
        let recv_info = recv.as_ref().map(|r| r.1);
        self.rg.last_call = None;
        let prog = self.prog;
        let sig0 = &prog.sigs[id as usize];
        let rs = sig0.regions.clone();
        let codes = |i: usize| rs.as_deref().map(|r| r.slots.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        // (the prelude writes no lifetimes: what its results borrow is not known from elision)
        let prelude = prog.item_of.get(id as usize).copied().flatten().is_none_or(|i| i >= prog.prelude_from);
        let mut rg = std::mem::take(&mut self.rg);
        let lts: Vec<u32> = rs.as_deref().map(|r| r.lts.iter().map(|_| rg.var()).collect()).unwrap_or_default();
        let targs: Vec<Option<RT>> = sol.iter().map(|t| t.and_then(|t| rg.fresh(prog, t))).collect();
        // (the regions a type argument brings are not the signature's own: elision does not see them)
        let substituted: Vec<u32> = targs.iter().flatten().flatten().copied().collect();
        let mut ptrees: Vec<Option<RT>> = vec![];
        let mut inputs = vec![];
        for (i, &pt) in sig0.params.iter().enumerate() {
            let tr = rg.instantiate(prog, pt, codes(i), &lts, &targs, &mut |me| me.var());
            if let Some(t) = &tr {
                inputs.extend(t.iter().copied().filter(|&v| v != STATIC && !substituted.contains(&v)));
            }
            ptrees.push(tr);
        }
        // (the arguments flow into the parameters)
        let mut atrees: Vec<(Option<RT>, Sp)> = vec![];
        if has_recv {
            atrees.push((recv.map(|r| r.0), recv_sp(&recv_info, sp)));
        }
        self.rg = rg;
        for a in args {
            let t = self.rt_expr(a).map(|x| x.1);
            atrees.push((t, super::borrowck::pos(a)));
        }
        let mut rg = std::mem::take(&mut self.rg);
        for (i, ((at, asp), pt)) in atrees.iter().zip(&ptrees).enumerate() {
            if let (Some(at), Some(pt), Some(&ty)) = (at, pt, params.get(i)) {
                rg.relate(prog, ty, at, pt, *asp);
            }
        }
        // (the callee's declared bounds hold of its arguments; not those of a trait's impl called from a generic
        // function's instance: the generic body called the trait's method, under its own bounds)
        let through_trait = !rg.inst_args.is_empty() && (sig0.impl_trait.is_some() || sig0.trait_of.is_some());
        if let Some(r) = rs.as_deref().filter(|_| !through_trait) {
            let m = |c: u8| if c == R_STATIC { Some(STATIC) } else { lts.get(c as usize).copied() };
            for &(a, b) in &r.outlives {
                if let (Some(x), Some(y)) = (m(a), m(b)) {
                    rg.edge(x, y, sp);
                }
            }
            for (tp, b) in &r.ty_outlives {
                let (Some(g), Some(y)) = (sig0.generics.iter().position(|x| x.0 == *tp), m(*b)) else { continue };
                if let Some(Some(at)) = targs.get(g) {
                    for &x in at {
                        rg.edge(x, y, sp);
                    }
                }
            }
        }
        // (the result: its elided regions are of `&self`, else of the one region among the parameters)
        // (a receiver `&self` / `&mut self`: of the impl's own type; another receiver type leaves the result's
        // elided regions unknown)
        let self_ref = sig0.has_self && matches!((sig0.params.first(), sig0.impl_of), (Some(Ty::Ref(_, Ty::Struct(s))), Some(i)) if prog.base_of(*s) == prog.base_of(i));
        let self_r = if self_ref { ptrees.first().and_then(|t| t.as_ref()).and_then(|t| t.first().copied()).filter(|v| !substituted.contains(v)) } else { None };
        let other_self = sig0.has_self && !self_ref;
        inputs.dedup();
        let one = if inputs.len() == 1 { Some(inputs[0]) } else { None };
        let n = sig0.params.len();
        let rtr = rg.instantiate(prog, sig0.ret, codes(n), &lts, &targs, &mut |me| match (prelude || (other_self && self_r.is_none()), self_r.or(one)) {
            (false, Some(v)) => v,
            _ => me.var(),
        });
        if let Some(rtr) = rtr {
            if count(prog, ret, 0) == Some(rtr.len()) {
                rg.last_call = Some((rg.depth, ret, rtr));
            }
        }
        self.rg = rg;
    }

    /// The tree of a method call's receiver as passed: recv's place, through `derefs` references, then borrowed again
    /// when the method takes `&self` / `&mut self` (passed as recv_ty)
    pub(crate) fn rg_recv(&mut self, recv: &Expr, recv_ty: Ty, sid: u32) {
        self.rg.recv = None;
        if !self.rg_on() {
            return;
        }
        let Some((mut t, mut tr, mut ds)) = self.rt_place(recv).or_else(|| self.rt_expr(recv).map(|(t, tr)| (t, tr, vec![]))) else { return };
        loop {
            match t {
                Ty::Struct(s) if s == sid => break,
                Ty::Ref(_, i) => {
                    let Some(&r) = tr.first() else { return };
                    ds.push(r);
                    tr = tr[1..].to_vec();
                    t = *i;
                }
                Ty::Box(i) => t = *i,
                _ => return,
            }
        }
        let tree = match recv_ty {
            Ty::Ref(..) => {
                let r = self.rg.var();
                let sp = super::borrowck::pos(recv);
                for d in ds {
                    self.rg.edge(d, r, sp);
                }
                let mut v = vec![r];
                v.extend(tr);
                v
            }
            _ => tr,
        };
        self.rg.recv = Some((tree, super::borrowck::pos(recv)));
    }

    /// A struct literal of type t: a fresh tree, the fields' values flow into its fields
    pub(crate) fn rg_struct_lit(&mut self, e: &Expr, t: Ty) {
        // (a field of a type written `'static`: from here on, checked)
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 {
            if let Ty::Struct(sid) = t {
                if self.prog.adt_regions.get(&self.prog.base_of(sid)).is_some_and(|r| r.fields.iter().flatten().any(|&c| c == R_STATIC)) {
                    self.rg_late_on();
                }
            }
        }
        if !self.rg_on() {
            return;
        }
        let (Expr::Struct(_, fs, None), Ty::Struct(sid)) = (e, t) else { return };
        let prog = self.prog;
        if prog.structs[sid as usize].is_enum {
            return;
        }
        let Some(tree) = self.rg.fresh(prog, t) else { return };
        for (n, v) in fs {
            let Some(i) = prog.structs[sid as usize].fields.iter().position(|f| f.0 == *n) else { continue };
            let ft = prog.structs[sid as usize].fields[i].1;
            let Some(ftr) = self.rg.field(prog, sid, i, &tree) else { continue };
            if let Some((_, vtr)) = self.rt_expr(v) {
                self.rg.relate(prog, ft, &vtr, &ftr, super::borrowck::pos(v));
            }
        }
        self.rg.calls.insert(e as *const Expr as usize, (t, tree));
    }

    /// A variant's value of type t (`Some(x)`, `None`): a fresh tree, the fields' values flow into its fields; kept as
    /// a call's result (see rg_took_call)
    pub(crate) fn rg_variant(&mut self, t: Ty, vi: usize, args: &[(usize, &Expr)]) {
        if !self.rg_on() {
            return;
        }
        // (nothing a field's call left is this value's)
        self.rg.last_call = None;
        let Ty::Struct(sid) = t else { return };
        let prog = self.prog;
        let Some(tree) = self.rg.fresh(prog, t) else { return };
        let sd = &prog.structs[sid as usize];
        let Some(v) = sd.variants.get(vi) else { return };
        let off: usize = sd.fields.len() + sd.variants[..vi].iter().map(|v| v.fields.len()).sum::<usize>();
        let fts: Vec<Ty> = v.fields.iter().map(|f| f.1).collect();
        for &(fi, ex) in args {
            let (Some(&ft), Some(ftr)) = (fts.get(fi), self.rg.field(prog, sid, off + fi, &tree)) else { continue };
            if let Some((_, vtr)) = self.rt_expr(ex) {
                self.rg.relate(prog, ft, &vtr, &ftr, super::borrowck::pos(ex));
            }
        }
        self.rg.last_call = Some((self.rg.depth + 1, t, tree));
    }

    /// The pattern matches through references (default binding modes) down to a value of type t
    pub(crate) fn rg_pat_align(&mut self, t: Ty) {
        let Some((mut pt, mut tr)) = self.rg.pat.take() else { return };
        while pt != t {
            let Ty::Ref(_, i) = pt else { return };
            let Some(&r) = tr.first() else { return };
            self.rg.pat_ref = Some(r);
            tr.remove(0);
            pt = *i;
        }
        self.rg.pat = Some((pt, tr));
    }

    /// Descend into field fi of the value the pattern matches (of type parent; vi: the variant); returns what to
    /// restore
    pub(crate) fn rg_pat_field(&mut self, parent: Ty, vi: Option<usize>, fi: usize) -> (Option<(Ty, RT)>, Option<u32>) {
        let saved = (self.rg.pat.clone(), self.rg.pat_ref);
        let prog = self.prog;
        let next = (|| {
            let (pt, tr) = self.rg.pat.clone()?;
            let Ty::Struct(sid) = pt else { return None };
            if pt != parent {
                return None;
            }
            let sd = &prog.structs[sid as usize];
            if prog.is_tuple(sid) {
                let (_, args) = self.rg.split_adt(prog, sid, &tr)?;
                return Some((*sd.args.get(fi)?, args.get(fi)?.clone()?));
            }
            let (off, ft) = match vi {
                Some(vi) => (sd.fields.len() + sd.variants[..vi].iter().map(|v| v.fields.len()).sum::<usize>() + fi, sd.variants.get(vi)?.fields.get(fi)?.1),
                None => (fi, sd.fields.get(fi)?.1),
            };
            let ftr = self.rg.field(prog, sid, off, &tr)?;
            Some((ft, ftr))
        })();
        if debug() {
            eprintln!("rg_pat_field {:?} vi={vi:?} fi={fi}: {:?} -> {:?}", self.prog.tn(parent), saved.0.as_ref().map(|x| (self.prog.tn(x.0), x.1.clone())), next.as_ref().map(|x| x.1.clone()));
        }
        self.rg.pat = next;
        saved
    }

    /// The pattern binds name (by_ref: by reference, through the reference matched last)
    pub(crate) fn rg_pat_bind(&mut self, name: &str, by_ref: bool) {
        let Some((_, tr)) = self.rg.pat.clone() else { return };
        let Some((loc, t)) = self.lookup(name) else { return };
        let tree = if by_ref {
            let Some(r) = self.rg.pat_ref else { return };
            let mut v = vec![r];
            v.extend(tr);
            v
        } else {
            tr
        };
        if count(self.prog, t, 0) == Some(tree.len()) {
            self.rg.locals.insert(loc, tree);
        }
    }

    /// The tree of e, kept while what it names is in scope (an arm's or a block's value, related after the match or
    /// block is lowered)
    pub(crate) fn rg_keep(&mut self, e: &Expr) {
        if !self.rg_on() || self.rg.calls.contains_key(&(e as *const Expr as usize)) {
            return;
        }
        if let Some(t) = self.rt_expr(e) {
            self.rg.calls.insert(e as *const Expr as usize, t);
        }
    }

    /// Keep the tree of the call just lowered, as the value of e
    pub(crate) fn rg_took_call(&mut self, e: &Expr) {
        // (only the call made by this expression's own call_fn: one level below)
        if let Some((d, t, tr)) = self.rg.last_call.take() {
            if d == self.rg.depth + 1 {
                self.rg.calls.insert(e as *const Expr as usize, (t, tr));
            }
        }
    }

    /// Report what the constraints require
    pub(crate) fn rg_check(&mut self) -> super::R<()> {
        if debug() {
            eprintln!("regionck {}: on={} univ={} edges={} loans={} missed={:?}", self.prog.names.get(self.rg.base as usize).map(|s| s.as_str()).unwrap_or("?"), self.rg.on, self.rg.univ.len(), self.rg.edges.len(), self.rg.loans.len(), self.rg.missed);
        }
        if !self.rg.on {
            return Ok(());
        }
        match self.rg.check() {
            Some((sp, m)) => Err(self.at(sp, m)),
            None => Ok(()),
        }
    }
}
