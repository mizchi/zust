//! Exhaustiveness of `match` (E0004): a small version of the usefulness algorithm (Maranget, "Warnings for pattern
//! matching"). Constructors are those of bool, enums (Option, Result ..), structs and tuples; references are looked
//! through (match ergonomics). Literals of other types (integers, chars, strings ..) never cover their type.
//! Patterns the check does not model (ranges, slices, macros, consts of non-enum types ..) make it give up, so it
//! reports no false errors.

use super::*;
use crate::parse::{Path, Pat};
use crate::program::PPath;
use crate::types::Ty;

/// A pattern reduced to what the check needs
#[derive(Clone)]
enum N {
    Wild,
    /// constructor index of the type, and the sub-patterns (as many as the constructor has fields)
    Ctor(usize, Vec<N>),
    /// a literal of a type with no finite constructors (it covers nothing as a whole)
    Lit,
    Or(Vec<N>),
}

/// Gave up (a pattern or type the check does not model)
struct Unknown;

const MAX_WORK: u32 = 20_000;

impl Cx<'_> {
    /// The index of the first arm that no value can reach (the arms before it, without guards, cover all it matches)
    pub(crate) fn unreachable_arm(&self, arms: &[crate::parse::Arm], ty: Ty) -> Option<usize> {
        let mut rows: Vec<Vec<N>> = vec![];
        let mut work = 0;
        for (i, a) in arms.iter().enumerate() {
            let q = self.norm(&a.pat, ty, 0).ok()?;
            if !rows.is_empty() && !self.useful_q(&rows, &[q.clone()], &[ty], &mut work).ok()? {
                return Some(i);
            }
            if a.guard.is_none() {
                rows.push(vec![q]);
            }
        }
        None
    }

    /// Whether the row q matches a value that none of rows matches
    fn useful_q(&self, rows: &[Vec<N>], q: &[N], tys: &[Ty], work: &mut u32) -> Result<bool, Unknown> {
        *work += 1 + rows.len() as u32;
        if *work > MAX_WORK {
            return Err(Unknown);
        }
        if tys.is_empty() {
            return Ok(rows.is_empty());
        }
        let mut rs: Vec<Vec<N>> = vec![];
        for r in rows {
            expand_or(r, &mut rs);
        }
        let t = tys[0];
        match &q[0] {
            N::Or(alts) => {
                for a in alts {
                    let mut q2 = vec![a.clone()];
                    q2.extend_from_slice(&q[1..]);
                    if self.useful_q(&rs, &q2, tys, work)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            N::Ctor(c, subs) => {
                let cs = self.ctors(t).ok_or(Unknown)?;
                let fts = &cs.get(*c).ok_or(Unknown)?.1;
                if subs.len() != fts.len() {
                    return Err(Unknown);
                }
                let spec: Vec<Vec<N>> = rs.iter().filter_map(|r| specialize(r, *c, fts.len())).collect();
                let mut q2 = subs.clone();
                q2.extend_from_slice(&q[1..]);
                let mut ntys = fts.clone();
                ntys.extend_from_slice(&tys[1..]);
                self.useful_q(&spec, &q2, &ntys, work)
            }
            // (literals are not told apart: a literal after a literal of the same column is not judged)
            N::Lit => {
                if rs.iter().any(|r| matches!(r[0], N::Lit | N::Ctor(..))) {
                    return Err(Unknown);
                }
                let default: Vec<Vec<N>> = rs.iter().filter(|r| matches!(r[0], N::Wild)).map(|r| r[1..].to_vec()).collect();
                self.useful_q(&default, &q[1..], &tys[1..], work)
            }
            N::Wild => {
                let ctors = self.ctors(t);
                let heads: Vec<usize> = rs.iter().filter_map(|r| if let N::Ctor(c, _) = &r[0] { Some(*c) } else { None }).collect();
                let live = |cs: &Vec<(String, Vec<Ty>)>, c: usize| cs[c].1.iter().all(|&f| self.inhabited(f, 0));
                match &ctors {
                    Some(cs) if (0..cs.len()).all(|c| heads.contains(&c) || !live(cs, c)) => {
                        for (ci, (_, fts)) in cs.iter().enumerate() {
                            if !live(cs, ci) {
                                continue;
                            }
                            let spec: Vec<Vec<N>> = rs.iter().filter_map(|r| specialize(r, ci, fts.len())).collect();
                            let mut q2 = vec![N::Wild; fts.len()];
                            q2.extend_from_slice(&q[1..]);
                            let mut ntys = fts.clone();
                            ntys.extend_from_slice(&tys[1..]);
                            if self.useful_q(&spec, &q2, &ntys, work)? {
                                return Ok(true);
                            }
                        }
                        Ok(false)
                    }
                    _ => {
                        let default: Vec<Vec<N>> = rs.iter().filter(|r| matches!(r[0], N::Wild)).map(|r| r[1..].to_vec()).collect();
                        self.useful_q(&default, &q[1..], &tys[1..], work)
                    }
                }
            }
        }
    }

    /// Some(witness) when the arms (those without a guard) do not cover `ty`
    pub(crate) fn non_exhaustive(&self, arms: &[crate::parse::Arm], ty: Ty) -> Option<String> {
        let pats: Vec<&crate::parse::Pat> = arms.iter().filter(|a| a.guard.is_none()).map(|a| &a.pat).collect();
        self.non_exhaustive_pats(&pats, ty)
    }

    /// Some(witness) when the patterns do not cover `ty` (a `let` pattern that is refutable, E0005)
    pub(crate) fn non_exhaustive_pats(&self, pats: &[&crate::parse::Pat], ty: Ty) -> Option<String> {
        let mut rows = vec![];
        for p in pats {
            rows.push(vec![self.norm(p, ty, 0).ok()?]);
        }
        let mut work = 0;
        match self.useful(&rows, &[ty], &mut work) {
            Ok(Some(w)) => Some(w.into_iter().next().unwrap_or_else(|| "_".into())),
            _ => None,
        }
    }

    fn strip_ref(t: Ty) -> Ty {
        let mut t = t;
        while let Ty::Ref(_, i) = t {
            t = *i;
        }
        t
    }

    /// The constructors of t: (name, field types) each, or None when it has no finite set
    fn ctors(&self, t: Ty) -> Option<Vec<(String, Vec<Ty>)>> {
        match Self::strip_ref(t) {
            Ty::Bool => Some(vec![("false".into(), vec![]), ("true".into(), vec![])]),
            Ty::Unit => Some(vec![("()".into(), vec![])]),
            Ty::Struct(sid) => {
                let sd = &self.prog.structs[sid as usize];
                if sd.symbolic {
                    return None;
                }
                if sd.is_enum {
                    let en = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
                    Some(sd.variants.iter().map(|v| (format!("{en}::{}", v.name), v.fields.iter().map(|f| f.1).collect())).collect())
                } else if self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name) {
                    None
                } else {
                    let name = if self.prog.is_tuple(sid) { String::new() } else { self.prog.structs[self.prog.base_of(sid) as usize].name.clone() };
                    Some(vec![(name, sd.fields.iter().map(|f| f.1).collect())])
                }
            }
            _ => None,
        }
    }

    /// Whether a value of t can exist: not `!`, an enum with no variants, or a type that holds one of them
    fn inhabited(&self, t: Ty, depth: u32) -> bool {
        if depth > 8 {
            return true;
        }
        match t {
            Ty::Never => false,
            Ty::Struct(sid) => {
                let sd = &self.prog.structs[sid as usize];
                if sd.symbolic {
                    return true;
                }
                if sd.is_enum {
                    sd.variants.iter().any(|v| v.fields.iter().all(|f| self.inhabited(f.1, depth + 1)))
                } else {
                    sd.fields.iter().all(|f| self.inhabited(f.1, depth + 1))
                }
            }
            // (a reference to an empty type is still a value for the check, as in rustc's patterns)
            _ => true,
        }
    }

    fn variant_index(&self, path: &Path, t: Ty) -> Option<usize> {
        let Ty::Struct(sid) = Self::strip_ref(t) else { return None };
        let (vsid, vi) = self.prog.variant_by_p(&PPath::of(path), self.self_sid)?;
        (self.prog.base_of(vsid) == self.prog.base_of(sid)).then_some(vi)
    }

    fn norm(&self, p: &Pat, t: Ty, depth: u32) -> Result<N, Unknown> {
        if depth > 32 {
            return Err(Unknown);
        }
        let t0 = Self::strip_ref(t);
        Ok(match p {
            Pat::Wild | Pat::Rest => N::Wild,
            Pat::Ref(_, inner) => return self.norm(inner, t0, depth + 1),
            Pat::Ident(..) if unit_variant_ident(self.prog, p).is_some() => {
                let path = unit_variant_ident(self.prog, p).unwrap();
                match self.variant_index(&path, t0) {
                    Some(vi) if self.ctors(t0).is_some_and(|cs| cs[vi].1.is_empty()) => N::Ctor(vi, vec![]),
                    _ => return Err(Unknown),
                }
            }
            Pat::Ident(..) if const_lit_pat(self.prog, p).is_some() => {
                if self.ctors(t0).is_some() {
                    return Err(Unknown);
                }
                N::Lit
            }
            // (a const of another type (`FOO` of a struct type): its value is not modeled)
            Pat::Ident(false, false, n, None) if self.prog.const_index.contains_key(n) || self.prog.const_by_last.contains_key(n) => {
                return Err(Unknown);
            }
            Pat::Ident(_, _, _, None) => N::Wild,
            Pat::Ident(_, _, _, Some(sub)) => return self.norm(sub, t0, depth + 1),
            Pat::Lit(e) => {
                let text = match &**e {
                    crate::parse::Expr::Lit(s, _) => s.as_str(),
                    _ => return Err(Unknown),
                };
                match (t0, text) {
                    (Ty::Bool, "false") => N::Ctor(0, vec![]),
                    (Ty::Bool, "true") => N::Ctor(1, vec![]),
                    (t, _) if self.ctors(t).is_none() => N::Lit,
                    _ => return Err(Unknown),
                }
            }
            Pat::Or(v) => {
                let mut out = vec![];
                for q in v {
                    out.push(self.norm(q, t0, depth + 1)?);
                }
                N::Or(out)
            }
            Pat::Tuple(v) => {
                let Some(cs) = self.ctors(t0) else { return Err(Unknown) };
                let [(_, fts)] = &cs[..] else { return Err(Unknown) };
                self.fields_positional(v, fts, 0, depth)?
            }
            Pat::TupleStruct(path, v) => {
                let Some(cs) = self.ctors(t0) else { return Err(Unknown) };
                let ci = if cs.len() == 1 && self.variant_index(path, t0).is_none() { 0 } else { self.variant_index(path, t0).ok_or(Unknown)? };
                self.fields_positional(v, &cs[ci].1, ci, depth)?
            }
            Pat::Struct(path, fs, _) => {
                let Some(cs) = self.ctors(t0) else { return Err(Unknown) };
                let ci = if cs.len() == 1 && self.variant_index(path, t0).is_none() { 0 } else { self.variant_index(path, t0).ok_or(Unknown)? };
                let names: Vec<String> = match t0 {
                    Ty::Struct(sid) => {
                        let sd = &self.prog.structs[sid as usize];
                        if sd.is_enum { sd.variants[ci].fields.iter().map(|f| f.0.clone()).collect() } else { sd.fields.iter().map(|f| f.0.clone()).collect() }
                    }
                    _ => return Err(Unknown),
                };
                let mut subs = vec![N::Wild; names.len()];
                for (n, q) in fs {
                    let i = names.iter().position(|x| x == n).ok_or(Unknown)?;
                    subs[i] = self.norm(q, cs[ci].1[i], depth + 1)?;
                }
                N::Ctor(ci, subs)
            }
            Pat::Path(path) => {
                let Some(cs) = self.ctors(t0) else { return Err(Unknown) };
                match self.variant_index(path, t0) {
                    Some(vi) if cs[vi].1.is_empty() => N::Ctor(vi, vec![]),
                    None if cs.len() == 1 && cs[0].1.is_empty() => N::Ctor(0, vec![]),
                    _ => return Err(Unknown),
                }
            }
            // (ranges, slices, macros: not modelled)
            _ => return Err(Unknown),
        })
    }

    /// `(a, .., z)` / `V(a, b)`: positional sub-patterns, `..` filled with wildcards
    fn fields_positional(&self, v: &[Pat], fts: &[Ty], ci: usize, depth: u32) -> Result<N, Unknown> {
        let rest = v.iter().position(|p| matches!(p, Pat::Rest));
        let mut subs = vec![N::Wild; fts.len()];
        match rest {
            Some(r) => {
                let post = v.len() - r - 1;
                if r + post > fts.len() {
                    return Err(Unknown);
                }
                for i in 0..r {
                    subs[i] = self.norm(&v[i], fts[i], depth + 1)?;
                }
                for k in 0..post {
                    let fi = fts.len() - post + k;
                    subs[fi] = self.norm(&v[r + 1 + k], fts[fi], depth + 1)?;
                }
            }
            None => {
                if v.len() != fts.len() {
                    return Err(Unknown);
                }
                for (i, q) in v.iter().enumerate() {
                    subs[i] = self.norm(q, fts[i], depth + 1)?;
                }
            }
        }
        Ok(N::Ctor(ci, subs))
    }

    /// Whether the all-wildcard vector is useful against the rows: Some(witness patterns) when it is
    fn useful(&self, rows: &[Vec<N>], tys: &[Ty], work: &mut u32) -> Result<Option<Vec<String>>, Unknown> {
        *work += 1 + rows.len() as u32;
        if *work > MAX_WORK {
            return Err(Unknown);
        }
        if tys.is_empty() {
            return Ok(if rows.is_empty() { Some(vec![]) } else { None });
        }
        // (or-patterns in the first column: one row per alternative)
        let mut rs: Vec<Vec<N>> = vec![];
        for r in rows {
            expand_or(r, &mut rs);
        }
        let t = tys[0];
        let ctors = self.ctors(t);
        let heads: Vec<usize> = rs.iter().filter_map(|r| if let N::Ctor(c, _) = &r[0] { Some(*c) } else { None }).collect();
        // (constructors that cannot have a value (`V(!)`, an empty enum's) need no arm)
        let live = |cs: &Vec<(String, Vec<Ty>)>, c: usize| cs[c].1.iter().all(|&f| self.inhabited(f, 0));
        let complete = ctors.as_ref().is_some_and(|cs| (0..cs.len()).all(|c| heads.contains(&c) || !live(cs, c)));
        if let (true, Some(cs)) = (complete, &ctors) {
            for (ci, (cname, fts)) in cs.iter().enumerate() {
                if !live(cs, ci) {
                    continue;
                }
                let spec: Vec<Vec<N>> = rs.iter().filter_map(|r| specialize(r, ci, fts.len())).collect();
                let mut ntys = fts.clone();
                ntys.extend_from_slice(&tys[1..]);
                if let Some(w) = self.useful(&spec, &ntys, work)? {
                    let (args, rest) = w.split_at(fts.len());
                    let mut out = vec![show(cname, args, is_tuple_ty(self.prog, t))];
                    out.extend_from_slice(rest);
                    return Ok(Some(out));
                }
            }
            return Ok(None);
        }
        // (not every constructor appears: the rows whose head is a wildcard decide, and a missing constructor is the
        // witness)
        let default: Vec<Vec<N>> = rs.iter().filter(|r| matches!(r[0], N::Wild)).map(|r| r[1..].to_vec()).collect();
        let Some(w) = self.useful(&default, &tys[1..], work)? else { return Ok(None) };
        let head = match &ctors {
            Some(cs) => {
                let ci = (0..cs.len()).find(|&c| !heads.contains(&c) && live(cs, c)).unwrap_or(0);
                let wild: Vec<String> = vec!["_".into(); cs[ci].1.len()];
                show(&cs[ci].0, &wild, is_tuple_ty(self.prog, t))
            }
            None => "_".into(),
        };
        let mut out = vec![head];
        out.extend(w);
        Ok(Some(out))
    }
}

fn is_tuple_ty(prog: &crate::program::Program, t: Ty) -> bool {
    matches!(Cx::strip_ref(t), Ty::Struct(sid) if prog.is_tuple(sid))
}

fn expand_or(r: &[N], out: &mut Vec<Vec<N>>) {
    if r.is_empty() {
        out.push(vec![]);
        return;
    }
    match &r[0] {
        N::Or(alts) => {
            for a in alts {
                let mut x = vec![a.clone()];
                x.extend_from_slice(&r[1..]);
                expand_or(&x, out);
            }
        }
        _ => out.push(r.to_vec()),
    }
}

/// The row specialized for constructor c of arity n (None when its head is another constructor or a literal)
fn specialize(r: &[N], c: usize, n: usize) -> Option<Vec<N>> {
    let mut out = match &r[0] {
        N::Wild => vec![N::Wild; n],
        N::Ctor(k, subs) if *k == c && subs.len() == n => subs.clone(),
        _ => return None,
    };
    out.extend_from_slice(&r[1..]);
    Some(out)
}

fn show(name: &str, args: &[String], tuple: bool) -> String {
    if args.is_empty() {
        return if tuple { "()".into() } else { name.to_string() };
    }
    format!("{name}({})", args.join(", "))
}
