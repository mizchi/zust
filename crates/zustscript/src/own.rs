//! Ownership for the Rust that `@wasm` functions are lowered to: what is Copy, and where a value that is not Copy
//! (an array, or a record or enum that holds one) can be moved instead of cloned.
//!
//! zustscript's values are immutable, so any sharing is safe in the language; the question is only how to say it in
//! Rust without copying. The lowering (lower.rs) decides by three facts:
//!
//! 1. Copy ([`is_copy`]): a record or enum whose fields are all Copy is `#[derive(Copy)]` and passed by value, as
//!    before; everything else is tracked.
//! 2. Last uses ([`liveness`]): a use of a local after which the local is dead. Only there can a value the function
//!    owns be moved (into a result, a field, an owned parameter, `push`, `into_iter`); elsewhere it is cloned. The
//!    analysis runs backward over the structured body (loops to a fixed point), on bindings rather than names, so
//!    shadowing is exact. It is conservative where Rust's borrows would otherwise meet a move:
//!    - a use inside a closure is never a last use (the closure runs many times, and captures by reference);
//!    - a local used by two operands of one call / method / record / array / operator is moved by neither (one of
//!      them may borrow it while the other moves it: `f(&xs, g(xs))`);
//!    - in the arms of a `match` and the body of a `for` that iterate a local by reference, that local is frozen (no
//!      last use), and if they assign it, the scrutinee / iterable is cloned first ([`Live::clone_root`]).
//! 3. Parameter modes and borrowed results: a fixed point in lower.rs over the call graph (see there).
//!
//! The Rust is then checked again by zustc's borrow checker, so a mistake here is a compile error, not unsafety.

use crate::ast::*;
use crate::check::{Info, Ty};
use std::collections::{HashMap, HashSet};

/// Is a value of type `t` Copy in the lowered Rust (scalars, and records / enums / tuples of them)
pub fn is_copy(t: &Ty, info: &Info) -> bool {
    copy_rec(t, info, &mut vec![])
}

fn copy_rec(t: &Ty, info: &Info, seen: &mut Vec<String>) -> bool {
    match t {
        Ty::Array(_) | Ty::Typed(_) | Ty::Str | Ty::Jsx | Ty::Fn(..) => false,
        Ty::Tuple(ts) => ts.iter().all(|t| copy_rec(t, info, seen)),
        Ty::Named(n, targs) => {
            if seen.contains(n) {
                return true;
            }
            let (params, fields): (&[String], Vec<&Ty>) = if let Some(r) = info.records.get(n) {
                (&r.params, r.fields.iter().map(|f| &f.1).collect())
            } else if let Some(e) = info.enums.get(n) {
                (&e.params, e.variants.iter().flat_map(|v| v.1.iter()).collect())
            } else if n == "Map" || n == "Set" {
                // (the built-in collections: they own their entries)
                return false;
            } else {
                return true;
            };
            let m: HashMap<&str, &Ty> = params.iter().map(|p| p.as_str()).zip(targs.iter()).collect();
            seen.push(n.clone());
            let ok = fields.iter().all(|f| match f {
                Ty::Param(p) => m.get(p.as_str()).is_none_or(|t| copy_rec(t, info, seen)),
                f => copy_rec(&subst(f, &m), info, seen),
            });
            seen.pop();
            ok
        }
        _ => true,
    }
}

fn subst(t: &Ty, m: &HashMap<&str, &Ty>) -> Ty {
    match t {
        Ty::Param(p) => m.get(p.as_str()).map_or(t.clone(), |t| (*t).clone()),
        Ty::Array(a) => Ty::Array(Box::new(subst(a, m))),
        Ty::Typed(a) => Ty::Typed(Box::new(subst(a, m))),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| subst(t, m)).collect()),
        Ty::Named(n, a) => Ty::Named(n.clone(), a.iter().map(|t| subst(t, m)).collect()),
        Ty::Fn(a, r) => Ty::Fn(a.iter().map(|t| subst(t, m)).collect(), Box::new(subst(r, m))),
        t => t.clone(),
    }
}

/// The facts of one function body (see the module comment)
#[derive(Default, Debug)]
pub struct Live {
    /// uses of a local (`EK::Var` ids) after which it is dead
    pub last: HashSet<Id>,
    /// `match` expressions whose scrutinee's root local is dead after the match and unused in its arms: the
    /// patterns can bind by move
    pub match_move: HashSet<Id>,
    /// `match` / `for` expressions whose arms / body assign the root local of the scrutinee / iterable: it is
    /// cloned before, so the loop does not borrow what it reassigns
    pub clone_root: HashSet<Id>,
    /// the expressions that contain an assignment (most do not: a check that a later operand assigns what an
    /// earlier one borrows can skip them)
    pub assigns_in: HashSet<Id>,
}

/// The key of a binding site: (the id of the expression that scopes it, the name)
type Key = (Id, String);
/// (a key for the function's parameters)
const PARAM: Id = Id::MAX;

/// Computes [`Live`] for a body whose parameters are `params`; `copy(id)`: expression `id` is of a Copy type (a
/// `match` on a Copy value borrows nothing)
pub fn liveness(params: &[String], body: &Expr, copy: &dyn Fn(Id) -> bool) -> Live {
    let mut r = Resolve { scopes: vec![HashMap::new()], binds: HashMap::new(), uses: HashMap::new(), depth: vec![], closures: 0 };
    for p in params {
        r.bind((PARAM, p.clone()));
    }
    r.expr(body);
    let mut lv = Lv { r: &r, copy, out: Live::default(), record: true, frozen: vec![], closure: 0, loops: vec![], memo: Default::default() };
    let mut live = HashSet::new();
    lv.expr(body, &mut live);
    fn mark(e: &Expr, out: &mut HashSet<Id>) -> bool {
        let mut any = matches!(e.kind, EK::Assign(..));
        for c in children(e) {
            any |= mark(c, out);
        }
        if any {
            out.insert(e.id);
        }
        any
    }
    mark(body, &mut lv.out.assigns_in);
    lv.out
}

/// Resolves each use (`EK::Var` id, and the `EK::Assign` id for its target) to a binding number
struct Resolve {
    scopes: Vec<HashMap<String, usize>>,
    binds: HashMap<Key, usize>,
    uses: HashMap<Id, usize>,
    /// how many closures enclose each binding
    depth: Vec<usize>,
    closures: usize,
}

impl Resolve {
    fn bind(&mut self, k: Key) {
        let b = self.depth.len();
        self.depth.push(self.closures);
        self.scopes.last_mut().unwrap().insert(k.1.clone(), b);
        self.binds.insert(k, b);
    }

    fn lookup(&self, n: &str) -> Option<usize> {
        self.scopes.iter().rev().find_map(|s| s.get(n).copied())
    }

    fn pat(&mut self, p: &Pat, scope: Id) {
        for n in pat_names(p) {
            self.bind((scope, n));
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            EK::Var(n) => {
                if let Some(b) = self.lookup(n) {
                    self.uses.insert(e.id, b);
                }
            }
            EK::Assign(n, x) => {
                self.expr(x);
                if let Some(b) = self.lookup(n) {
                    self.uses.insert(e.id, b);
                }
            }
            EK::Block(stmts, tail) => {
                self.scopes.push(HashMap::new());
                for st in stmts {
                    match st {
                        Stmt::Let(p, _, x) => {
                            self.expr(x);
                            self.pat(p, x.id);
                        }
                        Stmt::LetMut(n, _, x) => {
                            self.expr(x);
                            self.bind((x.id, n.clone()));
                        }
                        Stmt::Expr(x) => self.expr(x),
                    }
                }
                if let Some(t) = tail {
                    self.expr(t);
                }
                self.scopes.pop();
            }
            EK::Match(sc, arms) => {
                self.expr(sc);
                for a in arms {
                    self.scopes.push(HashMap::new());
                    self.pat(&a.pat, a.body.id);
                    if let Some(g) = &a.guard {
                        self.expr(g);
                    }
                    self.expr(&a.body);
                    self.scopes.pop();
                }
            }
            EK::For(x, it, body) => {
                self.expr(it);
                self.scopes.push(HashMap::new());
                self.bind((e.id, x.clone()));
                self.expr(body);
                self.scopes.pop();
            }
            EK::Closure(ps, body) => {
                self.closures += 1;
                self.scopes.push(HashMap::new());
                for (p, _) in ps {
                    self.bind((e.id, p.clone()));
                }
                self.expr(body);
                self.scopes.pop();
                self.closures -= 1;
            }
            // (a handler arm is a function the operation calls: its own bindings are one level deeper, as a closure's)
            EK::Handle(body, arms) => {
                self.expr(body);
                for a in arms {
                    self.closures += 1;
                    self.scopes.push(HashMap::new());
                    for p in &a.pats {
                        self.pat(p, a.body.id);
                    }
                    self.bind((a.body.id, a.k.clone()));
                    self.expr(&a.body);
                    self.scopes.pop();
                    self.closures -= 1;
                }
            }
            _ => {
                for c in children(e) {
                    self.expr(c);
                }
            }
        }
    }
}

/// The local names an expression reads and the ones it assigns (by name: conservative across shadowing)
pub fn names(e: &Expr, reads: &mut HashSet<String>, assigns: &mut HashSet<String>) {
    match &e.kind {
        EK::Var(n) => {
            reads.insert(n.clone());
        }
        EK::Assign(n, x) => {
            assigns.insert(n.clone());
            names(x, reads, assigns);
        }
        _ => {
            for c in children(e) {
                names(c, reads, assigns);
            }
        }
    }
}

fn jsx_children(j: &Jsx) -> Vec<&Expr> {
    j.attrs
        .iter()
        .map(|a| &a.1)
        .chain(j.children.iter().filter_map(|c| match c {
            JsxChild::Expr(e) => Some(e),
            JsxChild::Text(_) => None,
        }))
        .collect()
}

/// The names a pattern binds
pub fn pat_names(p: &Pat) -> Vec<String> {
    let mut out = vec![];
    fn go(p: &Pat, out: &mut Vec<String>) {
        match p {
            Pat::Bind(n) => out.push(n.clone()),
            Pat::Tuple(ps) | Pat::Variant(_, _, ps, _) => ps.iter().for_each(|p| go(p, out)),
            Pat::Record(_, fs, _, _) => fs.iter().for_each(|f| go(&f.1, out)),
            // (each alternative binds the same names)
            Pat::Or(alts) => alts.iter().take(1).for_each(|p| go(p, out)),
            Pat::Wild | Pat::Lit(_) => {}
        }
    }
    go(p, &mut out);
    out
}

/// The operands of an expression, in evaluation order (for the kinds without bindings or control flow)
fn children(e: &Expr) -> Vec<&Expr> {
    match &e.kind {
        EK::Call(f, args) => std::iter::once(&**f).chain(args).collect(),
        EK::Method(r, _, args) => std::iter::once(&**r).chain(args).collect(),
        EK::Field(x, _) | EK::Un(_, x) | EK::Cast(x, _) | EK::Await(x) | EK::Async(x) | EK::Try(x) => vec![x],
        EK::Bin(_, l, r) | EK::Index(l, r) => vec![l, r],
        EK::Record(_, fs, base) => fs.iter().map(|f| &f.1).chain(base.as_deref()).collect(),
        EK::Array(xs) | EK::Tuple(xs) | EK::Perform(_, _, xs) | EK::Interp(_, xs) => xs.iter().collect(),
        EK::If(c, t, el) => std::iter::once(&**c).chain(std::iter::once(&**t)).chain(el.as_deref()).collect(),
        EK::While(c, b) => vec![c, b],
        EK::Loop(b) => vec![b],
        EK::Return(x) => x.as_deref().into_iter().collect(),
        EK::Assign(_, x) => vec![x],
        EK::Match(sc, arms) => std::iter::once(&**sc).chain(arms.iter().flat_map(|a| a.guard.iter().chain(std::iter::once(&a.body)))).collect(),
        EK::For(_, it, b) => vec![it, b],
        EK::Closure(_, b) => vec![b],
        EK::Block(stmts, tail) => stmts
            .iter()
            .map(|s| match s {
                Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) | Stmt::Expr(x) => x,
            })
            .chain(tail.as_deref())
            .collect(),
        EK::Handle(b, arms) => std::iter::once(&**b).chain(arms.iter().map(|a| &a.body)).collect(),
        // (a JSX element's attribute values and expression children are evaluated as well)
        EK::Jsx(j) => jsx_children(j),
        EK::Lit(_) | EK::Var(_) | EK::Path(..) | EK::Break | EK::Continue => vec![],
    }
}

struct Lv<'a> {
    r: &'a Resolve,
    copy: &'a dyn Fn(Id) -> bool,
    out: Live,
    /// false while a loop iterates to its fixed point (only the final pass records)
    record: bool,
    frozen: Vec<usize>,
    closure: usize,
    /// (live at the loop's head, live after the loop)
    loops: Vec<(HashSet<usize>, HashSet<usize>)>,
    /// [`Lv::used`] of each expression, computed once (it is asked for at every operator of a nested chain)
    memo: std::cell::RefCell<HashMap<Id, std::rc::Rc<(HashSet<usize>, HashSet<usize>)>>>,
}

impl Lv<'_> {
    /// The bindings used (read or assigned) in `e`: (reads, assigns)
    fn used(&self, e: &Expr, reads: &mut HashSet<usize>, assigns: &mut HashSet<usize>) {
        let s = self.sets(e);
        reads.extend(s.0.iter().copied());
        assigns.extend(s.1.iter().copied());
    }

    fn sets(&self, e: &Expr) -> std::rc::Rc<(HashSet<usize>, HashSet<usize>)> {
        if let Some(s) = self.memo.borrow().get(&e.id) {
            return s.clone();
        }
        let (mut reads, mut assigns) = (HashSet::new(), HashSet::new());
        match &e.kind {
            EK::Var(_) => {
                if let Some(b) = self.r.uses.get(&e.id) {
                    reads.insert(*b);
                }
            }
            EK::Assign(_, x) => {
                if let Some(b) = self.r.uses.get(&e.id) {
                    assigns.insert(*b);
                }
                self.used(x, &mut reads, &mut assigns);
            }
            _ => {
                for c in children(e) {
                    self.used(c, &mut reads, &mut assigns);
                }
            }
        }
        let s = std::rc::Rc::new((reads, assigns));
        self.memo.borrow_mut().insert(e.id, s.clone());
        s
    }

    fn binding_of(&self, e: &Expr) -> Option<usize> {
        let mut x = e;
        loop {
            match &x.kind {
                EK::Var(_) => return self.r.uses.get(&x.id).copied(),
                EK::Field(y, _) | EK::Index(y, _) => x = y,
                _ => return None,
            }
        }
    }

    /// Operands evaluated left to right (`live` is what is live after the last one). A local read by two of them is
    /// frozen in all of them
    fn operands(&mut self, xs: &[&Expr], live: &mut HashSet<usize>) {
        let mut shared = vec![];
        if xs.len() > 1 {
            let mut seen: HashMap<usize, usize> = HashMap::new();
            for x in xs {
                let mut r = HashSet::new();
                self.used(x, &mut r, &mut HashSet::new());
                for b in r {
                    *seen.entry(b).or_default() += 1;
                }
            }
            shared = seen.into_iter().filter(|(_, n)| *n > 1).map(|(b, _)| b).collect();
        }
        let n = self.frozen.len();
        self.frozen.extend(shared);
        for x in xs.iter().rev() {
            self.expr(x, live);
        }
        self.frozen.truncate(n);
    }

    fn kill_pat(&self, p: &Pat, scope: Id, live: &mut HashSet<usize>) {
        for n in pat_names(p) {
            if let Some(b) = self.r.binds.get(&(scope, n)) {
                live.remove(b);
            }
        }
    }

    /// `live`: on entry what is live after `e`, on return what is live before it
    fn expr(&mut self, e: &Expr, live: &mut HashSet<usize>) {
        match &e.kind {
            EK::Var(_) => {
                if let Some(&b) = self.r.uses.get(&e.id) {
                    // (a local captured from outside a closure is never moved inside it; the closure's own are)
                    if self.record && self.r.depth[b] == self.closure && !live.contains(&b) && !self.frozen.contains(&b) {
                        self.out.last.insert(e.id);
                    }
                    live.insert(b);
                }
            }
            EK::Assign(_, x) => {
                if let Some(b) = self.r.uses.get(&e.id) {
                    live.remove(b);
                }
                self.expr(x, live);
            }
            // (nothing after it runs: only the value is live)
            EK::Return(x) => {
                live.clear();
                if let Some(x) = x {
                    self.expr(x, live);
                }
            }
            EK::Loop(body) => {
                // (left only by a `break`, which finds `after` live)
                let after = live.clone();
                let head = self.fixpoint(&after, |lv, h| {
                    let mut l = h.clone();
                    lv.expr(body, &mut l);
                    l
                });
                self.loops.push((head.clone(), after));
                let mut l = head;
                self.expr(body, &mut l);
                self.loops.pop();
                *live = l;
            }
            EK::Break => {
                if let Some((_, after)) = self.loops.last() {
                    *live = after.clone();
                }
            }
            EK::Continue => {
                if let Some((head, _)) = self.loops.last() {
                    *live = head.clone();
                }
            }
            EK::If(c, t, el) => {
                let mut other = live.clone();
                self.expr(t, live);
                if let Some(el) = el {
                    self.expr(el, &mut other);
                }
                live.extend(other);
                self.expr(c, live);
            }
            EK::Match(sc, arms) => {
                let after = live.clone();
                let root = self.binding_of(sc);
                let (mut reads, mut assigns) = (HashSet::new(), HashSet::new());
                for a in arms {
                    if let Some(g) = &a.guard {
                        self.used(g, &mut reads, &mut assigns);
                    }
                    self.used(&a.body, &mut reads, &mut assigns);
                }
                // (the locals the scrutinee reads: it may borrow from any of them, also through a call that returns
                // a borrow, `match pick(t) { .. }`)
                let mut sc_reads = HashSet::new();
                self.used(sc, &mut sc_reads, &mut HashSet::new());
                let mut frozen = vec![];
                if (self.copy)(sc.id) {
                    // (a Copy value: the patterns copy, nothing is borrowed)
                } else if sc_reads.iter().any(|b| assigns.contains(b)) {
                    // (checked first: a scrutinee that cannot be moved, `xs[0]`, would otherwise be borrowed across the
                    // assignment; lowering takes a value of it, moved when it can be)
                    if self.record {
                        self.out.clone_root.insert(e.id);
                    }
                } else if let Some(r) = root
                    && !after.contains(&r)
                    && !reads.contains(&r)
                {
                    if self.record {
                        self.out.match_move.insert(e.id);
                    }
                } else {
                    frozen.extend(sc_reads);
                }
                let n = self.frozen.len();
                self.frozen.extend(frozen);
                // (last arm first: a guard that fails goes on to the arms after it, their guards included, so what
                // is live after it is its body's and theirs)
                let mut acc: HashSet<usize> = HashSet::new();
                for a in arms.iter().rev() {
                    let mut l = after.clone();
                    self.expr(&a.body, &mut l);
                    if let Some(g) = &a.guard {
                        l.extend(acc.iter().copied());
                        self.expr(g, &mut l);
                    }
                    self.kill_pat(&a.pat, a.body.id, &mut l);
                    acc.extend(l);
                }
                self.frozen.truncate(n);
                *live = acc;
                self.expr(sc, live);
            }
            EK::Block(stmts, tail) => {
                if let Some(t) = tail {
                    self.expr(t, live);
                }
                for st in stmts.iter().rev() {
                    match st {
                        Stmt::Let(p, _, x) => {
                            self.kill_pat(p, x.id, live);
                            self.expr(x, live);
                        }
                        Stmt::LetMut(n, _, x) => {
                            if let Some(b) = self.r.binds.get(&(x.id, n.clone())) {
                                live.remove(b);
                            }
                            self.expr(x, live);
                        }
                        Stmt::Expr(x) => self.expr(x, live),
                    }
                }
            }
            EK::While(c, body) => {
                let after = live.clone();
                let head = self.fixpoint(&after, |lv, h| {
                    let mut l = h.clone();
                    lv.expr(body, &mut l);
                    l.extend(after.iter().copied());
                    lv.expr(c, &mut l);
                    l
                });
                // (the final pass, recording)
                self.loops.push((head.clone(), after.clone()));
                let mut l = head;
                self.expr(body, &mut l);
                l.extend(after.iter().copied());
                self.expr(c, &mut l);
                self.loops.pop();
                *live = l;
            }
            EK::For(x, it, body) => {
                let after = live.clone();
                let xb = self.r.binds.get(&(e.id, x.clone())).copied();
                let head = self.fixpoint(&after, |lv, h| {
                    let mut l = h.clone();
                    lv.expr(body, &mut l);
                    if let Some(xb) = xb {
                        l.remove(&xb);
                    }
                    l.extend(after.iter().copied());
                    l
                });
                // (the iterable may borrow from any local it reads, also through a call that returns a borrow. If
                // the body assigns one of them, the loop iterates a copy (or takes the array, if it is dead);
                // otherwise they are frozen in the body)
                let mut it_reads = HashSet::new();
                self.used(it, &mut it_reads, &mut HashSet::new());
                let (mut reads, mut assigns) = (HashSet::new(), HashSet::new());
                self.used(body, &mut reads, &mut assigns);
                let mut frozen = vec![];
                if it_reads.iter().any(|b| assigns.contains(b)) {
                    if self.record {
                        self.out.clone_root.insert(e.id);
                    }
                } else {
                    frozen.extend(it_reads);
                }
                self.loops.push((head.clone(), after.clone()));
                let n = self.frozen.len();
                self.frozen.extend(frozen);
                let mut l = head.clone();
                self.expr(body, &mut l);
                self.frozen.truncate(n);
                self.loops.pop();
                let mut l = head;
                self.expr(it, &mut l);
                *live = l;
            }
            EK::Closure(_, body) => {
                // (its uses happen while the call that takes it runs: they are live at the closure, never last)
                self.closure += 1;
                let mut l = live.clone();
                self.expr(body, &mut l);
                self.closure -= 1;
                live.extend(l);
            }
            // (the callee too: it may be an expression, `(if c { f } else { g })(x)`)
            EK::Call(f, args) => self.operands(&std::iter::once(&**f).chain(args).collect::<Vec<_>>(), live),
            // (the emitted JS evaluates a JSX element's parts in its own order (`key` after the props): as operands, a
            // local read by two of them is moved by none, whatever the order)
            EK::Jsx(j) => self.operands(&jsx_children(j), live),
            // (a handler's arms run once per operation, maybe many times: like a closure, a local from outside is
            // never moved in them; the body runs while they can)
            EK::Handle(body, arms) => {
                // (the arms are alternatives: each from what is live after the handle, then joined)
                self.closure += 1;
                let mut acc = live.clone();
                for a in arms {
                    let mut l = live.clone();
                    self.expr(&a.body, &mut l);
                    acc.extend(l);
                }
                self.closure -= 1;
                *live = acc;
                self.expr(body, live);
            }
            EK::Method(r, _, args) => self.operands(&std::iter::once(&**r).chain(args).collect::<Vec<_>>(), live),
            EK::Bin(op, a, b) if *op == "&&" || *op == "||" => {
                // (the right operand may not run: what is live before it joins what is live after the operator)
                let mut shared = HashSet::new();
                let (mut ra, mut rb) = (HashSet::new(), HashSet::new());
                self.used(a, &mut ra, &mut HashSet::new());
                self.used(b, &mut rb, &mut HashSet::new());
                shared.extend(ra.intersection(&rb).copied());
                let n = self.frozen.len();
                self.frozen.extend(shared);
                let mut l = live.clone();
                self.expr(b, &mut l);
                live.extend(l);
                self.expr(a, live);
                self.frozen.truncate(n);
            }
            EK::Bin(_, a, b) | EK::Index(a, b) => self.operands(&[a, b], live),
            EK::Record(_, fs, base) => self.operands(&fs.iter().map(|f| &f.1).chain(base.as_deref()).collect::<Vec<_>>(), live),
            EK::Array(xs) | EK::Tuple(xs) | EK::Interp(_, xs) => self.operands(&xs.iter().collect::<Vec<_>>(), live),
            _ => {
                for c in children(e).into_iter().rev() {
                    self.expr(c, live);
                }
            }
        }
    }

    /// Iterates `step` (what is live at the loop's head, from a guess of it) from `after` to its fixed point, without
    /// recording
    fn fixpoint(&mut self, after: &HashSet<usize>, step: impl Fn(&mut Self, &HashSet<usize>) -> HashSet<usize>) -> HashSet<usize> {
        let saved = self.record;
        self.record = false;
        let mut h = after.clone();
        loop {
            self.loops.push((h.clone(), after.clone()));
            let n = step(self, &h);
            self.loops.pop();
            if n == h {
                break;
            }
            h = n;
        }
        self.record = saved;
        h
    }
}

/// The JS layer's `push` / `concat` calls that can append in place (`(acc.push(x), acc)` instead of
/// `[...acc, x]`): the receiver is a `let mut` local whose array no one else can see, at its last use.
///
/// JS arrays are shared by reference, so "no one else can see it" is the whole question. A `let mut` local
/// qualifies when
/// - every value it is given is a new array: a literal, `range(..)`, or the result of `map` / `filter` / `push` /
///   `concat` (not a parameter or a call's result, which someone else may hold);
/// - every use reads it without keeping it (`len`, an index, a `for`, the receiver of `map` / `filter` / `fold` /
///   `any` / `all` / `push` / `concat`, an operator), or is its last use (then it may go anywhere: it is dead here).
///
/// A mutable local cannot be captured by a closure, so no closure can see it either.
/// `m = m.insert(k, f(m.get(k)))`: the old collection is dead after the change (`m` is assigned its result), and
/// the arguments use `m` only as the receiver of a Map's / Set's reading method (`get`, `contains_key`, `len`, ..).
/// They are all evaluated before the change and keep nothing of the collection, so it can be changed in place (no
/// closure, block or `match` in them: a name there could be another local)
pub fn assigned_change<'e>(e: &'e Expr, info: &Info) -> Option<&'e Expr> {
    let EK::Assign(n, x) = &e.kind else { return None };
    let EK::Method(r, _, args) = &x.kind else { return None };
    (coll_change(info, x.id) && matches!(&r.kind, EK::Var(m) if m == n) && reads_only(n, args, info)).then_some(x)
}

fn reads_only(name: &str, args: &[Expr], info: &Info) -> bool {
    use crate::check::{Builtin as B, On};
    fn walk(e: &Expr, name: &str, info: &Info, uses: &mut Vec<(Id, bool)>) -> bool {
        match &e.kind {
            EK::Closure(..) | EK::Async(_) | EK::Block(..) | EK::Match(..) | EK::Handle(..) => false,
            EK::Var(n) if n == name => {
                uses.push((e.id, false));
                true
            }
            EK::Method(r, _, a)
                if matches!(&r.kind, EK::Var(n) if n == name)
                    && matches!(info.builtin.get(&e.id), Some(B::Std(On::Map | On::Set, "get" | "contains_key" | "contains" | "len" | "is_empty"))) =>
            {
                uses.push((r.id, true));
                a.iter().all(|x| walk(x, name, info, uses))
            }
            _ => children(e).into_iter().all(|c| walk(c, name, info, uses)),
        }
    }
    let mut uses = vec![];
    args.iter().all(|a| walk(a, name, info, &mut uses)) && uses.iter().all(|u| u.1)
}

/// A Map's / Set's `insert` or `remove`: a new collection, made in place when no one else sees the old one
fn coll_change(info: &Info, id: Id) -> bool {
    use crate::check::{Builtin as B, On};
    matches!(info.builtin.get(&id), Some(B::Std(On::Map | On::Set, "insert" | "remove")))
}

pub fn inplace_pushes(params: &[String], body: &Expr, info: &Info) -> HashSet<Id> {
    use crate::check::{Builtin as B, Callee};
    let live = liveness(params, body, &|_| false);
    let mut r = Resolve { scopes: vec![HashMap::new()], binds: HashMap::new(), uses: HashMap::new(), depth: vec![], closures: 0 };
    for p in params {
        r.bind((PARAM, p.clone()));
    }
    r.expr(body);
    // the `let mut` bindings, and the values each is given
    let mut values: HashMap<usize, Vec<&Expr>> = HashMap::new();
    fn lets<'e>(e: &'e Expr, r: &Resolve, out: &mut HashMap<usize, Vec<&'e Expr>>) {
        match &e.kind {
            EK::Block(stmts, _) => {
                for st in stmts {
                    if let Stmt::LetMut(n, _, x) = st
                        && let Some(b) = r.binds.get(&(x.id, n.clone()))
                    {
                        out.entry(*b).or_default().push(x);
                    }
                }
            }
            EK::Assign(_, x) => {
                if let Some(b) = r.uses.get(&e.id) {
                    out.entry(*b).or_default().push(x);
                }
            }
            _ => {}
        }
        for c in children(e) {
            lets(c, r, out);
        }
    }
    lets(body, &r, &mut values);
    let fresh = |x: &Expr| match &x.kind {
        EK::Array(_) => !info.typed_lit.contains_key(&x.id),
        EK::Method(..) => matches!(info.builtin.get(&x.id), Some(B::Map | B::Filter | B::Push | B::Concat)) || coll_change(info, x.id),
        EK::Call(f, _) => matches!(info.callee.get(&f.id), Some(Callee::Range | Callee::Coll(..))),
        _ => false,
    };
    let mut ok: HashSet<usize> = values.iter().filter(|(_, vs)| vs.iter().all(|x| fresh(x))).map(|(b, _)| *b).collect();
    // every use: read without keeping, or the last
    fn uses(e: &Expr, keeps: bool, r: &Resolve, info: &Info, live: &Live, ok: &mut HashSet<usize>) {
        use crate::check::Builtin as B;
        if let EK::Var(_) = &e.kind {
            if keeps
                && let Some(b) = r.uses.get(&e.id)
                && !live.last.contains(&e.id)
            {
                ok.remove(b);
            }
            return;
        }
        match &e.kind {
            EK::Method(recv, _, args) => {
                // (a Map's / Set's methods give a new value or a part: none keeps the collection)
                let reads = matches!(info.builtin.get(&e.id), Some(B::Len | B::Map | B::Filter | B::Fold | B::Any | B::All | B::Push | B::Concat | B::Join | B::Std(crate::check::On::Map | crate::check::On::Set, _)));
                uses(recv, !reads, r, info, live, ok);
                for a in args {
                    uses(a, true, r, info, live, ok);
                }
            }
            EK::Index(x, i) => {
                uses(x, false, r, info, live, ok);
                uses(i, true, r, info, live, ok);
            }
            EK::For(_, it, b) => {
                // (JS's `for .. of` walks the array object itself: a local the iterable reads and the body assigns
                // must not be appended to in place, or the loop would see the new elements)
                let (mut reads, mut assigns) = (HashSet::new(), HashSet::new());
                bindings(it, r, &mut reads, &mut HashSet::new());
                bindings(b, r, &mut HashSet::new(), &mut assigns);
                for x in reads.intersection(&assigns) {
                    ok.remove(x);
                }
                uses(it, false, r, info, live, ok);
                uses(b, true, r, info, live, ok);
            }
            EK::Bin(_, a, b) => {
                // (the left operand's value is held while the right one runs: `xs == xs.push(2)` must not append
                // to the array it compares)
                uses(a, true, r, info, live, ok);
                uses(b, false, r, info, live, ok);
            }
            // (a block's statements are not its value: an expression statement keeps nothing)
            EK::Block(stmts, tail) => {
                for st in stmts {
                    match st {
                        Stmt::Expr(x) => uses(x, false, r, info, live, ok),
                        Stmt::Let(_, _, x) | Stmt::LetMut(_, _, x) => uses(x, true, r, info, live, ok),
                    }
                }
                if let Some(t) = tail {
                    uses(t, keeps, r, info, live, ok);
                }
            }
            EK::If(c, t, el) => {
                uses(c, false, r, info, live, ok);
                uses(t, keeps, r, info, live, ok);
                if let Some(el) = el {
                    uses(el, keeps, r, info, live, ok);
                }
            }
            _ => {
                for c in children(e) {
                    uses(c, true, r, info, live, ok);
                }
            }
        }
    }
    fn bindings(e: &Expr, r: &Resolve, reads: &mut HashSet<usize>, assigns: &mut HashSet<usize>) {
        match &e.kind {
            EK::Var(_) => reads.extend(r.uses.get(&e.id)),
            EK::Assign(..) => assigns.extend(r.uses.get(&e.id)),
            _ => {}
        }
        for c in children(e) {
            bindings(c, r, reads, assigns);
        }
    }
    uses(body, true, &r, info, &live, &mut ok);
    // the pushes and concats onto such a local, at its last use
    let mut out = HashSet::new();
    fn pushes(e: &Expr, r: &Resolve, info: &Info, live: &Live, ok: &HashSet<usize>, out: &mut HashSet<Id>) {
        use crate::check::Builtin as B;
        if let EK::Method(recv, _, args) = &e.kind
            && (matches!(info.builtin.get(&e.id), Some(B::Push | B::Concat)) || coll_change(info, e.id))
            && let EK::Var(_) = &recv.kind
            && let Some(b) = r.uses.get(&recv.id)
            && ok.contains(b)
            && live.last.contains(&recv.id)
        {
            // (an argument that assigns the receiver, `a.push({ a = [7]; 2 })`, would make the appended array and
            // the one the expression yields differ)
            let mut assigns = HashSet::new();
            for a in args {
                bindings(a, r, &mut HashSet::new(), &mut assigns);
            }
            if !assigns.contains(b) {
                out.insert(e.id);
            }
        }
        // (`m = m.insert(k, m.get(k) ..)`: the receiver is read by the arguments too, so it is not its last use; but
        // the old value is dead once the result is assigned)
        if let Some(x) = assigned_change(e, info)
            && let EK::Method(recv, ..) = &x.kind
            && r.uses.get(&recv.id).is_some_and(|b| ok.contains(b))
        {
            out.insert(x.id);
        }
        for c in children(e) {
            pushes(c, r, info, live, ok, out);
        }
    }
    pushes(body, &r, info, &live, &ok, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live_of(src: &str) -> (Live, Vec<Item>) {
        let items = crate::parse::Parser::new(src).file().unwrap();
        let Item::Fn { args, body, .. } = &items[0] else { panic!() };
        let ps: Vec<String> = args.iter().map(|a| a.0.clone()).collect();
        (liveness(&ps, body, &|_| false), items.clone())
    }

    /// The ids of the uses of `name`, in source order
    fn uses(items: &[Item], name: &str) -> Vec<Id> {
        let Item::Fn { body, .. } = &items[0] else { panic!() };
        let mut out = vec![];
        fn go(e: &Expr, name: &str, out: &mut Vec<(Pos, Id)>) {
            if let EK::Var(n) = &e.kind
                && n == name
            {
                out.push((e.pos, e.id));
            }
            for c in children(e) {
                go(c, name, out);
            }
        }
        let mut v = vec![];
        go(body, name, &mut v);
        v.sort_by_key(|(p, _)| (p.line, p.col));
        out.extend(v.into_iter().map(|x| x.1));
        out
    }

    fn lasts(src: &str, name: &str) -> Vec<bool> {
        let (l, items) = live_of(src);
        uses(&items, name).iter().map(|id| l.last.contains(id)).collect()
    }

    #[test]
    fn straight_line() {
        assert_eq!(lasts("fn f(xs: Array<f64>) -> Array<f64> { let n = xs.len(); g(xs) }", "xs"), [false, true]);
        // (two operands of one call: neither moves)
        assert_eq!(lasts("fn f(xs: Array<f64>) -> f64 { h(xs, g(xs)) }", "xs"), [false, false]);
        // (`&&`'s right operand may not run: the reassignment there does not make the left use the last)
        let src = "fn f(n: i32) -> Array<f64> { let mut xs = [1.0]; let ok = g(xs) && { xs = [2.0]; true }; xs }";
        assert_eq!(lasts(src, "xs"), [false, true]);
    }

    #[test]
    fn loops_and_reassignment() {
        // `acc = acc.push(x)`: the old acc is dead after the assignment, so the push can reuse it
        let src = "fn f(xs: Array<i32>) -> Array<i32> { let mut acc = []; for x in xs { acc = acc.push(x); } acc }";
        assert_eq!(lasts(src, "acc"), [true, true]);
        // read on the next iteration: not a last use
        let src = "fn f(n: i32) -> Array<i32> { let mut acc = []; let mut i = 0; while i < n { g(acc); i += 1; } acc }";
        assert_eq!(lasts(src, "acc"), [false, true]);
    }

    #[test]
    fn shadowing_is_by_binding() {
        let src = "fn f(v: Array<i32>) -> Array<i32> { let a = g(v); let w = { let v = 1; v }; h(v) }";
        // (the inner `v` is another binding: its only use is its last; the outer one is still live for `h`)
        assert_eq!(lasts(src, "v"), [false, true, true]);
    }

    #[test]
    fn closures_never_move() {
        assert_eq!(lasts("fn f(xs: Array<i32>, ys: Array<i32>) -> Array<i32> { xs.map((x) => g(ys)) }", "ys"), [false]);
        // ... but a closure's own parameters are: the fold can push into its accumulator in place
        assert_eq!(lasts("fn f(xs: Array<i32>) -> Array<i32> { xs.fold([], (acc, x) => acc.push(x)) }", "acc"), [true]);
    }

    #[test]
    fn branches() {
        let src = "fn f(c: bool, xs: Array<i32>) -> Array<i32> { if c { g(xs) } else { h(xs) } }";
        assert_eq!(lasts(src, "xs"), [true, true]);
    }

    #[test]
    fn match_and_for_roots() {
        let (l, items) = live_of("fn f(s: Shape) -> i32 { match s { A(xs) => g(xs), B => 0 } }");
        let Item::Fn { body, .. } = &items[0] else { panic!() };
        let EK::Block(_, Some(t)) = &body.kind else { panic!() };
        assert!(l.match_move.contains(&t.id));
        // the iterable is assigned in the body: iterate a clone
        let (l, items) = live_of("fn f(xs: Array<i32>) -> Array<i32> { let mut v = xs; for x in v { v = v.push(x); } v }");
        let Item::Fn { body, .. } = &items[0] else { panic!() };
        let EK::Block(st, _) = &body.kind else { panic!() };
        let Stmt::Expr(fe) = &st[1] else { panic!() };
        assert!(l.clone_root.contains(&fe.id));
        // ... so the push inside the loop can reuse v (the loop does not borrow it), while the iterable is not moved
        assert_eq!(lasts("fn f(xs: Array<i32>) -> Array<i32> { let mut v = xs; for x in v { v = v.push(x); } v }", "v"), [false, true, true]);
    }
}
