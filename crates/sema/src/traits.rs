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
    /// the associated types the bound binds (`Tr<Assoc = u8>`): (name, type)
    pub eqs: Vec<(String, Ty)>,
    /// every binding was read (else some are left out of eqs)
    pub eqs_read: bool,
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
    /// the where clauses on what is not a parameter (`where T::A: Foo`, `where (): Tr`) were all read (the driver
    /// reads them from the header's text: TraitDb::add_where)
    pub where_ok: bool,
    /// the associated types the impl defines (`type Assoc = u32;`), over its parameters
    pub assocs: Vec<(String, Ty)>,
    /// the head as written (the self type, then the trait's arguments, as Rust writes each), when it names a std type
    /// the prelude makes an alias of another (`OsString`, `Path`, `OsStr`): aliased_head
    pub alias: Option<Vec<String>>,
}

/// The program's impls of its own traits, by trait name
#[derive(Default, Debug, Clone)]
pub struct TraitDb {
    pub clauses: FxMap<String, Vec<Clause>>,
    /// the program's traits (only these are solved here; the rest are asked of `implements`)
    pub user: crate::fxhash::FxSet<String>,
    /// the traits the prelude declares: `implements` judges these exactly for scalars and plain structs
    pub prelude: crate::fxhash::FxSet<String>,
    /// the auto traits (Send, Sync) among each program trait's own supertraits (`trait Foo: Send`): the summaries'
    /// supertraits leave them out, the driver reads them from the trait's header
    pub auto_supers: FxMap<String, Vec<String>>,
    /// the names of associated types that impls zustc has no clauses of define (the prelude's, and the program's impls
    /// of std's traits): a projection by such a name alone (`T::Item`, no trait) is not normalized here
    pub other_assoc: crate::fxhash::FxSet<String>,
}

/// A bound as text with an instance's types for the item's parameters (`AsRef<T>` with T = str: `AsRef<str>`) and
/// no lifetimes (`Into<&'a str>`: `Into<&str>`, `Foo<'a>`: `Foo`): None when a parameter's type has no name zustc
/// could write back (a placeholder, a closure)
pub fn instantiate_bound(p: &Program, b: &str, gn: &[&str], inst: &[Ty]) -> Option<String> {
    let mut out = String::new();
    let cs: Vec<char> = b.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '\'' {
            // (a lifetime: dropped, with the comma or space after it)
            i += 1;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            while i < cs.len() && (cs[i] == ',' || cs[i] == ' ') {
                i += 1;
            }
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let st = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            let w: String = cs[st..i].iter().collect();
            let path = st > 0 && cs[st - 1] == ':';
            match gn.iter().position(|g| *g == w).filter(|_| !path) {
                Some(k) => {
                    let t = *inst.get(k)?;
                    if p.names_placeholder(t, 0) || matches!(t, Ty::Struct(s) if p.closure_of_struct.contains_key(&p.base_of(s))) {
                        return None;
                    }
                    out.push_str(&p.tn(t));
                }
                None => out.push_str(&w),
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    // (`Foo<>` left by a lifetime-only argument list)
    let out = out.replace("<>", "").replace("< >", "");
    Some(out.trim_end_matches([',', ' ']).to_string())
}

/// The std traits the solver answers of `()` from std's list
pub fn unit_judged(tr: &str) -> bool {
    STD_EXACT.contains(&tr) || matches!(tr, "Sized" | "Send" | "Sync" | "Iterator")
}

/// A type as rustc would print it in a message: an `impl Trait` argument's parameter (`__impl0`) as `impl Sync`,
/// without the module path zustc flattens names into (`a__b__Hidden` is `Hidden`) and zustc's own marks (`T#`,
/// `__u_`)
pub fn display_ty(p: &Program, t: Ty) -> String {
    let mut out = p.tn(t);
    // (the placeholders of `impl Trait` arguments in t, with their bounds)
    let mut todo = vec![t];
    let mut n = 0;
    while let Some(x) = todo.pop() {
        n += 1;
        if n > 64 {
            break;
        }
        match x {
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => todo.push(*i),
            Ty::Struct(sid) => {
                let sd = &p.structs[sid as usize];
                if let (Some(bs), true) = (&sd.placeholder, sd.name.starts_with("__impl")) {
                    let shown: Vec<String> = bs.iter().map(|b| bound_text(b)).collect();
                    out = out.replace(&sd.name, &format!("impl {}", shown.join(" + ")));
                }
                todo.extend(sd.args.iter().copied());
            }
            _ => {}
        }
    }
    display_name(&out)
}

/// A name without the module path zustc flattens it into (`a__b__Hidden`: `Hidden`) and its marks (`#`, `__u_`)
pub fn display_name(s: &str) -> String {
    let s = s.replace("__u_", "").replace('#', "");
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut after_seg = false;
    while i < cs.len() {
        // (a path segment `seg__`: starts a word (or follows one just dropped), a lowercase letter first, then `__` and
        // an identifier)
        let word_start = after_seg || i == 0 || !(cs[i - 1].is_alphanumeric() || cs[i - 1] == '_');
        after_seg = false;
        if word_start && cs[i].is_ascii_lowercase() {
            let mut j = i;
            while j < cs.len() && (cs[j].is_ascii_lowercase() || cs[j].is_ascii_digit() || (cs[j] == '_' && cs.get(j + 1) != Some(&'_'))) {
                j += 1;
            }
            if j + 2 < cs.len() && cs[j] == '_' && cs[j + 1] == '_' && (cs[j + 2].is_alphabetic()) {
                i = j + 2;
                after_seg = true;
                continue;
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

/// rustc's words for `t: tr` not holding
pub fn not_held(t: &str, tr: &str) -> String {
    let tr_shown = display_name(tr);
    let tr = tr_shown.as_str();
    match tr {
        "Send" => format!("`{t}` cannot be sent between threads safely"),
        "Sync" => format!("`{t}` cannot be shared between threads safely"),
        "Iterator" => format!("`{t}` is not an iterator"),
        "Display" | "Debug" => format!("`{t}` doesn't implement `{tr}`"),
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
    /// `header`: an item's impl header when the item is an impl's members (their associated types are the impl's)
    pub fn build(p: &Program, sums: &[Summary], user: usize, header: &dyn Fn(usize) -> Option<usize>) -> TraitDb {
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
        // (the associated types of each impl: its own summary's and its members')
        let mut defs: FxMap<usize, Vec<&crate::types::AssocSyn>> = FxMap::default();
        for (ii, s) in sums.iter().enumerate() {
            for a in &s.assocs {
                defs.entry(header(ii).unwrap_or(ii)).or_default().push(a);
            }
        }
        for (ii, s) in sums.iter().enumerate() {
            let h = header(ii).unwrap_or(ii);
            let tr = sums.get(h).and_then(|x| x.impl_trait.as_deref()).map(|t| t.split('<').next().unwrap_or(t).trim()).map(|t| t.rsplit("::").next().unwrap_or(t));
            if ii >= user || tr.is_none_or(|t| !db.user.contains(t)) {
                for a in &s.assocs {
                    db.other_assoc.insert(a.name.clone());
                }
            }
        }
        if db.user.is_empty() {
            return db;
        }
        // (the program's own aliases, for the heads written through them: aliased_head)
        let own: OwnAliases = sums[..user.min(sums.len())].iter().flat_map(|s| s.aliases.iter().map(|(n, g, t)| (n.as_str(), (g.as_slice(), t)))).collect();
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
            let mut exact = self_ty.is_some() && args.is_some();
            for (i, (_, bs)) in s.impl_generics.iter().enumerate() {
                for b in bs {
                    if let Some(pr) = pred_of(p, Ty::Param(i as u8), b, &gn) {
                        // (an associated type's binding of std's trait (`Iterator<Item = u8>`) is left out, and of one
                        // of the program's that was not read; arguments not read)
                        if (b.contains('=') && !(db.user.contains(&pr.tr) && pr.eqs_read)) || (db.user.contains(&pr.tr) && pr.args.is_none()) {
                            exact = false;
                        }
                        body.push(pr);
                    }
                }
            }
            let mut assocs = vec![];
            for a in defs.get(&ii).into_iter().flatten() {
                // (a generic associated type is not normalized here)
                if a.generics.iter().any(|g| g == "#gat") {
                    continue;
                }
                let agn: Vec<&str> = a.generics.iter().map(|g| g.as_str()).collect();
                if agn == gn {
                    if let Ok(v) = p.resolve_g(&a.value, &gn) {
                        assocs.push((a.name.clone(), v));
                    }
                }
            }
            if self_ty.is_some_and(|t| has_assoc(p, t, 0)) || args.as_ref().is_some_and(|a| a.iter().any(|&t| has_assoc(p, t, 0))) {
                exact = false;
            }
            // (a head zustc represents by two types: `Cow<'_, T>` is the prelude's Cow<T> (a Cow of [T]) or __CowStr (a
            // Cow of str). Its head stands for both, so it matches anything: weakened)
            let mut self_ty = self_ty;
            let mut args = args;
            if self_ty.is_some_and(|t| two_reps(p, t, 0)) {
                self_ty = None;
                exact = false;
            }
            if args.as_ref().is_some_and(|a| a.iter().any(|&t| two_reps(p, t, 0))) {
                args = None;
                exact = false;
            }
            let alias = aliased_head(p, s, &gn, user, &own);
            db.clauses.entry(tr).or_default().push(Clause { n: gn.len(), self_ty, args, body, item: ii, exact, where_ok: s.where_types.is_empty(), assocs, alias });
        }
        db
    }

    /// The where clauses of impl `item` on what is not one of its parameters, read by the driver from its header
    /// (`where T::Assoc: Foo`, `where (): Array<Element = &()>`): added to its clause; `all`: every one was read
    pub fn add_where(&mut self, item: usize, preds: Vec<Pred>, all: bool) {
        for cs in self.clauses.values_mut() {
            for c in cs.iter_mut().filter(|c| c.item == item) {
                c.body.extend(preds.iter().cloned());
                c.where_ok = all;
            }
        }
    }

    /// The associated type `name` of the ground type t, by trait tr (None: any trait of the program's that defines
    /// it, when no other impl does): `normalize` of Normalize.lean
    pub fn normalize(&self, p: &Program, t: Ty, name: &str, tr: Option<&str>) -> Option<Ty> {
        let cx = Cx { db: self, p, stack: vec![], steps: 0, strict: true, rigid_env: false, norm_depth: 0 };
        cx.normalize(t, name, tr)
    }

    /// Whether a bound b that a trait puts on its associated type holds of the impl's definition t (`type Sibling:
    /// Bar2<Ok = Self::Ok>` with `type Sibling = Foo2`): `Self` in b is the impl's self type, and its associated types
    /// are normalized by the solver. Some((projection, it, asked)) when a binding is another type (E0271)
    pub fn assoc_bound(&self, p: &Program, t: Ty, b: &str, self_ty: Ty) -> (Ans, Option<(String, Ty, Ty)>) {
        let Some(pr) = pred_of(p, t, b, &["Self"]) else { return (Ans::Overflow, None) };
        let cx = Cx { db: self, p, stack: vec![], steps: 0, strict: false, rigid_env: true, norm_depth: 0 };
        let sol = [Some(self_ty)];
        let mut w = pr.clone();
        if let Some(a) = &pr.args {
            let mut v = vec![];
            for &x in a {
                match cx.isubst(x, &sol, 0) {
                    Some(y) => v.push(y),
                    None => return (Ans::Overflow, None),
                }
            }
            w.args = Some(v);
        }
        let mut eqs = vec![];
        for (n, x) in &pr.eqs {
            match cx.isubst(*x, &sol, 0) {
                Some(y) => eqs.push((n.clone(), y)),
                None => return (Ans::Overflow, None),
            }
        }
        w.eqs = eqs;
        if self.user.contains(&w.tr) && w.eqs_read {
            for (n, x) in &w.eqs {
                if let Some(u) = cx.normalize(t, n, Some(&w.tr)) {
                    if u != *x {
                        return (Ans::No, Some((format!("<{} as {}>::{n}", p.tn(t), w.tr), u, *x)));
                    }
                }
            }
        }
        let mut cx = cx;
        (cx.solve_pred(FUEL, &w), None)
    }

    /// A bound's binding that the trait solver finds another type for: (`<t as Tr>::name`, what it is, what the
    /// bound asks), for E0271
    pub fn binding_mismatch(&self, p: &Program, t: Ty, b: &str, gn: &[&str], inst: &[Ty]) -> Option<(String, Ty, Ty)> {
        let pr = pred_of(p, t, b, gn)?;
        for (n, x) in &pr.eqs {
            let x = p.subst(*x, inst);
            if let Some(u) = self.normalize(p, t, n, Some(&pr.tr)) {
                if u != x && !p.has_param(x) {
                    return Some((format!("<{} as {}>::{n}", p.tn(t), pr.tr), u, x));
                }
            }
        }
        None
    }

    /// A type parameter's placeholder t surely lacks std's trait tr (one of STD_EXACT): its bounds are all traits zustc
    /// knows the supertraits of, and none of them gives tr (Copy gives Clone, Eq PartialEq, Ord Eq and PartialOrd,
    /// PartialOrd PartialEq; the program's traits their recorded supertraits). Its bounds are its whole environment,
    /// so this holds whatever the parameter is
    pub fn placeholder_lacks(&self, p: &Program, t: Ty, tr: &str) -> bool {
        let Ty::Struct(sid) = t else { return false };
        let Some(bs) = &p.structs[sid as usize].placeholder else { return false };
        if !STD_EXACT.contains(&tr) {
            return false;
        }
        let std_gives = |b: &str, tr: &str| {
            b == tr
                || (b == "Copy" && tr == "Clone")
                || (b == "Eq" && tr == "PartialEq")
                || (b == "PartialOrd" && tr == "PartialEq")
                || (b == "Ord" && matches!(tr, "Eq" | "PartialOrd" | "PartialEq"))
        };
        let mut todo: Vec<String> = bs.iter().map(|b| b.trim().to_string()).collect();
        let mut seen: Vec<String> = vec![];
        while let Some(b) = todo.pop() {
            if b.starts_with('\'') || b.starts_with('?') || seen.contains(&b) {
                continue;
            }
            seen.push(b.clone());
            if b.contains('<') || b.contains('(') || b.contains('=') {
                return false;
            }
            let n = b.rsplit("::").next().unwrap_or(&b).to_string();
            if STD_EXACT.contains(&n.as_str()) {
                if std_gives(&n, tr) {
                    return false;
                }
            } else if self.user.contains(&n) {
                for sup in p.trait_supers.get(&n).into_iter().flatten().chain(p.trait_marker_supers.get(&n).into_iter().flatten()) {
                    if !sup.starts_with('#') {
                        todo.push(sup.trim().to_string());
                    }
                }
            } else if !matches!(n.as_str(), "Sized" | "Send" | "Sync" | "Unpin") {
                // (a std trait zustc does not know the supertraits of)
                return false;
            }
        }
        true
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
            // (a std trait zustc does not know the supertraits of, met on the way: the answer is open only if no other
            // path gives the auto trait)
            let mut unknown = false;
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
                    } else if n == tr {
                        return Some(true);
                    } else if !STD_EXACT.contains(&n.as_str()) && !n.starts_with('#') && !matches!(n.as_str(), "Sized" | "Send" | "Sync" | "Unpin") {
                        unknown = true;
                    }
                }
                seen.push(t);
            }
            return if unknown { None } else { Some(false) };
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
        let mut cx = Cx { db: self, p, stack: vec![], steps: 0, strict: false, rigid_env: true, norm_depth: 0 };
        cx.solve_pred(FUEL, &pr)
    }

    pub fn holds_in(&self, p: &Program, t: Ty, b: &str, gn: &[&str], inst: &[Ty]) -> Ans {
        let Some(mut pr) = pred_of(p, t, b, gn) else { return Ans::Overflow };
        if let Some(a) = pr.args.as_mut() {
            for x in a.iter_mut() {
                *x = p.subst(*x, inst);
            }
        }
        for e in pr.eqs.iter_mut() {
            e.1 = p.subst(e.1, inst);
        }
        let mut cx = Cx { db: self, p, stack: vec![], steps: 0, strict: false, rigid_env: false, norm_depth: 0 };
        cx.solve_pred(FUEL, &pr)
    }
}

/// The first marker for the parameters unification leaves free: types that are not ground (so a where clause that
/// names one is not proved, `Overflow`), one per parameter (so two free parameters are not taken for one)
const FREE: u8 = 200;

/// The outermost constructor of an impl's self type, as far as `unify` tells two apart: two types of different heads
/// never unify. None for what unifies with anything or is not unified at all (a parameter, an associated type, an
/// array of a const parameter's length, a placeholder)
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Head {
    Leaf(Ty),
    Ref(bool),
    Ptr(bool),
    Slice,
    Box,
    Array(u32),
    Fn(usize),
    Struct(u32),
    Dyn,
}

fn head(p: &Program, t: Ty) -> Option<Head> {
    Some(match t {
        Ty::Param(_) | Ty::Assoc(_) => return None,
        Ty::Ref(m, _) => Head::Ref(m),
        Ty::Ptr(m, _) => Head::Ptr(m),
        Ty::Slice(_) => Head::Slice,
        Ty::Box(_) => Head::Box,
        Ty::Array(_, n) if n >= CONST_LEN_BASE => return None,
        Ty::Array(_, n) => Head::Array(n),
        Ty::Fn(f) => Head::Fn(f.params.len()),
        Ty::Struct(x) => {
            let s = &p.structs[x as usize];
            if s.placeholder.is_some() {
                return None;
            }
            Head::Struct(s.base.unwrap_or(x))
        }
        Ty::Dyn(_) => Head::Dyn,
        t => Head::Leaf(t),
    })
}

/// Two ascending lists of indices as one
fn merge_sorted(a: &[usize], b: &[usize]) -> Vec<usize> {
    let (mut v, mut i, mut j) = (Vec::with_capacity(a.len() + b.len()), 0, 0);
    while i < a.len() || j < b.len() {
        if j == b.len() || (i < a.len() && a[i] < b[j]) {
            v.push(a[i]);
            i += 1;
        } else {
            v.push(b[j]);
            j += 1;
        }
    }
    v
}

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
            // (which impls may be compared at all, once per impl rather than once per pair: a crate's macro writes
            // hundreds of impls of one trait, `impl FromHex for [u8; N]` for each N, whose pairs this check tries.
            // An impl without a self type is no witness: both_apply needs both)
            let ok: Vec<bool> = cs.iter().map(|c| c.exact && c.self_ty.is_some() && !unknowable_head(p, c)).collect();
            // (and which pairs: two self types of different heads do not unify (`[u8; 4]`, `[u8; 8]`), so an impl is
            // compared only with the earlier ones of its head and those whose self type is a parameter (`impl<T> Tr
            // for T`), in the order of all the pairs: the first conflict found is the one the whole search finds)
            let heads: Vec<Option<Head>> = cs.iter().map(|c| c.self_ty.and_then(|t| head(p, t))).collect();
            let mut by_head: FxMap<Head, Vec<usize>> = FxMap::default();
            let mut open: Vec<usize> = vec![];
            for (j, b) in cs.iter().enumerate() {
                if !ok[j] {
                    continue;
                }
                let earlier: Vec<usize> = match heads[j] {
                    None => (0..j).filter(|&i| ok[i]).collect(),
                    Some(h) => merge_sorted(by_head.get(&h).map_or(&[][..], |v| v.as_slice()), &open),
                };
                match heads[j] {
                    None => open.push(j),
                    Some(h) => by_head.entry(h).or_default().push(j),
                }
                for i in earlier {
                    let a = &cs[i];
                    if two_written(p, a, b) {
                        continue;
                    }
                    if let Some(g) = self.both_apply(p, a, b).filter(|_| read_all(a.item) && read_all(b.item)) {
                        let (x, y) = if a.item > b.item { (a.item, b.item) } else { (b.item, a.item) };
                        return Some((x, y, tr.clone(), g));
                    }
                }
            }
        }
        None
    }

    /// Some(common self type, when ground) when both impls surely apply to one goal (both `exact`, neither of an
    /// `unknowable_head`: the caller's checks)
    fn both_apply(&self, p: &Program, a: &Clause, b: &Clause) -> Option<Option<Ty>> {
        let (sa, sb) = (a.self_ty?, b.self_ty?);
        let (aa, ba) = (a.args.as_ref()?, b.args.as_ref()?);
        let n = a.n + b.n;
        // (the markers FREE + i stay below HOLE and the other reserved parameters: more parameters than that are
        // not checked (no conflict is reported))
        if aa.len() != ba.len() || n >= FREE as usize || FREE as usize + n > crate::program::HOLE as usize {
            return None;
        }
        // (b's parameters after a's: one namespace. An impl with none has nothing to rename)
        let ba: std::borrow::Cow<[Ty]> = if b.n == 0 {
            std::borrow::Cow::Borrowed(ba.as_slice())
        } else {
            let ren: Vec<Ty> = (0..b.n).map(|i| Ty::Param((a.n + i) as u8)).collect();
            std::borrow::Cow::Owned(ba.iter().map(|&t| p.subst(t, &ren)).collect())
        };
        let sb = if b.n == 0 { sb } else { p.subst(sb, &(0..b.n).map(|i| Ty::Param((a.n + i) as u8)).collect::<Vec<Ty>>()) };
        let mut sol: Vec<Option<Ty>> = vec![None; n];
        if unify(p, sa, sb, &mut sol, 0) != Some(true) {
            return None;
        }
        for (&x, &y) in aa.iter().zip(ba.iter()) {
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
        if g != p.subst(sb, &fin) || aa.iter().zip(ba.iter()).any(|(&x, &y)| p.subst(x, &fin) != p.subst(y, &fin)) {
            return None;
        }
        let sub_b: Vec<Ty> = (0..b.n).map(|i| fin[a.n + i]).collect();
        for (c, sub) in [(a, &fin[..a.n]), (b, &sub_b[..])] {
            for w in &c.body {
                let t = p.subst(w.ty, sub);
                let args: Option<Vec<Ty>> = w.args.as_ref().map(|v| v.iter().map(|&x| p.subst(x, sub)).collect());
                let mut cx = Cx { db: self, p, stack: vec![], steps: 0, strict: true, rigid_env: false, norm_depth: 0 };
                if cx.solve(FUEL, t, &w.tr, &w.raw, args.as_deref()) != Ans::Yes {
                    return None;
                }
                // (the bound's bindings too (`T: Select<Item = u8>` with u8's `Item = u16`: not for u8): each must
                // normalize to what it is bound to; one not read or not known is no witness of a conflict)
                if !w.eqs_read {
                    return None;
                }
                for (name, want) in &w.eqs {
                    let want = p.subst(*want, sub);
                    if self.normalize(p, t, name, Some(&w.tr)) != Some(want) {
                        return None;
                    }
                }
            }
        }
        Some(ground(p, g, 0).then_some(g))
    }
}

/// t holds a prelude type that zustc represents by two types depending on its argument: `Cow<T>` of a parameter (a
/// Cow of `[T]`, or of `str`, which is `__CowStr`)
fn two_reps(p: &Program, t: Ty, depth: u32) -> bool {
    if depth > 32 {
        return true;
    }
    match t {
        Ty::Ref(_, x) | Ty::Ptr(_, x) | Ty::Array(x, _) | Ty::Slice(x) | Ty::Box(x) => two_reps(p, *x, depth + 1),
        Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&x| two_reps(p, x, depth + 1)),
        Ty::Struct(s) => {
            let sd = &p.structs[s as usize];
            let b = p.base_of(s);
            // (the Cow whose `Cow<str>` is __CowStr: the prelude's, when it has __CowStr)
            let cow = p.struct_index.get("Cow") == Some(&b) && p.struct_index.contains_key("__CowStr");
            (cow && sd.args.iter().any(|a| matches!(a, Ty::Param(_))))
                || sd.args.iter().any(|&x| two_reps(p, x, depth + 1))
        }
        _ => false,
    }
}

/// The program's own type aliases (`type MyOs = OsString;`), by name: their parameters and body (aliased_head)
type OwnAliases<'a> = FxMap<&'a str, (&'a [String], &'a crate::types::TySyn)>;

/// The head of impl s (its self type, then its trait arguments) as written, each as Rust writes the type (each path
/// by its last segment, the program's own aliases replaced by what they name), when it names one of the std types the
/// prelude makes an alias of another (`type OsString = String`, `type Path = str`, `type OsStr = str`): not a parameter
/// of the impl (gn) of that name, nor a type or an alias of the program's (its first `user` items are the program's;
/// `type Path = str;` of its own is str, and conflicts with an impl for str as in rustc)
fn aliased_head(p: &Program, s: &Summary, gn: &[&str], user: usize, own: &OwnAliases) -> Option<Vec<String>> {
    use crate::types::TySyn as S;
    // (the cheap look first: a name of the three, or of an alias of the program's)
    fn may(t: &S, own: &OwnAliases, depth: u32) -> bool {
        let named = |n: &str| matches!(n.rsplit("::").next().unwrap_or(n), "OsString" | "OsStr" | "Path") || own.contains_key(n);
        depth <= 32
            && match t {
                S::Named(n) => named(n),
                S::App(n, xs) => named(n) || xs.iter().any(|x| may(x, own, depth + 1)),
                S::Ref(_, x) | S::Ptr(_, x) | S::Slice(x) | S::Array(x, _) | S::Assoc(x, _) => may(x, own, depth + 1),
                S::Tuple(xs) | S::FnPtr(xs) => xs.iter().any(|x| may(x, own, depth + 1)),
                _ => false,
            }
    }
    // (t with each alias of the program's as what it names, its parameters as the arguments: not a parameter of the
    // impl's of that name, nor the alias's own name inside its body (`type Path = std::path::Path;`: std's Path))
    fn expand<'a>(t: &S, gn: &[&str], own: &OwnAliases<'a>, inside: &mut Vec<&'a str>, depth: u32) -> S {
        if depth > 32 {
            return t.clone();
        }
        let each = |xs: &[S], inside: &mut Vec<&'a str>| xs.iter().map(|x| expand(x, gn, own, inside, depth + 1)).collect::<Vec<S>>();
        let alias = |n: &str, k: usize, inside: &[&str]| own.get_key_value(n).filter(|&(&a, &(g, _))| g.len() == k && !gn.contains(&a) && !inside.contains(&a)).map(|(&a, &(g, b))| (a, g, b));
        let (name, g, body, args) = match t {
            S::Named(n) => match alias(n, 0, inside) {
                Some((a, g, b)) => (a, g, b, vec![]),
                None => return t.clone(),
            },
            S::App(n, xs) => {
                let xs = each(xs, inside);
                match alias(n, xs.len(), inside) {
                    Some((a, g, b)) => (a, g, b, xs),
                    None => return S::App(n.clone(), xs),
                }
            }
            S::Ref(m, x) => return S::Ref(*m, Box::new(expand(x, gn, own, inside, depth + 1))),
            S::Ptr(m, x) => return S::Ptr(*m, Box::new(expand(x, gn, own, inside, depth + 1))),
            S::Slice(x) => return S::Slice(Box::new(expand(x, gn, own, inside, depth + 1))),
            S::Array(x, l) => return S::Array(Box::new(expand(x, gn, own, inside, depth + 1)), l.clone()),
            S::Assoc(x, n) => return S::Assoc(Box::new(expand(x, gn, own, inside, depth + 1)), n.clone()),
            S::Tuple(xs) => return S::Tuple(each(xs, inside)),
            S::FnPtr(xs) => return S::FnPtr(each(xs, inside)),
            _ => return t.clone(),
        };
        fn subst(t: &S, g: &[String], args: &[S], depth: u32) -> S {
            if depth > 32 {
                return t.clone();
            }
            let each = |xs: &[S]| xs.iter().map(|x| subst(x, g, args, depth + 1)).collect::<Vec<S>>();
            match t {
                S::Named(n) => g.iter().position(|x| x == n).map_or_else(|| t.clone(), |i| args[i].clone()),
                S::App(n, xs) => S::App(n.clone(), each(xs)),
                S::Ref(m, x) => S::Ref(*m, Box::new(subst(x, g, args, depth + 1))),
                S::Ptr(m, x) => S::Ptr(*m, Box::new(subst(x, g, args, depth + 1))),
                S::Slice(x) => S::Slice(Box::new(subst(x, g, args, depth + 1))),
                S::Array(x, l) => S::Array(Box::new(subst(x, g, args, depth + 1)), l.clone()),
                S::Assoc(x, n) => S::Assoc(Box::new(subst(x, g, args, depth + 1)), n.clone()),
                S::Tuple(xs) => S::Tuple(each(xs)),
                S::FnPtr(xs) => S::FnPtr(each(xs)),
                _ => t.clone(),
            }
        }
        let body = if g.is_empty() { body.clone() } else { subst(body, g, &args, 0) };
        inside.push(name);
        let r = expand(&body, gn, own, inside, depth + 1);
        inside.pop();
        r
    }
    fn names(p: &Program, t: &S, gn: &[&str], user: usize, depth: u32) -> bool {
        if depth > 32 {
            return false;
        }
        let named = |n: &str| {
            let last = n.rsplit("::").next().unwrap_or(n);
            matches!(last, "OsString" | "OsStr" | "Path")
                && !gn.contains(&n)
                && match p.resolve_g(&S::Named(n.to_string()), &[]) {
                    Ok(Ty::Str) => last != "OsString",
                    Ok(Ty::Struct(s)) => last == "OsString" && p.struct_index.get("String") == Some(&s) && p.struct_item.get(s as usize).is_some_and(|&i| i as usize >= user),
                    _ => false,
                }
        };
        match t {
            S::Named(n) => named(n),
            S::App(n, xs) => named(n) || xs.iter().any(|x| names(p, x, gn, user, depth + 1)),
            S::Ref(_, x) | S::Ptr(_, x) | S::Slice(x) | S::Array(x, _) | S::Assoc(x, _) => names(p, x, gn, user, depth + 1),
            S::Tuple(xs) | S::FnPtr(xs) => xs.iter().any(|x| names(p, x, gn, user, depth + 1)),
            _ => false,
        }
    }
    let heads: Vec<&S> = s.impl_self_ty.iter().chain(&s.impl_trait_args).collect();
    if !heads.iter().any(|t| may(t, own, 0)) {
        return None;
    }
    let heads: Vec<S> = heads.into_iter().map(|t| if own.is_empty() { t.clone() } else { expand(t, gn, own, &mut vec![], 0) }).collect();
    if !heads.iter().any(|t| names(p, t, gn, user, 0)) {
        return None;
    }
    Some(heads.iter().map(|t| ty_text(t, 0)).collect())
}

/// A written type as Rust writes it, each path by its last segment (`std::ffi::OsString` and `OsString` are one type):
/// `[OsString; 2]`, `&mut (u8, OsString)`, `*const Path`, `fn(u8) -> OsString`, `<T as Tr>::A`
fn ty_text(t: &crate::types::TySyn, depth: u32) -> String {
    use crate::ast_types::{split_trait_assoc, GAT_PREFIX, NO_STD_ASSOC, TARGS_PREFIX};
    use crate::types::{ArrLen, TySyn as S};
    if depth > 32 {
        return "_".into();
    }
    let last = |n: &str| n.rsplit("::").next().unwrap_or(n).replace("__u_", "");
    let list = |xs: &[S]| xs.iter().map(|x| ty_text(x, depth + 1)).collect::<Vec<_>>().join(", ");
    match t {
        S::Prim(Ty::Unit) => "()".into(),
        S::Prim(Ty::Never) => "!".into(),
        S::Prim(p) => format!("{p:?}").to_lowercase(),
        S::Str => "str".into(),
        S::Infer => "_".into(),
        S::Named(n) => last(n),
        S::App(n, xs) => format!("{}<{}>", last(n), list(xs)),
        S::Ref(m, x) => format!("&{}{}", if *m { "mut " } else { "" }, ty_text(x, depth + 1)),
        S::Ptr(m, x) => format!("*{} {}", if *m { "mut" } else { "const" }, ty_text(x, depth + 1)),
        S::Slice(x) => format!("[{}]", ty_text(x, depth + 1)),
        S::Array(x, ArrLen::Lit(k)) => format!("[{}; {k}]", ty_text(x, depth + 1)),
        S::Array(x, ArrLen::Const(c)) => format!("[{}; {}]", ty_text(x, depth + 1), last(c)),
        S::Tuple(xs) if xs.len() == 1 => format!("({},)", ty_text(&xs[0], depth + 1)),
        S::Tuple(xs) => format!("({})", list(xs)),
        S::Dyn(d) => format!("dyn {d}"),
        // (the last is the return value; `()` is not written)
        S::FnPtr(xs) => match xs.split_last() {
            Some((r, ps)) if matches!(r, S::Prim(Ty::Unit)) || matches!(r, S::Tuple(v) if v.is_empty()) => format!("fn({})", list(ps)),
            Some((r, ps)) => format!("fn({}) -> {}", list(ps), ty_text(r, depth + 1)),
            None => "fn()".into(),
        },
        // (`P::Pointer<String>`, `<S as Trait<usize>>::Assoc`, `<&J as BitAnd>::Output`, `T::Item`)
        S::Assoc(b, n) => match (&**b, n.strip_prefix(GAT_PREFIX), n.strip_prefix(TARGS_PREFIX)) {
            (S::Tuple(parts), Some(g), _) if !parts.is_empty() => format!("{}::{g}<{}>", ty_text(&parts[0], depth + 1), list(&parts[1..])),
            (S::Tuple(parts), _, Some(rest)) if !parts.is_empty() => {
                let (tr, name) = rest.split_once('#').unwrap_or(("", rest));
                format!("<{} as {}<{}>>::{}", ty_text(&parts[0], depth + 1), last(tr), list(&parts[1..]), name.strip_prefix(NO_STD_ASSOC).unwrap_or(name))
            }
            _ => {
                let (name, tr) = split_trait_assoc(n);
                let name = name.strip_prefix(NO_STD_ASSOC).unwrap_or(name);
                match (tr, &**b) {
                    (Some(tr), b) => format!("<{} as {}>::{name}", ty_text(b, depth + 1), last(tr)),
                    (None, S::Named(_)) => format!("{}::{name}", ty_text(b, depth + 1)),
                    (None, b) => format!("<{}>::{name}", ty_text(b, depth + 1)),
                }
            }
        },
    }
}

/// Two impls whose heads are one type here only because the prelude makes a std type an alias of another: neither head
/// names a parameter, and they are written as two types, at least one through such an alias (stable_deref_trait's `impl
/// StableDeref for OsString` and `.. for String`, `.. for Cow<'a, Path>` and `.. for Cow<'a, OsStr>`). No conflict
/// (E0119) between them. A generic impl is compared as any other: `impl<T: Marker> Tr for T` with `impl Marker for
/// OsString` conflicts with `impl Tr for OsString`
fn two_written(p: &Program, a: &Clause, b: &Clause) -> bool {
    if a.alias.is_none() && b.alias.is_none() {
        return false;
    }
    let ground_head = |c: &Clause| c.self_ty.is_some_and(|t| !p.has_param(t)) && c.args.as_ref().is_some_and(|v| v.iter().all(|&t| !p.has_param(t)));
    a.alias != b.alias && ground_head(a) && ground_head(b)
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

/// An impl whose head has a fn pointer with a reference or a trait object: `Ty` keeps neither the one's
/// higher-rankedness (`for<'a> fn(&'a u8)` is not `fn(&'static u8)`, though both are one Ty) nor the other's lifetime
/// (`dyn Error + 'a`), so no conflict of it is reported
fn unknowable_head(p: &Program, c: &Clause) -> bool {
    c.self_ty.into_iter().chain(c.args.iter().flatten().copied()).any(|t| fn_ref(p, t, 0) || has_dyn(p, t, 0))
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
    // (the occurs check: x in t, through the bindings of the parameters t mentions, each binding visited once)
    fn occurs(p: &Program, t: Ty, x: u8, sol: &[Option<Ty>]) -> bool {
        let mut seen = vec![false; sol.len()];
        let mut todo = vec![t];
        while let Some(t) = todo.pop() {
            if mentions(p, t, x, 0) {
                return true;
            }
            for (j, b) in sol.iter().enumerate() {
                if let Some(b) = b {
                    if !seen[j] && mentions(p, t, j as u8, 0) {
                        seen[j] = true;
                        todo.push(*b);
                    }
                }
            }
        }
        false
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
            if (x as usize) >= sol.len() || t.is_unsized() {
                return None;
            }
            if occurs(p, t, x, sol) {
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
    let mut eqs = vec![];
    let mut eqs_read = true;
    if b.contains('=') {
        let bs = crate::ast_summary::assoc_bindings(b);
        // (as many bindings as `=` signs at the bound's own level: else one was not read)
        if bs.len() != b.matches('=').count() {
            eqs_read = false;
        }
        for (n, x) in bs {
            match p.resolve_g(&x, gn) {
                Ok(x) => eqs.push((n, x)),
                Err(_) => eqs_read = false,
            }
        }
    }
    Some(Pred { ty: t, tr, raw: b.to_string(), args, eqs, eqs_read })
}

/// A where clause's bound on a type that is not a bare parameter, as predicates (pub for the driver)
pub fn preds_of(p: &Program, t: Ty, bounds: &[&str], gn: &[&str]) -> Vec<Pred> {
    bounds.iter().filter_map(|b| pred_of(p, t, b, gn)).collect()
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
    /// how deep normalization is nested (an associated type defined by another's)
    norm_depth: u32,
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
            // (`dyn Tr<A = T>` is well formed only when T has the bounds Tr puts on A: `dyn X<Y = str>` with `type Y:
            // Clone` is no X)
            for part in d.split('+') {
                let part = part.trim();
                // (`dyn for<'x> X<'x, F = i32>`: the binder is not the trait)
                let part = match part.strip_prefix("for") {
                    Some(r) if r.trim_start().starts_with('<') => r[r.find('>').map_or(0, |i| i + 1)..].trim(),
                    _ => part,
                };
                let pn = part.split('<').next().unwrap_or(part).trim();
                if pn.rsplit("::").next() != Some(tr) || !part.contains('=') {
                    continue;
                }
                for (n, x) in crate::ast_summary::assoc_bindings(part) {
                    let Ok(x) = self.p.resolve_g(&x, &[]) else { continue };
                    for b in self.p.assoc_bounds.get(&(tr.to_string(), n.clone())).cloned().unwrap_or_default() {
                        if let Some(pr) = pred_of(self.p, x, &b, &[]) {
                            if self.solve_pred(fuel - 1, &pr) == Ans::No {
                                return Ans::No;
                            }
                        }
                    }
                }
            }
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
        let lenient = !self.strict && !EXPLICIT_ARGS.with(|e| e.get());
        if let Some(st) = c.self_ty {
            if !mtch(self.p, st, t, &mut sol, lenient, 0) {
                return Ans::No;
            }
        }
        // (arguments not matched (not read, or not as many): any. Then a `Yes` is a weakened one)
        let mut weak = !c.exact || !c.where_ok;
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
            // (the where clause under what the head bound, its associated types normalized: not ground when it names
            // a parameter the head does not bind, or a projection the solver cannot normalize)
            let Some(wt) = self.isubst(w.ty, sol, 0) else { return Ans::Overflow };
            let mut wp = w.clone();
            wp.ty = wt;
            if let Some(a) = &w.args {
                let mut v = Vec::new();
                for &x in a {
                    match self.isubst(x, sol, 0) {
                        Some(y) => v.push(y),
                        None => return Ans::Overflow,
                    }
                }
                wp.args = Some(v);
            }
            let mut eqs = vec![];
            for (n, x) in &w.eqs {
                match self.isubst(*x, sol, 0) {
                    Some(y) => eqs.push((n.clone(), y)),
                    None => return Ans::Overflow,
                }
            }
            wp.eqs = eqs;
            r = r.and(self.solve_pred(fuel - 1, &wp));
            if r == Ans::No {
                return r;
            }
        }
        r
    }

    /// A predicate: its trait, and the associated types it binds (`T: Tr<Assoc = X>` needs `<T as Tr>::Assoc` to
    /// normalize to X: `binding_no_sound` of Normalize.lean)
    fn solve_pred(&mut self, fuel: u32, w: &Pred) -> Ans {
        let mut r = self.solve(fuel, w.ty, &w.tr, &w.raw, w.args.as_deref());
        if r == Ans::No || w.eqs.is_empty() {
            return r;
        }
        if !self.db.user.contains(&w.tr) || !w.eqs_read {
            return if self.strict && r == Ans::Yes { Ans::Overflow } else { r };
        }
        for (n, x) in &w.eqs {
            match self.normalize(w.ty, n, Some(&w.tr)) {
                Some(u) if u == *x => {}
                Some(_) => return Ans::No,
                None => r = r.and(Ans::Overflow),
            }
        }
        r
    }

    /// `normalize` of Normalize.lean: the impls of tr (or of any program trait that defines `name`) whose head
    /// matches t exactly and whose where clauses hold (a strict `Yes`), and their definition of `name` under what
    /// matching found. One answer or none: two impls that give two (an incoherent program) or one zustc cannot decide
    /// leave it unknown
    fn normalize(&self, t: Ty, name: &str, tr: Option<&str>) -> Option<Ty> {
        if self.norm_depth > 8 || !ground(self.p, t, 0) {
            return None;
        }
        if tr.is_none() && self.db.other_assoc.contains(name) {
            return None;
        }
        let mut found: Option<Ty> = None;
        let mut trs: Vec<&String> = self.db.clauses.keys().filter(|k| tr.is_none_or(|t| k.as_str() == t)).collect();
        trs.sort();
        for k in trs {
            for c in &self.db.clauses[k] {
                let Some(&(_, v)) = c.assocs.iter().find(|a| a.0 == name).map(|a| a) else { continue };
                let mut sol: Vec<Option<Ty>> = vec![None; c.n];
                let Some(st) = c.self_ty else { return None };
                if !mtch(self.p, st, t, &mut sol, false, 0) {
                    continue;
                }
                // (an impl of the trait with arguments: another impl may be the one for other arguments)
                if c.args.as_ref().is_none_or(|a| !a.is_empty()) || !c.exact || !c.where_ok {
                    return None;
                }
                let mut sub = Cx { db: self.db, p: self.p, stack: vec![], steps: 0, strict: true, rigid_env: false, norm_depth: self.norm_depth + 1 };
                match sub.body(FUEL, c, &sol) {
                    Ans::Yes => {}
                    Ans::No => continue,
                    Ans::Overflow => return None,
                }
                let u = sub.isubst(v, &sol, 0)?;
                if found.is_some_and(|f| f != u) {
                    return None;
                }
                found = Some(u);
            }
        }
        found
    }

    /// t under the impl's parameters' solution, its associated types normalized by the solver (not by
    /// Program::assoc_of, which takes the first definition of the name): None when the result is not ground
    fn isubst(&self, t: Ty, sol: &[Option<Ty>], depth: u32) -> Option<Ty> {
        if depth > 32 {
            return None;
        }
        let r = match t {
            Ty::Param(i) => sol.get(i as usize).copied().flatten()?,
            Ty::Assoc(a) => {
                let b = self.isubst(a.base, sol, depth + 1)?;
                self.normalize(b, &a.name, None)?
            }
            Ty::Ref(m, i) => Ty::Ref(m, crate::types::intern(self.isubst(*i, sol, depth + 1)?)),
            Ty::Ptr(m, i) => Ty::Ptr(m, crate::types::intern(self.isubst(*i, sol, depth + 1)?)),
            Ty::Box(i) => Ty::Box(crate::types::intern(self.isubst(*i, sol, depth + 1)?)),
            Ty::Slice(i) => Ty::Slice(crate::types::intern(self.isubst(*i, sol, depth + 1)?)),
            Ty::Array(i, n) => {
                let n = if n >= CONST_LEN_BASE {
                    match sol.get((n - CONST_LEN_BASE) as usize).copied().flatten() {
                        Some(Ty::Array(_, m)) => m,
                        _ => return None,
                    }
                } else {
                    n
                };
                Ty::Array(crate::types::intern(self.isubst(*i, sol, depth + 1)?), n)
            }
            Ty::Fn(f) => {
                let mut ps = vec![];
                for &x in &f.params {
                    ps.push(self.isubst(x, sol, depth + 1)?);
                }
                let ret = self.isubst(f.ret, sol, depth + 1)?;
                Ty::Fn(crate::types::intern_fn(crate::types::FnTy { params: ps, ret }))
            }
            Ty::Struct(sid) if self.p.has_param(t) || has_assoc(self.p, t, 0) => {
                let sd = &self.p.structs[sid as usize];
                let base = sd.base?;
                let mut args = vec![];
                for &a in &sd.args.clone() {
                    args.push(self.isubst(a, sol, depth + 1)?);
                }
                Ty::Struct(self.p.instantiate(base, args).ok()?)
            }
            t => t,
        };
        ground(self.p, r, 0).then_some(r)
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
            // (an auto trait the object is written with, or one its trait gives: else not, as rustc's (it may hold any
            // type of its trait); a std trait zustc does not know the supertraits of is left open)
            Ty::Dyn(d) => {
                let gives: Vec<Option<bool>> = d.split('+').map(|b| self.db.gives_auto(self.p, b, tr)).collect();
                if gives.contains(&Some(true)) {
                    Ans::Yes
                } else if gives.contains(&None) {
                    Ans::Overflow
                } else {
                    Ans::No
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
                        // (std's atomics, AtomicPtr<T> too, are Send and Sync whatever they point to)
                        n if n.starts_with("Atomic") => Ans::Yes,
                        "Rc" => Ans::No,
                        "Cell" | "RefCell" | "OnceCell" if sync => Ans::No,
                        // (std's stdout guard holds a reentrant lock owned by its thread: not Send)
                        "StdoutLock" if !sync => Ans::No,
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
                // (a capture held by reference is a raw pointer: the reference it stands for. Both kinds are Sync when T
                // is; a `&mut T` (one the body mutates) is Send when T is, and one that may be either kind needs both)
                let mut fs: Vec<(Ty, bool)> = vec![];
                for (i, f) in sd.fields.iter().enumerate() {
                    match (f.1, self.p.ref_capture(sid, i)) {
                        (Ty::Ptr(_, inner), Some(true)) => fs.push((*inner, sync)),
                        (Ty::Ptr(_, inner), Some(false)) => {
                            fs.push((*inner, true));
                            if !sync {
                                fs.push((*inner, false));
                            }
                        }
                        _ => fs.push((f.1, sync)),
                    }
                }
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
        // (a Fn trait of a value that is never callable on stable Rust: a scalar, `()`, str, an array or slice, a struct
        // of the program's that is not a closure)
        if matches!(tr, "Fn" | "FnMut" | "FnOnce") {
            let never = t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Unit | Ty::Str | Ty::Array(..) | Ty::Slice(_))
                || matches!(t, Ty::Struct(x) if {
                    let b = self.p.base_of(x);
                    self.p.structs[x as usize].placeholder.is_none() && !self.p.closure_of_struct.contains_key(&b)
                        && self.p.struct_item.get(b as usize).is_some_and(|&i| i < self.p.prelude_from)
                });
            if never {
                return Ans::No;
            }
        }
        // (an unsized type (str, a slice, a trait object) is never Copy, Clone, Sized or Default: those need a value of
        // the type itself)
        if matches!(t, Ty::Str | Ty::Slice(_) | Ty::Dyn(_)) && matches!(tr, "Copy" | "Clone" | "Sized" | "Default") && raw.trim() == tr {
            return Ans::No;
        }
        // (`()`: std's impls for it, which `implements` does not know: all the derivable traits, not Display, nor an
        // iterator)
        if t == Ty::Unit && !raw.contains('<') && !raw.contains('(') {
            if STD_EXACT.contains(&tr) || matches!(tr, "Sized" | "Send" | "Sync" | "Iterator") {
                return if matches!(tr, "Display" | "Iterator") { Ans::No } else { Ans::Yes };
            }
        }
        // (not `()` below: implements knows few of std's impls for it (`(): Default`, `(): Debug` say no); scalars it judges
        // exactly, checked against rustc for each of these traits)
        let judged = t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char)
            || matches!(t, Ty::Struct(x) if self.p.structs[x as usize].base.is_none() && self.p.structs[x as usize].generics.is_empty() && self.p.structs[x as usize].placeholder.is_none());
        let exact = judged && !raw.contains('<') && !raw.contains('(')
            && (self.db.prelude.contains(tr) || matches!(tr, "Copy" | "Clone" | "Sized" | "Send" | "Sync" | "Debug" | "Default" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash"));
        // (the trait as `implements` knows it: `core::fmt::Debug` is `Debug`, its arguments kept)
        let raw_n = match raw.find('<') {
            Some(lt) => format!("{}{}", raw[..lt].rsplit("::").next().unwrap_or(&raw[..lt]).trim(), &raw[lt..]),
            None => raw.rsplit("::").next().unwrap_or(raw).trim().to_string(),
        };
        let raw = raw_n.as_str();
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

thread_local! {
    /// While a call with a turbofish is resolved (`require::<i32>()`): its types are written, not literals zustc
    /// defaulted, so the solver does not take an i32 / f64 for any integer / float
    pub static EXPLICIT_ARGS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
            // (a const argument zustc could not infer is `()` (`foo(Const)` with `impl<const N: usize> Foo<N> for
            // Const<N>`): it may be any const, the goal weakened as for a defaulted literal)
            // (and the first const value that meets it replaces it, so a where clause on the parameter sees that value)
            Some(Some(Ty::Unit)) if lenient && matches!(t, Ty::Array(Ty::Unit, _)) => {
                sol[i as usize] = Some(t);
                true
            }
            Some(Some(prev)) => prev == t || (lenient && matches!((prev, t), (Ty::Array(Ty::Unit, _), Ty::Unit))),
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

    #[test]
    fn display_names() {
        assert_eq!(super::display_name("a__b__Hidden"), "Hidden");
        assert_eq!(super::display_name("Vec<m__Foo>"), "Vec<Foo>");
        assert_eq!(super::display_name("__CowStr"), "__CowStr");
        assert_eq!(super::display_name("T#"), "T");
        assert_eq!(super::display_name("__u_Error"), "Error");
        assert_eq!(super::display_name("snake_case__Item"), "Item");
    }

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
