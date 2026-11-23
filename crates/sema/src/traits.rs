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
    /// zustc read all of the impl: nothing was weakened (no where clause left out, the head resolved and with no
    /// associated type). Only such a clause's `Yes` counts where a `Yes` is reported (E0119)
    pub exact: bool,
}

/// The program's impls of its own traits, by trait name
#[derive(Default, Debug)]
pub struct TraitDb {
    pub clauses: FxMap<String, Vec<Clause>>,
    /// the program's traits (only these are solved here; the rest are asked of `implements`)
    pub user: crate::fxhash::FxSet<String>,
    /// the traits the prelude declares: `implements` judges these exactly for scalars and plain structs
    pub prelude: crate::fxhash::FxSet<String>,
    /// the auto traits (Send, Sync) among each program trait's own supertraits (`trait Foo: Send`): the summaries'
    /// supertraits leave them out, the driver reads them from the trait's header
    pub auto_supers: FxMap<String, Vec<String>>,
}

/// rustc's words for `t: tr` not holding
pub fn not_held(t: &str, tr: &str) -> String {
    match tr {
        "Send" => format!("`{t}` cannot be sent between threads safely"),
        "Sync" => format!("`{t}` cannot be shared between threads safely"),
        _ => format!("the trait bound `{t}: {}` is not satisfied", bound_text(tr)),
    }
}

/// A bound's text as rustc prints it: `Trait<isize>`, not the tokens' `Trait < isize >`
pub fn bound_text(b: &str) -> String {
    b.split_whitespace().collect::<Vec<_>>().join(" ").replace(" <", "<").replace("< ", "<").replace(" >", ">").replace(" ,", ",")
}

/// std's traits that `implements` judges exactly for scalars and plain structs, and that a type parameter has only by
/// its bounds
const STD_EXACT: &[&str] = &["Copy", "Clone", "Debug", "Display", "Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash"];

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
            // (a where clause on a type that is not a parameter (`where Vec<T>: Debug`) is left out)
            let mut exact = s.where_types.is_empty() && self_ty.is_some() && args.is_some();
            for (i, (_, bs)) in s.impl_generics.iter().enumerate() {
                for b in bs {
                    if let Some(pr) = pred_of(p, Ty::Param(i as u8), b, &gn) {
                        // (an associated type's binding (`Iterator<Item = u8>`) is left out; arguments not read)
                        if b.contains('=') || (db.user.contains(&pr.tr) && pr.args.is_none()) {
                            exact = false;
                        }
                        body.push(pr);
                    }
                }
            }
            if self_ty.is_some_and(|t| has_assoc(p, t, 0)) || args.as_ref().is_some_and(|a| a.iter().any(|&t| has_assoc(p, t, 0))) {
                exact = false;
            }
            db.clauses.entry(tr).or_default().push(Clause { n: gn.len(), self_ty, args, body, item: ii, exact });
        }
        db
    }

    /// Whether a bound b of a type parameter gives it the auto trait tr: b itself, or a trait of the program's with
    /// tr among its supertraits, all the way up (Some(false): surely not; None: a trait zustc does not know)
    pub fn gives_auto(&self, p: &Program, b: &str, tr: &str) -> Option<bool> {
        let b = b.trim();
        let name = b.split('<').next().unwrap_or(b).split('(').next().unwrap_or(b).trim();
        let name = name.rsplit("::").next().unwrap_or(name);
        if name == tr {
            return Some(true);
        }
        if self.user.contains(name) {
            let mut seen: Vec<String> = vec![];
            let mut todo = vec![name.to_string()];
            while let Some(t) = todo.pop() {
                if seen.contains(&t) {
                    continue;
                }
                if self.auto_supers.get(&t).is_some_and(|v| v.iter().any(|x| x == tr)) {
                    return Some(true);
                }
                for sup in p.trait_supers.get(&t).into_iter().flatten() {
                    let n = sup.split('<').next().unwrap_or(sup).trim();
                    let n = n.rsplit("::").next().unwrap_or(n).to_string();
                    if self.user.contains(&n) {
                        todo.push(n);
                    } else if !STD_EXACT.contains(&n.as_str()) && !n.starts_with('#') && !matches!(n.as_str(), "Sized" | "Send" | "Sync" | "Unpin") {
                        // (a std trait zustc does not know the supertraits of)
                        return None;
                    }
                }
                seen.push(t);
            }
            return Some(false);
        }
        // (std's traits zustc knows: none of them has an auto trait as a supertrait; nor do the Fn traits)
        if STD_EXACT.contains(&name) || matches!(name, "Sized" | "Send" | "Sync" | "Unpin" | "Fn" | "FnMut" | "FnOnce" | "Iterator" | "IntoIterator") || b.starts_with('\'') {
            return Some(false);
        }
        None
    }

    /// Whether `t: b` holds (b a bound as written: `Tr`, `Tr<u8>`, `a::Tr`); `generics`: the names of the type
    /// parameters b may name (none for a ground goal)
    pub fn holds(&self, p: &Program, t: Ty, b: &str) -> Ans {
        self.holds_in(p, t, b, &[], &[])
    }

    /// holds, for a bound of an item whose type parameters are `gn`, instantiated with `inst` (`U: Eq<T>` of
    /// `fn f<T, U: Eq<T>>`: T in the bound is the item's parameter, not a type of that name)
    /// holds, of a type whose placeholders are an impl's parameters with all their bounds (for the impl's supertrait
    /// obligations): a placeholder holds a trait only by its bounds (through their supertraits) or by an impl
    pub fn holds_rigid(&self, p: &Program, t: Ty, b: &str) -> Ans {
        let Some(pr) = pred_of(p, t, b, &[]) else { return Ans::Overflow };
        let mut cx = Cx { db: self, p, stack: vec![], steps: 0, strict: false, rigid_env: true };
        cx.solve(FUEL, pr.ty, &pr.tr, &pr.raw, pr.args.as_deref())
    }

    pub fn holds_in(&self, p: &Program, t: Ty, b: &str, gn: &[&str], inst: &[Ty]) -> Ans {
        let Some(mut pr) = pred_of(p, t, b, gn) else { return Ans::Overflow };
        if let Some(a) = pr.args.as_mut() {
            for x in a.iter_mut() {
                *x = p.subst(*x, inst);
            }
        }
        let mut cx = Cx { db: self, p, stack: vec![], steps: 0, strict: false, rigid_env: false };
        cx.solve(FUEL, pr.ty, &pr.tr, &pr.raw, pr.args.as_deref())
    }
}

/// The first marker for the parameters unification leaves free: types that are not ground (so a where clause that
/// names one is not proved, `Overflow`), one per parameter (so two free parameters are not taken for one)
const FREE: u8 = 200;

/// Two impls of one trait that surely conflict (E0119): (the later impl's item, the earlier's, the trait, the
/// common instance of their self types when it is ground). `conflict_sound` of Coherence.lean: substitutions that
/// make the heads equal (found by unification, and checked equal) under which the solver proves both impls' where
/// clauses: both apply to one goal. What zustc cannot unify exactly (an associated type) or prove is not reported
impl TraitDb {
    /// `read_all(item)`: the caller's check that the impl's where clauses are all on its parameters (the summaries
    /// leave out the others without a trace: `where W<U>: Tr`)
    pub fn conflict(&self, p: &Program, read_all: &dyn Fn(usize) -> bool) -> Option<(usize, usize, String, Option<Ty>)> {
        let mut trs: Vec<&String> = self.clauses.keys().collect();
        trs.sort();
        for tr in trs {
            let cs = &self.clauses[tr];
            for (j, b) in cs.iter().enumerate() {
                for a in &cs[..j] {
                    if let Some(g) = self.both_apply(p, a, b).filter(|_| read_all(a.item) && read_all(b.item)) {
                        let (x, y) = if a.item > b.item { (a.item, b.item) } else { (b.item, a.item) };
                        return Some((x, y, tr.clone(), g));
                    }
                }
            }
        }
        None
    }

    /// Some(common self type, when ground) when both impls surely apply to one goal
    fn both_apply(&self, p: &Program, a: &Clause, b: &Clause) -> Option<Option<Ty>> {
        if !a.exact || !b.exact {
            return None;
        }
        // (a fn pointer with a reference is higher-ranked or not by lifetimes `Ty` does not keep: `for<'a> fn(&'a
        // u8)` is not `fn(&'static u8)`, though both are one Ty)
        // (nor a trait object: `dyn Tr` keeps neither its auto traits nor its lifetime, so `dyn Error + 'a` and
        // `dyn Error + Send + 'a` are one Ty)
        let heads = |c: &Clause| c.self_ty.into_iter().chain(c.args.iter().flatten().copied()).any(|t| fn_ref(p, t, 0) || has_dyn(p, t, 0));
        if heads(a) || heads(b) {
            return None;
        }
        let (sa, sb) = (a.self_ty?, b.self_ty?);
        let (aa, ba) = (a.args.as_ref()?, b.args.as_ref()?);
        let n = a.n + b.n;
        if aa.len() != ba.len() || n >= FREE as usize {
            return None;
        }
        // (b's parameters after a's: one namespace)
        let ren: Vec<Ty> = (0..b.n).map(|i| Ty::Param((a.n + i) as u8)).collect();
        let sb = p.subst(sb, &ren);
        let ba: Vec<Ty> = ba.iter().map(|&t| p.subst(t, &ren)).collect();
        let mut sol: Vec<Option<Ty>> = vec![None; n];
        if unify(p, sa, sb, &mut sol, 0) != Some(true) {
            return None;
        }
        for (&x, &y) in aa.iter().zip(&ba) {
            if unify(p, x, y, &mut sol, 0) != Some(true) {
                return None;
            }
        }
        // (the most general unifier, each free parameter a marker of its own)
        let mut fin: Vec<Ty> = (0..n).map(|i| Ty::Param(FREE + i as u8)).collect();
        for _ in 0..=n {
            let next: Vec<Ty> = (0..n).map(|i| sol[i].map_or(Ty::Param(FREE + i as u8), |t| p.subst(t, &fin))).collect();
            if next == fin {
                break;
            }
            fin = next;
        }
        if fin.iter().any(|&t| (0..n as u8).any(|i| mentions(p, t, i, 0))) {
            return None;
        }
        // (the heads are equal under it: the hypothesis of conflict_sound, checked rather than trusted)
        let g = p.subst(sa, &fin);
        if g != p.subst(sb, &fin) || aa.iter().zip(&ba).any(|(&x, &y)| p.subst(x, &fin) != p.subst(y, &fin)) {
            return None;
        }
        let sub_b: Vec<Ty> = (0..b.n).map(|i| fin[a.n + i]).collect();
        for (c, sub) in [(a, &fin[..a.n]), (b, &sub_b[..])] {
            for w in &c.body {
                let t = p.subst(w.ty, sub);
                let args: Option<Vec<Ty>> = w.args.as_ref().map(|v| v.iter().map(|&x| p.subst(x, sub)).collect());
                let mut cx = Cx { db: self, p, stack: vec![], steps: 0, strict: true, rigid_env: false };
                if cx.solve(FUEL, t, &w.tr, &w.raw, args.as_deref()) != Ans::Yes {
                    return None;
                }
            }
        }
        Some(ground(p, g, 0).then_some(g))
    }
}

/// t holds an associated type
fn has_assoc(p: &Program, t: Ty, depth: u32) -> bool {
    if depth > 32 {
        return true;
    }
    match t {
        Ty::Assoc(_) => true,
        Ty::Ref(_, x) | Ty::Ptr(_, x) | Ty::Array(x, _) | Ty::Slice(x) | Ty::Box(x) => has_assoc(p, *x, depth + 1),
        Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&x| has_assoc(p, x, depth + 1)),
        Ty::Struct(s) => p.structs[s as usize].args.iter().any(|&x| has_assoc(p, x, depth + 1)),
        _ => false,
    }
}

/// t holds a reference somewhere (its lifetimes, which `Ty` does not keep, may tell two such types apart)
fn has_ref(p: &Program, t: Ty, depth: u32) -> bool {
    if depth > 32 {
        return true;
    }
    match t {
        Ty::Ref(..) => true,
        Ty::Ptr(_, x) | Ty::Array(x, _) | Ty::Slice(x) | Ty::Box(x) => has_ref(p, *x, depth + 1),
        Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&x| has_ref(p, x, depth + 1)),
        Ty::Struct(s) => p.structs[s as usize].args.iter().any(|&x| has_ref(p, x, depth + 1)),
        _ => false,
    }
}

/// t holds a trait object
fn has_dyn(p: &Program, t: Ty, depth: u32) -> bool {
    if depth > 32 {
        return true;
    }
    match t {
        Ty::Dyn(_) => true,
        Ty::Ref(_, x) | Ty::Ptr(_, x) | Ty::Array(x, _) | Ty::Slice(x) | Ty::Box(x) => has_dyn(p, *x, depth + 1),
        Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&x| has_dyn(p, x, depth + 1)),
        Ty::Struct(s) => p.structs[s as usize].args.iter().any(|&x| has_dyn(p, x, depth + 1)),
        _ => false,
    }
}

/// t holds a fn pointer type with a reference in it
fn fn_ref(p: &Program, t: Ty, depth: u32) -> bool {
    if depth > 32 {
        return true;
    }
    match t {
        Ty::Fn(_) => has_ref(p, t, 0),
        Ty::Ref(_, x) | Ty::Ptr(_, x) | Ty::Array(x, _) | Ty::Slice(x) | Ty::Box(x) => fn_ref(p, *x, depth + 1),
        Ty::Struct(s) => p.structs[s as usize].args.iter().any(|&x| fn_ref(p, x, depth + 1)),
        _ => false,
    }
}

/// t names the parameter i
fn mentions(p: &Program, t: Ty, i: u8, depth: u32) -> bool {
    if depth > 64 {
        return true;
    }
    match t {
        Ty::Param(j) => j == i,
        Ty::Assoc(a) => mentions(p, a.base, i, depth + 1),
        Ty::Ref(_, x) | Ty::Ptr(_, x) | Ty::Array(x, _) | Ty::Slice(x) | Ty::Box(x) => mentions(p, *x, i, depth + 1),
        Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&x| mentions(p, x, i, depth + 1)),
        Ty::Struct(s) => p.structs[s as usize].args.iter().any(|&x| mentions(p, x, i, depth + 1)),
        _ => false,
    }
}

/// First-order unification over one namespace of parameters (sol: what each is bound to), with the occurs check.
/// Some(false): the types surely differ; None: zustc cannot tell (an associated type, a const parameter's length)
fn unify(p: &Program, a: Ty, b: Ty, sol: &mut [Option<Ty>], depth: u32) -> Option<bool> {
    if depth > 64 {
        return None;
    }
    let walk = |mut t: Ty, sol: &[Option<Ty>]| {
        for _ in 0..sol.len() + 1 {
            match t {
                Ty::Param(i) => match sol.get(i as usize).copied().flatten() {
                    Some(u) => t = u,
                    None => break,
                },
                _ => break,
            }
        }
        t
    };
    let (a, b) = (walk(a, sol), walk(b, sol));
    if a == b {
        return Some(true);
    }
    // (a parameter's binding, followed through the others bound so far: for the occurs check)
    fn deep(p: &Program, t: Ty, sol: &[Option<Ty>], depth: u32) -> Ty {
        if depth > 32 {
            return t;
        }
        let args: Vec<Ty> = (0..sol.len()).map(|i| sol[i].map_or(Ty::Param(i as u8), |u| deep(p, u, sol, depth + 1))).collect();
        p.subst(t, &args)
    }
    let all = |xs: &[Ty], ys: &[Ty], sol: &mut [Option<Ty>]| -> Option<bool> {
        let mut r = Some(true);
        for (&x, &y) in xs.iter().zip(ys) {
            match unify(p, x, y, sol, depth + 1) {
                Some(false) => return Some(false),
                None => r = None,
                Some(true) => {}
            }
        }
        r
    };
    match (a, b) {
        (Ty::Assoc(_), _) | (_, Ty::Assoc(_)) => None,
        (Ty::Param(x), t) | (t, Ty::Param(x)) => {
            // (an impl's parameter is Sized unless it says `?Sized`, which the summaries do not keep: one bound to an
            // unsized type cannot be told)
            if (x as usize) >= sol.len() || matches!(t, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) {
                return None;
            }
            if mentions(p, deep(p, t, sol, 0), x, 0) {
                return Some(false);
            }
            sol[x as usize] = Some(t);
            Some(true)
        }
        (Ty::Ref(m, x), Ty::Ref(n, y)) | (Ty::Ptr(m, x), Ty::Ptr(n, y)) => {
            if m != n {
                Some(false)
            } else {
                unify(p, *x, *y, sol, depth + 1)
            }
        }
        (Ty::Slice(x), Ty::Slice(y)) | (Ty::Box(x), Ty::Box(y)) => unify(p, *x, *y, sol, depth + 1),
        (Ty::Array(x, n), Ty::Array(y, m)) => {
            if n >= CONST_LEN_BASE || m >= CONST_LEN_BASE {
                None
            } else if n != m {
                Some(false)
            } else {
                unify(p, *x, *y, sol, depth + 1)
            }
        }
        (Ty::Fn(f), Ty::Fn(g)) => {
            if f.params.len() != g.params.len() {
                return Some(false);
            }
            if has_ref(p, a, 0) || has_ref(p, b, 0) {
                return None;
            }
            let mut xs = f.params.clone();
            xs.push(f.ret);
            let mut ys = g.params.clone();
            ys.push(g.ret);
            all(&xs, &ys, sol)
        }
        (Ty::Struct(x), Ty::Struct(y)) => {
            let (sx, sy) = (&p.structs[x as usize], &p.structs[y as usize]);
            // (a placeholder is no type an impl names)
            if sx.placeholder.is_some() || sy.placeholder.is_some() {
                return None;
            }
            if sx.base.unwrap_or(x) != sy.base.unwrap_or(y) {
                return Some(false);
            }
            if sx.args.len() != sy.args.len() || sx.base.is_none() || sy.base.is_none() {
                return None;
            }
            let (xa, ya) = (sx.args.clone(), sy.args.clone());
            all(&xa, &ya, sol)
        }
        (Ty::Dyn(x), Ty::Dyn(y)) => {
            // (`dyn Send + Sync` is `dyn Sync + Send`)
            let parts = |d: &str| {
                let mut v: Vec<String> = d.split('+').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                v.sort();
                v.dedup();
                v
            };
            Some(parts(x) == parts(y))
        }
        _ => Some(false),
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
    /// a `Yes` is reported (E0119): nothing weakened may give one (clauses not `exact`, a literal's leniency, std's
    /// traits but where `implements` is exact)
    strict: bool,
    /// the placeholders are an impl's parameters, whose bounds are all its where clauses on them (the caller checked):
    /// their bounds are the whole environment, so a `No` of one holds
    rigid_env: bool,
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
        if matches!(tr, "Send" | "Sync") && raw.trim() == tr {
            return self.auto_goal(t, tr);
        }
        if !self.db.user.contains(tr) {
            if let Some(bs) = self.rigid_bounds(t) {
                // (an impl's parameter: a std trait only by its bounds)
                return if bs.iter().any(|b| self.p.bound_implies(b, &[tr], true, 0)) || self.p.implements(t, raw) {
                    Ans::Yes
                } else if bs.iter().all(|b| self.known(b)) && STD_EXACT.contains(&tr) && !raw.contains('<') {
                    Ans::No
                } else {
                    Ans::Overflow
                };
            }
            return self.std_goal(t, tr, raw);
        }
        // (the environment: a placeholder's own bounds, the traits a trait object names, through their supertraits)
        let mut rigid = false;
        if let Ty::Struct(sid) = t {
            if let Some(bs) = &self.p.structs[sid as usize].placeholder {
                if bs.iter().any(|b| self.p.bound_implies(b, &[tr], self.rigid_env, 0)) {
                    return if self.strict { Ans::Overflow } else { Ans::Yes };
                }
                // (an impl's parameter whose bounds are all known: they are the whole environment)
                rigid = !(self.rigid_env && bs.iter().all(|b| self.known(b)));
            }
        }
        if let Ty::Dyn(d) = t {
            if d.split('+').any(|b| if self.strict { b.trim() == tr } else { self.p.bound_implies(b.trim(), &[tr], false, 0) }) {
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
        let lenient = !self.strict;
        if let Some(st) = c.self_ty {
            if !mtch(self.p, st, t, &mut sol, lenient, 0) {
                return Ans::No;
            }
        }
        // (arguments not matched (not read, or not as many): any. Then a `Yes` is a weakened one)
        let mut weak = !c.exact;
        match (&c.args, args) {
            (Some(ha), Some(ga)) if ha.len() == ga.len() => {
                for (&h, &g) in ha.iter().zip(ga) {
                    if !mtch(self.p, h, g, &mut sol, lenient, 0) {
                        return Ans::No;
                    }
                }
            }
            (Some(ha), None) if ha.is_empty() => {}
            _ => weak = true,
        }
        let r = self.body(fuel, c, &sol);
        if self.strict && weak && r == Ans::Yes {
            return Ans::Overflow;
        }
        r
    }

    /// The where clauses of a candidate under what matching its head found
    fn body(&mut self, fuel: u32, c: &Clause, sol: &[Option<Ty>]) -> Ans {
        let mut r = Ans::Yes;
        for w in &c.body {
            // (the where clause under what the head bound: not ground when it names a parameter the head does not
            // bind)
            let Some(wt) = ground_subst(self.p, w.ty, sol) else { return Ans::Overflow };
            let wargs = match &w.args {
                Some(a) => {
                    let mut v = Vec::new();
                    for &x in a {
                        match ground_subst(self.p, x, sol) {
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

    /// The bounds of an impl's parameter (rigid_env), None for other types
    fn rigid_bounds(&self, t: Ty) -> Option<&Vec<String>> {
        match t {
            Ty::Struct(sid) if self.rigid_env => self.p.structs[sid as usize].placeholder.as_ref(),
            _ => None,
        }
    }

    /// A bound whose trait zustc knows all of: one of the program's (its supertraits are recorded) or std's that
    /// `implements` judges, written without arguments or bindings
    fn known(&self, b: &str) -> bool {
        let b = b.trim();
        !b.contains('<') && !b.contains('(') && !b.contains('=') && (self.db.user.contains(b) || STD_EXACT.contains(&b) || matches!(b, "Sized" | "Send" | "Sync"))
    }

    /// An auto trait (Send, Sync): coinductive, `csolve` of Coinductive.lean over the structural rules as clauses
    /// (a struct is Send when its fields are; a type that comes back round holds). A placeholder has it only by its
    /// bounds; what the rules here do not cover is `Overflow`
    fn auto_goal(&self, t: Ty, tr: &str) -> Ans {
        let mut stack = vec![];
        self.auto_rec(t, tr == "Sync", &mut stack, 0)
    }

    fn auto_rec(&self, t: Ty, sync: bool, stack: &mut Vec<(Ty, bool)>, depth: u32) -> Ans {
        let tr = if sync { "Sync" } else { "Send" };
        if depth > 24 {
            return Ans::Overflow;
        }
        let all = |cx: &Self, ts: &[(Ty, bool)], stack: &mut Vec<(Ty, bool)>| {
            let mut r = Ans::Yes;
            for &(x, sy) in ts {
                r = r.and(cx.auto_rec(x, sy, stack, depth + 1));
                if r == Ans::No {
                    break;
                }
            }
            r
        };
        match t {
            Ty::Ptr(..) => Ans::No,
            // (`&T: Send` needs `T: Sync`; `&mut T: Send` needs `T: Send`; both are Sync when T is)
            Ty::Ref(m, i) => self.auto_rec(*i, sync || !m, stack, depth + 1),
            Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.auto_rec(*i, sync, stack, depth + 1),
            Ty::Param(_) | Ty::Assoc(_) => Ans::Overflow,
            Ty::Dyn(d) => {
                if d.split('+').any(|b| self.db.gives_auto(self.p, b, tr) == Some(true)) {
                    Ans::Yes
                } else {
                    Ans::Overflow
                }
            }
            Ty::Struct(sid) => {
                let sd = &self.p.structs[sid as usize];
                if let Some(bs) = &sd.placeholder {
                    let mut r = Ans::No;
                    for b in bs {
                        match self.db.gives_auto(self.p, b, tr) {
                            Some(true) => return Ans::Yes,
                            Some(false) => {}
                            None => r = Ans::Overflow,
                        }
                    }
                    return r;
                }
                // (a type that comes back round: the set being built assumes it)
                if stack.contains(&(t, sync)) {
                    return Ans::Yes;
                }
                let b = self.p.base_of(sid);
                // (a user's `unsafe impl Send for T`)
                if self.p.impls.contains(&(b, tr.to_string())) {
                    return Ans::Yes;
                }
                let prelude = self.p.struct_item.get(b as usize).is_some_and(|&i| i >= self.p.prelude_from);
                let args: Vec<Ty> = sd.args.clone();
                if prelude && !self.p.is_tuple(b) {
                    let name = self.p.structs[b as usize].name.as_str();
                    let each = |sy: bool| args.iter().map(|&a| (a, sy)).collect::<Vec<_>>();
                    return match name {
                        "Rc" => Ans::No,
                        "Cell" | "RefCell" if sync => Ans::No,
                        "Cell" | "RefCell" | "Option" | "Vec" | "VecDeque" | "String" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "BinaryHeap" | "Result" | "PhantomData" => {
                            all(self, &each(sync), stack)
                        }
                        // (Arc<T> is Send and Sync when T is both; Mutex<T> when T is Send)
                        "Arc" => {
                            let mut v = each(false);
                            v.extend(each(true));
                            all(self, &v, stack)
                        }
                        "Mutex" => all(self, &each(false), stack),
                        _ => {
                            if !self.p.names_placeholder(t, 0) && self.p.implements(t, tr) {
                                Ans::Yes
                            } else {
                                Ans::Overflow
                            }
                        }
                    };
                }
                // (a struct, enum, tuple or closure: its fields (captures))
                let mut fs: Vec<(Ty, bool)> = sd.fields.iter().map(|f| (f.1, sync)).collect();
                for v in &sd.variants {
                    fs.extend(v.fields.iter().map(|f| (f.1, sync)));
                }
                stack.push((t, sync));
                let r = all(self, &fs, stack);
                stack.pop();
                r
            }
            _ => Ans::Yes,
        }
    }

    /// A trait that is not the program's: `implements`, believed when it says no only for the types and the
    /// prelude's traits it judges exactly
    fn std_goal(&self, t: Ty, tr: &str, raw: &str) -> Ans {
        let judged = t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Unit)
            || matches!(t, Ty::Struct(x) if self.p.structs[x as usize].base.is_none() && self.p.structs[x as usize].generics.is_empty() && self.p.structs[x as usize].placeholder.is_none());
        let exact = judged && !raw.contains('<') && !raw.contains('(')
            && (self.db.prelude.contains(tr) || matches!(tr, "Copy" | "Clone" | "Sized" | "Send" | "Sync" | "Debug" | "Default" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash"));
        if self.p.implements(t, raw) {
            // (`implements` may say yes of what it does not judge exactly)
            return if self.strict && !exact { Ans::Overflow } else { Ans::Yes };
        }
        if exact {
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
pub fn mtch(p: &Program, pt: Ty, t: Ty, sol: &mut [Option<Ty>], lenient: bool, depth: u32) -> bool {
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
        (Ty::Ref(m, a), Ty::Ref(n, b)) | (Ty::Ptr(m, a), Ty::Ptr(n, b)) => m == n && mtch(p, *a, *b, sol, lenient, depth + 1),
        (Ty::Slice(a), Ty::Slice(b)) | (Ty::Box(a), Ty::Box(b)) => mtch(p, *a, *b, sol, lenient, depth + 1),
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
                ok && mtch(p, *a, *b, sol, lenient, depth + 1)
            } else {
                n == m && mtch(p, *a, *b, sol, lenient, depth + 1)
            }
        }
        (Ty::Fn(a), Ty::Fn(b)) => {
            a.params.len() == b.params.len()
                && a.params.iter().zip(&b.params).all(|(&x, &y)| mtch(p, x, y, sol, lenient, depth + 1))
                && mtch(p, a.ret, b.ret, sol, lenient, depth + 1)
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
                    sa.args.iter().zip(&sb.args).all(|(&x, &y)| mtch(p, x, y, sol, lenient, depth + 1))
                }
                _ => false,
            }
        }
        // (a goal's i32 / f64 may be an integer / float literal zustc defaulted before an impl could choose its type
        // (`f(0, 2)` with `impl A<isize> for isize`): it matches any integer / float, the goal weakened)
        (_, Ty::I32) if lenient && pt.is_int() => true,
        (_, Ty::F64) if lenient && pt.is_float() => true,
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
