//! Well-formedness of the types an item writes, as far as one item shows it: `&'a T` needs every lifetime in T to
//! outlive 'a (E0491) and every type parameter in it to be declared to (E0309, E0310 for 'static). What is known is
//! what the item declares (`T: 'a`, `'b: 'a`) and what its signature's types imply (`&'a &'b u8` implies 'b: 'a).
//! What another item decides (a struct's requirements, which its fields imply; a trait's `: 'a` supertraits) is not
//! seen here: the names it could relate are left unchecked
use crate::parse::{Bound, GArg, GParam, Generics, Pos, Ty as PTy};
use crate::types::{Owner, WfObl, R_STATIC};

/// What a scope knows of outlives, by name (a lifetime with its quote, a type parameter without)
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Known {
    /// (longer, shorter)
    pub pairs: Vec<(String, String)>,
    /// Names another item may relate (left unchecked)
    pub vague: Vec<String>,
    /// The lifetimes and type parameters in scope
    pub lts: Vec<String>,
    pub tparams: Vec<String>,
}

/// A trait of this name may require Self to outlive a lifetime: not one of std's (Any aside, which no program
/// implements)
pub fn may_bound_self(name: &str) -> bool {
    !(PLAIN.contains(&name) || matches!(name, "Drop" | "Fn" | "FnMut" | "FnOnce" | "Index" | "IndexMut" | "Iterator" | "IntoIterator"))
}

/// Traits of std with no outlives requirement on Self (a bound by another may imply one: `T: Any` is `T: 'static`)
const PLAIN: &[&str] = &[
    "Debug", "Display", "Clone", "Copy", "Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "Send", "Sync", "Sized", "Fn", "FnMut",
    "FnOnce", "Iterator", "IntoIterator", "From", "Into", "AsRef", "AsMut", "Deref", "DerefMut", "Add", "Sub", "Mul", "Div", "Rem", "Neg",
    "Not", "ToString", "FromStr", "Unpin", "Error", "Borrow", "ToOwned", "Hasher", "Read", "Write", "ExactSizeIterator",
    "DoubleEndedIterator", "Extend", "FromIterator",
];

/// Types of std that require nothing of their arguments
const PLAIN_ADTS: &[&str] = &[
    "Option", "Vec", "Box", "Result", "String", "Rc", "Arc", "Cell", "RefCell", "HashMap", "HashSet", "BTreeMap", "BTreeSet", "VecDeque",
    "PhantomData", "Mutex", "RwLock", "Weak", "BinaryHeap", "LinkedList",
];

impl Known {
    /// What an impl's or trait's header knows (a member's scope starts there)
    pub fn of_owner(o: &Owner) -> Known {
        let mut k = Known::default();
        for (n, bs) in &o.generics {
            if n == "Self" {
                continue;
            }
            k.tparams.push(n.clone());
            for b in bs {
                if b.starts_with('\'') {
                    k.pairs.push((n.clone(), b.clone()));
                } else {
                    let t = b.split('<').next().unwrap_or("").trim().trim_start_matches('?');
                    if !PLAIN.contains(&t) || b.contains('\'') {
                        k.vague.push(n.clone());
                    }
                }
            }
        }
        if let Some(r) = &o.regions {
            let name = |c: u8| if c == R_STATIC { Some("'static".to_string()) } else { r.lts.get(c as usize).cloned() };
            k.lts.extend(r.lts.iter().cloned());
            for &(a, b) in &r.outlives {
                if let (Some(a), Some(b)) = (name(a), name(b)) {
                    k.pairs.push((a, b));
                }
            }
            for (t, b) in &r.ty_outlives {
                if let Some(b) = name(*b) {
                    k.pairs.push((t.clone(), b));
                }
            }
            k.pairs.extend(r.wf_pairs.iter().cloned());
            k.vague.extend(r.wf_vague.iter().cloned());
        }
        k
    }

    /// The declared generics (an impl's or trait's, then a fn's)
    pub fn add_generics(&mut self, g: &Generics) {
        for p in &g.params {
            match p {
                GParam::Lifetime(l) => self.lts.push(l.clone()),
                GParam::Ty(n, bs, _) => {
                    self.tparams.push(n.clone());
                    self.bounds(n, bs);
                }
                GParam::Const(..) => {}
            }
        }
        for (t, bs) in &g.preds {
            match t {
                PTy::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => {
                    let n = p.segs[0].name.clone();
                    self.bounds(&n, bs);
                }
                // (a where clause on another type must itself be well formed: check_bounds)
                _ => {}
            }
        }
    }

    fn bounds(&mut self, n: &str, bs: &[Bound]) {
        for b in bs {
            match b {
                Bound::Lifetime(l) => self.pairs.push((n.to_string(), l.clone())),
                Bound::Trait(maybe, p) => {
                    let plain = *maybe || p.segs.last().is_some_and(|s| PLAIN.contains(&s.name.as_str()) && s.args.iter().all(|a| !matches!(a, GArg::Lifetime(_))));
                    if !plain {
                        self.vague.push(n.to_string());
                    }
                }
            }
        }
    }

    /// What the well-formedness of a type the scope's callers provide (a parameter's, the self type) implies
    pub fn implied(&mut self, t: &PTy, depth: u32) {
        if depth > 12 {
            return;
        }
        match t {
            PTy::Ref(l, _, i) => {
                if let Some(x) = l {
                    let (mut ls, mut ts, mut vague) = (vec![], vec![], false);
                    self.components(i, &mut ls, &mut ts, &mut vague, 0);
                    for c in ls.into_iter().chain(ts) {
                        self.pairs.push((c, x.clone()));
                    }
                }
                self.implied(i, depth + 1);
            }
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => self.implied(i, depth + 1),
            PTy::Tuple(ts) => ts.iter().for_each(|t| self.implied(t, depth + 1)),
            PTy::Path(None, p) => {
                let last = p.segs.last();
                let tparam = p.segs.len() == 1 && last.is_some_and(|s| s.args.is_empty() && self.tparams.contains(&s.name));
                if !tparam && !last.is_some_and(|s| PLAIN_ADTS.contains(&s.name.as_str())) {
                    // (a struct's requirements on its arguments, which its fields imply: among its lifetime
                    // arguments, of a type argument (`struct Ref<'a, T: 'a>`, or `T: 'static`))
                    let mut ns = vec![];
                    for s in &p.segs {
                        for a in &s.args {
                            match a {
                                GArg::Ty(t) | GArg::Bind(_, t) => names(t, &mut ns),
                                GArg::Lifetime(l) => ns.push(l.clone()),
                                GArg::Const(_) => {}
                            }
                        }
                    }
                    for a in &ns {
                        if a.starts_with('\'') {
                            for b in ns.iter().filter(|b| b.starts_with('\'') && *b != a) {
                                self.pairs.push((a.clone(), b.clone()));
                            }
                        } else {
                            self.vague.push(a.clone());
                        }
                    }
                }
                for s in &p.segs {
                    for a in &s.args {
                        if let GArg::Ty(t) | GArg::Bind(_, t) = a {
                            self.implied(t, depth + 1);
                        }
                    }
                }
            }
            // (a projection: normalized, it may be anything)
            PTy::Path(Some(_), _) | PTy::Dyn(_) | PTy::Impl(_) => {
                let mut ns = vec![];
                names(t, &mut ns);
                self.vague.extend(ns);
            }
            _ => {}
        }
    }

    /// The lifetimes and type parameters a value of type t holds (vague: t has a part that is not followed)
    fn components(&self, t: &PTy, ls: &mut Vec<String>, ts: &mut Vec<String>, vague: &mut bool, depth: u32) {
        if depth > 12 {
            *vague = true;
            return;
        }
        match t {
            PTy::Ref(l, _, i) => {
                match l {
                    Some(l) => ls.push(l.clone()),
                    None => *vague = true,
                }
                self.components(i, ls, ts, vague, depth + 1);
            }
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => self.components(i, ls, ts, vague, depth + 1),
            PTy::Tuple(xs) => xs.iter().for_each(|x| self.components(x, ls, ts, vague, depth + 1)),
            PTy::Path(None, p) => {
                if let [s] = p.segs.as_slice() {
                    if s.args.is_empty() && self.tparams.contains(&s.name) {
                        ts.push(s.name.clone());
                        return;
                    }
                    if s.name == "Self" {
                        *vague = true;
                        return;
                    }
                }
                // (`T::Assoc`: a projection)
                if p.segs.len() > 1 && self.tparams.contains(&p.segs[0].name) {
                    *vague = true;
                    return;
                }
                for s in &p.segs {
                    if s.fn_args.is_some() {
                        *vague = true;
                    }
                    for a in &s.args {
                        match a {
                            GArg::Ty(t) => self.components(t, ls, ts, vague, depth + 1),
                            GArg::Lifetime(l) => ls.push(l.clone()),
                            _ => *vague = true,
                        }
                    }
                }
            }
            PTy::Dyn(bs) => {
                for b in bs {
                    match b {
                        Bound::Lifetime(l) => ls.push(l.clone()),
                        Bound::Trait(..) => *vague = true,
                    }
                }
            }
            PTy::Never => {}
            _ => *vague = true,
        }
    }

    /// a outlives b, as far as is known
    pub fn outlives(&self, a: &str, b: &str) -> bool {
        if a == b || a == "'static" {
            return true;
        }
        let mut seen: Vec<&str> = vec![a];
        let mut st = vec![a];
        while let Some(x) = st.pop() {
            for (l, s) in &self.pairs {
                if l == x && !seen.contains(&s.as_str()) {
                    if s == b || s == "'static" {
                        return true;
                    }
                    seen.push(s);
                    st.push(s);
                }
            }
        }
        false
    }

    /// The first part of t that is not well formed: (where, message). statics: only `&'static T`
    pub fn check(&self, t: &PTy, statics: bool, depth: u32) -> Option<(Option<Pos>, String)> {
        if depth > 12 {
            return None;
        }
        match t {
            PTy::Ref(l, _, i) => {
                if let Some(x) = l.as_ref().filter(|x| (*x == "'static" || (!statics && self.lts.contains(x))) && !self.vague.contains(x)) {
                    let (mut ls, mut ts, mut vague) = (vec![], vec![], false);
                    self.components(i, &mut ls, &mut ts, &mut vague, 0);
                    if !vague {
                        for y in &ls {
                            if y != "'_" && (self.lts.contains(y) || y == "'static") && !self.vague.contains(y) && !self.outlives(y, x) {
                                return Some((pos_of(i), "reference has a longer lifetime than the data it references (E0491)".to_string()));
                            }
                        }
                        for p in &ts {
                            if !self.vague.contains(p) && !self.outlives(p, x) {
                                let code = if x == "'static" { "E0310" } else { "E0309" };
                                return Some((pos_of(i), format!("the parameter type `{p}` may not live long enough ({code})")));
                            }
                        }
                    }
                }
                self.check(i, statics, depth + 1)
            }
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => self.check(i, statics, depth + 1),
            PTy::Tuple(xs) => xs.iter().find_map(|x| self.check(x, statics, depth + 1)),
            PTy::Path(q, p) => {
                if let Some(q) = q {
                    if let Some(e) = self.check(q, statics, depth + 1) {
                        return Some(e);
                    }
                }
                for s in &p.segs {
                    for a in &s.args {
                        if let GArg::Ty(t) | GArg::Bind(_, t) = a {
                            if let Some(e) = self.check(t, statics, depth + 1) {
                                return Some(e);
                            }
                        }
                    }
                    if let Some((ps, r)) = &s.fn_args {
                        for t in ps.iter().chain(r.as_deref()) {
                            if let Some(e) = self.check(t, statics, depth + 1) {
                                return Some(e);
                            }
                        }
                    }
                }
                None
            }
            PTy::Fn(ps, r) => ps.iter().chain(std::iter::once(&**r)).find_map(|t| self.check(t, statics, depth + 1)),
            PTy::Dyn(bs) | PTy::Impl(bs) => bs.iter().find_map(|b| match b {
                Bound::Trait(_, p) => self.check(&PTy::Path(None, p.clone()), statics, depth + 1),
                Bound::Lifetime(_) => None,
            }),
            _ => None,
        }
    }

    /// The lifetimes and type parameters a value of type t holds (None: a part of it is not followed)
    pub fn sub_of(&self, t: &PTy) -> Option<Vec<String>> {
        let (mut ls, ts, mut vague) = (vec![], vec![], false);
        let mut ts = ts;
        self.components(t, &mut ls, &mut ts, &mut vague, 0);
        if vague {
            return None;
        }
        ls.retain(|l| l != "'_" && l != "'static");
        ls.extend(ts);
        ls.sort();
        ls.dedup();
        Some(ls)
    }

    /// An obligation that the lifetimes and type parameters `sub` outlive what the trait tr (written with the
    /// lifetime arguments of its path) requires of Self
    pub fn obl(&self, tr: &crate::parse::Path, sub: Vec<String>, line: u64) -> Option<WfObl> {
        let last = tr.segs.last()?;
        if !may_bound_self(&last.name) {
            return None;
        }
        let args = last.args.iter().filter_map(|a| if let GArg::Lifetime(l) = a { Some(l.clone()) } else { None }).collect();
        Some(WfObl { tr: last.name.clone(), args, sub, known: self.pairs.clone(), vague: self.vague.clone(), line })
    }

    /// The trait objects in t: each object's lifetime (written, else the default: the reference around it, else
    /// 'static) must outlive what its trait requires of Self
    pub fn dyn_obls(&self, t: &PTy, parent: Option<&str>, out: &mut Vec<WfObl>, depth: u32) {
        if depth > 12 {
            return;
        }
        match t {
            PTy::Ref(l, _, i) => self.dyn_obls(i, l.as_deref().or(Some("'_")), out, depth + 1),
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => self.dyn_obls(i, None, out, depth + 1),
            PTy::Tuple(ts) => ts.iter().for_each(|t| self.dyn_obls(t, None, out, depth + 1)),
            PTy::Path(_, p) => {
                for s in &p.segs {
                    for a in &s.args {
                        if let GArg::Ty(t) = a {
                            self.dyn_obls(t, None, out, depth + 1);
                        }
                    }
                }
            }
            PTy::Dyn(bs) => {
                let Some(p) = bs.iter().find_map(|b| if let Bound::Trait(false, p) = b { Some(p) } else { None }) else { return };
                // (only an object lifetime written out: an elided one defaults to the trait's own lifetime bound, which
                // it then meets (`&'a dyn Any` is `&'a (dyn Any + 'static)`, as `Any: 'static`))
                let _ = parent;
                let Some(obj) = bs.iter().find_map(|b| if let Bound::Lifetime(l) = b { Some(l.as_str()) } else { None }) else { return };
                if obj == "'_" || obj == "'static" || self.vague.iter().any(|v| v == obj) {
                    return;
                }
                if let Some(o) = self.obl(p, vec![obj.to_string()], p.pos.line as u64) {
                    out.push(o);
                }
            }
            _ => {}
        }
    }

    /// The bounds of the generics: the types their trait bounds name, and the types where clauses bound
    pub fn check_bounds(&self, g: &Generics) -> Option<(Option<Pos>, String)> {
        let of_bounds = |bs: &[Bound]| {
            bs.iter().find_map(|b| match b {
                Bound::Trait(_, p) => self.check(&PTy::Path(None, p.clone()), false, 0),
                Bound::Lifetime(_) => None,
            })
        };
        for p in &g.params {
            if let GParam::Ty(_, bs, _) = p {
                if let Some(e) = of_bounds(bs) {
                    return Some(e);
                }
            }
        }
        for (t, bs) in &g.preds {
            if let Some(e) = self.check(t, false, 0).or_else(|| of_bounds(bs)) {
                return Some(e);
            }
        }
        None
    }
}

/// The bounds or where clauses write a reference with a lifetime (whose well-formedness check_bounds checks)
pub fn bounds_have_refs(g: &Generics) -> bool {
    fn has(t: &PTy) -> bool {
        match t {
            PTy::Ref(l, _, i) => l.is_some() || has(i),
            PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => has(i),
            PTy::Tuple(ts) => ts.iter().any(has),
            PTy::Fn(ps, r) => ps.iter().any(has) || has(r),
            PTy::Path(q, p) => q.as_deref().is_some_and(has) || path_has(p),
            _ => false,
        }
    }
    fn path_has(p: &crate::parse::Path) -> bool {
        p.segs.iter().any(|s| {
            s.args.iter().any(|a| matches!(a, GArg::Ty(t) | GArg::Bind(_, t) if has(t)))
                || s.fn_args.as_ref().is_some_and(|(ps, r)| ps.iter().any(has) || r.as_deref().is_some_and(has))
        })
    }
    let bs = |bs: &[Bound]| bs.iter().any(|b| matches!(b, Bound::Trait(_, p) if path_has(p)));
    g.params.iter().any(|p| matches!(p, GParam::Ty(_, b, _) if bs(b))) || g.preds.iter().any(|(t, b)| has(t) || bs(b))
}

/// The lifetimes and type names in t
fn names(t: &PTy, out: &mut Vec<String>) {
    match t {
        PTy::Ref(l, _, i) => {
            if let Some(l) = l {
                out.push(l.clone());
            }
            names(i, out);
        }
        PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => names(i, out),
        PTy::Tuple(ts) => ts.iter().for_each(|t| names(t, out)),
        PTy::Fn(ps, r) => {
            ps.iter().for_each(|t| names(t, out));
            names(r, out);
        }
        PTy::Path(q, p) => {
            if let Some(q) = q {
                names(q, out);
            }
            for s in &p.segs {
                out.push(s.name.clone());
                for a in &s.args {
                    match a {
                        GArg::Ty(t) | GArg::Bind(_, t) => names(t, out),
                        GArg::Lifetime(l) => out.push(l.clone()),
                        GArg::Const(_) => {}
                    }
                }
                if let Some((ps, r)) = &s.fn_args {
                    ps.iter().for_each(|t| names(t, out));
                    if let Some(r) = r {
                        names(r, out);
                    }
                }
            }
        }
        PTy::Dyn(bs) | PTy::Impl(bs) => {
            for b in bs {
                match b {
                    Bound::Lifetime(l) => out.push(l.clone()),
                    Bound::Trait(_, p) => names(&PTy::Path(None, p.clone()), out),
                }
            }
        }
        _ => {}
    }
}

/// Where t is written (its first path)
pub fn pos_of(t: &PTy) -> Option<Pos> {
    match t {
        PTy::Path(_, p) if p.pos.line != 0 => Some(p.pos),
        PTy::Path(Some(q), _) => pos_of(q),
        PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => pos_of(i),
        PTy::Tuple(ts) => ts.iter().find_map(pos_of),
        PTy::Dyn(bs) | PTy::Impl(bs) => bs.iter().find_map(|b| match b {
            Bound::Trait(_, p) if p.pos.line != 0 => Some(p.pos),
            _ => None,
        }),
        _ => None,
    }
}

/// The message at a position in an item (its lines counted from 1)
pub fn at(e: (Option<Pos>, String)) -> String {
    match e.0 {
        Some(p) => format!("{}:{}: {}", p.line, p.col + 1, e.1),
        None => e.1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_file, ItemKind};

    fn fn_err(src: &str) -> Option<String> {
        let f = parse_file(src).unwrap();
        let ItemKind::Fn(fs, _) = &f.items[0].kind else { panic!() };
        let mut k = Known::default();
        k.add_generics(&fs.generics);
        for (_, t) in &fs.params {
            k.implied(t, 0);
        }
        k.check_bounds(&fs.generics).map(|e| e.1)
    }

    #[test]
    fn where_clause_types() {
        assert!(fn_err("fn f<'a, 'b, F>(a: &'a usize, b: &'b usize, x: F) where F: FnOnce(&'a &'b usize) {}").unwrap().contains("E0491"));
        assert_eq!(fn_err("fn f<'a, 'b: 'a, F>(a: &'a usize, b: &'b usize, x: F) where F: FnOnce(&'a &'b usize) {}"), None);
        // (implied by a parameter's type)
        assert_eq!(fn_err("fn f<'a, 'b, F>(a: &'a &'b usize, x: F) where F: FnOnce(&'a &'b usize) {}"), None);
        assert!(fn_err("fn f<'a, K, L: Tr<&'a K>>() {}").unwrap().contains("`K` may not live long enough (E0309)"));
        assert_eq!(fn_err("fn f<'a, K: 'a, L: Tr<&'a K>>() {}"), None);
        // (a bound by a trait of the program may imply it)
        assert_eq!(fn_err("fn f<'a, K: Mine, L: Tr<&'a K>>() {}"), None);
        // (elided and bound lifetimes are not checked)
        assert_eq!(fn_err("fn f<K, F: Fn(&K)>(x: F) {}"), None);
        assert_eq!(fn_err("fn f<K, F: for<'r> Fn(&'r K)>(x: F) {}"), None);
    }
}
