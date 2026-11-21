//! The regions of signatures and of struct / enum fields, as region slots (see `types::RSigSyn`). The types of the
//! summaries (`TySyn`) and of the program (`Ty`) carry no regions: the region checker reads these beside them.

use crate::parse::{Bound, FnSig as PSig, GArg, GParam, Generics, Ty as PTy};
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
fn lifetimes_in(t: &PTy, out: &mut Vec<String>) {
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
    /// A named lifetime (or `'static`) was seen: without one every region is elided, and the slots follow from the
    /// type alone, so the summary leaves them out
    pub any: bool,
    /// Inside a fn pointer's type: its elided regions and the lifetimes it binds are its own (R_BOUND)
    pub in_fn: u32,
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
        Slots { any: !lts.is_empty(), lts, tparams, self_slots: outer.map(|o| o.self_slots.as_slice()).unwrap_or(&[]), in_fn: 0 }
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
            let PTy::Path(None, p) = lhs else { return };
            let [s] = p.segs.as_slice() else { return };
            for b in bs {
                let Bound::Lifetime(l) = b else { continue };
                let c = me.code(Some(l));
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

/// The regions of a fn signature (None when it names no lifetime: then every region in it is elided)
pub fn sig_regions(sig: &PSig, outer: Option<&RImplSyn>, outer_tparams: &[&str]) -> Option<Box<RSigSyn>> {
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
    s.bounds(&sig.generics, &mut outlives, &mut ty_outlives);
    // (a parameter's type that is a projection (`<&'a &'b () as Tr>::Type`) is normalized away, and with it what its
    // well-formedness implies ('b: 'a): kept as declared bounds)
    for (_, t) in &sig.params {
        s.qself_implied(t, &mut outlives);
    }
    // (or a self type that writes 'static: `impl A<&'static u8>`)
    let self_static = outer.is_some_and(|o| o.self_slots.contains(&R_STATIC));
    if !s.any && outlives.is_empty() && ty_outlives.is_empty() && !self_static {
        return None;
    }
    let self_slots = outer.map(|o| o.self_slots.clone()).filter(|v| v.iter().any(|&c| c <= R_STATIC)).unwrap_or_default();
    let outer = outer.map_or(0, |o| o.lts.len().min(u8::MAX as usize) as u8);
    Some(Box::new(RSigSyn { lts: s.lts, slots, outlives, ty_outlives, outer, self_slots }))
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

/// The regions of an impl / trait header
pub fn hdr_regions(g: &Generics, self_ty: Option<&PTy>) -> Option<Box<RImplSyn>> {
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
    Some(Box::new(RImplSyn { lts: s.lts, self_slots, outlives, ty_outlives, free: vec![], wf_pairs: vec![], wf_vague: vec![] }))
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
                ItemKind::Fn(fs, _) => return sig_regions(fs, hdr.as_deref(), &[]),
                ItemKind::Impl(g, _, t, items) => {
                    hdr = hdr_regions(g, Some(t));
                    for m in items {
                        if let ItemKind::Fn(fs, _) = &m.kind {
                            return sig_regions(fs, hdr.as_deref(), &[]);
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
