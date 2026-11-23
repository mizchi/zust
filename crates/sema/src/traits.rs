//! The trait solver for the program's own traits: whether a goal `T: Tr<A..>` holds by the program's impls.
//!
//! It is the algorithm `solve` of formal/lean/TraitSolver/Solver.lean, over zustc's types:
//!
//! - an impl is a clause: its head (self type and trait arguments, over the impl's parameters) and its body (the
//!   bounds of its parameters, where clauses on them included);
//! - a goal is proved depth first by the impls whose heads match it (one-way matching: `mtch`, exact as the
//!   model's: `&mut T` is not `&T`), with the goals being proved on a stack: a goal that comes back round is `No`
//!   for that candidate (an inductive cycle: `impl<T: B> A for T` with `impl<T: A> B for T` proves neither);
//! - it answers `Yes`, `No` or `Overflow` (`Ans`). The model proves `No` only when the goal does not hold
//!   (`solve_no_sound`), and `Yes` only when it does. `Overflow` (out of fuel, a goal that is not ground, a where
//!   clause naming a parameter the head does not bind) is "cannot tell": an error is reported on `No` only.
//!
//! What zustc cannot read of an impl is weakened, never guessed: a where clause it cannot parse (an associated
//! type's bound, a bound on a concrete type) is left out of the body, a self type or trait argument it cannot resolve
//! matches anything. A weaker program proves more goals, so a `No` of it is a `No` of the real program, and the
//! model's theorem still covers what is reported. The traits that are not the program's (std's) are asked of
//! `Program::implements`, and believed only where it judges exactly.

use crate::fxhash::FxMap;
use crate::program::Program;
use crate::types::{Summary, Ty, CONST_GENERIC, CONST_LEN_BASE};

/// The mark of an error the solver proved (`No`) of a goal that names no placeholder: it holds whatever the type
/// parameters of the fn being checked are, so a placeholder instance reports it
pub const SOLVED_NO: &str = "(E0277, solved)";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ans {
    Yes,
    No,
    Overflow,
}

impl Ans {
    /// Res.and: both goals (`No` as soon as one is, `Yes` when both are)
    pub fn and(self, o: Ans) -> Ans {
        match (self, o) {
            (Ans::No, _) | (_, Ans::No) => Ans::No,
            (Ans::Yes, Ans::Yes) => Ans::Yes,
            _ => Ans::Overflow,
        }
    }
    /// Res.or: either candidate (`Yes` as soon as one is, `No` when both are)
    pub fn or(self, o: Ans) -> Ans {
        match (self, o) {
            (Ans::Yes, _) | (_, Ans::Yes) => Ans::Yes,
            (Ans::No, Ans::No) => Ans::No,
            _ => Ans::Overflow,
        }
    }
}

/// A where clause of an impl: `P: Tr<A..>` over the impl's parameters
#[derive(Clone, Debug)]
pub struct Pred {
    pub ty: Ty,
    /// the trait's name (one segment)
    pub tr: String,
    /// the bound as written (`Fn(u8) -> u8`, `Iterator<Item = u8>`): what `implements` is asked for a trait not the
    /// program's
    pub raw: String,
    /// the trait's type arguments (None: not read, any)
    pub args: Option<Vec<Ty>>,
}

/// An impl of one of the program's traits
#[derive(Clone, Debug)]
pub struct Clause {
    /// the number of the impl's parameters (`Ty::Param(i)`, i < n)
    pub n: usize,
    /// the self type (None: not resolved, any type)
    pub self_ty: Option<Ty>,
    /// the trait's arguments (None: not resolved, any)
    pub args: Option<Vec<Ty>>,
    pub body: Vec<Pred>,
    /// the impl's item, for reports
    pub item: usize,
}

/// The program's impls of its own traits, by trait name
#[derive(Default, Debug)]
pub struct TraitDb {
    pub clauses: FxMap<String, Vec<Clause>>,
    /// the program's traits (only these are solved here; the rest are asked of `implements`)
    pub user: crate::fxhash::FxSet<String>,
    /// the traits the prelude declares: `implements` judges these exactly for scalars and plain structs
    pub prelude: crate::fxhash::FxSet<String>,
}

/// The recursion limit (rustc's is 128; zustc's goals are smaller)
const FUEL: u32 = 48;
/// The most goals one question may visit
const BUDGET: u32 = 20_000;

/// A goal on the stack: (type, trait, arguments)
type Key = (Ty, String, Vec<Ty>);

impl TraitDb {
    /// The clauses of the impls of the program's traits in its first `user` summaries
    pub fn build(p: &Program, sums: &[Summary], user: usize) -> TraitDb {
        let mut db = TraitDb::default();
        for s in &sums[..user.min(sums.len())] {
            if let Some(tr) = &s.trait_decl {
                db.user.insert(tr.clone());
            }
        }
        for s in &sums[user.min(sums.len())..] {
            if let Some(tr) = &s.trait_decl {
                db.prelude.insert(tr.clone());
            }
        }
        if db.user.is_empty() {
            return db;
        }
        for (ii, s) in sums.iter().enumerate().take(user) {
            let Some(tr) = &s.impl_trait else { continue };
            // (`impl a::Tr<u8> for S`: the trait's own name)
            let tr = tr.split('<').next().unwrap_or(tr).trim();
            let tr = tr.rsplit("::").next().unwrap_or(tr).to_string();
            if !db.user.contains(&tr) {
                continue;
            }
            let gn: Vec<&str> = s.impl_generics.iter().map(|g| g.0.as_str()).collect();
            let self_ty = s.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &gn).ok());
            let mut args = Some(Vec::new());
            for a in &s.impl_trait_args {
                match (p.resolve_g(a, &gn), args.as_mut()) {
                    (Ok(t), Some(v)) => v.push(t),
                    _ => args = None,
                }
            }
            let mut body = Vec::new();
            for (i, (_, bs)) in s.impl_generics.iter().enumerate() {
                for b in bs {
                    if let Some(pr) = pred_of(p, Ty::Param(i as u8), b, &gn) {
                        body.push(pr);
                    }
                }
            }
            db.clauses.entry(tr).or_default().push(Clause { n: gn.len(), self_ty, args, body, item: ii });
        }
        db
    }

    /// Whether `t: b` holds (b a bound as written: `Tr`, `Tr<u8>`, `a::Tr`); `generics`: the names of the type
    /// parameters b may name (none for a ground goal)
    pub fn holds(&self, p: &Program, t: Ty, b: &str) -> Ans {
        self.holds_in(p, t, b, &[], &[])
    }

    /// holds, for a bound of an item whose type parameters are `gn`, instantiated with `inst` (`U: Eq<T>` of
    /// `fn f<T, U: Eq<T>>`: T in the bound is the item's parameter, not a type of that name)
    pub fn holds_in(&self, p: &Program, t: Ty, b: &str, gn: &[&str], inst: &[Ty]) -> Ans {
        let Some(mut pr) = pred_of(p, t, b, gn) else { return Ans::Overflow };
        if let Some(a) = pr.args.as_mut() {
            for x in a.iter_mut() {
                *x = p.subst(*x, inst);
            }
        }
        let mut cx = Cx { db: self, p, stack: vec![], steps: 0 };
        cx.solve(FUEL, pr.ty, &pr.tr, &pr.raw, pr.args.as_deref())
    }
}

/// A bound b on t as a predicate: None when it is no trait bound (`?Sized`, a lifetime, a const parameter's type)
fn pred_of(p: &Program, t: Ty, b: &str, gn: &[&str]) -> Option<Pred> {
    let b = b.trim();
    if b.is_empty() || b.starts_with('?') || b.starts_with('\'') || b.starts_with(CONST_GENERIC) {
        return None;
    }
    let head = b.split('<').next().unwrap_or(b).split('(').next().unwrap_or(b).trim();
    let tr = head.rsplit("::").next().unwrap_or(head).to_string();
    let args = if b.contains('(') {
        None
    } else {
        crate::ast_summary::bound_args(b).and_then(|(_, a)| {
            let mut v = Vec::new();
            for x in &a {
                v.push(p.resolve_g(x, gn).ok()?);
            }
            Some(v)
        })
    };
    Some(Pred { ty: t, tr, raw: b.to_string(), args })
}

struct Cx<'a> {
    db: &'a TraitDb,
    p: &'a Program,
    stack: Vec<Key>,
    steps: u32,
}

impl Cx<'_> {
    fn solve(&mut self, fuel: u32, t: Ty, tr: &str, raw: &str, args: Option<&[Ty]>) -> Ans {
        self.steps += 1;
        if fuel == 0 || self.steps > BUDGET {
            return Ans::Overflow;
        }
        // (a goal that is not ground: a parameter, an associated type not normalized)
        if !ground(self.p, t, 0) || args.is_some_and(|a| a.iter().any(|&x| !ground(self.p, x, 0))) {
            return Ans::Overflow;
        }
        if !self.db.user.contains(tr) {
            return self.std_goal(t, tr, raw);
        }
        // (the environment: a placeholder's own bounds, the traits a trait object names, through their supertraits)
        let mut rigid = false;
        if let Ty::Struct(sid) = t {
            if let Some(bs) = &self.p.structs[sid as usize].placeholder {
                if bs.iter().any(|b| self.p.bound_implies(b, &[tr], false, 0)) {
                    return Ans::Yes;
                }
                rigid = true;
            }
        }
        if let Ty::Dyn(d) = t {
            if d.split('+').any(|b| self.p.bound_implies(b.trim(), &[tr], false, 0)) {
                return Ans::Yes;
            }
            rigid = true;
        }
        // (an inductive cycle: this goal is being proved above. Only for a goal whose arguments are known: one
        // whose are not stands for any of them, and the cut that makes the stack sound is of one goal)
        let key = args.map(|a| (t, tr.to_string(), a.to_vec()));
        if let Some(k) = &key {
            if self.stack.contains(k) {
                return Ans::No;
            }
        }
        let Some(cs) = self.db.clauses.get(tr) else {
            return if rigid { Ans::Overflow } else { Ans::No };
        };
        if let Some(k) = key.clone() {
            self.stack.push(k);
        }
        let mut r = Ans::No;
        for c in cs {
            r = r.or(self.candidate(fuel, c, t, args));
            if r == Ans::Yes {
                break;
            }
        }
        if key.is_some() {
            self.stack.pop();
        }
        // (a placeholder or a trait object may hold the trait through bounds zustc does not read: a where clause on
        // an associated type, a supertrait with arguments)
        if rigid && r == Ans::No {
            return Ans::Overflow;
        }
        r
    }

    /// One impl as a candidate for the goal (`candidate` of the model)
    fn candidate(&mut self, fuel: u32, c: &Clause, t: Ty, args: Option<&[Ty]>) -> Ans {
        let mut sol: Vec<Option<Ty>> = vec![None; c.n];
        if let Some(st) = c.self_ty {
            if !mtch(self.p, st, t, &mut sol, 0) {
                return Ans::No;
            }
        }
        if let (Some(ha), Some(ga)) = (&c.args, args) {
            if ha.len() == ga.len() {
                for (&h, &g) in ha.iter().zip(ga) {
                    if !mtch(self.p, h, g, &mut sol, 0) {
                        return Ans::No;
                    }
                }
            }
        }
        let mut r = Ans::Yes;
        for w in &c.body {
            // (the where clause under what the head bound: not ground when it names a parameter the head does not
            // bind)
            let Some(wt) = ground_subst(self.p, w.ty, &sol) else { return Ans::Overflow };
            let wargs = match &w.args {
                Some(a) => {
                    let mut v = Vec::new();
                    for &x in a {
                        match ground_subst(self.p, x, &sol) {
                            Some(y) => v.push(y),
                            None => return Ans::Overflow,
                        }
                    }
                    Some(v)
                }
                None => None,
            };
            r = r.and(self.solve(fuel - 1, wt, &w.tr, &w.raw, wargs.as_deref()));
            if r == Ans::No {
                return r;
            }
        }
        r
    }

    /// A trait that is not the program's: `implements`, believed when it says no only for the types and the
    /// prelude's traits it judges exactly
    fn std_goal(&self, t: Ty, tr: &str, raw: &str) -> Ans {
        if self.p.implements(t, raw) {
            return Ans::Yes;
        }
        let judged = t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Unit)
            || matches!(t, Ty::Struct(x) if self.p.structs[x as usize].base.is_none() && self.p.structs[x as usize].generics.is_empty() && self.p.structs[x as usize].placeholder.is_none());
        if judged && self.db.prelude.contains(tr) && !raw.contains('<') && !raw.contains('(') {
            Ans::No
        } else {
            Ans::Overflow
        }
    }
}

/// t names no type parameter, associated type or generic struct without its arguments (`W` for `W<T>`)
fn ground(p: &Program, t: Ty, depth: u32) -> bool {
    if depth > 32 {
        return false;
    }
    match t {
        Ty::Param(_) | Ty::Assoc(_) => false,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => ground(p, *i, depth + 1),
        Ty::Fn(f) => f.params.iter().chain([&f.ret]).all(|&x| ground(p, x, depth + 1)),
        Ty::Struct(s) => {
            let sd = &p.structs[s as usize];
            (sd.base.is_some() || sd.generics.is_empty()) && sd.args.iter().all(|&a| ground(p, a, depth + 1))
        }
        _ => !p.has_param(t),
    }
}

/// t with the impl's parameters replaced by what matching found: None when one it names is not bound
fn ground_subst(p: &Program, t: Ty, sol: &[Option<Ty>]) -> Option<Ty> {
    let r = p.partial_subst(t, sol);
    if p.has_param(r) {
        None
    } else {
        Some(r)
    }
}

/// One-way matching of a pattern (an impl's head, over its parameters) against a ground type: `mtch` of the model.
/// Exact: references and pointers agree in mutability, arrays in length, other types are equal. A part of the pattern
/// zustc cannot match exactly (an associated type) matches anything: the head is weakened, never strengthened
pub fn mtch(p: &Program, pt: Ty, t: Ty, sol: &mut [Option<Ty>], depth: u32) -> bool {
    if depth > 64 {
        return true;
    }
    match (pt, t) {
        (Ty::Assoc(_), _) => true,
        (Ty::Param(i), _) => match sol.get(i as usize).copied() {
            Some(Some(prev)) => prev == t,
            Some(None) => {
                sol[i as usize] = Some(t);
                true
            }
            // (a parameter that is not the impl's: `_`)
            None => true,
        },
        (Ty::Ref(m, a), Ty::Ref(n, b)) | (Ty::Ptr(m, a), Ty::Ptr(n, b)) => m == n && mtch(p, *a, *b, sol, depth + 1),
        (Ty::Slice(a), Ty::Slice(b)) | (Ty::Box(a), Ty::Box(b)) => mtch(p, *a, *b, sol, depth + 1),
        (Ty::Array(a, n), Ty::Array(b, m)) => {
            if n >= CONST_LEN_BASE {
                // (a const parameter's length: the parameter is the array of that length, as match_ty has it)
                let k = (n - CONST_LEN_BASE) as usize;
                let len = Ty::Array(crate::types::intern(Ty::Unit), m);
                let ok = match sol.get(k).copied() {
                    Some(Some(prev)) => prev == len,
                    Some(None) => {
                        sol[k] = Some(len);
                        true
                    }
                    None => true,
                };
                ok && mtch(p, *a, *b, sol, depth + 1)
            } else {
                n == m && mtch(p, *a, *b, sol, depth + 1)
            }
        }
        (Ty::Fn(a), Ty::Fn(b)) => {
            a.params.len() == b.params.len()
                && a.params.iter().zip(&b.params).all(|(&x, &y)| mtch(p, x, y, sol, depth + 1))
                && mtch(p, a.ret, b.ret, sol, depth + 1)
        }
        (Ty::Struct(a), Ty::Struct(b)) => {
            if a == b {
                return true;
            }
            let (sa, sb) = (&p.structs[a as usize], &p.structs[b as usize]);
            // (`impl Tr for W` of a generic W, its arguments not written: every instance)
            if sb.base == Some(a) {
                return true;
            }
            match (sa.base, sb.base) {
                (Some(x), Some(y)) if x == y && sa.args.len() == sb.args.len() => {
                    sa.args.iter().zip(&sb.args).all(|(&x, &y)| mtch(p, x, y, sol, depth + 1))
                }
                _ => false,
            }
        }
        // (a goal's i32 / f64 may be an integer / float literal zustc defaulted before an impl could choose its type
        // (`f(0, 2)` with `impl A<isize> for isize`): it matches any integer / float, the goal weakened)
        (_, Ty::I32) if pt.is_int() => true,
        (_, Ty::F64) if pt.is_float() => true,
        _ => pt == t,
    }
}

#[cfg(test)]
mod tests {
    use super::Ans;

    /// The tables of `Res.and` / `Res.or` in Solver.lean, case by case
    #[test]
    fn and_or_tables() {
        use Ans::*;
        let all = [Yes, No, Overflow];
        let and = |a, b| match (a, b) {
            (Yes, Yes) => Yes,
            (Yes, No) => No,
            (Yes, Overflow) => Overflow,
            (No, _) => No,
            (Overflow, Yes) => Overflow,
            (Overflow, No) => No,
            (Overflow, Overflow) => Overflow,
        };
        let or = |a, b| match (a, b) {
            (No, No) => No,
            (No, Yes) => Yes,
            (No, Overflow) => Overflow,
            (Yes, _) => Yes,
            (Overflow, No) => Overflow,
            (Overflow, Yes) => Yes,
            (Overflow, Overflow) => Overflow,
        };
        for a in all {
            for b in all {
                assert_eq!(a.and(b), and(a, b));
                assert_eq!(a.or(b), or(a, b));
            }
        }
    }
}
