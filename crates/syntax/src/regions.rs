//! The regions of signatures and of struct / enum fields, as region slots (see `types::RSigSyn`). The types of the
//! summaries (`TySyn`) and of the program (`Ty`) carry no regions: the region checker reads these beside them.

use crate::parse::{Bound, FnSig as PSig, GArg, GParam, Generics, Path, Ty as PTy};
use crate::types::{RAdtSyn, RImplSyn, RSigSyn, R_BOUND, R_DYN_ARGS, R_DYN_DEFAULT, R_ELIDED, R_ELIDED_ADT, R_FN, R_STATIC, R_UNKNOWN};

const PRIMS: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str",
];

/// t names a lifetime (not `'_`) somewhere: the fast check before the work (most signatures name none)
pub fn names_lt(t: &PTy) -> bool {
    let named = |l: &str| l != "'_";
    match t {
        PTy::Ref(l, _, i) => l.as_deref().is_some_and(named) || names_lt(i),
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => names_lt(i),
        PTy::Tuple(ts) => ts.iter().any(names_lt),
        PTy::Path(q, p) => q.as_deref().is_some_and(names_lt) || path_names_lt(p),
        PTy::Fn(ps, r) => ps.iter().any(names_lt) || names_lt(r),
        PTy::Impl(bs) | PTy::Dyn(bs) => bounds_name_lt(bs),
        PTy::Never | PTy::Infer | PTy::Macro(_) => false,
    }
}

/// The source writes a lifetime in a path's generic arguments: in a turbofish (`S::<&'a u32>`,
/// `E::V::<Cell<&'static u32>>`) or before a `::` (`<A<'a>>::new`, `<A<'a> as Tr>::IC`). Only then do a body's
/// statements look for paths that write lifetimes (writes_path_lts): most programs have none
pub fn src_path_lts(src: &str) -> bool {
    // (from each `'`, within its statement: a `<` around it right after `::` (a turbofish), or a `>` around it right
    // before `::`, whitespace between them too (a turbofish formatted on several lines). Looking for the `'`s, which
    // are few, costs less than looking for every `::`)
    let b = src.as_bytes();
    let stop = |c: u8| matches!(c, b';' | b'{' | b'}');
    let after = |j: usize| -> bool {
        let mut k = j;
        while k < b.len() && b[k].is_ascii_whitespace() {
            k += 1;
        }
        b.get(k..k + 2) == Some(b"::")
    };
    let before = |j: usize| -> bool {
        let mut k = j;
        while k > 0 && b[k - 1].is_ascii_whitespace() {
            k -= 1;
        }
        k >= 2 && &b[k - 2..k] == b"::"
    };
    src.match_indices('\'').any(|(i, _)| {
        let mut d = 0i32;
        for j in (0..i).rev() {
            match b[j] {
                b'>' if j == 0 || b[j - 1] != b'-' => d += 1,
                b'<' if d == 0 => {
                    if before(j) {
                        return true;
                    }
                }
                b'<' => d -= 1,
                c if stop(c) => break,
                _ => {}
            }
        }
        let mut d = 0i32;
        for j in i + 1..b.len() {
            match b[j] {
                b'<' => d += 1,
                b'>' if b[j - 1] == b'-' => {}
                b'>' if d == 0 => {
                    if after(j + 1) {
                        return true;
                    }
                }
                b'>' => d -= 1,
                c if stop(c) => break,
                _ => {}
            }
        }
        false
    })
}

/// e calls or builds a constructor, calls a function or names a value through a path that writes a lifetime
/// (`combine(E::V(&c), E::U::<&'static u32>)`, `A::<'a>::new(&v)`): the region check of a function it left out is
/// turned on before e, so the borrows e makes before the path is lowered are followed
pub fn writes_path_lts(e: &crate::parse::Expr) -> bool {
    use crate::parse::Expr;
    let written = |q: &Option<Box<PTy>>, p: &crate::parse::Path| q.as_deref().is_some_and(names_lt) || (p.segs.iter().any(|s| !s.args.is_empty()) && path_names_lt(p));
    let here = match e {
        Expr::Path(q, p) => written(q, p),
        Expr::Struct(p, ..) => p.segs.iter().any(|s| !s.args.is_empty()) && path_names_lt(p),
        Expr::Call(f, _) => matches!(&**f, Expr::Path(q, p) if written(q, p)),
        // (a method call's turbofish: `q.take::<&'static u32>(&c)`)
        Expr::MethodCall(_, _, gargs, _, _) => gargs.iter().any(|a| match a {
            GArg::Lifetime(l) => l != "'_",
            GArg::Ty(t) | GArg::Bind(_, t) => names_lt(t),
            GArg::Const(_) => false,
        }),
        // (a closure's body is checked on its own)
        Expr::Closure(..) => return false,
        _ => false,
    };
    if here {
        return true;
    }
    let mut found = false;
    crate::ast_closure::each_child_pub(e, &mut |x| found = found || writes_path_lts(x));
    found
}

pub fn path_names_lt(p: &crate::parse::Path) -> bool {
    p.segs.iter().any(|s| {
        s.args.iter().any(|a| match a {
            GArg::Lifetime(l) => l != "'_",
            GArg::Ty(t) | GArg::Bind(_, t) => names_lt(t),
            GArg::Const(_) => false,
        }) || s.fn_args.as_ref().is_some_and(|(ps, r)| ps.iter().any(names_lt) || r.as_deref().is_some_and(names_lt))
    })
}

fn bounds_name_lt(bs: &[Bound]) -> bool {
    bs.iter().any(|b| match b {
        Bound::Lifetime(_) => true,
        Bound::Trait(_, p) => path_names_lt(p),
    })
}

/// t names the lifetime l anywhere (fn pointer, `dyn` and `impl` types too)
pub fn mentions_lt(t: &PTy, l: &str) -> bool {
    let in_path = |p: &crate::parse::Path| {
        p.segs.iter().any(|s| {
            s.args.iter().any(|a| match a {
                GArg::Ty(t) | GArg::Bind(_, t) => mentions_lt(t, l),
                GArg::Lifetime(x) => x == l,
                _ => false,
            }) || s.fn_args.as_ref().is_some_and(|(ps, r)| ps.iter().any(|t| mentions_lt(t, l)) || r.as_deref().is_some_and(|t| mentions_lt(t, l)))
        })
    };
    match t {
        PTy::Ref(x, _, i) => x.as_deref() == Some(l) || mentions_lt(i, l),
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => mentions_lt(i, l),
        PTy::Tuple(ts) => ts.iter().any(|t| mentions_lt(t, l)),
        PTy::Path(q, p) => q.as_deref().is_some_and(|q| mentions_lt(q, l)) || in_path(p),
        PTy::Fn(ps, r) => ps.iter().any(|t| mentions_lt(t, l)) || mentions_lt(r, l),
        PTy::Impl(bs) | PTy::Dyn(bs) => bs.iter().any(|b| match b {
            Bound::Lifetime(x) => x == l,
            Bound::Trait(_, p) => in_path(p),
        }),
        PTy::Never | PTy::Infer | PTy::Macro(_) => false,
    }
}

/// A `let` of e whose annotation writes `'static` (`let p: &'static mut usize = &mut self.food;`), not in a closure
/// in e (checked on its own)
pub fn annotates_static(e: &crate::parse::Expr) -> bool {
    use crate::parse::{Block, Expr, Stmt};
    // (a `let .. else`'s block too, which the walk of the children does not enter)
    fn block(b: &Block) -> bool {
        b.stmts.iter().any(|s| match s {
            Stmt::Let(_, t, _, els, ..) => t.as_ref().is_some_and(mentions_static) || els.as_ref().is_some_and(|b| block(b) || annotates_static(&Expr::Block(None, false, b.clone()))),
            _ => false,
        })
    }
    let here = match e {
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) | Expr::If(_, b, _) | Expr::While(_, _, b) | Expr::For(_, _, _, b) => block(b),
        Expr::Closure(..) => return false,
        _ => false,
    };
    if here {
        return true;
    }
    let mut found = false;
    crate::ast_closure::each_child_pub(e, &mut |x| found = found || annotates_static(x));
    found
}

/// A closure's body writes a lifetime the region check follows: a `let` annotation that names one
/// (`let y: &'static u32 = &c;`, `let y: &'a u32 = ..` of the function's 'a), a cast to a type that does, or a path's
/// or a method's turbofish (`W::<&'a u32>`, `s.take::<&'static u32>(..)`). Then the body is checked with the function's regions though nothing else asks for it
/// (no bound, annotated parameter or capture). In a nested closure too: what it requires of its parameters is
/// required of the calls the body makes of it (`let f = W::<&'a u32>; f(&c);`)
pub fn writes_lifetime(e: &crate::parse::Expr) -> bool {
    use crate::parse::{Block, Expr, Stmt};
    fn block(b: &Block) -> bool {
        b.stmts.iter().any(|s| match s {
            Stmt::Let(_, t, _, els, ..) => t.as_ref().is_some_and(names_lt) || els.as_ref().is_some_and(|b| block(b) || b.stmts.iter().any(|s| matches!(s, Stmt::Expr(x, _) if writes_lifetime(x)))),
            _ => false,
        })
    }
    let here = match e {
        Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) | Expr::If(_, b, _) | Expr::While(_, _, b) | Expr::For(_, _, _, b) => block(b),
        Expr::Path(None, p) => p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, GArg::Ty(t) if names_lt(t)) || matches!(a, GArg::Lifetime(l) if l != "'_"))),
        Expr::Cast(_, t) => names_lt(t),
        // (a method's turbofish: `S.take::<&'static u32>(&c)`)
        Expr::MethodCall(_, _, gs, _, _) => gs.iter().any(|a| matches!(a, GArg::Ty(t) if names_lt(t)) || matches!(a, GArg::Lifetime(l) if l != "'_")),
        _ => false,
    };
    if here {
        return true;
    }
    let mut found = false;
    crate::ast_closure::each_child_pub(e, &mut |x| found = found || writes_lifetime(x));
    found
}

/// The bounds of an `impl Trait` result capture no lifetime (before edition 2024): none named or elided in them, no
/// reference, no `dyn`, and a `use<..>` that lists none. Conservative: a bound it cannot read says it may capture one
pub fn captures_no_lifetime(bs: &[Bound]) -> bool {
    fn ty_ok(t: &PTy, depth: u32) -> bool {
        depth < 16
            && match t {
                PTy::Ref(..) | PTy::Dyn(_) | PTy::Impl(_) | PTy::Fn(..) | PTy::Macro(_) => false,
                PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => ty_ok(i, depth + 1),
                PTy::Tuple(ts) => ts.iter().all(|t| ty_ok(t, depth + 1)),
                PTy::Path(q, p) => q.is_none() && path_ok(p, depth),
                PTy::Never | PTy::Infer => true,
            }
    }
    fn path_ok(p: &crate::parse::Path, depth: u32) -> bool {
        p.segs.iter().all(|s| {
            s.fn_args.is_none()
                && s.args.iter().all(|a| match a {
                    GArg::Lifetime(_) => false,
                    GArg::Ty(t) | GArg::Bind(_, t) => ty_ok(t, depth + 1),
                    GArg::Const(_) => true,
                })
        })
    }
    if let Some(u) = bs.iter().find_map(|b| match b {
        Bound::Trait(_, p) if p.segs.len() == 1 && p.segs[0].name == "use" => Some(&p.segs[0]),
        _ => None,
    }) {
        return !u.args.iter().any(|a| matches!(a, GArg::Lifetime(_)));
    }
    bs.iter().all(|b| match b {
        Bound::Lifetime(_) => false,
        Bound::Trait(_, p) => path_ok(p, 0),
    })
}

/// t writes `'static` somewhere (outside fn pointer, `impl` and `dyn` types)
pub fn mentions_static(t: &PTy) -> bool {
    match t {
        PTy::Ref(l, _, i) => l.as_deref() == Some("'static") || mentions_static(i),
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => mentions_static(i),
        PTy::Tuple(ts) => ts.iter().any(mentions_static),
        PTy::Path(None, p) => path_mentions_static(p),
        PTy::Fn(ps, r) => ps.iter().any(mentions_static) || mentions_static(r),
        PTy::Dyn(bs) | PTy::Impl(bs) => bs.iter().any(|b| match b {
            Bound::Lifetime(l) => l == "'static",
            Bound::Trait(_, p) => path_mentions_static(p),
        }),
        _ => false,
    }
}

fn path_mentions_static(p: &crate::parse::Path) -> bool {
    p.segs.iter().any(|s| {
        s.args.iter().any(|a| match a {
            GArg::Lifetime(l) => l == "'static",
            GArg::Ty(t) | GArg::Bind(_, t) => mentions_static(t),
            _ => false,
        }) || s.fn_args.as_ref().is_some_and(|(ps, r)| ps.iter().any(mentions_static) || r.as_deref().is_some_and(mentions_static))
    })
}

/// Generics declare or bound with a lifetime
pub fn generics_name_lt(g: &Generics) -> bool {
    g.params.iter().any(|p| match p {
        GParam::Lifetime(_) => true,
        GParam::Ty(_, bs, d) => bounds_name_lt(bs) || d.as_ref().is_some_and(names_lt),
        GParam::Const(..) => false,
    }) || g.preds.iter().any(|(t, bs)| names_lt(t) || bounds_name_lt(bs))
}

/// A fn signature names a lifetime
pub fn sig_names_lt(sig: &PSig) -> bool {
    generics_name_lt(&sig.generics) || sig.recv_ty.as_ref().is_some_and(names_lt) || sig.params.iter().any(|p| names_lt(&p.1)) || sig.ret.as_ref().is_some_and(names_lt)
}

/// Collects the region slots of types against the lifetimes in scope
/// What the well-formedness of the self types of the projections in t implies (`<&'a &'b () as Tr>::Type`:
/// 'b: 'a), as (longer, shorter): a projection is normalized away, and with it its implied bounds
pub fn qself_outlives(t: &PTy, out: &mut Vec<(String, String)>, depth: u32) {
    if depth > 8 {
        return;
    }
    match t {
        PTy::Path(q, p) => {
            if let Some(q) = q {
                wf_outlives(q, out, depth + 1);
                qself_outlives(q, out, depth + 1);
            }
            for s in &p.segs {
                for a in &s.args {
                    if let crate::parse::GArg::Ty(a) = a {
                        qself_outlives(a, out, depth + 1);
                    }
                }
            }
        }
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => qself_outlives(i, out, depth + 1),
        PTy::Tuple(ts) => ts.iter().for_each(|t| qself_outlives(t, out, depth + 1)),
        _ => {}
    }
}

/// `&'x T` is well formed when every lifetime named in T outlives 'x
fn wf_outlives(t: &PTy, out: &mut Vec<(String, String)>, depth: u32) {
    if depth > 8 {
        return;
    }
    match t {
        PTy::Ref(Some(x), _, i) => {
            let mut inner = vec![];
            lifetimes_in(i, &mut inner);
            for y in inner {
                if y != *x && y != "'_" && x != "'_" && !out.contains(&(y.clone(), x.clone())) {
                    out.push((y, x.clone()));
                }
            }
            wf_outlives(i, out, depth + 1);
        }
        PTy::Ref(None, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => wf_outlives(i, out, depth + 1),
        PTy::Tuple(ts) => ts.iter().for_each(|t| wf_outlives(t, out, depth + 1)),
        _ => {}
    }
}

/// The lifetimes named in t
pub(crate) fn lifetimes_in(t: &PTy, out: &mut Vec<String>) {
    match t {
        PTy::Ref(l, _, i) => {
            if let Some(l) = l {
                out.push(l.clone());
            }
            lifetimes_in(i, out);
        }
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => lifetimes_in(i, out),
        PTy::Tuple(ts) => ts.iter().for_each(|t| lifetimes_in(t, out)),
        PTy::Path(q, p) => {
            if let Some(q) = q {
                lifetimes_in(q, out);
            }
            for s in &p.segs {
                for a in &s.args {
                    match a {
                        crate::parse::GArg::Ty(a) => lifetimes_in(a, out),
                        crate::parse::GArg::Lifetime(l) => out.push(l.clone()),
                        _ => {}
                    }
                }
            }
        }
        PTy::Dyn(bs) | PTy::Impl(bs) => {
            for b in bs {
                if let crate::parse::Bound::Lifetime(l) = b {
                    out.push(l.clone());
                }
            }
        }
        _ => {}
    }
}

/// t writes `'_` (outside fn pointer, `dyn` and `impl` types)
fn has_anon(t: &PTy) -> bool {
    match t {
        PTy::Ref(l, _, i) => l.as_deref() == Some("'_") || has_anon(i),
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => has_anon(i),
        PTy::Tuple(ts) => ts.iter().any(has_anon),
        PTy::Path(None, p) => p.segs.iter().any(|s| {
            s.args.iter().any(|a| match a {
                GArg::Lifetime(l) => l == "'_",
                GArg::Ty(t) => has_anon(t),
                _ => false,
            })
        }),
        _ => false,
    }
}

/// Names each `'_` written in an impl's self type (outside fn pointer, `dyn` and `impl` types): `'_0`, `'_1`, ..
fn name_anon(t: &mut PTy, out: &mut Vec<String>) {
    let fresh = |out: &mut Vec<String>| {
        let n = format!("'_{}", out.len());
        out.push(n.clone());
        n
    };
    match t {
        PTy::Ref(l, _, i) => {
            if l.as_deref() == Some("'_") {
                *l = Some(fresh(out));
            }
            name_anon(i, out)
        }
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => name_anon(i, out),
        PTy::Tuple(ts) => ts.iter_mut().for_each(|t| name_anon(t, out)),
        PTy::Path(None, p) => {
            for s in &mut p.segs {
                for a in &mut s.args {
                    match a {
                        GArg::Lifetime(l) if l == "'_" => *l = fresh(out),
                        GArg::Ty(t) => name_anon(t, out),
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
}

/// The lifetimes named anywhere in a path's arguments (with those of `Fn(..)` sugar)
fn path_lts(p: &crate::parse::Path, out: &mut Vec<String>) {
    fn ty(t: &PTy, out: &mut Vec<String>) {
        match t {
            PTy::Ref(l, _, i) => {
                out.extend(l.clone());
                ty(i, out)
            }
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => ty(i, out),
            PTy::Tuple(ts) => ts.iter().for_each(|t| ty(t, out)),
            PTy::Fn(ps, r) => {
                ps.iter().for_each(|t| ty(t, out));
                ty(r, out)
            }
            PTy::Path(q, p) => {
                if let Some(q) = q {
                    ty(q, out);
                }
                path_lts(p, out)
            }
            PTy::Dyn(bs) | PTy::Impl(bs) => {
                for b in bs {
                    match b {
                        Bound::Trait(_, p) => path_lts(p, out),
                        Bound::Lifetime(l) => out.push(l.clone()),
                    }
                }
            }
            PTy::Never | PTy::Infer | PTy::Macro(_) => {}
        }
    }
    for s in &p.segs {
        for a in &s.args {
            match a {
                GArg::Lifetime(l) => out.push(l.clone()),
                GArg::Ty(t) | GArg::Bind(_, t) => ty(t, out),
                GArg::Const(_) => {}
            }
        }
        if let Some((ps, r)) = &s.fn_args {
            ps.iter().for_each(|t| ty(t, out));
            if let Some(r) = r {
                ty(r, out);
            }
        }
    }
}

pub struct Slots<'a> {
    pub lts: Vec<String>,
    /// Type parameters in scope (they have no slots of their own: an instance substitutes them)
    pub tparams: Vec<&'a str>,
    /// The slots of `Self`
    pub self_slots: &'a [u8],
    /// The names of the types around each region of `Self` (RImplSyn::self_paths)
    pub self_paths: &'a [(String, u8)],
    /// A named lifetime (or `'static`) was seen: without one every region is elided, and the slots follow from the
    /// type alone, so the summary leaves them out
    pub any: bool,
    /// Inside a fn pointer's type: its elided regions and the lifetimes it binds are its own (R_BOUND)
    pub in_fn: u32,
    /// A fn signature's bounds: what a bound on a type implies of its parts goes here (RSigSyn::derived); None for
    /// the others, which keep the mark only
    pub derived: Option<(Vec<(String, u8, u8)>, Vec<(String, String, u8)>)>,
}

impl<'a> Slots<'a> {
    pub fn new(g: &'a Generics, outer: Option<&'a RImplSyn>, outer_tparams: &[&'a str]) -> Slots<'a> {
        let mut lts: Vec<String> = outer.map(|o| o.lts.clone()).unwrap_or_default();
        let mut tparams: Vec<&'a str> = outer_tparams.to_vec();
        for p in &g.params {
            match p {
                GParam::Lifetime(l) => lts.push(l.clone()),
                GParam::Ty(n, _, _) => tparams.push(n),
                GParam::Const(..) => {}
            }
        }
        Slots { any: !lts.is_empty(), lts, tparams, self_slots: outer.map(|o| o.self_slots.as_slice()).unwrap_or(&[]), self_paths: outer.map(|o| o.self_paths.as_slice()).unwrap_or(&[]), in_fn: 0, derived: None }
    }

    /// The outlives relations the well-formedness of the self types of projections in t implies
    pub fn qself_implied(&mut self, t: &PTy, out: &mut Vec<(u8, u8)>) {
        let mut pairs = vec![];
        qself_outlives(t, &mut pairs, 0);
        for (a, b) in pairs {
            let (a, b) = (self.code(Some(&a)), self.code(Some(&b)));
            if a < R_STATIC && b < R_STATIC && a != b && !out.contains(&(a, b)) {
                out.push((a, b));
            }
        }
    }

    pub fn code(&mut self, l: Option<&str>) -> u8 {
        match l {
            None | Some("'_") if self.in_fn > 0 => R_BOUND,
            Some(l) if self.in_fn > 0 && l != "'static" && !self.lts.iter().any(|x| x == l) => R_BOUND,
            None | Some("'_") => R_ELIDED,
            Some("'static") => {
                self.any = true;
                R_STATIC
            }
            Some(l) => {
                self.any = true;
                match self.lts.iter().position(|x| x == l) {
                    Some(i) if i < R_STATIC as usize => i as u8,
                    _ => R_UNKNOWN,
                }
            }
        }
    }

    /// The slots of t. `arg`: t is a parameter's type (`impl Trait` there is a type parameter)
    pub fn slots(&mut self, t: &PTy, arg: bool, out: &mut Vec<u8>) {
        match t {
            PTy::Ref(l, _, i) => {
                out.push(self.code(l.as_deref()));
                self.slots(i, arg, out);
            }
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => self.slots(i, arg, out),
            PTy::Tuple(ts) => {
                for t in ts {
                    self.slots(t, arg, out);
                }
            }
            PTy::Path(None, p) => {
                if let [s] = p.segs.as_slice() {
                    if s.args.is_empty() && s.fn_args.is_none() {
                        let n = s.name.as_str();
                        if n == "Self" {
                            // (an explicit region of the impl's self type (`impl Tr for &'static S`) is kept)
                            if self.self_slots.iter().any(|&c| c != R_ELIDED && c != R_ELIDED_ADT) {
                                self.any = true;
                            }
                            out.extend_from_slice(self.self_slots);
                            return;
                        }
                        if self.tparams.contains(&n) || PRIMS.contains(&n) {
                            return;
                        }
                    }
                }
                if p.segs.iter().any(|s| s.fn_args.is_some()) {
                    out.push(R_UNKNOWN);
                    return;
                }
                let last = p.segs.last();
                let lts: Vec<&String> = last.map(|s| s.args.iter().filter_map(|a| if let GArg::Lifetime(l) = a { Some(l) } else { None }).collect()).unwrap_or_default();
                if lts.is_empty() {
                    out.push(R_ELIDED_ADT);
                } else {
                    for l in lts {
                        let c = self.code(Some(l));
                        out.push(c);
                    }
                }
                for s in &p.segs {
                    for a in &s.args {
                        match a {
                            GArg::Ty(t) => self.slots(t, false, out),
                            GArg::Bind(..) => out.push(R_UNKNOWN),
                            GArg::Lifetime(_) | GArg::Const(_) => {}
                        }
                    }
                }
            }
            // (`impl Trait` as a parameter's type is a type parameter of its own)
            PTy::Impl(_) if arg => {}
            // (a trait object's lifetime: the one written as a bound, else the default; the regions in the trait's
            // own arguments are not followed)
            PTy::Dyn(bs) => {
                let code = match bs.iter().find_map(|b| if let Bound::Lifetime(l) = b { Some(l.clone()) } else { None }) {
                    // (`dyn Tr + '_` is not the default (`Box<dyn Tr>` is 'static): the signature says something)
                    Some(l) if l == "'_" && self.in_fn == 0 => {
                        self.any = true;
                        R_ELIDED
                    }
                    Some(l) => self.code(Some(&l)),
                    None => R_DYN_DEFAULT,
                };
                out.push(code);
                // (the lifetimes in scope its trait's arguments name)
                let mut named = vec![];
                for b in bs {
                    if let Bound::Trait(_, p) = b {
                        path_lts(p, &mut named);
                    }
                }
                let mut codes: Vec<u8> = vec![];
                for l in &named {
                    if let Some(i) = self.lts.iter().position(|x| x == l).filter(|&i| i < R_STATIC as usize) {
                        if !codes.contains(&(i as u8)) {
                            codes.push(i as u8);
                        }
                    }
                }
                if !codes.is_empty() && codes.len() < 16 {
                    self.any = true;
                    out.push(R_DYN_ARGS);
                    out.push(codes.len() as u8);
                    out.extend(codes);
                }
            }
            // (a fn pointer: its parameters' regions, then its result's; those it binds are its own)
            PTy::Fn(ps, r) => {
                out.push(R_FN);
                self.in_fn += 1;
                for p in ps {
                    self.slots(p, false, out);
                }
                self.slots(r, false, out);
                self.in_fn -= 1;
            }
            PTy::Path(Some(_), _) | PTy::Impl(_) | PTy::Infer | PTy::Macro(_) => out.push(R_UNKNOWN),
            PTy::Never => {}
        }
    }

    /// Declared outlives relations of generics: `'a: 'b` and `T: 'a`, inline or in the where clause
    pub fn bounds(&mut self, g: &Generics, outlives: &mut Vec<(u8, u8)>, ty_outlives: &mut Vec<(String, u8)>) {
        let mut add = |me: &mut Self, lhs: &PTy, bs: &[Bound]| {
            // (a type parameter's associated type, `T::Output: 'b` or `<T as Tr<'a>>::Output: 'b`: by the name
            // `T::Output`, the trait's arguments aside (a placeholder's associated type outlives it, regionck))
            let proj = match lhs {
                PTy::Path(None, p) => match p.segs.as_slice() {
                    [t, n] if t.args.is_empty() && n.args.is_empty() && !t.name.starts_with('\'') => Some(format!("{}::{}", t.name, n.name)),
                    _ => None,
                },
                PTy::Path(Some(q), p) => match (&**q, p.segs.last()) {
                    (PTy::Path(None, qp), Some(n)) if qp.segs.len() == 1 && qp.segs[0].args.is_empty() && n.args.is_empty() => Some(format!("{}::{}", qp.segs[0].name, n.name)),
                    _ => None,
                },
                _ => None,
            };
            if let Some(name) = proj {
                for b in bs {
                    if let Bound::Lifetime(l) = b {
                        let c = me.code(Some(l));
                        ty_outlives.push((name.clone(), c));
                    }
                }
                return;
            }
            // (another type bounded by a lifetime (`StrategyFor<A>: 'static` of a type alias): the mark `#?`, as what
            // the function may assume is not all read. Its parts outlive the bound too, as rustc's outlives components
            // do: `D<'a>: 'b` gives `'a: 'b`, `E<'a, T>: 'b` also `T: 'b`)
            let plain = matches!(lhs, PTy::Path(None, p) if matches!(p.segs.as_slice(), [s] if s.args.is_empty() || s.name.starts_with('\'')));
            if !plain {
                let (mut lts, mut tys) = (vec![], vec![]);
                if me.derived.is_some() {
                    let tps = me.tparams.clone();
                    ty_parts(lhs, "", &tps, &mut lts, &mut tys);
                }
                for b in bs {
                    if let Bound::Lifetime(l) = b {
                        let c = me.code(Some(l));
                        ty_outlives.push(("#?".to_string(), c));
                        let mut found = vec![];
                        for (paths, a) in &lts {
                            let a = me.code(Some(a));
                            if a < R_STATIC && c <= R_STATIC && a != c {
                                found.push((paths.clone(), a, c));
                            }
                        }
                        let found_ty: Vec<(String, String, u8)> = tys.iter().filter(|t| me.tparams.contains(&t.1.as_str())).map(|(paths, t)| (paths.clone(), t.clone(), c)).collect();
                        if let Some((d, dt)) = me.derived.as_mut() {
                            d.extend(found);
                            dt.extend(found_ty);
                        }
                    }
                }
                return;
            }
            let PTy::Path(None, p) = lhs else { return };
            let [s] = p.segs.as_slice() else { return };
            for b in bs {
                let Bound::Lifetime(l) = b else { continue };
                let c = me.code(Some(l));
                // (`Self: 'b` in an impl's method: the lifetimes of the impl's self type outlive 'b. Kept apart, with the
                // names of the types around each (self_paths): the program adds those where none is a type alias,
                // which may drop the lifetime (`impl<'a> Tr for Erased<'a>` with `type Erased<'a> = ();`). The impl's
                // type parameters are not among them: no type parameter has lifetime arguments)
                if s.name == "Self" && c <= R_STATIC && me.derived.is_some() {
                    let ss = me.self_slots;
                    let mut found = vec![];
                    let mut i = 0;
                    while i < ss.len() {
                        let (from, to) = if ss[i] == R_DYN_ARGS { (i + 2, (i + 2 + ss.get(i + 1).copied().unwrap_or(0) as usize).min(ss.len())) } else { (i, i + 1) };
                        for &a in &ss[from..to] {
                            if a < R_STATIC && a != c {
                                // (a named region the paths do not record (a type nested past lt_paths' depth):
                                // nothing, as what is around it is not known)
                                for (paths, _) in me.self_paths.iter().filter(|x| x.1 == a) {
                                    if !found.contains(&(paths.clone(), a, c)) {
                                        found.push((paths.clone(), a, c));
                                    }
                                }
                            }
                        }
                        i = to.max(i + 1);
                    }
                    if let Some((d, _)) = me.derived.as_mut() {
                        d.extend(found);
                    }
                }
                if s.name.starts_with('\'') {
                    let a = me.code(Some(&s.name));
                    outlives.push((a, c));
                } else if s.args.is_empty() {
                    ty_outlives.push((s.name.clone(), c));
                }
            }
        };
        for p in &g.params {
            if let GParam::Ty(n, bs, _) = p {
                let lhs = PTy::Path(None, crate::parse::Path { global: false, segs: vec![crate::parse::Seg { name: n.clone(), args: vec![], fn_args: None, text: String::new() }], pos: Default::default() });
                add(self, &lhs, bs);
            }
        }
        for (t, bs) in &g.preds {
            add(self, t, bs);
        }
    }
}

/// The lifetimes and the plain type names (a parameter's, maybe) that a type is made of, for the outlives its bound
/// implies, each with the names of the paths it is in (comma-separated: one of them may be a type alias, which the
/// program knows). Not through a projection (`<T as Tr>::X: 'b` says nothing of T, nor `T::A<'a>: 'b` of 'a: it is one
/// component of its own) or a trait object's bounds
/// The named lifetimes of a type, each with the names of the types around it (`D<'a, Erased<'b>>`: ("D", 'a),
/// ("D,Erased", 'b)), also those of a trait object (its bounds and its traits' arguments)
fn lt_paths(t: &PTy, paths: &str, out: &mut Vec<(String, String)>, d: u32) {
    if d > 32 {
        return;
    }
    let seg_args = |segs: &[crate::parse::Seg], inner: &str, out: &mut Vec<(String, String)>| {
        for s in segs {
            for a in &s.args {
                match a {
                    GArg::Lifetime(l) => out.push((inner.to_string(), l.clone())),
                    GArg::Ty(t) | GArg::Bind(_, t) => lt_paths(t, inner, out, d + 1),
                    _ => {}
                }
            }
            if let Some((ps, r)) = &s.fn_args {
                for t in ps.iter().chain(r.as_deref()) {
                    lt_paths(t, inner, out, d + 1);
                }
            }
        }
    };
    match t {
        PTy::Path(q, p) => {
            if let Some(q) = q {
                lt_paths(q, paths, out, d + 1);
            }
            let Some(last) = p.segs.last() else { return };
            let inner = if paths.is_empty() { last.name.clone() } else { format!("{paths},{}", last.name) };
            seg_args(&p.segs, &inner, out);
        }
        PTy::Ref(l, _, t) => {
            if let Some(l) = l {
                out.push((paths.to_string(), l.clone()));
            }
            lt_paths(t, paths, out, d + 1);
        }
        PTy::Ptr(_, t) | PTy::Slice(t) | PTy::Array(t, _) => lt_paths(t, paths, out, d + 1),
        PTy::Tuple(ts) => {
            for t in ts {
                lt_paths(t, paths, out, d + 1);
            }
        }
        PTy::Dyn(bs) | PTy::Impl(bs) => {
            for b in bs {
                match b {
                    Bound::Lifetime(l) => out.push((paths.to_string(), l.clone())),
                    Bound::Trait(_, p) => {
                        let Some(last) = p.segs.last() else { continue };
                        let inner = if paths.is_empty() { last.name.clone() } else { format!("{paths},{}", last.name) };
                        seg_args(&p.segs, &inner, out);
                    }
                }
            }
        }
        PTy::Fn(ps, r) => {
            for t in ps.iter().chain(std::iter::once(&**r)) {
                lt_paths(t, paths, out, d + 1);
            }
        }
        _ => {}
    }
}

fn ty_parts(t: &PTy, paths: &str, tps: &[&str], lts: &mut Vec<(String, String)>, tys: &mut Vec<(String, String)>) {
    match t {
        PTy::Path(None, p) if p.segs.len() > 1 && (p.segs[0].name == "Self" || tps.contains(&p.segs[0].name.as_str())) => {}
        PTy::Path(None, p) => {
            if let [s] = p.segs.as_slice() {
                if s.args.is_empty() && s.fn_args.is_none() && !s.name.starts_with('\'') {
                    tys.push((paths.to_string(), s.name.clone()));
                }
            }
            let Some(last) = p.segs.last() else { return };
            let inner = if paths.is_empty() { last.name.clone() } else { format!("{paths},{}", last.name) };
            for s in &p.segs {
                for a in &s.args {
                    match a {
                        GArg::Lifetime(l) if l != "'_" && l != "'static" => lts.push((inner.clone(), l.clone())),
                        GArg::Ty(t) | GArg::Bind(_, t) => ty_parts(t, &inner, tps, lts, tys),
                        _ => {}
                    }
                }
                if let Some((ps, r)) = &s.fn_args {
                    for t in ps.iter().chain(r.as_deref()) {
                        ty_parts(t, &inner, tps, lts, tys);
                    }
                }
            }
        }
        PTy::Ref(l, _, t) => {
            if let Some(l) = l.as_ref().filter(|l| *l != "'_" && *l != "'static") {
                lts.push((paths.to_string(), l.clone()));
            }
            ty_parts(t, paths, tps, lts, tys);
        }
        PTy::Ptr(_, t) | PTy::Slice(t) | PTy::Array(t, _) => ty_parts(t, paths, tps, lts, tys),
        PTy::Tuple(ts) => {
            for t in ts {
                ty_parts(t, paths, tps, lts, tys);
            }
        }
        _ => {}
    }
}

/// The regions of a fn signature (None when it names no lifetime: then every region in it is elided)
/// impl_lts: the lifetime bounds of the parameters' `impl Trait`s, by the type parameters they are lifted to
/// (`impl Debug + 'static`: (`__implN`, `'static`)), which a caller must meet (E0310)
pub fn sig_regions(sig: &PSig, outer: Option<&RImplSyn>, outer_tparams: &[&str], impl_lts: &[(String, String)]) -> Option<Box<RSigSyn>> {
    if outer.is_none() && !sig_names_lt(sig) {
        return None;
    }
    let mut s = Slots::new(&sig.generics, outer, outer_tparams);
    let mut slots = vec![];
    if let Some(r) = &sig.recv_ty {
        let mut v = vec![];
        s.slots(r, true, &mut v);
        slots.push(v);
    }
    for (_, t) in &sig.params {
        let mut v = vec![];
        s.slots(t, true, &mut v);
        slots.push(v);
    }
    let mut v = vec![];
    if let Some(r) = &sig.ret {
        s.slots(r, false, &mut v);
    }
    slots.push(v);
    let (mut outlives, mut ty_outlives) = (outer.map(|o| o.outlives.clone()).unwrap_or_default(), outer.map(|o| o.ty_outlives.clone()).unwrap_or_default());
    s.derived = Some((vec![], vec![]));
    s.bounds(&sig.generics, &mut outlives, &mut ty_outlives);
    for (g, l) in impl_lts {
        let c = s.code(Some(l));
        ty_outlives.push((g.clone(), c));
    }
    let (derived, derived_ty) = s.derived.take().unwrap_or_default();
    // (a parameter's type that is a projection (`<&'a &'b () as Tr>::Type`) is normalized away, and with it what its
    // well-formedness implies ('b: 'a): kept as declared bounds)
    for (_, t) in &sig.params {
        s.qself_implied(t, &mut outlives);
    }
    // (or a self type that writes 'static: `impl A<&'static u8>`)
    let self_static = outer.is_some_and(|o| o.self_slots.contains(&R_STATIC));
    if !s.any && outlives.is_empty() && ty_outlives.is_empty() && !self_static && derived.is_empty() && derived_ty.is_empty() {
        return None;
    }
    let self_slots = outer.map(|o| o.self_slots.clone()).filter(|v| v.iter().any(|&c| c <= R_STATIC)).unwrap_or_default();
    let outer = outer.map_or(0, |o| o.lts.len().min(u8::MAX as usize) as u8);
    Some(Box::new(RSigSyn { lts: s.lts, slots, outlives, ty_outlives, outer, self_slots, derived, derived_ty }))
}

/// The regions of a struct / enum definition with fields of types tys (all variants' in order)
pub fn adt_regions(g: &Generics, tys: &[&PTy]) -> Option<Box<RAdtSyn>> {
    if !generics_name_lt(g) && !tys.iter().any(|t| names_lt(t)) {
        return None;
    }
    let mut s = Slots::new(g, None, &[]);
    let mut fields = vec![];
    for t in tys {
        let mut v = vec![];
        s.slots(t, false, &mut v);
        fields.push(v);
    }
    let (mut outlives, mut ty_outlives) = (vec![], vec![]);
    s.bounds(g, &mut outlives, &mut ty_outlives);
    if !s.any && outlives.is_empty() && ty_outlives.is_empty() {
        return None;
    }
    Some(Box::new(RAdtSyn { lts: s.lts, fields, outlives, ty_outlives }))
}

/// Whether t has a trait object with no lifetime bound that is not behind a reference (behind: t is a reference's
/// referent)
fn bare_dyn(t: &PTy, behind: bool) -> bool {
    match t {
        // (not one whose trait takes lifetimes (`dyn Obj<'a>`): the trait's own bound, `trait Obj<'a>: 'a`, may be
        // its default, which the header does not see)
        PTy::Dyn(bs) => {
            !behind
                && !bs.iter().any(|b| matches!(b, Bound::Lifetime(_)))
                && !bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, GArg::Lifetime(_))))))
        }
        PTy::Ref(_, _, i) => bare_dyn(i, true),
        // (not one among a path's arguments: the type's parameter may take a lifetime bound that is its default
        // (`struct W<'a, T: ?Sized + 'a>`, `impl<'a> W<'a, dyn Obj>`), and the header cannot tell a std type
        // (`Box<dyn Tr>`, 'static) from a user's of the same name)
        _ => false,
    }
}

/// t with `+ 'static` on each trait object bare_dyn finds
fn static_dyn(t: &mut PTy, behind: bool) {
    match t {
        PTy::Dyn(_) => {
            if bare_dyn(t, behind) {
                if let PTy::Dyn(bs) = t {
                    bs.push(Bound::Lifetime("'static".to_string()));
                }
            }
        }
        PTy::Ref(_, _, i) => static_dyn(i, true),
        _ => {}
    }
}

/// The regions of a trait impl's head (RHeadSyn): None where it names no lifetime, binds none in a fn pointer type and
/// declares no outlives, so that its regions are all its own
pub fn impl_head(g: &Generics, self_ty: &PTy, tr: &Path) -> Option<Box<crate::types::RHeadSyn>> {
    // (`impl Tr for S<'_>`: each `'_` of the self type is a lifetime parameter of the impl of its own)
    let mut anon = vec![];
    let named = has_anon(self_ty).then(|| {
        let mut t = self_ty.clone();
        name_anon(&mut t, &mut anon);
        t
    });
    let self_ty = named.as_ref().unwrap_or(self_ty);
    let mut s = Slots::new(g, None, &[]);
    s.lts.extend(anon);
    let mut self_slots = vec![];
    s.slots(self_ty, false, &mut self_slots);
    let last = tr.segs.last()?;
    let mut trait_lts = vec![];
    let mut trait_args = vec![];
    for a in &last.args {
        match a {
            GArg::Lifetime(l) => trait_lts.push(s.code(Some(l))),
            GArg::Ty(t) => {
                let mut c = vec![];
                s.slots(t, false, &mut c);
                trait_args.push(c);
            }
            _ => {}
        }
    }
    // (`Fn(&u8)`-style arguments: not a trait of the program's, which this is for)
    if last.fn_args.is_some() {
        return None;
    }
    let (mut outlives, mut ty_outlives) = (vec![], vec![]);
    s.bounds(g, &mut outlives, &mut ty_outlives);
    let bound = self_slots.iter().chain(trait_args.iter().flatten()).any(|&c| c == R_BOUND);
    if !s.any && !bound && outlives.is_empty() {
        return None;
    }
    Some(Box::new(crate::types::RHeadSyn { lts: s.lts, self_slots, trait_lts, trait_args, outlives }))
}

/// The regions of an impl / trait header
pub fn hdr_regions(g: &Generics, self_ty: Option<&PTy>) -> Option<Box<RImplSyn>> {
    // (the self type a trait object, `impl dyn Tr` / `impl X for dyn Tr`, is `dyn Tr + 'static`: its default is
    // decided by the header, not by the `&self` that later holds Self)
    let static_obj = self_ty.filter(|t| bare_dyn(t, false)).map(|t| {
        let mut t = t.clone();
        static_dyn(&mut t, false);
        t
    });
    let self_ty = static_obj.as_ref().or(self_ty);
    // (`impl S<'_>`: each `'_` in the self type is a lifetime parameter of the impl of its own, named `'_0`, `'_1`, ..)
    let mut anon = vec![];
    let named = self_ty.filter(|t| has_anon(t)).map(|t| {
        let mut t = t.clone();
        name_anon(&mut t, &mut anon);
        t
    });
    let self_ty = named.as_ref().or(self_ty);
    if !generics_name_lt(g) && !self_ty.is_some_and(names_lt) {
        return None;
    }
    let mut s = Slots::new(g, None, &[]);
    s.lts.extend(anon);
    let mut self_slots = vec![];
    if let Some(t) = self_ty {
        s.slots(t, false, &mut self_slots);
    }
    let (mut outlives, mut ty_outlives) = (vec![], vec![]);
    s.bounds(g, &mut outlives, &mut ty_outlives);
    if !s.any && outlives.is_empty() && ty_outlives.is_empty() {
        return None;
    }
    let mut self_paths = vec![];
    if let Some(t) = self_ty {
        let mut named = vec![];
        lt_paths(t, "", &mut named, 0);
        for (paths, l) in named {
            let c = s.code(Some(&l));
            if c < R_STATIC && !self_paths.contains(&(paths.clone(), c)) {
                self_paths.push((paths, c));
            }
        }
    }
    Some(Box::new(RImplSyn { lts: s.lts, self_slots, outlives, ty_outlives, free: vec![], wf_pairs: vec![], wf_vague: vec![], trait_lts: vec![], self_paths }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_file, ItemKind};

    fn sig_of(src: &str) -> Option<RSigSyn> {
        sig_of_b(src).map(|b| *b)
    }

    fn sig_of_b(src: &str) -> Option<Box<RSigSyn>> {
        let f = parse_file(src).unwrap();
        let mut hdr = None;
        for it in &f.items {
            match &it.kind {
                ItemKind::Fn(fs, _) => return sig_regions(fs, hdr.as_deref(), &[], &[]),
                ItemKind::Impl(g, _, t, items) => {
                    hdr = hdr_regions(g, Some(t));
                    for m in items {
                        if let ItemKind::Fn(fs, _) = &m.kind {
                            return sig_regions(fs, hdr.as_deref(), &[], &[]);
                        }
                    }
                }
                _ => {}
            }
        }
        panic!("no fn")
    }

    #[test]
    fn slots_of_signatures() {
        let r = sig_of("fn f<'a, 'b: 'a>(x: &'a u32, y: &'b mut Vec<&'a u8>) -> &'a u32 { x }").unwrap();
        assert_eq!(r.lts, ["'a", "'b"]);
        assert_eq!(r.slots, vec![vec![0], vec![1, R_ELIDED_ADT, 0], vec![0]]);
        assert_eq!(r.outlives, vec![(1, 0)]);
        // (a lifetime written only in the return type changes the regions, so the interface's fingerprint)
        let s = sig_of("fn f<'a, 'b: 'a>(x: &'a u32, y: &'b mut Vec<&'a u8>) -> &'static u32 { x }").unwrap();
        assert_ne!(r, s);
        assert_eq!(sig_of("fn g(x: u64, v: Vec<u8>) -> u64 { x }"), None);
        let t = sig_of("fn h<T: 'static>(x: &T, t: (T, &'static str)) where T: Clone {}").unwrap();
        assert_eq!(t.slots, vec![vec![R_ELIDED], vec![R_STATIC], vec![]]);
        assert_eq!(t.ty_outlives, vec![("T".to_string(), R_STATIC)]);
        assert_eq!(sig_of("fn k(f: &dyn Fn(u8)) {}"), None);
        assert_eq!(sig_of("fn k<'a>(f: &'a dyn Fn(u8)) {}").unwrap().slots[0], vec![0, R_DYN_DEFAULT]);
        assert_eq!(sig_of("fn k<'a>(f: Box<dyn Fn(u8) + 'a>) {}").unwrap().slots[0], vec![R_ELIDED_ADT, 0]);
    }

    #[test]
    fn slots_in_impls() {
        let r = sig_of("impl<'a> Parser<'a> { fn next(&mut self, s: &Self) -> Tok<'a> { todo!() } }").unwrap();
        assert_eq!(r.lts, ["'a"]);
        assert_eq!(r.slots, vec![vec![R_ELIDED, 0], vec![R_ELIDED, 0], vec![0]]);
    }

    #[test]
    fn slots_of_adts() {
        let f = parse_file("struct P<'a, T: 'a> { s: &'a str, t: &'a mut T, v: Vec<Tok<'a>>, n: u32 }").unwrap();
        let ItemKind::Struct(_, g, crate::parse::Fields::Named(fs)) = &f.items[0].kind else { panic!() };
        let tys: Vec<&PTy> = fs.iter().map(|x| &x.1).collect();
        let r = *adt_regions(g, &tys).unwrap();
        assert_eq!(r.lts, ["'a"]);
        assert_eq!(r.fields, vec![vec![0], vec![0], vec![R_ELIDED_ADT, 0], vec![]]);
        assert_eq!(r.ty_outlives, vec![("T".to_string(), 0)]);
        let f = parse_file("struct Q<T> { v: Vec<T> }").unwrap();
        let ItemKind::Struct(_, g, crate::parse::Fields::Named(fs)) = &f.items[0].kind else { panic!() };
        assert_eq!(adt_regions(g, &[&fs[0].1]), None);
    }
}
