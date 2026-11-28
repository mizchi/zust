use super::*;
use super::clif::{TrapCode, Type, Value, types};
use super::lit::Lit;
use crate::parse::{Block as PBlock, Expr, GArg, Pat, Stmt};
use crate::program::PPath;
use crate::types::{Ty, intern};

// --- 書き換えで式を作るための小さな helper ------------------------------------

/// `(e)`
fn paren(e: Expr) -> Expr {
    Expr::Paren(Box::new(e))
}

/// `let name = init;`
fn let_stmt(name: &str, init: Expr) -> Stmt {
    Stmt::Let(Pat::Ident(false, false, name.to_string(), None), None, Some(init), None, Default::default())
}

/// `..` (添字の全範囲)
fn full_range() -> Expr {
    Expr::Range(None, None, false)
}

/// `e as T` の T (名前だけの型)
fn ty_named(n: &str) -> Box<crate::parse::Ty> {
    Box::new(crate::parse::Ty::Path(None, path_of(n)))
}

/// 名前とリテラルなどだけからなる式のソースのテキスト (`vec![..]` のトークン列を作るのに使う)
/// 式の列を `, ` でつないだテキスト (どれかが書けなければ None)
fn simple_texts(xs: &[Expr]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for x in xs {
        parts.push(simple_text(x)?);
    }
    Some(parts.join(", "))
}

fn simple_text(e: &Expr) -> Option<String> {
    Some(match e {
        Expr::Lit(t, _) => t.clone(),
        Expr::Path(None, p) => crate::parse::path_text(p),
        Expr::Unary(op, x, _) => format!("{op}{}", simple_text(x)?),
        Expr::Ref(m, x) => format!("&{}{}", if *m { "mut " } else { "" }, simple_text(x)?),
        Expr::Paren(x) => format!("({})", simple_text(x)?),
        Expr::Field(b, n, _) => format!("{}.{n}", simple_text(b)?),
        Expr::Tuple(xs) if xs.len() == 1 => format!("({},)", simple_text(&xs[0])?),
        Expr::Tuple(xs) => format!("({})", simple_texts(xs)?),
        Expr::Array(xs) => format!("[{}]", simple_texts(xs)?),
        _ => return None,
    })
}

/// `vec![es]` (syn 版の `parse_quote!(vec![#es])`)。
/// 自前の構文木には式の表示がないので、名前・リテラルなどで書けない要素は先に let で束縛してその名前を並べる
pub(crate) fn vec_of(es: &[Expr]) -> Expr {
    let mut lets = vec![];
    let mut items = vec![];
    for (i, e) in es.iter().enumerate() {
        match simple_text(e) {
            Some(t) => items.push(t),
            None => {
                let n = format!("__exr_va{i}");
                lets.push(let_stmt(&n, e.clone()));
                items.push(n);
            }
        }
    }
    let m = Expr::Macro(path_of("vec"), format!("[{}]", items.join(", ")), crate::parse::Span::default());
    if lets.is_empty() {
        return m;
    }
    lets.push(Stmt::Expr(m, false));
    Expr::Block(None, false, PBlock { stmts: lets, spos: Default::default() })
}

impl Cx<'_> {
    /// `recv.m(args)`: レシーバを自動 deref して struct を見つけ、
    /// メソッドの self 型に合わせて自動参照 (`&self` / `&mut self`) かコピー (`self`) で渡す
    /// A prelude iterator adapter that holds a `Copied` / `Cloned` iterator (`Filter<Copied<Iter<&str>>, F>`): what it
    /// yields are copies
    fn copies_items(&self, sid: u32, depth: u32) -> bool {
        let b = self.prog.base_of(sid);
        if depth > 8 || self.prog.struct_item.get(b as usize).is_none_or(|&i| i < self.prog.prelude_from) {
            return false;
        }
        matches!(self.prog.structs[b as usize].name.as_str(), "Copied" | "Cloned")
            || self.prog.structs[sid as usize].args.iter().any(|&a| matches!(a, Ty::Struct(x) if self.copies_items(x, depth + 1)))
    }

    /// Whether the type of `r` (past its references) is one with a Drop impl
    #[cold]
    fn has_drop_impl(&mut self, r: &Expr) -> bool {
        matches!(self.peek(r).map(|t| self.strip_refs(t)), Some(Ty::Struct(sid)) if self.prog.drop_impls.contains(&self.prog.base_of(sid)))
    }

    pub(crate) fn method_call(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // (invalid_atomic_ordering, deny by default: an atomic's load / store / failure ordering it cannot have)
        if let Expr::MethodCall(r, m, _, args, sp) = e {
            if self.user_code
                && matches!(m.as_bytes().first(), Some(b'l' | b's' | b'c' | b'f'))
                && matches!(m.as_str(), "load" | "store" | "compare_exchange" | "compare_exchange_weak" | "fetch_update")
            {
                if let Some(er) = self.atomic_ordering(r, m, args) {
                    return Err(er);
                }
            }
            // (E0658: a closure's, a fn pointer's or an `F: Fn..` parameter's `call` / `call_mut` / `call_once`, which
            // need `fn_traits`)
            if self.user_code && !self.prog.fn_traits && m.starts_with("call") && matches!(m.as_str(), "call" | "call_mut" | "call_once") && self.fn_like(r) {
                return Err(self.at(sp.at, "use of unstable library feature `fn_traits` (E0658)".to_string()));
            }
            // (E0040: `x.drop()` of a value whose type has a Drop impl)
            if self.user_code && args.is_empty() && m == "drop" && self.has_drop_impl(r) {
                return Err(self.at(sp.at, "explicit use of destructor method (E0040)".to_string()));
            }
        }
        // (a call that named its trait: set while this call is resolved, and the outer call's back after)
        let forced = match e {
            Expr::MethodCall(_, m, ..) => self.explicit_traits.get(&(e as *const Expr as usize)).map(|t| (m.clone(), t.clone())),
            _ => None,
        };
        let prev = std::mem::replace(&mut self.explicit_trait, forced);
        let n = self.caller_locs.len();
        // (unused_mut: an array's methods are lowered on its memory, not all through a recorded `&mut`: a method that
        // mutates an array the local owns (`a.swap(0, 1)`, `a[..2].copy_from_slice(..)`) counts as needing it `mut`.
        // Not one that reads it (`a.len()`))
        if self.bk.lint_mut {
            if let Expr::MethodCall(r, mname, ..) = e {
                let mut base: &Expr = strip_paren(r);
                while let Expr::Index(b, ..) = base {
                    base = strip_paren(b);
                }
                if is_place_expr(base) && matches!(self.peek(base), Some(Ty::Array(..))) && MUT_SLICE_METHODS.contains(&mname.as_str()) {
                    self.bk_mut_use(base);
                }
            }
        }
        // (`opt.copied()` / `opt.cloned()` of an `Option<&U>`: the U copied out does not hold the borrow that reached it
        // (bk_copy_out), as `*opt.unwrap()` does not)
        // (and an item of a `Copied` / `Cloned` iterator (`w.iter().copied().next()`): what the iterator's items are
        // copied from holds it)
        let base_name = |this: &Self, t: Option<Ty>| match t.map(|t| this.strip_refs(t)) {
            Some(Ty::Struct(sid)) => Some((sid, this.prog.structs[this.prog.base_of(sid) as usize].name.as_str())),
            _ => None,
        };
        let copy_out = match e {
            // (`r.clone()` of a `&&str`: the `&str` copied out)
            Expr::MethodCall(r, m, _, a, _) if self.bk_on() && a.is_empty() && m == "clone" && matches!(self.peek(r), Some(Ty::Ref(_, u)) if self.bk_ty(*u) == (true, true)) => match self.peek(r) {
                Some(Ty::Ref(_, u)) => Some(Some(*u)),
                _ => None,
            },
            Expr::MethodCall(r, m, _, a, _) if self.bk_on() => match base_name(self, self.peek(r)) {
                Some((sid, "Option")) if a.is_empty() && matches!(m.as_str(), "copied" | "cloned") => match self.prog.structs[sid as usize].args.first() {
                    Some(Ty::Ref(_, u)) if self.bk_ty(**u).1 => Some(Some(**u)),
                    _ => None,
                },
                Some((_, "Copied" | "Cloned")) => Some(None),
                // (an adapter over one (`names.iter().copied().filter(..)`): its items are those copies too)
                Some((sid, _)) if self.copies_items(sid, 0) => Some(None),
                _ => None,
            },
            _ => None,
        };
        if copy_out.is_some() {
            self.bk_frame();
        }
        // (a method call is a value: a `&mut` place around it (`&mut *s.0.borrow_mut()`) does not make its receiver a
        // mutable place, the method's `self` does)
        let pm = std::mem::replace(&mut self.bk.place_mut, false);
        let r = self.method_call_inner(e, expected);
        self.bk.place_mut = pm;
        if let Some(u) = copy_out {
            let srcs = self.bk.frames.pop().unwrap_or_default();
            // (an iterator's item: the result's type, or what its `Option` holds, when it is not the iterator itself)
            let u = u.or_else(|| match r.as_ref().ok().map(|x| x.1) {
                Some(t @ Ty::Struct(sid)) => match base_name(self, Some(t)) {
                    Some((_, "Option")) => self.prog.structs[sid as usize].args.first().copied(),
                    Some((_, "Copied" | "Cloned")) => None,
                    _ => Some(t),
                },
                t => t,
            });
            match u.filter(|&u| self.bk_ty(u).1) {
                Some(u) => self.bk_copy_out(&srcs, u),
                None => {
                    for s in srcs {
                        self.bk.src(s);
                    }
                }
            }
        }
        self.caller_locs.truncate(n);
        self.explicit_trait = prev;
        r
    }

    /// The receiver of a `call` is a closure, a fn pointer or a fn item, or a type parameter one of whose bounds is an
    /// `Fn` trait (behind references)
    #[cold]
    #[inline(never)]
    fn fn_like(&self, r: &Expr) -> bool {
        match self.peek(r).map(|t| self.strip_refs(t)) {
            Some(Ty::Struct(sid)) => {
                let sd = &self.prog.structs[sid as usize];
                // (std's `Fn` traits by name, not one of the program's that starts so (`FnLike`))
                let fn_trait = |b: &String| matches!(b.split(['<', '(']).next().map(str::trim), Some("Fn" | "FnMut" | "FnOnce"));
                sd.name.contains("{closure") || sd.placeholder.as_ref().is_some_and(|bs| bs.iter().any(fn_trait))
            }
            Some(Ty::Fn(..)) => true,
            _ => false,
        }
    }

    /// invalid_atomic_ordering (rustc 1.100's messages, at the ordering): a std atomic's `load` with `Release` /
    /// `AcqRel`, `store` with `Acquire` / `AcqRel`, `compare_exchange(_weak)` / `fetch_update` with a failure ordering
    /// of `Release` / `AcqRel`, written as `Ordering::X` or `X`
    #[cold]
    #[inline(never)]
    fn atomic_ordering(&self, r: &Expr, m: &str, args: &[Expr]) -> Option<String> {
        let Ty::Struct(sid) = self.strip_refs(self.peek(r)?) else { return None };
        if !self.prog.structs[self.prog.base_of(sid) as usize].name.starts_with("Atomic") {
            return None;
        }
        let (k, bad, msg) = match m {
            "load" => (0, ["Release", "AcqRel"], "atomic loads cannot have `Release` or `AcqRel` ordering".to_string()),
            "store" => (1, ["Acquire", "AcqRel"], "atomic stores cannot have `Acquire` or `AcqRel` ordering".to_string()),
            _ => (
                if m == "fetch_update" { 1 } else { 3 },
                ["Release", "AcqRel"],
                format!("`{m}`'s failure ordering may not be `Release` or `AcqRel`, since a failed `{m}` does not result in a write"),
            ),
        };
        let Expr::Path(None, p) = strip_paren(args.get(k)?) else { return None };
        let n = p.segs.last()?.name.as_str();
        // (std's `atomic::Ordering` is the prelude's `__AtOrd_`)
        let ordering = p.segs.len() == 1 || matches!(p.segs[p.segs.len() - 2].name.as_str(), "Ordering" | "__AtOrd_");
        (ordering && bad.contains(&n)).then(|| self.at(p.pos, msg))
    }

    /// An impl's method i is for t: its impl's self type (its parameters solved) is t (`impl A for W<u8>`'s is not
    /// W<u16>'s); one without a self type is
    /// Method id is inherent and every method of its name is in an impl for another instance of sid's type than sid,
    /// which has no `deref` to look further in
    #[cold]
    fn other_instances_only(&self, id: u32, sid: u32) -> bool {
        let t = Ty::Struct(sid);
        let sg = &self.prog.sigs[id as usize];
        // (only an instance of a generic type has impls for other instances; an impl's own self type fits)
        if self.prog.structs[sid as usize].base.is_none() || !sg.has_self || sg.impl_trait.is_some() || sg.self_ty.is_none_or(|st| st == t) {
            return false;
        }
        // (an impl for every instance, `impl<T> Vec<T>`)
        if let Some(Ty::Struct(ss)) = sg.self_ty {
            if self.prog.structs[ss as usize].args.iter().all(|a| matches!(a, Ty::Param(_))) {
                return false;
            }
        }
        let fits = |c: u32| {
            let sg = &self.prog.sigs[c as usize];
            sg.self_ty.is_none_or(|st| self.prog.match_ty(st, t, &mut vec![None; sg.generics.len().max(1)]))
        };
        !fits(id)
            && !self.prog.has_param(t)
            && !has_hole(self.prog, t)
            && self.prog.method(sid, "deref").is_none()
            && self.prog.overloads.get(&self.prog.names[id as usize]).is_none_or(|g| !g.iter().any(|&c| fits(c)))
    }

    pub(crate) fn self_fits(&self, i: u32, t: Ty) -> bool {
        let sg = &self.prog.sigs[i as usize];
        sg.self_ty.is_none_or(|st| self.prog.match_ty(st, t, &mut vec![None; sg.generics.len().max(1)]) && self.prog.refs_agree(st, t, 0))
    }

    /// rustc's method lookup of `x.name()`, x of type t, among the methods two traits of the program's give t's type
    /// (an impl's method of each, or a default of a trait t implements): `Err(())` when two traits' are at the first
    /// receiver candidate any method takes (E0034), `Ok(Some((trait, k)))` when one trait's is there, at candidate k
    /// (t derefed k / 3 times, then taken by value, `&` or `&mut`), `Ok(None)` for an inherent method or none
    #[cold]
    #[inline(never)]
    fn method_pick(&self, t: Ty, name: &str) -> Result<Option<(&str, usize)>, ()> {
        let base = self.strip_refs(t);
        // (the candidates, in order: t and each type it derefs to, each followed by its `&` and `&mut`. The method is
        // the one at the first candidate any method takes; only those there compete: `A::foo(&self)` beats
        // `B::foo(&mut self)`)
        let mut steps: Vec<Ty> = vec![];
        let mut u = t;
        loop {
            steps.extend([u, Ty::Ref(false, intern(u)), Ty::Ref(true, intern(u))]);
            match u {
                Ty::Ref(_, i) => u = *i,
                _ => break,
            }
        }
        // (match_ty does not tell `&` from `&mut`)
        fn same_refs(p: Ty, c: Ty) -> bool {
            match (p, c) {
                (Ty::Ref(m, a), Ty::Ref(n, b)) => m == n && same_refs(*a, *b),
                (Ty::Ref(..), _) => false,
                _ => true,
            }
        }
        // (the candidate a method's self type takes, its impl's parameters (`impl<T> W<T>`) or the trait's Self solved)
        let at = |p: Ty, self_ty: Option<Ty>, ng: usize| -> Option<usize> {
            steps.iter().position(|&c| {
                let mut sol = vec![None; ng.max(1)];
                sol[0] = self_ty;
                same_refs(p, c) && self.prog.match_ty(p, c, &mut sol)
            })
        };
        // (the first candidate so far, whether an inherent method is there, the traits whose methods are)
        let mut st: (usize, bool, Vec<&str>) = (usize::MAX, false, vec![]);
        fn add<'a>(st: &mut (usize, bool, Vec<&'a str>), i: usize, tr: Option<&'a str>) {
            if i < st.0 {
                *st = (i, false, vec![]);
            }
            if i == st.0 {
                match tr {
                    None => st.1 = true,
                    Some(tr) if !st.2.contains(&tr) => st.2.push(tr),
                    Some(_) => {}
                }
            }
        }
        let key = format!("{}::{name}", self.ty_key(base));
        let mut explicit: Vec<&str> = vec![];
        // (the traits with an impl for a reference (`impl A for &S`): a call is not rewritten to theirs)
        let mut of_ref: Vec<&str> = vec![];
        let cands = self.prog.index.get(&key).copied().into_iter().chain(self.prog.overloads.get(&key).into_iter().flatten().copied());
        for c in cands {
            let sg = &self.prog.sigs[c as usize];
            let Some(i) = sg.params.first().filter(|_| sg.has_self).and_then(|&p| at(p, None, sg.generics.len())) else {
                continue;
            };
            let tr = sg.impl_trait.as_deref().map(|tr| tr.split('<').next().unwrap_or(tr).trim());
            explicit.extend(tr);
            if matches!(sg.self_ty, Some(Ty::Ref(..))) {
                of_ref.extend(tr);
            }
            add(&mut st, i, tr);
        }
        for (tr, ds) in &self.prog.traits {
            if explicit.contains(&tr.as_str()) {
                continue;
            }
            let Some(d) = ds.iter().copied().find(|&d| {
                let sg = &self.prog.sigs[d as usize];
                sg.has_body && sg.has_self && self.prog.names[d as usize].rsplit("::").next() == Some(name)
            }) else {
                continue;
            };
            // (a blanket impl counts only without bounds: `impl<T: Display> A for T` may not cover t. An impl for t's
            // struct counts when it is for t's instance: `impl A for W<u8>` is not W<u16>'s)
            let concrete = self.prog.implements_concrete(base, tr) && self.prog.impl_applies(base, tr);
            if !(concrete || self.prog.unbounded_blankets.contains(tr)) {
                continue;
            }
            let sg = &self.prog.sigs[d as usize];
            // (a blanket impl's Self is what each candidate makes it: `A::foo(self)` of `impl<T> A for T` takes `&S` by
            // value on `(&S).foo()`)
            if let Some(i) = sg.params.first().and_then(|&p| at(p, concrete.then_some(base), sg.generics.len())) {
                add(&mut st, i, Some(tr));
            }
        }
        match st {
            (_, true, _) => Ok(None),
            (k, false, trs) => match trs.as_slice() {
                [tr] if !of_ref.contains(tr) => Ok(Some((tr, k))),
                [] | [_] => Ok(None),
                _ => Err(()),
            },
        }
    }

    /// `x.fmt(f)` (#256): Display's or Debug's fmt, which the impls keep as `S::<Display>::fmt`, not under `S::fmt`,
    /// so the ordinary lookup does not find it. The trait is the one the call names (`fmt::Debug::fmt(x, f)`), else
    /// the one of x's type's that is in scope (imported, or the trait of the impl the call is in, which rustc takes to
    /// be in scope there); of both, the impl's, else Display. A type with its own impl of it calls that (the lookup
    /// goes on with the trait named); the others (numbers, str, a derive) are written as `{}` / `{:?}` writes them.
    /// None: the ordinary lookup (a fmt of the type's own, a trait object, a type parameter)
    #[cold]
    #[inline(never)]
    fn fmt_method(&mut self, recv: &Expr, f: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Some(t) = self.peek(recv) else { return Ok(None) };
        if let Some(r) = self.radix_fmt_method(recv, t, f, expected)? {
            return Ok(Some(r));
        }
        let base = self.strip_refs(t);
        if matches!(base, Ty::Dyn(_) | Ty::Param(_) | Ty::Box(_)) || self.prog.has_param(base) {
            return Ok(None);
        }
        if let Ty::Struct(sid) = base {
            if self.prog.structs[sid as usize].placeholder.is_some() || self.prog.self_method(sid, "fmt").is_some() || self.prog.self_default_method(sid, "fmt").is_some() {
                return Ok(None);
            }
        }
        // (`x.fmt(f)` of a number with LowerHex / Binary / LowerExp .. the one fmt trait in scope (imported, or the impl's
        // own): that one's, `ff` of a u8 under `use std::fmt::LowerHex;`; with another in scope too, rustc's E0034, the
        // ordinary lookup)
        if self.explicit_trait.is_none() && (base.is_int() || base.is_float()) && !self.param_has_fmt_bound(recv) {
            let inside = self.prog.sigs.get(self.root as usize).and_then(|s| s.impl_trait.as_deref()).map(|t| t.split('<').next().unwrap_or(t).trim().rsplit("::").next().unwrap_or(t));
            const TRS: [&str; 8] = ["Display", "Debug", "LowerHex", "UpperHex", "Binary", "Octal", "LowerExp", "UpperExp"];
            let imp = self.prog.fmt_imports;
            let applies = |k: usize| k < 2 || k >= 6 || base.is_int();
            let scope: Vec<usize> = (0..8).filter(|&k| applies(k) && (imp & (1 << k) != 0 || inside == Some(TRS[k]))).collect();
            let pick = match inside.and_then(|t| scope.iter().copied().find(|&k| TRS[k] == t)) {
                Some(k) => Some(k),
                None if scope.len() == 1 => Some(scope[0]),
                None if scope.iter().any(|&k| k >= 2) => return Ok(None),
                None => None,
            };
            if let Some(k) = pick.filter(|&k| k >= 2) {
                // (as the trait's path writes it, the prelude's impl for the number: `LowerHex::fmt(&x, f)`, through x's
                // references; the prelude's trait under its own name where the program has one of that name)
                let shadowed = format!("__p_{}", TRS[k]);
                let tn = if self.prog.traits.contains_key(&shadowed) { shadowed } else { TRS[k].to_string() };
                let mut refs = 0;
                let mut u = t;
                while let Ty::Ref(_, i) = u {
                    refs += 1;
                    u = *i;
                }
                let (rv, rt) = self.expr(recv, None)?;
                let Some(rv) = rv else { return Ok(Some((None, Ty::Never))) };
                let (fv, ft) = self.expr(f, None)?;
                let Some(fv) = fv else { return Ok(Some((None, Ty::Never))) };
                self.scopes.push(vec![]);
                self.bind("__exr_xv".into(), rt, Some(rv));
                self.bind("__exr_xf".into(), ft, Some(fv));
                let e = crate::parse::parse_expr(&format!("{tn}::fmt(&{}__exr_xv, __exr_xf)", "*".repeat(refs)))?;
                let e = self.keep_expr(e);
                let r = self.expr(e, expected);
                self.scopes.pop();
                return r.map(Some);
            }
        }
        // (a `fmt` of a trait of the program's that the type implements, `impl Unsigned for u32 { fn fmt(self, buf) }`)
        let user_fmt = |tr: &String| {
            self.prog.traits.get(tr).is_some_and(|ds| ds.iter().any(|&d| self.prog.names[d as usize].rsplit("::").next() == Some("fmt"))) && self.prog.implements(base, tr)
        };
        // (not where Display / Debug is named, `fmt::Display::fmt(n, f)` of a number: the compiler's own, whatever `fmt` the
        // prelude's `impl LowerHex for u32` gives the number)
        let named_std = (base.is_int() || base.is_float() || matches!(base, Ty::Bool | Ty::Char | Ty::Str))
            && self.explicit_trait.as_ref().is_some_and(|(m, tr)| m == "fmt" && matches!(tr.as_str(), "Display" | "Debug"));
        if !named_std && (self.prog.prim_method(base, "fmt").is_some() || self.prog.blanket.contains_key("fmt") || self.prog.user_traits.iter().any(user_fmt)) {
            return Ok(None);
        }
        let tr = match self.explicit_trait.as_ref().filter(|(m, _)| m == "fmt") {
            Some((_, tr)) if matches!(tr.as_str(), "Display" | "Debug") => tr.clone(),
            Some(_) => return Ok(None),
            // (a value of a type parameter with a Display or Debug bound, `x: T` with `T: Debug`, in its instance: the
            // bound's, as rustc's lookup takes it, whatever is imported (bound_method_call's choice for other methods))
            None if self.param_fmt_bound(recv).is_some() => self.param_fmt_bound(recv).unwrap_or("Display").to_string(),
            None => {
                let inside = self.prog.sigs.get(self.root as usize).and_then(|s| s.impl_trait.as_deref()).map(|t| t.split('<').next().unwrap_or(t).trim().rsplit("::").next().unwrap_or(t));
                let (d, g) = (self.prog.implements(base, "Display"), self.prog.implements(base, "Debug"));
                // (the fmt traits in scope at the call, as rustc's lookup sees them: the impl's own, and those the
                // program imports (`use std::fmt::Debug;`). Of the program, not of this scope: one imported in another
                // module counts too, so where both are, the impl's trait decides, which is in scope here for sure;
                // with both in scope rustc's is E0034 (multiple applicable items), with neither E0599)
                let imp = self.prog.fmt_imports;
                let vd = d && (imp & 1 != 0 || inside == Some("Display"));
                let vg = g && (imp & 2 != 0 || inside == Some("Debug"));
                // (both imported and no impl to decide: which one this module sees is not known here (one module may
                // import Debug, another Display), so no pick, which could be the wrong one: the ordinary lookup, which
                // finds no method. Both in this one module is rustc's E0034 anyway)
                if vd && vg && !matches!(inside, Some("Display" | "Debug")) {
                    return Ok(None);
                }
                let pick = |d: bool, g: bool| match (d, g) {
                    (true, false) => Some("Display"),
                    (false, true) => Some("Debug"),
                    (true, true) if inside == Some("Debug") => Some("Debug"),
                    (true, true) => Some("Display"),
                    _ => None,
                };
                match pick(vd, vg).or_else(|| pick(d, g)) {
                    Some(t) => t.to_string(),
                    None => return Ok(None),
                }
            }
        };
        if let Ty::Struct(sid) = base {
            let own = |s: u32| format!("{}::<{tr}>::fmt", self.prog.structs[s as usize].name);
            if self.prog.index.contains_key(&own(sid)) || self.prog.index.contains_key(&own(self.prog.base_of(sid))) {
                self.explicit_trait = Some(("fmt".to_string(), tr));
                return Ok(None);
            }
        }
        let h = if tr == "Display" { "__exr_display_fmt" } else { "__exr_debug_fmt" };
        if !self.prog.index.contains_key(h) {
            return Ok(None);
        }
        let call = Expr::Call(Box::new(path_expr(h)), vec![Expr::Ref(false, Box::new(paren(recv.clone()))), f.clone()]);
        let call: &Expr = self.keep_expr(call);
        self.expr(call, expected).map(Some)
    }

    /// `fmt::Pointer::fmt(p, f)` of a reference / raw pointer / Box: written as `{:p}` writes it, with the formatter's
    /// width and flags. None for another receiver (a type's own impl: the ordinary lookup)
    #[cold]
    #[inline(never)]
    fn radix_fmt_method(&mut self, recv: &Expr, t: Ty, f: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Some(tr) = self.explicit_trait.as_ref().filter(|(m, _)| m == "fmt").map(|(_, tr)| tr.rsplit("::").next().unwrap_or(tr).to_string()) else {
            return Ok(None);
        };
        // (std's Pointer only: where the program has a Pointer of its own, std's is `__p_Pointer` and the program's
        // goes through its impls)
        if tr != if self.prog.traits.contains_key("__p_Pointer") { "__p_Pointer" } else { "Pointer" } {
            return Ok(None);
        }
        // (Pointer's receiver is `&self`: the pointer is what it points to, `&&u8`'s `&u8`)
        // (the pointer: `*recv` of a `&&u8` receiver, or the receiver itself, `r` of `fmt::Pointer::fmt(&r, f)` as the
        // method call has it)
        // (a fat one to a slice / str, `&str` / `&[T]` / `Box<[T]>`, too: `Pointer { addr, metadata }`; not a raw one, nor
        // one to a `dyn`)
        let ptr_like = |p: Ty| match p {
            Ty::Ref(_, i) | Ty::Box(i) => !i.is_unsized() || matches!(*i, Ty::Slice(_) | Ty::Str),
            Ty::Ptr(..) => !p.is_fat(),
            _ => false,
        };
        // (not `&E` of a type with its own Pointer impl, `Pointer::fmt(e_ref, f)`: E's impl, the ordinary lookup)
        let own = |p: Ty| matches!(p, Ty::Ref(_, Ty::Struct(_)));
        // (`&p` of a fat pointer the above does not take, a raw one (`*const [u8]`) or one to a `dyn`: p itself, its two
        // words read through the reference (never the address of the variable p, the outer reference's))
        let fat_behind = |p: Ty| {
            matches!(p, Ty::Ref(..) | Ty::Ptr(..) | Ty::Box(_)) && !ptr_like(p) && matches!(p, Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) if matches!(*i, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)))
        };
        let unsupported = |p: Ty| matches!(p, Ty::Ref(..) | Ty::Ptr(..) | Ty::Box(_)) && !ptr_like(p) && !fat_behind(p);
        let ptr_at: Option<(Ty, bool)> = match t {
            Ty::Ref(_, p) if ptr_like(*p) || fat_behind(*p) => Some((*p, true)),
            Ty::Ref(_, p) if unsupported(*p) => None,
            p if ptr_like(p) && !own(p) => Some((p, false)),
            _ => None,
        };
        if ptr_at.is_none() {
            return Ok(None);
        }
        self.explicit_trait = None;
        let (rv, rt) = self.expr(recv, None)?;
        let Some(rv) = rv else { return Ok(Some((None, Ty::Never))) };
        let (fv, ft) = self.expr(f, None)?;
        let Some(fv) = fv else { return Ok(Some((None, Ty::Never))) };
        self.scopes.push(vec![]);
        self.bind("__exr_xv".into(), rt, Some(rv));
        self.bind("__exr_xf".into(), ft, Some(fv));
        let text = {
            // (by the evaluated receiver's type, which match ergonomics may give one more reference than peek's)
            let (pt, through) = match rt {
                Ty::Ref(_, p) if ptr_like(*p) || fat_behind(*p) => (*p, true),
                Ty::Ref(_, p) if unsupported(*p) => ptr_at.unwrap(),
                p if ptr_like(p) && !own(p) => (p, false),
                _ => ptr_at.unwrap(),
            };
            let tn = self.prog.tn(match pt {
                Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) => *i,
                _ => unreachable!(),
            });
            let p = if through { "*__exr_xv" } else { "__exr_xv" };
            let star = if matches!(pt, Ty::Box(_)) { format!("&*{p}") } else { p.to_string() };
            let dyn_pt = matches!(pt, Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) if matches!(*i, Ty::Dyn(_)));
            if through && fat_behind(pt) || through && dyn_pt {
                let w = if matches!(rt, Ty::Ref(..)) { "__exr_xv" } else { "&__exr_xv" };
                let h = if dyn_pt { "__exr_dyn_pointer_fmt" } else { "__exr_fat_pointer_fmt" };
                format!("{{ let (a, m) = __exr_fat_words({w}); {h}::<()>(a, m, __exr_xf) }}")
            } else if pt.is_fat() {
                format!("{{ let s = {star}; __exr_fat_pointer_fmt::<()>(s.as_ptr() as usize, s.len(), __exr_xf) }}")
            } else {
                format!("__exr_pointer_fmt::<()>({star} as *const {tn} as usize, __exr_xf)")
            }
        };
        let e = crate::parse::parse_expr(&text)?;
        let e = self.keep_expr(e);
        let r = self.expr(e, expected);
        self.scopes.pop();
        r.map(Some)
    }

    /// Whether recv is a parameter of a type parameter with a fmt trait among its bounds (`x: T` with `T: UpperHex`), as
    /// param_fmt_bound finds it: what its bounds say, not the traits the program imports
    fn param_has_fmt_bound(&self, recv: &Expr) -> bool {
        if self.env.params.is_empty() || self.generic_args.is_empty() {
            return false;
        }
        let Expr::Path(None, rp) = strip_paren(recv) else { return false };
        let Some(rn) = ident_of(rp) else { return false };
        let Some(declared) = self.generic_args.iter().find(|a| a.0 == rn).map(|a| a.1.clone()) else { return false };
        if self.scopes.iter().rposition(|s| s.iter().any(|b| b.0 == rn)) != Some(0) {
            return false;
        }
        let Some(g) = self.prog.sigs.get(self.root as usize).and_then(|s| s.generics.iter().find(|g| g.0 == declared)) else { return false };
        g.1.iter().any(|b| {
            matches!(
                b.split('<').next().unwrap_or(b).rsplit("::").next().map(str::trim),
                Some("Display" | "Debug" | "LowerHex" | "UpperHex" | "Binary" | "Octal" | "LowerExp" | "UpperExp" | "Pointer")
            )
        })
    }

    /// The fmt trait a value of a type parameter takes from its bounds (`x: T` / `x: &T` with `T: Debug`), as
    /// bound_method_call finds the parameter: a parameter of the fn itself, not a `let` shadowing it. None without one
    /// such bound, or with both (rustc's E0034)
    fn param_fmt_bound(&self, recv: &Expr) -> Option<&'static str> {
        if self.env.params.is_empty() || self.generic_args.is_empty() {
            return None;
        }
        let Expr::Path(None, rp) = strip_paren(recv) else { return None };
        let rn = ident_of(rp)?;
        let declared = self.generic_args.iter().find(|a| a.0 == rn).map(|a| a.1.clone())?;
        if self.scopes.iter().rposition(|s| s.iter().any(|b| b.0 == rn)) != Some(0) {
            return None;
        }
        let bounds = &self.prog.sigs.get(self.root as usize)?.generics.iter().find(|g| g.0 == declared)?.1;
        let has = |n: &str| bounds.iter().any(|b| b.split('<').next().unwrap_or(b).rsplit("::").next().map(str::trim) == Some(n));
        match (has("Display"), has("Debug")) {
            (true, false) => Some("Display"),
            (false, true) => Some("Debug"),
            _ => None,
        }
    }

    /// In a generic fn's instance, the U of its bound `T: Into<U>` whose T is t there (foldhash's `x.into() << n` with
    /// `T: Into<u128>`): what `x.into()` of a `x: T` converts to when nothing is expected of it (#256). None without
    /// such a bound, or with two that disagree
    fn into_bound_target(&self, t: Ty) -> Option<Ty> {
        if self.env.params.is_empty() {
            return None;
        }
        let sig = self.prog.sigs.get(self.root as usize)?;
        let mut found = None;
        for (i, (g, bs)) in sig.generics.iter().enumerate() {
            // (x's type as the type inference left it, the parameter (by its index alone), or as the instance makes it)
            let hit = match t {
                Ty::Param(k) => k as usize == i,
                _ => self.env.params.iter().any(|(n, it)| n == g && *it == t),
            };
            if !hit {
                continue;
            }
            for b in bs {
                let n = b.split('<').next().unwrap_or(b).trim();
                if n.rsplit("::").next() != Some("Into") || !b.contains('<') {
                    continue;
                }
                let f = crate::parse::parse_file(&format!("type __B = dyn {b};")).ok()?;
                let Some(crate::parse::ItemKind::Type(_, _, _, Some(crate::parse::Ty::Dyn(dbs)))) = f.items.first().map(|i| &i.kind) else { return None };
                let Some(crate::parse::Bound::Trait(_, p)) = dbs.first() else { return None };
                let Some(crate::parse::GArg::Ty(a)) = p.segs.last()?.args.first() else { return None };
                let u = self.parse_ty(a).ok()?;
                if found.is_some_and(|x| x != u) {
                    return None;
                }
                found = Some(u);
            }
        }
        found
    }

    fn method_call_inner(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::MethodCall(recv, method, gargs, margs, msp) = e else { unreachable!() };
        let msp = *msp;
        // (E0624: a private fn of the receiver's inherent impl, where the program has such)
        if self.prog.field_priv.as_deref().is_some_and(|fp| !fp.methods.is_empty()) {
            if let Some(Ty::Struct(sid)) = self.peek(recv).map(|t| self.strip_refs(t)) {
                self.method_privacy(sid, method, msp.at)?;
            }
        }
        if method == "fmt" && margs.len() == 1 && gargs.is_empty() {
            if let Some(r) = self.fmt_method(recv, &margs[0], expected)? {
                return Ok(r);
            }
        }
        // (`0.0.cmp()` with `impl MyCmp for f32` only: an unsuffixed literal takes the one numeric type whose impl has
        // the method, when its default type (f64 / i32) has none)
        if let Expr::Lit(t, sp) = strip_paren(recv) {
            let (default, cands): (Ty, &[Ty]) = if is_float_lit(recv) {
                (Ty::F64, &[Ty::F32])
            } else if is_int_lit(recv) && matches!(lit::lit(t), Some(Lit::Int(i)) if i.suffix().is_empty()) {
                (Ty::I32, &[Ty::I8, Ty::I16, Ty::I64, Ty::I128, Ty::Isize, Ty::U8, Ty::U16, Ty::U32, Ty::U64, Ty::U128, Ty::Usize])
            } else {
                (Ty::Unit, &[])
            };
            if !cands.is_empty() && self.prog.prim_method(default, method).is_none() {
                let mut found = cands.iter().filter(|&&c| self.prog.prim_method(c, method).is_some());
                if let (Some(&c), None) = (found.next(), found.next()) {
                    let lit = Expr::Lit(format!("{t}{}", self.prog.tn(c)), *sp);
                    let ne: &Expr = self.keep_expr(Expr::MethodCall(Box::new(lit), method.clone(), gargs.clone(), margs.clone(), msp));
                    return self.expr(ne, expected);
                }
            }
        }
        // (E0034: `x.foo()` that two traits of the program's give x's type, with no inherent `foo`. Not in a generic fn's
        // instance: there `s.scaled()` of `S: Shape0` is Shape0's, by the bound, whatever else S's instance implements)
        if self.user_code && self.explicit_trait.is_none() && !self.prog.has_mods && self.env.params.is_empty() && !self.prog.ambiguous_methods.is_empty() {
            let maybe = self.prog.ambiguous_methods.get(method.as_str()).and_then(|ts| {
                let t = self.peek(recv).filter(|&t| !self.prog.has_param(t))?;
                let k = match self.strip_refs(t) {
                    Ty::Struct(sid) => Ty::Struct(self.prog.base_of(sid)),
                    k => k,
                };
                ts.contains(&k).then_some(t)
            });
            if let Some(t) = maybe {
                match self.method_pick(t, method) {
                    Err(()) => return err(SP, "multiple applicable items in scope (E0034)"),
                    // (the one trait's at the first candidate: the call is lowered as written, with that trait named
                    // for its lookup (as `Tr::m(x)` names it), so its receiver's borrow, its position and its type
                    // arguments are the method call's: `m.bar()` is `Shared::bar(&self)`'s, not the other trait's
                    // `bar(&mut self)`)
                    // (but the reference itself by value, `(&s).foo()` of a blanket impl's default `foo(self)` with
                    // Self = &S: that default for &S, the receiver as it is (a copy, no borrow taken))
                    // (only a default taking `self` by value: one taking `&self` / `&mut self` goes the ordinary way,
                    // which records the receiver's loan)
                    Ok(Some((tr, 0)))
                        if matches!(t, Ty::Ref(..))
                            && self.trait_default(tr, method, t).is_some_and(|d| !matches!(self.prog.sigs[d as usize].params.first(), Some(Ty::Ref(..))))
                            && self.trait_candidate(&format!("{}::{method}", self.ty_key(self.strip_refs(t))), tr).is_none() =>
                    {
                        let d = self.trait_default(tr, method, t).unwrap();
                        self.check_unsafe_call(d, msp.start)?;
                        let (v, vt) = self.expr(recv, None)?;
                        let Some(v) = v else { return Ok((None, Ty::Never)) };
                        let args: Vec<&Expr> = margs.iter().collect();
                        let tf: Option<&[GArg]> = (!gargs.is_empty()).then_some(&gargs[..]);
                        return self.call_fn(d, Some((v, vt)), &args, tf, expected, SP);
                    }
                    Ok(Some((tr, _))) => self.explicit_trait = Some((method.clone(), tr.to_string())),
                    Ok(_) => {}
                }
            }
        }
        // (`x.m()` with x of a type parameter `T: Tr` in a generic fn, instantiated as a primitive: Tr's `m`, which a
        // builtin method of the same name (`to_string` ..) would otherwise answer: `<T as Tr>::m(&x, ..)`)
        // (`w.m()` with w of the program's struct with a `DerefMut`, and m a `&mut self` method of what it derefs to, not
        // w's own: `(&mut *w).m()`, as autoderef takes `deref_mut` for it, before the rewrite of a deref to a slice below
        // (`c.reverse()` of a `C: DerefMut<Target = [i32]>`); w is borrowed mutably (E0596 without `mut`,
        // E0502 against a shared loan of it) and the user's `deref_mut` runs, not `deref`)
        if let Some((t0, (refs, target))) = (!self.prog.deref_mut_of.is_empty()).then(|| self.peek(recv)).flatten().and_then(|t| Some((t, self.deref_mut_recv(t, method)?))) {
            let mut d = recv.clone();
            let at = expr_start(recv);
            for _ in 0..=refs {
                d = Box::new(Expr::Unary("*".into(), d, crate::parse::Span { start: at, at }));
            }
            let inner = Expr::Paren(Box::new(Expr::Ref(true, d)));
            let rt = Ty::Ref(true, intern(target));
            let ne: &Expr = self.keep_expr(Expr::MethodCall(Box::new(inner), method.to_string(), gargs.clone(), margs.clone(), msp));
            // (the new receiver's type, for what looks the method up by it: String's and Vec's builtin methods; and each
            // deref's, so that the user's deref is called once, on the place: `**w` of a w: &mut W)
            if let Expr::MethodCall(r, ..) = ne {
                self.lets.insert(&**r as *const Expr as usize, rt);
                if let Expr::Paren(i) = &**r {
                    self.lets.insert(&**i as *const Expr as usize, rt);
                    if let Expr::Ref(_, u) = &**i {
                        // (outermost first: the target, then the struct, then what each reference or box reaches)
                        let mut reached = vec![];
                        let mut tt = t0;
                        while let Ty::Ref(_, x) | Ty::Box(x) = tt {
                            reached.push(*x);
                            tt = *x;
                        }
                        let mut u = &**u;
                        for ty in std::iter::once(target).chain(reached.into_iter().rev()) {
                            let Expr::Unary(_, x, _) = u else { break };
                            self.lets.insert(u as *const Expr as usize, ty);
                            u = &**x;
                        }
                    }
                }
            }
            return self.expr(ne, expected);
        }
        // (`impl Deref<Target = [T; N]>` (or a slice, str) with no method m of its own: `t.m()` is `t.deref().m()`,
        // decided before anything is evaluated, so the user's deref runs once; only for the structs Program found such a
        // deref of)
        if let Some(Ty::Struct(sid)) = self.peek(recv).map(|t| self.strip_refs(t)).filter(|t| matches!(t, Ty::Struct(s) if !self.prog.deref_to_seq.is_empty() && self.prog.deref_to_seq.contains(&self.prog.base_of(*s)))) {
            if self.prog.self_method(sid, method).is_none() && self.prog.self_default_method(sid, method).is_none() {
                if let Some(did) = self.prog.method(sid, "deref") {
                    if let Ty::Ref(_, target @ (Ty::Array(..) | Ty::Slice(_) | Ty::Str)) = self.prog.sigs[did as usize].ret {
                        // (`deref_mut` for a method that takes `&mut self` (`c.reverse()`), as autoderef takes it)
                        let m = if self.prog.method(sid, "deref_mut").is_some() && self.takes_mut_self(*target, method) { "deref_mut" } else { "deref" };
                        let d = Expr::MethodCall(Box::new(Expr::Paren(recv.clone())), m.to_string(), vec![], vec![], msp);
                        let ne: &Expr = self.keep_expr(Expr::MethodCall(Box::new(d), method.to_string(), gargs.clone(), margs.clone(), msp));
                        return self.expr(ne, expected);
                    }
                }
            }
        }
        // (`g.m()` with g a RefCell's Ref / RefMut or a MutexGuard, and m not the guard's own: `(*g).m()`, so the value's
        // methods are found as for the value itself, the builtin ones of String / Vec / slices too)
        if let Some((sid, target, deref_mut)) = self.peek(recv).and_then(|t| self.guard_of(self.strip_refs(t))) {
            // (`(&*g).m()`, or `(&mut *g).m()` for a RefMut's when m takes `&mut self` (as autoderef picks `deref` or
            // `deref_mut`): a method that takes self by value moves out of a reference, an error (E0507) as through
            // any reference)
            if self.prog.self_method(sid, method).is_none() && self.prog.self_default_method(sid, method).is_none() {
                let mutable = deref_mut && self.takes_mut_self(target, method);
                let deref = Expr::Unary("*".into(), recv.clone(), Default::default());
                let inner = Expr::Paren(Box::new(Expr::Ref(mutable, Box::new(deref))));
                let target = Ty::Ref(mutable, intern(target));
                let ne: &Expr = self.keep_expr(Expr::MethodCall(Box::new(inner), method.to_string(), gargs.clone(), margs.clone(), msp));
                // (the new receiver's type, for what looks the method up by it: String's and Vec's builtin methods)
                if let Expr::MethodCall(r, ..) = ne {
                    self.lets.insert(&**r as *const Expr as usize, target);
                    if let Expr::Paren(i) = &**r {
                        self.lets.insert(&**i as *const Expr as usize, target);
                    }
                }
                return self.expr(ne, expected);
            }
        }
        if let Some(ne) = self.bound_method_call(recv, method, gargs, margs) {
            let ne: &Expr = self.keep_expr(ne);
            return self.expr(ne, expected);
        }
        // (unused_allocation: `Box::new([1]).len()`, a `&self` method of the array / slice the box holds)
        if self.prog.deny_unused_alloc && self.user_code && is_box_new(recv) {
            if let Some(Ty::Box(Ty::Array(..) | Ty::Slice(_) | Ty::Str)) = self.peek(recv) {
                if matches!(method.as_str(), "len" | "is_empty" | "iter" | "first" | "last" | "get" | "contains" | "to_vec" | "starts_with" | "ends_with") {
                    return err(expr_start(recv), "unnecessary allocation, use `&` instead");
                }
            }
        }
        // (`b.m()` with b: Box<[T]>: the slice's method, through `&*b` / `&mut *b`)
        let local_ty = match &**recv {
            Expr::Path(None, p) => ident_of(p).and_then(|n| self.lookup(n)).map(|x| x.1),
            _ => None,
        };
        if let Some(Ty::Box(Ty::Slice(el))) = local_ty.or_else(|| self.peek(recv)) {
            let mutating = matches!(method.as_str(), "sort" | "sort_unstable" | "sort_by" | "sort_by_key" | "reverse" | "swap" | "fill" | "iter_mut" | "copy_from_slice" | "clone_from_slice" | "rotate_left" | "rotate_right" | "as_mut_ptr" | "first_mut" | "last_mut" | "get_mut" | "split_at_mut" | "chunks_mut");
            if !matches!(method.as_str(), "clone" | "into_vec" | "into_iter") {
                let r = Expr::Ref(mutating, Box::new(Expr::Unary("*".into(), recv.clone(), Default::default())));
                let ne: &Expr = self.keep_expr(Expr::MethodCall(Box::new(paren(r)), method.to_string(), gargs.clone(), margs.clone(), msp));
                // (the new receiver's type, for what looks the method up by it)
                if let Expr::MethodCall(r, ..) = ne {
                    let rt = Ty::Ref(mutating, intern(Ty::Slice(el)));
                    self.lets.insert(&**r as *const Expr as usize, rt);
                    if let Expr::Paren(i) = &**r {
                        self.lets.insert(&**i as *const Expr as usize, rt);
                    }
                }
                return self.expr(ne, expected);
            }
        }
        // (`b.len()` / `b.iter()` with b: Box<[T; N]>: the array's method, through the deref)
        if matches!(local_ty.or_else(|| self.peek(recv)), Some(Ty::Box(Ty::Array(..))))
            && !matches!(method.as_str(), "clone" | "into" | "into_iter" | "to_owned" | "eq" | "ne" | "fmt" | "hash" | "cmp" | "partial_cmp")
            && self.prog.prim_method(local_ty.or_else(|| self.peek(recv)).unwrap(), method).is_none()
        {
            let d = Expr::Unary("*".into(), recv.clone(), Default::default());
            // (a method that changes the elements: on `&mut (*b)[..]`)
            let mutating = matches!(method.as_str(), "sort" | "sort_unstable" | "sort_by" | "sort_by_key" | "reverse" | "swap" | "fill" | "iter_mut" | "copy_from_slice" | "clone_from_slice" | "rotate_left" | "rotate_right" | "as_mut_ptr" | "first_mut" | "last_mut" | "get_mut" | "split_at_mut" | "chunks_mut");
            let r = if mutating {
                Expr::Ref(true, Box::new(Expr::Index(Box::new(paren(d)), Box::new(Expr::Range(None, None, false)), Default::default())))
            } else {
                d
            };
            let ne: &Expr = self.keep_expr(Expr::MethodCall(
                Box::new(paren(r)),
                method.to_string(),
                gargs.clone(),
                margs.clone(),
                msp,
            ));
            // (the new receiver's type, for what looks the method up by it)
            if let (Some(Ty::Box(at @ Ty::Array(el, _))), Expr::MethodCall(r, ..)) = (local_ty.or_else(|| self.peek(recv)), ne) {
                let rt = if mutating { Ty::Ref(true, intern(Ty::Slice(el))) } else { *at };
                self.lets.insert(&**r as *const Expr as usize, rt);
                if let Expr::Paren(i) = &**r {
                    self.lets.insert(&**i as *const Expr as usize, rt);
                }
            }
            return self.expr(ne, expected);
        }
        // `let b: Box<_> = 22.into();` (`From<T> for Box<T>`): `Box::new(22)`
        if method == "into" && margs.is_empty() && matches!(expected, Some(Ty::Box(_))) {
            let rt = self.peek(recv);
            if rt.is_none_or(|t| !matches!(t, Ty::Box(_) | Ty::Ref(..) | Ty::Str | Ty::Slice(_)) && !matches!(t, Ty::Struct(sid) if self.prog.method(sid, "into").is_some())) {
                let seg = |n: &str| crate::parse::Seg { name: n.to_string(), args: vec![], fn_args: None, text: n.to_string() };
                let bn = crate::parse::Path { global: false, segs: vec![seg("Box"), seg("new")], pos: Default::default() };
                let c = Expr::Call(Box::new(Expr::Path(None, bn)), vec![(**recv).clone()]);
                let c = self.keep_expr(c);
                return self.expr(c, expected);
            }
        }
        // `2.eq(&3)` / `x.lt(&y)` on a number, bool or char: the operator (`2 == *&3`)
        if margs.len() == 1 {
            let op = match &method[..] {
                "eq" => "==",
                "ne" => "!=",
                "lt" => "<",
                "le" => "<=",
                "gt" => ">",
                "ge" => ">=",
                _ => "",
            };
            if !op.is_empty() && self.peek(recv).is_some_and(|t| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char)) {
                let rhs = Expr::Unary("*".into(), Box::new(paren(margs[0].clone())), crate::parse::Span::default());
                let b = Expr::Binary(op.into(), Box::new(paren((**recv).clone())), Box::new(rhs), msp);
                let b = self.keep_expr(b);
                return self.expr(b, expected);
            }
        }
        // (`v[1..].iter()`: 範囲の添字の受け手は参照にする (自動参照)。文字列は str のメソッドが受ける)
        if let Expr::Index(b, ix, _) = strip_paren(recv) {
            let bt = self.peek(b).map(|t| self.strip_refs(t));
            let is_text = bt.is_some_and(|t| t == Ty::Str || matches!(t, Ty::Struct(sid) if self.prog.structs[sid as usize].name == "String"));
            if matches!(&**ix, Expr::Range(..)) && !is_text && method != "try_into" {
                let ne = Expr::MethodCall(Box::new(Expr::Ref(false, Box::new(strip_paren(recv).clone()))), method.clone(), gargs.clone(), margs.clone(), crate::parse::Span::default());
                let ne = self.keep_expr(ne);
                return self.method_call(ne, expected);
            }
        }
        let recv: &Expr = recv;
        let method = method.as_str();
        if matches!(method, "unwrap" | "expect") {
            if let (Some(t), Expr::MethodCall(r2, m2, _, a2, _)) = (expected, strip_paren(recv)) {
                if m2 == "try_into" && a2.is_empty() {
                    // (`999.try_into().expect(..)`: 型に収まる整数リテラルなら定数)
                    if let (true, Expr::Lit(l, _)) = (t.is_int(), strip_paren(r2)) {
                        if let Some(Lit::Int(i)) = lit::lit(l) {
                            if let Some(v) = i.value().filter(|&v| v <= i128::MAX as u128).map(|v| v as i128) {
                                let lo = crate::program::prim_assoc_const(t, "MIN").unwrap_or(0);
                                let hi = crate::program::prim_assoc_const(t, "MAX").unwrap_or(0);
                                if (t == Ty::U128 && v >= 0) || (lo <= v && v <= hi) {
                                    return Ok((Some(self.int_const(t, v)), t));
                                }
                            }
                        }
                    }
                    self.try_into_hint = Some(t);
                }
            }
        }
        // `s.get_unchecked(i)` / `get_unchecked_mut(a..)` (配列・スライス・Vec): `&s[i]` / `&mut s[a..]` (範囲は検査する)
        if matches!(method, "get_unchecked" | "get_unchecked_mut") && margs.len() == 1 {
            let seq = match self.peek(recv).map(|t| self.strip_refs(t)) {
                Some(Ty::Array(..) | Ty::Slice(_)) => true,
                Some(Ty::Struct(sid)) => self.vec_fields(sid).is_some(),
                _ => false,
            };
            if seq {
                self.check_unsafe_op(method, msp.at)?;
                let ix = Expr::Index(Box::new(paren(recv.clone())), Box::new(margs[0].clone()), Default::default());
                let ne = Expr::Ref(method == "get_unchecked_mut", Box::new(ix));
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        // `x.as_ref()` (AsRef: 文字列・Vec・配列・スライス): 先を期待型か、関数の境界 `T: AsRef<X>` で決める
        if method == "as_ref" && margs.is_empty() {
            if let Some(ne) = self.as_ref_rewrite(recv, expected) {
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        // `s.as_mut()` (AsMut<str> of std's String / str): `s.as_mut_str()` / `&mut *s`, of a receiver that is one of
        // them or `&mut` to one (a Vec's / array's / slice's below: as_mut_of_vec)
        if method == "as_mut" && margs.is_empty() {
            if let Some(ne) = self.as_mut_of_text(recv) {
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        let tf: Option<&[GArg]> = (!gargs.is_empty()).then_some(&gargs[..]);
        // `x.unwrap()` / `x.expect(..)`: panic の場所 (メソッド名の位置) を先に書いておく (#[track_caller] 相当)
        // (and `c.borrow()` / `c.replace(..)` .. of a RefCell, whose overlapping borrow panics at the call, as in std)
        let refcell = matches!(method, "borrow" | "borrow_mut" | "replace" | "replace_with" | "take" | "swap")
            && self.user_code
            && self.peek(recv).is_some_and(|t| self.is_refcell(t));
        if (refcell || matches!(method, "unwrap" | "expect" | "unwrap_err" | "expect_err")) && self.prog.panic_handler.is_none() {
            if let Some(&ci) = self.prog.const_index.get("__EXR_CALLER") {
                let (l, c) = self.loc(msp.at);
                let loc = ((l as i64) << 16) | c as i64;
                let v = self.b.ins().iconst(types::I64, loc);
                let a = self.static_sym(ci);
                self.b.ins().store(mf(), v, a, 0);
                // (and again right before the call (call_fn_inner): a call in the receiver or the arguments,
                // `c.borrow().get(9).unwrap()`, stores its own location in between)
                self.caller_locs.push((method.to_string(), loc));
            }
        }
        // 受け手が `a.as_slice()` (a: 配列) なら `(&a[..])` にして呼び直す (`a.as_slice().len()`)
        // (an array's `as_mut()` too: its one AsMut is `AsMut<[T]>`, `seed.as_mut().iter_mut()` in rand_core)
        if let Expr::MethodCall(ir, iname, _, iargs, _) = strip_paren(recv) {
            // (and a String's / str's `as_mut()`, `s.as_mut().len()`: `(s.as_mut_str())`, as the call alone is lowered,
            // so the method is the str's)
            if iargs.is_empty() && iname == "as_mut" {
                if let Some(nr) = self.as_mut_of_text(ir) {
                    // (the call's span: an unsafe method's check needs it)
                    let m2 = Expr::MethodCall(Box::new(paren(nr)), method.to_string(), gargs.clone(), margs.clone(), msp);
                    return self.method_call(&m2, expected);
                }
            }
            if iargs.is_empty() && matches!(iname.as_str(), "as_slice" | "as_mut_slice" | "as_mut") {
                if let Some(Ty::Array(..)) = self.peek(ir).map(|t| self.strip_refs(t)) {
                    let r = (**ir).clone();
                    let ix = Expr::Index(Box::new(paren(r)), Box::new(full_range()), crate::parse::Span::default());
                    let nr = paren(Expr::Ref(iname != "as_slice", Box::new(ix)));
                    let m2 = Expr::MethodCall(Box::new(nr), method.to_string(), gargs.clone(), margs.clone(), crate::parse::Span::default());
                    return self.method_call(&m2, expected);
                }
            }
        }
        // `[T; N]::as_slice()` / `as_mut_slice()`: `&a[..]` / `&mut a[..]`
        // (and a slice's `as_mut()` (`T: AsMut<[u8]> + ?Sized` at `[u8]`), a Vec's where it is its AsMut<[T]>, or
        // its AsMut<Vec<T>>, the Vec itself: as_mut_of_vec)
        if margs.is_empty() && matches!(method, "as_slice" | "as_mut_slice" | "as_mut") {
            let array = match self.peek(recv).map(|t| self.strip_refs(t)) {
                Some(Ty::Array(..)) => true,
                Some(Ty::Slice(_)) => method == "as_mut",
                _ => false,
            };
            let of_vec = if method == "as_mut" && !array { self.as_mut_of_vec(recv, expected) } else { None };
            if array || matches!(of_vec, Some((true, _))) {
                let ix = Expr::Index(Box::new(paren(recv.clone())), Box::new(full_range()), crate::parse::Span::default());
                let e = Expr::Ref(method != "as_slice", Box::new(ix));
                return self.expr(&e, expected);
            }
            if of_vec.is_some() {
                // (`&mut *v` through the receiver's references: `&mut **r` of an `r: &mut Vec<T>`)
                let mut e = paren(recv.clone());
                let mut t = self.peek(recv);
                while let Some(Ty::Ref(_, i)) = t {
                    e = paren(Expr::Unary("*".into(), Box::new(e), crate::parse::Span::default()));
                    t = Some(*i);
                }
                return self.expr(&Expr::Ref(true, Box::new(e)), expected);
            }
        }
        // `x.hash(state)` (x: タプル / Option で Hash の impl がない): std と同じく要素を順に (Option は判別子 (isize) と中身)
        if method == "hash" && margs.len() == 1 {
            if let Some(Ty::Struct(sid)) = self.peek(recv).map(|t| self.strip_refs(t)) {
                let st = &margs[0];
                // (参照ならそのまま使う: ローカル変数の参照を新たに取らない)
                let rv: Expr = if matches!(self.peek(recv), Some(Ty::Ref(..))) {
                    recv.clone()
                } else {
                    Expr::Ref(false, Box::new(paren(recv.clone())))
                };
                if self.prog.method(sid, "hash").is_none() {
                    if self.prog.is_tuple(sid) {
                        let n = self.prog.structs[sid as usize].fields.len();
                        let mut stmts = vec![let_stmt("__hs", st.clone()), let_stmt("__hv", rv)];
                        for i in 0..n {
                            let f = paren(Expr::Field(Box::new(path_expr("__hv")), i.to_string(), crate::parse::Span::default()));
                            let hs = Expr::Ref(true, Box::new(Expr::Unary("*".into(), Box::new(path_expr("__hs")), crate::parse::Span::default())));
                            stmts.push(Stmt::Expr(mcall(f, "hash", vec![hs]), true));
                        }
                        let e = Expr::Block(None, false, PBlock { stmts, spos: Default::default() });
                        return self.expr(&e, expected);
                    }
                    if self.prog.structs[self.prog.base_of(sid) as usize].name == "Option" {
                        let mut e = crate::parse::parse_expr(
                            "{
                                let __hs = 0;
                                match 0 {
                                    None => (0isize).hash(&mut *__hs),
                                    Some(__hx) => {
                                        (1isize).hash(&mut *__hs);
                                        (*__hx).hash(&mut *__hs)
                                    }
                                }
                            }",
                        )?;
                        // (テンプレートの `0` の所に、状態と受け手を入れる)
                        if let Expr::Block(_, _, b) = &mut e {
                            if let Some(Stmt::Let(_, _, init, _, ..)) = b.stmts.get_mut(0) {
                                *init = Some(st.clone());
                            }
                            if let Some(Stmt::Expr(Expr::Match(s, _), _)) = b.stmts.get_mut(1) {
                                **s = rv;
                            }
                        }
                        return self.expr(&e, expected);
                    }
                }
            }
        }
        // `it.flatten()` (要素が &Vec<T> / &[T] / &[T; N]): `it.flat_map(|x| x.iter())`
        if method == "flatten" && margs.is_empty() {
            let item = self.peek(recv).filter(|&t| !self.prog.has_param(t)).and_then(|t| self.prog.assoc_of(t, "Item"));
            let by_ref = match item {
                Some(Ty::Ref(_, Ty::Slice(_) | Ty::Array(..))) => true,
                Some(Ty::Ref(_, Ty::Struct(sid))) => self.vec_fields(*sid).is_some(),
                _ => false,
            };
            if let (true, Some(Ty::Ref(_, inner))) = (by_ref, item) {
                let el = match inner {
                    Ty::Slice(e) | Ty::Array(e, _) => **e,
                    Ty::Struct(sid) => self.prog.structs[*sid as usize].args[0],
                    _ => unreachable!(),
                };
                let (v, t) = self.expr(recv, None)?;
                let Some(v) = v else { return Ok((None, Ty::Never)) };
                return self.call_named("__exr_flatten_ref", vec![t, el], &[(v, t)], SP);
            }
        }
        // `v.get(a..b)` (Vec): スライスにしてから (`(&v[..]).get(a..b)`)
        if (method == "get" || method == "get_mut")
            && matches!(margs.first(), Some(Expr::Range(..)))
            && matches!(self.peek(recv).map(|t| self.strip_refs(t)), Some(Ty::Struct(sid)) if self.vec_fields(sid).is_some())
        {
            let r: Expr = if matches!(self.peek(recv), Some(Ty::Ref(..))) {
                paren(Expr::Unary("*".into(), Box::new(recv.clone()), crate::parse::Span::default()))
            } else {
                recv.clone()
            };
            let ix = Expr::Index(Box::new(paren(r)), Box::new(full_range()), crate::parse::Span::default());
            let ne = Expr::MethodCall(Box::new(ix), method.to_string(), vec![], margs.clone(), crate::parse::Span::default());
            let t0 = self.lets.get(&(e as *const Expr as usize)).copied();
            if let Some(t) = t0 {
                self.lets.insert(&ne as *const Expr as usize, t);
            }
            let r = self.expr(&ne, expected);
            if t0.is_some() {
                self.lets.remove(&(&ne as *const Expr as usize));
            }
            return r;
        }
        // `v[a..b].iter()` / `s[1..].len()`: 範囲の添字の受け手は自動で借用する (書き換えるメソッドは &mut)
        if let Expr::Index(_, ix, _) = strip_paren(recv) {
            if matches!(&**ix, Expr::Range(..))
                && !matches!(
                    method,
                    "sort" | "sort_unstable" | "sort_by" | "sort_unstable_by" | "sort_by_key" | "sort_unstable_by_key" | "sort_by_cached_key"
                )
            {
                let mutating = matches!(
                    method,
                    "reverse" | "swap" | "fill" | "rotate_left" | "rotate_right" | "iter_mut" | "copy_from_slice" | "clone_from_slice" | "split_at_mut"
                );
                let nr = paren(Expr::Ref(mutating, Box::new(recv.clone())));
                let ne = Expr::MethodCall(Box::new(nr), method.to_string(), gargs.clone(), margs.clone(), crate::parse::Span::default());
                // (型推論が元の呼び出しに記録した型を、作り直したノードにも写す: `s[..4].try_into()`)
                let t0 = self.lets.get(&(e as *const Expr as usize)).copied();
                if let Some(t) = t0 {
                    self.lets.insert(&ne as *const Expr as usize, t);
                }
                let r = self.expr(&ne, expected);
                if t0.is_some() {
                    self.lets.remove(&(&ne as *const Expr as usize));
                }
                return r;
            }
        }
        // 値としての `[a, b].into_iter()`: Vec にしてから (配列の変数で要素が Copy なら iter().copied())
        if method == "into_iter" && margs.is_empty() {
            if let Expr::Array(es) = strip_paren(recv) {
                let e = mcall(vec_of(es), "into_iter", vec![]);
                return self.expr(&e, expected);
            }
            if let Some(Ty::Array(el, _)) = self.peek(recv) {
                if !self.needs_drop(*el) && self.prog.struct_index.contains_key("Copied") {
                    let e = mcall(mcall(paren(recv.clone()), "iter", vec![]), "copied", vec![]);
                    return self.expr(&e, expected);
                }
            }
        }
        // ブランケット impl のメソッドをプリミティブ型・&str で呼ぶ (`5.describe()` / `"x".describe()`)
        // (and `()`, and a reference to one (`r.use_mut()` with `r: &mut ()`): Self is the referent, and the reference
        // itself is the receiver, as rustc's probe matches `&mut self` with it)
        if let Some(&bid) = self.prog.blanket.get(method) {
            let pt = self.peek(recv);
            let prim = |t: Ty| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char);
            // (or the reference's own impl, `impl Tr for &u8` (`u8::__ref_m`): rustc's probe takes the receiver's type
            // as it is first)
            let own = match pt.map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.self_method(sid, method).is_some(),
                _ => false,
            } || matches!(pt, Some(Ty::Ref(_, i)) if (prim(*i) || *i == Ty::Str) && self.prog.index.contains_key(&format!("{}::__ref_{method}", self.prog.tn(*i))))
                // (and a number's own impl, `impl MaybeIntoWasmAbi for u32` beside `impl<T: OptionIntoWasmAbi> .. for T`)
                || pt.is_some_and(|t| prim(t) && self.prog.index.get(&format!("{}::{method}", self.prog.tn(t))).is_some_and(|&i| i != bid));
            // (`()`, and what is behind a reference (a `Box` too: Self is the box, before the deref to what it
            // holds), when the impl's bounds hold of it: `impl<T: ?Sized> FutureExt for T where T: Future` is not
            // `()`'s)
            let bsig = &self.prog.sigs[bid as usize];
            let bounds = bsig.trait_of.as_deref().or(bsig.impl_trait.as_deref()).and_then(|tr| self.prog.blanket_bounds.get(tr));
            let prog = self.prog;
            let holds = |t: Ty| bounds.is_none_or(|bs| blanket_impls_hold(prog, t, bs, &mut vec![], &mut vec![]));
            let extra = |t: Ty| (prim(t) || matches!(t, Ty::Unit | Ty::Box(_))) && holds(t);
            let to_prim = matches!(pt, Some(Ty::Ref(_, i)) if extra(*i));
            // (a number's only when the blanket impl's bounds hold of it)
            if !own && pt.is_some_and(|t| (prim(t) && holds(t)) || matches!(t, Ty::Ref(_, Ty::Str)) || (t == Ty::Unit && holds(t))) || (!own && to_prim) {
                // (an `unsafe fn` of the trait: E0133 outside `unsafe`, as for any method)
                self.check_unsafe_call(bid, msp.start)?;
                let (v, t) = self.expr(recv, None)?;
                if t == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                let args: Vec<&Expr> = margs.iter().collect();
                if let (Ty::Ref(m, i), Some(v)) = (t, v) {
                    if extra(*i) {
                        // (a `&self` method through a `&mut`: the reference is shared for it; `self` by value keeps it
                        // as it is)
                        let shared_self = matches!(self.prog.sigs[bid as usize].params.first(), Some(Ty::Ref(false, _)));
                        let rt = Ty::Ref(m && !shared_self, i);
                        // (the receiver is reborrowed: `*r`, which a result that borrows from self holds
                        // (`let r2 = r.get(); *r = 5;` while r2 is used is E0506))
                        let t0 = self.bk.call_temps.len();
                        if self.bk_on() {
                            // (`self` by value of a reference whose result is Self (`fn id(self) -> Self`): the result
                            // may be the reborrow. Not another parameter (`fn pick<U>(self, v: U) -> U`))
                            let bs = &self.prog.sigs[bid as usize];
                            let by_value = !matches!(bs.params.first(), Some(Ty::Ref(..))) && mentions_param0(self.prog, bs.ret, 0);
                            let flows = by_value || self.bk_ret_from(bid, 0);
                            let ev = self.bk_recv_ev(recv, rt, flows);
                            // (a `&mut` receiver whose arguments name it (`r.touch({ *r = 5; })`): its loan lasts while
                            // they are evaluated)
                            let hold = match (&ev, borrowck::root_local(recv)) {
                                (Some(borrowck::Ev::Acc(_, borrowck::Acc::Borrow(ln), _)), Some(n)) if matches!(rt, Ty::Ref(true, _)) && margs.iter().any(|a| borrowck::mentions(a, n)) => Some(*ln),
                                _ => None,
                            };
                            self.bk_push(ev);
                            if let Some(ln) = hold {
                                self.bk_call_temp(&[borrowck::Src::Loan(ln)], true);
                            }
                        }
                        let r = self.call_fn(bid, Some((v, rt)), &args, tf, expected, SP);
                        self.bk_call_temps_end(t0);
                        return r;
                    }
                }
                // (`self` by value, `fn into_maybe_abi(self)`: the value itself, Self the number)
                if !matches!(self.prog.sigs[bid as usize].params.first(), Some(Ty::Ref(..))) && t != Ty::Unit {
                    if let Some(v) = v {
                        return self.call_fn(bid, Some((v, t)), &args, tf, expected, SP);
                    }
                }
                // (&self に渡すのは値のアドレス。&str は fat pointer の置き場所のアドレスがそのまま &&str; `()` は
                // 1 バイトの置き場所)
                let addr = match v {
                    Some(v) if t.is_fat() => v,
                    Some(v) => {
                        let ss = self.slot(t);
                        self.b.ins().stack_store(v, ss, 0);
                        self.b.ins().stack_addr(PTR, ss, 0)
                    }
                    None if t == Ty::Unit => {
                        let ss = self.slot(Ty::U8);
                        self.b.ins().stack_addr(PTR, ss, 0)
                    }
                    None => return Ok((None, Ty::Never)),
                };
                return self.call_fn(bid, Some((addr, Ty::Ref(false, intern(t)))), &args, tf, expected, SP);
            }
            // (`self` by value on a reference to a struct the blanket impl covers as it is, `(&s).into_maybe_abi()` of
            // `impl<T: OptionIntoWasmAbi> MaybeIntoWasmAbi for T` with `&S: OptionIntoWasmAbi`: Self is the reference,
            // as rustc's probe takes the receiver's type first; no move out of it)
            // (not when the struct's inherent method takes the reference as it is, `fn pick(&self)` for `(&s).pick()`:
            // an inherent method comes first at the same receiver type)
            let by_value = !matches!(self.prog.sigs[bid as usize].params.first(), Some(Ty::Ref(..)));
            if let Some(rt @ Ty::Ref(rm, Ty::Struct(sid))) = pt {
                let ref_own = self.prog.method(*sid, &format!("__ref_{method}")).is_some();
                let inherent = self.prog.self_method(*sid, method).is_some_and(|id| {
                    let s = &self.prog.sigs[id as usize];
                    s.impl_trait.is_none() && s.trait_of.is_none() && matches!(s.params.first(), Some(Ty::Ref(m, _)) if *m == rm)
                });
                if by_value && !ref_own && !inherent && bounds.is_some() && holds(rt) {
                    self.check_unsafe_call(bid, msp.start)?;
                    let (v, t) = self.expr(recv, None)?;
                    let Some(v) = v else { return Ok((None, Ty::Never)) };
                    let args: Vec<&Expr> = margs.iter().collect();
                    return self.call_fn(bid, Some((v, t)), &args, tf, expected, SP);
                }
            }
        }
        // `b.as_ref()` / `b.as_mut()` (b: Box<T> / &Box<T>): 中身への参照 (Box と参照は同じ表現)
        if matches!(method, "as_ref" | "as_mut") && margs.is_empty() {
            let mut t = self.peek(recv);
            while let Some(Ty::Ref(_, i)) = t {
                t = Some(*i);
            }
            if let Some(Ty::Box(inner)) = t {
                let (v, mut vt) = self.read_no_move(recv)?;
                let Some(mut v) = v else { return Ok((None, Ty::Never)) };
                while let Ty::Ref(_, i) = vt {
                    v = if i.is_fat() { v } else { self.b.ins().load(PTR, mf(), v, 0) };
                    vt = *i;
                }
                return Ok((Some(v), Ty::Ref(method == "as_mut", inner)));
            }
        }
        // prelude の `v.__push_auto(x)` (partition など): x が `&T` で v が `Vec<T>` なら `*x` を push
        if method == "__push_auto" && margs.len() == 1 {
            let a = &margs[0];
            let elem = match self.peek(recv).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.vec_fields(sid).map(|f| f.2),
                _ => None,
            };
            let e = match (self.peek(a), elem) {
                (Some(Ty::Ref(_, i)), Some(el)) if *i == el => {
                    mcall(paren(recv.clone()), "push", vec![Expr::Unary("*".into(), Box::new(paren(a.clone())), crate::parse::Span::default())])
                }
                _ => mcall(paren(recv.clone()), "push", vec![a.clone()]),
            };
            return self.expr(&e, expected);
        }
        // `os_string.push("..")` (OsString は String): 文字列を足す
        if method == "push" && margs.len() == 1 {
            let is_string = |t: Option<Ty>| {
                matches!(t.map(|t| self.strip_refs(t)), Some(Ty::Struct(sid)) if self.prog.structs[sid as usize].base.is_none() && self.prog.structs[sid as usize].name == "String")
            };
            let a = &margs[0];
            let at = self.peek(a);
            if is_string(self.peek(recv)) && (matches!(at.map(|t| self.strip_refs(t)), Some(Ty::Str)) || (matches!(at, Some(Ty::Ref(..))) && is_string(at))) {
                let e = mcall(paren(recv.clone()), "push_str", vec![a.clone()]);
                return self.expr(&e, expected);
            }
        }
        // prelude の `s.__push_piece(c)` (String への collect): 要素の型で push / push_str
        if method == "__push_piece" && margs.len() == 1 {
            let a = &margs[0];
            let r = paren(recv.clone());
            let e = match self.peek(a).map(|t| self.strip_refs(t)) {
                Some(Ty::Char) if matches!(self.peek(a), Some(Ty::Ref(..))) => mcall(r, "push", vec![Expr::Unary("*".into(), Box::new(paren(a.clone())), crate::parse::Span::default())]),
                Some(Ty::Char) | None => mcall(r, "push", vec![a.clone()]),
                Some(Ty::Str) => mcall(r, "push_str", vec![a.clone()]),
                _ => mcall(r, "push_str", vec![mcall(paren(a.clone()), "as_str", vec![])]),
            };
            return self.expr(&e, expected);
        }
        // `x.into()`: 期待型の `T::from(x)` (String は String::from、整数は広げるキャスト)
        if method == "into" && margs.is_empty() {
            let recv_own = match self.peek(recv) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, "into").is_some(),
                // (a `&S` whose S has an inherent `into(&self)`: rustc's probe takes it at the reference itself, before
                // the blanket `Into` of `&S`, so the ordinary lookup calls it (its side effects too). Not one taking
                // `self`, which is no candidate there, nor through two references)
                Some(Ty::Ref(_, Ty::Struct(sid))) => {
                    self.prog.method(*sid, "into").is_some_and(|id| self.prog.sigs[id as usize].has_self && matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(..))))
                }
                _ => false,
            };
            let target = expected.or_else(|| self.lets.get(&(e as *const Expr as usize)).copied());
            // (into its own type, std's `impl<T> From<T> for T`: the value itself. `true.into()` as the `bool` a
            // `T: Into<u64>` fn returns stays a bool, which the bound below must not make a u64)
            let rt = self.peek(recv);
            if let (Some(t), Some(r), false) = (target, rt, recv_own) {
                if t == r && !self.prog.has_param(t) {
                    return self.expr(recv, Some(t));
                }
            }
            // (else, with no number or struct expected of it, the U of the fn's bound `T: Into<U>` on x's type: in an
            // instance the receiver is the parameter's type (u8), and foldhash's `x.into() << n` expects no useful type)
            let target = match (target, rt) {
                (Some(t), _) if t.is_int() || t.is_float() || matches!(t, Ty::Struct(_)) => Some(t),
                (_, Some(r)) => self.into_bound_target(r).or(target),
                _ => target,
            };
            if !recv_own {
                match target {
                    // `"msg".into()` / `e.into()` → Box<dyn Error>
                    Some(Ty::Struct(sid)) if self.prog.structs[sid as usize].name == "__DynError" => {
                        let (v, t) = self.expr(recv, None)?;
                        let Some(v) = v else { return Ok((None, Ty::Never)) };
                        return self.call_named("__exr_dyn_error", vec![t], &[(v, t)], SP);
                    }
                    Some(Ty::Struct(sid)) => {
                        if let Some(id) = self.prog.method(sid, "from") {
                            // (an unsized receiver, `b"ab"[..].into()` as a `Cow<[u8]>`: by reference, as the method
                            // probe takes it (`From<&[T]>`))
                            let recv: &Expr = if matches!(self.peek(recv), Some(Ty::Slice(_) | Ty::Str)) { self.keep_expr(Expr::Ref(false, Box::new(paren(recv.clone())))) } else { recv };
                            let args: [&Expr; 1] = [recv];
                            return self.call_fn(id, None, &args, None, Some(Ty::Struct(sid)), SP);
                        }
                    }
                    Some(t) if t.is_int() || t.is_float() => {
                        // (a value of a type of the program's, `impl From<J> for f64`: that `from`; a number's is the
                        // widening cast)
                        let num = self.peek(recv).is_some_and(|a| a.is_int() || a.is_float() || matches!(a, Ty::Bool | Ty::Char));
                        let tn = self.prog.tn(t);
                        let e = if !num && self.trait_candidate(&format!("{tn}::from"), "From").is_some() {
                            let mut fp = path_of(&tn);
                            fp.segs.push(crate::parse::Seg { name: "from".into(), args: vec![], fn_args: None, text: "from".into() });
                            Expr::Call(Box::new(Expr::Path(None, fp)), vec![recv.clone()])
                        } else {
                            Expr::Cast(Box::new(paren(recv.clone())), ty_named(&tn))
                        };
                        return self.expr(&e, Some(t));
                    }
                    _ => {}
                }
            }
        }
        // `x.try_into()` into `Result<U, E>` where U implements TryFrom (`impl TryFrom<&str> for Alphabet`): `U::try_from(x)`,
        // as std's blanket `impl<T, U: TryFrom<T>> TryInto<U> for T` gives it. Integers and slices are handled elsewhere
        if method == "try_into" && margs.is_empty() {
            // (a receiver's own try_into, also through references (`(&source).try_into()`), is that method)
            let recv_own = match self.peek(recv).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, "try_into").is_some(),
                _ => false,
            };
            let target = expected.or_else(|| self.lets.get(&(e as *const Expr as usize)).copied());
            // (`let a: Alphabet = "x".try_into().unwrap()`: the target is the unwrap's, left in try_into_hint)
            if !recv_own && target.is_none() {
                if let Some(Ty::Struct(uid)) = self.try_into_hint {
                    if let Some(id) = self.prog.method(uid, "try_from") {
                        self.try_into_hint = None;
                        let args: [&Expr; 1] = [recv];
                        return self.call_fn(id, None, &args, None, None, SP);
                    }
                }
            }
            if !recv_own {
                if let Some(Ty::Struct(rs)) = target {
                    if self.prog.structs[rs as usize].name.starts_with("Result") {
                        if let Some(&Ty::Struct(uid)) = self.prog.structs[rs as usize].args.first() {
                            if let Some(id) = self.prog.method(uid, "try_from") {
                                let args: [&Expr; 1] = [recv];
                                return self.call_fn(id, None, &args, None, target, SP);
                            }
                        }
                    }
                }
            }
        }
        // (and to a number or another type whose `try_from` is the program's impl of TryFrom, `impl TryFrom<J> for u32`:
        // `u32::try_from(x)`; between numbers, the builtin one)
        // (not a receiver's own try_into, `impl Source { fn try_into(&self) }`)
        let own_try_into = || matches!(self.peek(recv).map(|t| self.strip_refs(t)), Some(Ty::Struct(sid)) if self.prog.method(sid, "try_into").is_some());
        if method == "try_into" && margs.is_empty() && !own_try_into() {
            let target = expected.or_else(|| self.lets.get(&(e as *const Expr as usize)).copied());
            let num = |a: Ty| a.is_int() || a.is_float() || matches!(a, Ty::Bool | Ty::Char);
            let num_recv = self.peek(recv).is_some_and(num);
            if let Some(Ty::Struct(rs)) = target {
                let r = &self.prog.structs[rs as usize];
                if self.prog.structs[self.prog.base_of(rs) as usize].name == "Result" && !r.args.is_empty() {
                    let t = r.args[0];
                    let key = format!("{}::try_from", self.ty_key(t));
                    if !self.prog.has_param(t) && !(num_recv && num(t)) && self.trait_candidate(&key, "TryFrom").is_some() {
                        let mut fp = path_of(&self.prog.tn(t));
                        if let Ty::Struct(sid) = t {
                            fp = path_of(&self.prog.structs[self.prog.base_of(sid) as usize].name);
                        }
                        fp.segs.push(crate::parse::Seg { name: "try_from".into(), args: vec![], fn_args: None, text: "try_from".into() });
                        let c = self.keep_expr(Expr::Call(Box::new(Expr::Path(None, fp)), vec![recv.clone()]));
                        return self.expr(c, expected);
                    }
                }
            }
        }
        // `x.to_string()` (Display を実装した型。&str は str_method が String::from にする)
        if method == "to_string" && margs.is_empty() && self.prog.index.contains_key("__exr_display") {
            if let Some(t) = self.peek(recv) {
                let mut b = t;
                while let Ty::Ref(_, i) = b {
                    b = *i;
                }
                let own = match b {
                    Ty::Struct(sid) => self.prog.method(sid, "to_string").is_some(),
                    _ => false,
                };
                if !own && b != Ty::Str && self.fmt_ok(t, false) {
                    return self.to_string_value(recv, SP);
                }
            }
        }
        if let Some(r) = self.ord_method(e)? {
            return Ok(r);
        }
        if let Some(r) = self.extend_call(e, expected)? {
            return Ok(r);
        }
        if let Some(r) = self.clone_call(e)? {
            return Ok(r);
        }
        // `x.clamp(lo, hi)` (整数も Ord な型も): prelude の __exr_clamp
        if method == "clamp" && margs.len() == 2 && self.prog.index.contains_key("__exr_clamp") {
            let own = match self.peek(recv).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, "clamp").is_some(),
                Some(_) => false,
                // (型が分からない: 自分の型に clamp があるときだけ普通のメソッド呼び出しにする)
                None => self.prog.names.iter().any(|n| n.ends_with("::clamp")),
            };
            if !own {
                let e = Expr::Call(Box::new(path_expr("__exr_clamp")), vec![recv.clone(), margs[0].clone(), margs[1].clone()]);
                return self.expr(&e, expected);
            }
        }
        // `a.max(b)` / `a.min(b)` (Ord を実装した struct / タプル / String): prelude の __exr_max / __exr_min
        if matches!(method, "max" | "min") && margs.len() == 1 && self.prog.index.contains_key("__exr_max") {
            if let Some(Ty::Struct(sid)) = self.peek(recv) {
                if self.prog.self_method(sid, method).is_none() && self.prog.self_default_method(sid, method).is_none() {
                    let f = if method == "max" { "__exr_max" } else { "__exr_min" };
                    let e = Expr::Call(Box::new(path_expr(f)), vec![recv.clone(), margs[0].clone()]);
                    return self.expr(&e, expected);
                }
            }
        }
        // (str / スライスへの trait 実装は、組み込みのメソッドより先に: `"x".hash(&mut h)`)
        if self.peek(recv).is_some_and(|t| matches!(self.strip_refs(t), Ty::Str | Ty::Slice(_) | Ty::Array(..))) {
            if let Some(r) = self.prim_trait_method(e, expected)? {
                return Ok(r);
            }
        }
        if let Some(r) = self.builtin_method(e)? {
            self.rg_builtin(e, recv, r.1);
            return Ok(r);
        }
        if let Some(r) = self.prim_trait_method(e, expected)? {
            return Ok(r);
        }
        // (メソッドがなく、Deref の先が構造体でない (`PathBuf` → str): `recv.deref().m(..)` にして str などのメソッドを探す)
        if let Some(Ty::Struct(sid)) = self.peek(recv).map(|t| self.strip_refs(t)) {
            if self.prog.self_method(sid, method).is_none() && self.prog.self_default_method(sid, method).is_none() {
                if let Some(did) = self.prog.method(sid, "deref") {
                    if matches!(self.prog.sigs[did as usize].ret, Ty::Ref(_, Ty::Str | Ty::Slice(_))) {
                        let d = mcall(paren(recv.clone()), "deref", vec![]);
                        let e = Expr::MethodCall(Box::new(d), method.to_string(), gargs.clone(), margs.clone(), crate::parse::Span::default());
                        return self.expr(&e, expected);
                    }
                }
            }
        }
        // (`15_isize.add(6)` with `use std::ops::Add`: the operator trait's method on a number is the operator, when no
        // trait of the program gives the number that method)
        if margs.len() == 1 && gargs.is_empty() {
            let op = match &method[..] {
                "add" => "+",
                "sub" => "-",
                "mul" => "*",
                "div" => "/",
                "rem" => "%",
                "bitand" => "&",
                "bitor" => "|",
                "bitxor" => "^",
                "shl" => "<<",
                "shr" => ">>",
                _ => "",
            };
            if !op.is_empty() {
                if let Some(t) = self.peek(recv) {
                    if (t.is_int() || t.is_float()) && self.prog.prim_self_method(t, method).is_none() {
                        let b = Expr::Binary(op.to_string(), Box::new(paren((*recv).clone())), Box::new(paren(margs[0].clone())), msp);
                        let b: &Expr = self.keep_expr(b);
                        return self.expr(b, expected);
                    }
                }
            }
        }
        // (`r.into_iter()` with r: `&Vec<T>` / `&HashMap<..>` (a generic fn's `I: IntoIterator`): what `for x in r`
        // does, `r.iter()` (or the collection's `__ref_into_iter`); `Vec::into_iter` takes the collection by value)
        if method == "into_iter" && margs.is_empty() {
            if let Some(Ty::Ref(false, Ty::Struct(sid))) = self.peek(recv) {
                let sid = *sid;
                let by_value = self.prog.method(sid, "into_iter").is_some_and(|id| !matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(..))));
                if by_value {
                    let m = if self.prog.method(sid, "__ref_into_iter").is_some() { "__ref_into_iter" } else { "iter" };
                    if self.prog.method(sid, m).is_some() {
                        let ne: &Expr = self.keep_expr(mcall(Expr::Paren(Box::new(recv.clone())), m, vec![]));
                        return self.expr(ne, expected);
                    }
                    return err(SP, "cannot move out of a shared reference (E0507): `into_iter` takes the collection by value");
                }
            }
        }
        // (`it.into_iter()` with `it` an iterator with no into_iter of its own (`(0..3).into_iter()`, a Map ..): std's
        // `impl<I: Iterator> IntoIterator for I`, the iterator itself)
        if method == "into_iter" && margs.is_empty() {
            if let Some(Ty::Struct(sid)) = self.peek(recv) {
                if self.prog.method(sid, "into_iter").is_none() && self.prog.method(sid, "next").is_some() {
                    return self.expr(recv, expected);
                }
            }
        }
        // (`f.m()` with f a fn pointer and `impl Tr for fn(..)`: the `__fnptr` method whose self type is f's)
        // (or with f a `&[T; N]` / `&[T]` and `impl<T> Tr for &[T; N]`: the method of that impl)
        let ref_seq = |t: Ty| matches!(t, Ty::Ref(_, Ty::Array(..) | Ty::Slice(_)) | Ty::Array(..));
        if let Some(ft) = local_ty.filter(|&t| ref_seq(t)).or_else(|| self.peek(recv)).filter(|&t| matches!(t, Ty::Fn(_) | Ty::Ptr(..)) || ref_seq(t)) {
            let id = self.prog.prim_method(ft, method).filter(|&id| match ft {
                Ty::Ref(..) if ref_seq(ft) => matches!(self.prog.sigs[id as usize].self_ty, Some(Ty::Ref(..))),
                // (a by-value impl for arrays: `impl<T> Tr for [T; 3]`, whose method takes self)
                Ty::Array(..) => matches!(self.prog.sigs[id as usize].self_ty, Some(Ty::Array(..))) && self.prog.sigs[id as usize].has_self,
                _ => true,
            });
            if let Some(id) = id {
                let (v, t) = self.expr(recv, Some(ft))?;
                let v = v.ok_or("internal: fn pointer without a value")?;
                let (rv, rt) = match self.prog.sigs[id as usize].params.first() {
                    // (`&self` of the receiver's type; `self` of an impl for `&[T; 3]` is the reference itself)
                    Some(Ty::Ref(m, pi)) if std::mem::discriminant(*pi) == std::mem::discriminant(&ft) => {
                        let tmp = self.temp(t);
                        self.b.ins().store(mf(), v, tmp, 0);
                        (tmp, Ty::Ref(*m, intern(t)))
                    }
                    _ => (v, t),
                };
                let args: Vec<&Expr> = margs.iter().collect();
                return self.call_fn(id, Some((rv, rt)), &args, None, expected, SP);
            }
        }
        // (`(*p).m()` with p: `&dyn Tr` / `*const dyn Tr`: the vtable call on the fat pointer itself)
        if let Expr::Unary(op, inner, sp) = strip_paren(recv) {
            if op == "*" {
                if let Some(t @ (Ty::Ref(_, Ty::Dyn(tr)) | Ty::Ptr(_, Ty::Dyn(tr)))) = self.peek(inner) {
                    if matches!(t, Ty::Ptr(..)) && self.unsafe_ok == 0 && sp.start.line != 0 {
                        return err(sp.start, "dereference of raw pointer is unsafe and requires unsafe block (E0133)");
                    }
                    if let (Some(v), vt) = self.expr(inner, None)? {
                        // (a copy of the pair: an argument that reassigns the pointer must not change the receiver)
                        let tmp = self.temp(vt);
                        self.copy(tmp, v, vt);
                        return self.dyn_method_call_on(tmp, tr, e, Some(vt), expected);
                    }
                }
            }
        }
        let mut recv_temp = None;
        // (the hidden local that holds a temporary receiver's loans for its drop (bk_drop_temp): a `self` by value
        // takes it, so its loans end at the call, `D(&x).consume()`)
        let mut recv_holder: Option<u32> = None;
        // (whether the receiver was reached through a reference: a `self` by value there moves out of it)
        let mut through_ref = false;
        // (a Box value that is not a place: `Box::new(s).m()`, for `self: Box<Self>`)
        let mut recv_box: Option<(Value, Ty)> = None;
        // (the reference the struct is reached through last, for self_method_at: None by value, a Box or a deref; a
        // reference value that is not a place (`(&mut s).f()`) is taken off here already)
        let mut wrap: Option<bool> = None;
        let mut p = if is_place_expr(recv) {
            self.place(recv)?
        } else {
            // (a temporary receiver whose drop may use what it borrows (`D(&*v).iter()` with `impl Drop for D`): it
            // holds its loans until the end of the statement, as a field's base does (struct_base))
            let watch = self.bk_on() && self.bk.level >= 2 && self.user_code;
            if watch {
                self.bk_frame();
            }
            let r = self.expr(recv, None);
            let srcs = if watch { self.bk_frame_end() } else { vec![] };
            let r = r?;
            if !srcs.is_empty() && self.dropck(r.1, 0) {
                recv_holder = Some(self.bk_drop_temp(r.1, srcs));
            }
            // 一時値の受け手は文の終わりまで所有する (値で受けるメソッドならムーブ)
            if let (Some(v), t) = r {
                if t != Ty::Never {
                    recv_temp = self.own_temp(v, t);
                }
            }
            match r {
                (_, Ty::Never) => return Ok((None, Ty::Never)),
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(v), t @ (Ty::Ref(_, i) | Ty::Box(i))) => {
                    through_ref |= matches!(t, Ty::Ref(..));
                    if matches!(t, Ty::Box(_)) {
                        recv_box = Some((v, t));
                    }
                    if let Ty::Ref(m, _) = t {
                        wrap = Some(m);
                    }
                    Place::Mem(v, 0, *i)
                }
                // (a vector value whose type inference left open (`eq1.and(eq2).movemask()`, `and` of two traits): bound to
                // a local of its type, and the method called on that, as on any vector local (its receiver's borrow and
                // regions checked as theirs: a `&self` result does not outlive the statement)
                // (and a raw pointer's, `L::LUT.as_ptr().cast()` of an associated const of a type parameter: its builtin
                // methods (#256))
                (Some(v), t @ (Ty::Vec(_) | Ty::Ptr(..))) => {
                    // (the temporary's loan, as prim_trait_method records it: a `&self` result does not outlive it)
                    if let Some(id) = self.prog.prim_self_method(t, method) {
                        if let (Some(Ty::Ref(m, _)), true) = (self.prog.sigs[id as usize].params.first().copied(), self.bk_on()) {
                            if self.bk_ret_from(id, 0) {
                                self.bk_temp_borrow(recv, m, t);
                            }
                        }
                    }
                    let n = "__exr_vrecv".to_string();
                    self.scopes.push(vec![]);
                    self.bind(n.clone(), t, Some(v));
                    let ne: &Expr = self.keep_expr(Expr::MethodCall(Box::new(path_expr(&n)), method.to_string(), gargs.clone(), margs.clone(), msp));
                    // (a pointer's `cast()` with no turbofish is to what the type inference gave the call, or to what is
                    // expected of it, `_mm_loadu_si128(p.cast())`)
                    if let Ty::Ptr(..) = t {
                        if let Some(rt) = self.lets.get(&(e as *const Expr as usize)).copied().filter(|x| matches!(x, Ty::Ptr(..))).or(expected.filter(|x| matches!(x, Ty::Ptr(..)))) {
                            self.lets.insert(ne as *const Expr as usize, rt);
                        }
                    }
                    let r = self.expr(ne, expected);
                    self.scopes.pop();
                    return r;
                }
                (_, t) => {
                    return err(SP, format!("no method `{}` on type `{}`", method, self.prog.tn(t)));
                }
            }
        };
        let name = method.to_string();
        // (the receiver before any deref: a `self: Pin<&mut Self>` method takes the Pin itself)
        let p0 = p;
        // (メソッドがなく `impl Deref` があれば、deref した先で探す: `rc.borrow_mut()`)
        let mut via_deref = false;
        // (the first type reached through whose `Deref` has no `DerefMut` (`Rc`): what follows is shared)
        let mut shared_deref: Option<String> = None;
        let mut derefs = 0;
        // (the receiver's type before the derefs: `(*self).into_abi()` of a `&str` that type inference did not record
        // reaches `str`; `impl Tr for &str`'s `str::__ref_into_abi` is its method)
        let p0_ty = p.ty();
        // (the receiver's place as evaluated, before the derefs below: a fallback that calls an impl for the reference
        // takes its value from here, not from the receiver evaluated again (`refs[next()]` once))
        let p_recv = p;
        // (or `&Box<str>`'s, `Box::__ref_into_abi` among the impls for references to Boxes: the one whose self type
        // is the receiver's)
        let ref_impl = |t: Ty| -> Option<String> {
            let Ty::Ref(_, it) = t else { return None };
            let base = match *it {
                Ty::Box(_) => "Box".to_string(),
                it if it.is_int() || it.is_float() || matches!(it, Ty::Bool | Ty::Char | Ty::Str) => self.prog.tn(it),
                _ => return None,
            };
            let f = format!("{base}::__ref_{method}");
            let &first = self.prog.index.get(&f)?;
            let fits = |c: u32| self.prog.sigs[c as usize].self_ty.is_none_or(|st| self.prog.match_ty_exact(st, t, &mut vec![None; self.prog.sigs[c as usize].generics.len()]));
            let c = std::iter::once(first).chain(self.prog.overloads.get(&f).into_iter().flatten().copied()).find(|&c| fits(c))?;
            Some(self.prog.names[c as usize].clone())
        };
        let ref_call = ref_impl(p0_ty).filter(|_| is_place_expr(recv));
        let (a, o, sid, id) = loop {
            let (a, o, sid) = loop {
                wrap = match p.ty() {
                    Ty::Ref(m, _) => Some(m),
                    Ty::Struct(_) => wrap,
                    _ => None,
                };
                p = match p {
                    Place::Mem(a, o, Ty::Struct(sid)) => break (a, o, sid),
                    Place::Mem(a, o, rt @ (Ty::Ref(_, Ty::Dyn(tr)) | Ty::Box(Ty::Dyn(tr)))) => {
                        let fat = self.addr_plus(a, o);
                        return self.dyn_method_call_on(fat, tr, e, Some(rt), expected);
                    }
                    // (a `dyn Tr` place (the tail of a struct, `x.inner.get()`): its place is its pair)
                    Place::Mem(a, o, t @ Ty::Dyn(tr)) => {
                        let fat = self.addr_plus(a, o);
                        return self.dyn_method_call_on(fat, tr, e, Some(Ty::Ref(true, intern(t))), expected);
                    }
                    // (a reference to a struct with an unsized tail: the place of its pair, as struct_base takes it)
                    Place::Mem(a, o, Ty::Ref(_, i) | Ty::Box(i)) if i.is_dst_struct() => {
                        through_ref |= matches!(p.ty(), Ty::Ref(..));
                        Place::Mem(self.addr_plus(a, o), 0, *i)
                    }
                    Place::Var(v, Ty::Ref(_, i) | Ty::Box(i)) if i.is_dst_struct() => {
                        through_ref |= matches!(p.ty(), Ty::Ref(..));
                        Place::Mem(self.b.use_var(v), 0, *i)
                    }
                    Place::Mem(a, o, Ty::Ref(_, i) | Ty::Box(i)) => {
                        through_ref |= matches!(p.ty(), Ty::Ref(..));
                        let ptr = self.b.ins().load(PTR, mf(), a, o);
                        Place::Mem(ptr, 0, *i)
                    }
                    Place::Var(v, Ty::Ref(_, i) | Ty::Box(i)) => {
                        through_ref |= matches!(p.ty(), Ty::Ref(..));
                        let ptr = self.b.use_var(v);
                        Place::Mem(ptr, 0, *i)
                    }
                    p => {
                        // (the reference the receiver's place holds, read once: a thin one (a `&str` is a pair, which the
                        // paths below pass))
                        let recv_ref = |this: &mut Self| match p_recv {
                            Place::Var(v, t @ Ty::Ref(..)) if !t.is_fat() => Some(this.b.use_var(v)),
                            Place::Mem(a, o, t @ Ty::Ref(..)) if !t.is_fat() => Some(this.b.ins().load(PTR, mf(), a, o)),
                            _ => None,
                        };
                        if let Some(f) = &ref_call {
                            let id = self.prog.index.get(f.split('#').next().unwrap_or(f)).copied();
                            let chosen = self.prog.names.iter().position(|n| n == f).map(|i| i as u32);
                            // (an `unsafe fn` of the impl: E0133 at the method call outside `unsafe`, as for the others)
                            if let Some(c) = chosen.or(id) {
                                self.check_unsafe_call(c, msp.start)?;
                                if let Some(v) = recv_ref(self) {
                                    let args: Vec<&Expr> = margs.iter().collect();
                                    return self.call_fn(c, Some((v, p0_ty)), &args, tf, expected, SP);
                                }
                            }
                        }
                        // (a receiver whose reference is not read here: the call written again, for one evaluated again
                        // without effects, `refs[i]`, not `refs[next()]`)
                        if let Some(f) = ref_call.as_ref().filter(|_| self.pure_place(recv)) {
                            let mut args = vec![recv.clone()];
                            args.extend(margs.iter().cloned());
                            let id = self.prog.index.get(f.split('#').next().unwrap_or(f)).copied();
                            let c = self.keep_expr(Expr::Call(Box::new(with_targs(f.split('#').next().unwrap_or(f), gargs)), args));
                            // (the overload chosen: the call of its name resolves to it)
                            let chosen = self.prog.names.iter().position(|n| n == f).map(|i| i as u32);
                            if chosen != id {
                                self.keep_overload.set(chosen);
                            }
                            let r = self.expr(c, expected);
                            self.keep_overload.set(None);
                            return r;
                        }
                        // (a blanket impl's `self`-by-value method for the reference as it is, `(*self).into_maybe_abi()`
                        // of a `&Box<str>` with `impl<T: OptionIntoWasmAbi> MaybeIntoWasmAbi for T`)
                        if let Some(&bid) = self.prog.blanket.get(method).filter(|_| matches!(p0_ty, Ty::Ref(..)) && is_place_expr(recv)) {
                            let bsig = &self.prog.sigs[bid as usize];
                            let by_value = !matches!(bsig.params.first(), Some(Ty::Ref(..)));
                            let bounds = bsig.trait_of.as_deref().or(bsig.impl_trait.as_deref()).and_then(|tr| self.prog.blanket_bounds.get(tr));
                            if by_value && bounds.is_some_and(|bs| blanket_impls_hold(self.prog, p0_ty, bs, &mut vec![], &mut vec![])) {
                                // (an `unsafe fn` of the trait: E0133 outside `unsafe`, as the other blanket calls)
                                self.check_unsafe_call(bid, msp.start)?;
                                let args: Vec<&Expr> = margs.iter().collect();
                                if let Some(v) = recv_ref(self) {
                                    return self.call_fn(bid, Some((v, p0_ty)), &args, tf, expected, SP);
                                }
                                if self.pure_place(recv) {
                                    let (v, t) = self.expr(recv, None)?;
                                    let Some(v) = v else { return Ok((None, Ty::Never)) };
                                    return self.call_fn(bid, Some((v, t)), &args, tf, expected, SP);
                                }
                            }
                        }
                        let t = self.prog.tn(p.ty());
                        return err(SP, format!("no method `{}` on type `{t}`", method));
                    }
                }
            };
            // (a type parameter's placeholder: the method its bounds' traits declare, called with the trait's signature
            // (its instance is not compiled: zustc check's last round follows closures only))
            if let Some(id) = self.placeholder_method(sid, &name) {
                break (a, o, sid, id);
            }
            // (`C::baz(&w)` of a trait of the program's: its default when the impl of C does not override it, not an
            // inherent `baz` or another trait's)
            // (and its impl's method when it has one, chosen here: the receiver is then taken as that method takes it,
            // `(&q).qux()` of ByRef's `qux(&self)` beside ByValue's `qux(self)`)
            if let Some((_, tr)) = self.explicit_trait.as_ref().filter(|(m, _)| *m == name) {
                // (`Display::fmt(self, f)` in the type's Debug: its Display / Debug methods are `S::<Display>::fmt`)
                // (of a generic type's instance, the generic impl's: `VecDeque<i32>`'s is `VecDeque::<Debug>::fmt`)
                if matches!(tr.as_str(), "Display" | "Debug") {
                    let own = format!("{}::<{tr}>::{name}", self.prog.structs[sid as usize].name);
                    let generic = format!("{}::<{tr}>::{name}", self.prog.structs[self.prog.base_of(sid) as usize].name);
                    if let Some(&id) = self.prog.index.get(&own).or_else(|| self.prog.index.get(&generic)) {
                        break (a, o, sid, id);
                    }
                }
                let key = format!("{}::{name}", self.prog.structs[self.prog.base_of(sid) as usize].name);
                let user = |d: u32| self.prog.item_of.get(d as usize).copied().flatten().is_some_and(|x| x < self.prog.prelude_from);
                match self.trait_candidate_where(&key, tr, |i| self.self_fits(i, Ty::Struct(sid))) {
                    Some(c) if user(c) && self.prog.sigs[c as usize].has_self => break (a, o, sid, c),
                    Some(_) => {}
                    None => {
                        if let Some(d) = self.trait_default(tr, &name, Ty::Struct(sid)).filter(|&d| user(d)) {
                            break (a, o, sid, d);
                        }
                    }
                }
            }
            // (a method with `self`: an associated function of the name is none, `v.get(i)` beside `impl Wire for Vec<T>
            // { fn get(r: &[u8]) -> Self }` is the inherent `get` (self_method))
            if let Some(id) = self.prog.self_method_at(sid, &name, wrap).or_else(|| self.prog.self_default_method(sid, &name)) {
                // (inherent overloads, in impls for other instances of the type: the one for this receiver's, before its
                // receiver is borrowed (or not) as that method takes self)
                let c = self.prog.inherent_overload_for(id, Ty::Struct(sid));
                // (only in impls for other instances (`impl Point<f64> { fn distance(&self) }` and a `Point<i32>`):
                // rustc finds no method)
                if self.user_code && self.other_instances_only(c, sid) {
                    let b = self.prog.base_of(sid);
                    let what = if self.prog.structs[b as usize].is_enum { "enum" } else { "struct" };
                    return err(SP, format!("no method named `{name}` found for {what} `{}` in the current scope (E0599)", self.prog.tn(Ty::Struct(sid))));
                }
                break (a, o, sid, c);
            }
            let sd = &self.prog.structs[sid as usize];
            let gens = sd.args.clone();
            let base = sd.name.split('<').next().unwrap_or("").to_string();
            if let Some(did) = self.prog.method(sid, "deref") {
                let target = match self.prog.sigs[did as usize].ret {
                    Ty::Ref(_, t) => *t,
                    _ => return err(SP, "bad `deref` signature"),
                };
                let target = self.prog.partial_subst(target, &gens.iter().map(|&g| Some(g)).collect::<Vec<_>>());
                let recv = self.addr_plus(a, o);
                let _ = base;
                let (v, vt) = self.call_fn_vals(did, &[(recv, Ty::Ref(false, intern(Ty::Struct(sid))))], SP)?;
                let inner_t = match vt {
                    Ty::Ref(_, t) => *t,
                    _ => target,
                };
                p = Place::Mem(v.unwrap(), 0, inner_t);
                via_deref = true;
                wrap = None;
                if shared_deref.is_none() {
                    shared_deref = self.shared_deref(sid);
                }
                derefs += 1;
                if derefs > 16 {
                    let sname = self.prog.structs[sid as usize].name.clone();
                    return err(SP, format!("reached the recursion limit while auto-dereferencing `{sname}` (E0055)"));
                }
                continue;
            }
            // (`(0..3).into_iter()`: an iterator is its own IntoIterator; a copy of it, when nothing in it needs drop)
            if name == "into_iter" && margs.is_empty() && derefs == 0 && self.prog.method(sid, "next").is_some() && !self.needs_drop(Ty::Struct(sid)) {
                let t = Ty::Struct(sid);
                let tmp = self.temp_fresh(t);
                let src = self.addr_plus(a, o);
                self.copy(tmp, src, t);
                return Ok((Some(tmp), t));
            }
            let sname = self.prog.structs[sid as usize].name.clone();
            // (only an associated function of the name, or a trait's default one: rustc's note)
            if self.prog.method(sid, &name).or_else(|| self.prog.default_method(sid, &name)).is_some_and(|d| !self.prog.sigs[d as usize].has_self) {
                return err(SP, format!("`{sname}::{name}` is an associated function, not a method"));
            }
            return err(SP, format!("no method named `{name}` found for `{sname}`"));
        };
        let sname = self.prog.structs[sid as usize].name.clone();
        if !self.prog.sigs[id as usize].has_self {
            return err(SP, format!("`{sname}::{name}` is an associated function, not a method"));
        }
        // (a `&mut self` method of what a `Deref` with no `DerefMut` reaches (`rc.set(0, 0)` with `rc: Rc<Point>`):
        // it would write shared data (E0596))
        if let (Some(n), true, true) = (&shared_deref, self.user_code, matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(true, _)))) {
            let what = if matches!(n.as_str(), "Rc" | "Arc") { format!("data in an `{n}`") } else { format!("data in dereference of `{n}`") };
            return err(SP, format!("cannot borrow {what} as mutable (E0596)"));
        }
        // struct のアドレスを渡す: `&self` ならそのまま、`self` なら call_fn がコピーする
        let mut recv_v = self.addr_plus(a, o);
        let st = Ty::Struct(sid);
        // (unused_allocation: `Box::new(s).m()` with `fn m(&self)` of S itself (not `self: &Box<Self>`))
        if self.prog.deny_unused_alloc && self.user_code && recv_box.is_some() && derefs == 0 && is_box_new(recv) {
            if let Some(Ty::Ref(_, Ty::Struct(ps))) = self.prog.sigs[id as usize].params.first() {
                if self.prog.base_of(*ps) == self.prog.base_of(sid) {
                    return err(expr_start(recv), "unnecessary allocation, use `&` instead");
                }
            }
        }
        // (`r.take()` with `fn take(self)` and r: &S, S not Copy: moving out of what r points to (E0507))
        if through_ref && !via_deref && self.user_code {
            let p0 = self.prog.sigs[id as usize].params.first().copied();
            // (a derived Copy of a generic type holds only when its arguments are Copy: `Option<String>` is not)
            let copy = self.prog.implements(st, "Copy") && self.prog.structs[sid as usize].args.iter().all(|&a| self.prog.implements(a, "Copy"));
            if matches!(p0, Some(Ty::Struct(_) | Ty::Param(_))) && self.prog.sigs[id as usize].has_self && !copy {
                let sname = self.prog.tn(st);
                return err(SP, format!("cannot move out of a shared reference: `{name}` takes `self` by value and `{sname}` is not `Copy` (E0507)"));
            }
        }
        let recv_ty = match self.prog.sigs[id as usize].params[0] {
            Ty::Ref(m, _) => Ty::Ref(m, intern(st)),
            _ => st,
        };
        // (`Pin::new(&mut s).poll(cx)` with `fn poll(self: Pin<&mut Self>, ..)`: the Pin value, not what it points to)
        let p0t = p0.ty();
        // (`self: Box<Self>`: the Box itself)
        if let Ty::Box(bi) = self.prog.sigs[id as usize].params[0] {
            let bv = match recv_box {
                Some((v, t)) if t == Ty::Box(bi) => Some(v),
                _ if p0t == Ty::Box(bi) => self.read(p0).0,
                _ => None,
            };
            if *bi == st {
                if let Some(bv) = bv {
                    match recv_temp {
                        Some(f) => self.clear_flag(f),
                        None => {
                            self.mark_moved(recv);
                            // (the Box moves into the call: a use of it after is E0382)
                            self.bk_acc(recv, borrowck::Acc::Move);
                        }
                    }
                    // (the receiver is the Box: of its type also where p0 is what a temporary Box points to,
                    // `Box::new(S).f()`; an `unsafe fn` is so here too, E0133 outside `unsafe`)
                    self.check_unsafe_call(id, msp.start)?;
                    // (the receiver's tree for the region check, as on the other path: a result that borrows
                    // through the Box's value is tied to what that borrows, `Box::new(S(&x)).get()`)
                    if !via_deref {
                        let user = self.prog.item_of.get(id as usize).copied().flatten().is_some_and(|i| i < self.prog.prelude_from);
                        self.rg_recv(recv, Ty::Box(bi), sid, user.then_some(e));
                    }
                    if self.rg_on() || self.rg.eligible {
                        self.rg.call_sp = borrowck::pos(e);
                    }
                    let r = self.call_fn(id, Some((bv, Ty::Box(bi))), &margs.iter().collect::<Vec<_>>(), tf, expected, SP);
                    // (the result's tree is this call's, as on the other path)
                    self.rg_took_call(e);
                    // (the call takes the receiver after its arguments: what the holder holds ends there, not
                    // before them (`D(&x).consume({ x = 2; })` is E0506))
                    if let Some(h) = recv_holder {
                        self.bk_drop_held(&[h], SP);
                        self.bk_ev(borrowck::Ev::Def(h, vec![]));
                    }
                    return r;
                }
            }
        }
        if let (Ty::Struct(ps), true) = (self.prog.sigs[id as usize].params[0], via_deref) {
            if self.prog.structs[self.prog.base_of(ps) as usize].name == "Pin" && p0t == Ty::Struct(ps) {
                if let Place::Mem(a0, o0, _) = p0 {
                    let v0 = self.addr_plus(a0, o0);
                    // (a `Pin<&mut Self>` moves into the call, as it is no `&mut` a call reborrows: a use of it after is
                    // E0382 (moves/pin-mut-reborrow.rs))
                    if recv_temp.is_none() && (!self.bk_ty(p0t).0 || super::p6_match_drop::holds_mut_ref(self.prog, p0t, 0)) {
                        self.bk_acc(recv, borrowck::Acc::Move);
                    }
                    return self.call_fn(id, Some((v0, p0t)), &margs.iter().collect::<Vec<_>>(), tf, expected, SP);
                }
            }
        }
        // (`self` by value out of a capture held by value (`move || foo.unwrap()`): the value moves to a temporary
        // and the capture's slot becomes empty)
        if !matches!(recv_ty, Ty::Ref(..)) && recv_temp.is_none() && self.needs_drop(st) && self.zeroable(st) {
            if let Expr::Path(_, p) = strip_paren(recv) {
                if ident_of(p).and_then(|id| self.lookup(id).map(|(l, _)| (id, l))).is_some_and(|(id, l)| self.is_value_cap(id, l)) {
                    let tmp = self.temp(st);
                    self.copy(tmp, recv_v, st);
                    self.zero(recv_v, st);
                    recv_v = tmp;
                }
            }
        }
        let mut bk_late = None;
        let (mut res_tid, mut bk_act) = (None, None);
        let mut bk_op_temp = false;
        let t0 = self.bk.call_temps.len();
        if self.bk_on() && !via_deref {
            let flows = self.bk_ret_from(id, 0);
            // (a receiver a call made, borrowed by `&self` / `&mut self` for a result that borrows it, in a `let`'s
            // initializer: a temporary freed at the end of the statement (E0716 when the result is used after))
            if let (Ty::Ref(m, _), false, true) = (recv_ty, is_place_expr(recv), flows) {
                // (not a receiver a call returned as a reference (`id(&a).get()`): dropping the reference frees
                // nothing)
                if !through_ref && !matches!(p0.ty(), Ty::Ref(..)) {
                    self.bk_temp_borrow(recv, m, st);
                }
            }
            let bk_recv_ty = if self.bk.shared_recv == e as *const Expr as usize { Ty::Ref(false, intern(st)) } else { recv_ty };
            let ev = self.bk_recv_ev(recv, bk_recv_ty, flows);
            // (a receiver written as a borrow (`(&mut x).with(|v| x = ..)`): its loan lasts while the arguments are
            // evaluated, when they name x)
            if ev.is_none() {
                if let Expr::Ref(_, inner) = strip_paren(recv) {
                    // (also `(&mut *w)` through a user's `DerefMut`: the loan its `deref_mut` takes of w)
                    // (through each `*`: `(&mut **w)` of a `w: &mut W`)
                    let root = borrowck::root_local(inner).or_else(|| {
                        let mut base = strip_paren(inner);
                        while let Expr::Unary(op, x, _) = base {
                            if op != "*" {
                                break;
                            }
                            base = strip_paren(x);
                        }
                        borrowck::root_local(base)
                    });
                    if let (Some(&ln), Some(n)) = (self.bk.loan_of.get(&(&**inner as *const Expr as usize)), root) {
                        if margs.iter().any(|a| borrowck::mentions(a, n)) {
                            self.bk_call_temp(&[borrowck::Src::Loan(ln)], true);
                            bk_op_temp = true;
                        }
                    }
                }
            }
            if let (true, Some(borrowck::Ev::Acc(_, borrowck::Acc::Borrow(ln), _))) = (flows, &ev) {
                self.bk.loan_of.insert(e as *const Expr as usize, *ln);
            }
            // (the receiver's loan, for what the region check finds it must outlive (Rg::holders))
            if let (true, Some(borrowck::Ev::Acc(_, borrowck::Acc::Borrow(ln), _))) = (self.rg.on, &ev) {
                self.bk.recv_loans.insert(e as *const Expr as usize, *ln);
            }
            // (two-phase for a call written as a method call; an operator (`x += x` → `add_assign`, synthesized
            // without a position) borrows first)
            if let (true, true, Some(borrowck::Ev::Acc(place, borrowck::Acc::Borrow(ln), sp))) =
                (matches!(bk_recv_ty, Ty::Ref(true, _)), self.bk.op_assign_call == e as *const Expr as usize, &ev)
            {
                // (the reservation: a shared loan of the place, ended once the arguments are evaluated; the
                // `&mut` loan is taken then)
                let l = &self.bk.loans[*ln as usize];
                let res = borrowck::Loan { place: l.place.clone(), mutable: false, sp: l.sp, at: l.at, quiet: l.quiet, capture: l.capture };
                let rs = self.bk.loans.len() as u32;
                self.bk.loans.push(res);
                self.bk_push(Some(borrowck::Ev::Acc(place.clone(), borrowck::Acc::Borrow(rs), *sp)));
                self.bk_call_temp(&[borrowck::Src::Loan(rs)], true);
                // (the `&mut` loan is taken at the call (call_fn), after the right side: an error on the right side
                // (`x += x` moves x while it is reserved) comes first)
                let act = match (ev, margs.last()) {
                    (Some(borrowck::Ev::Acc(p, a, _)), Some(r)) => Some(borrowck::Ev::Acc(p, a, err_pos(r))),
                    (ev, _) => ev,
                };
                if let (Some(act), Some(&tid)) = (act, self.bk.call_temps.last()) {
                    self.bk.activate = Some((act, tid));
                }
            } else if matches!(bk_recv_ty, Ty::Ref(true, _)) && self.bk.op_call != e as *const Expr as usize {
                // (an argument that names the receiver (`v.push(v.pop().unwrap())`): the two-phase borrow's
                // reservation, a shared loan of the place, lasts while the arguments are evaluated)
                if let (Some(borrowck::Ev::Acc(place, borrowck::Acc::Borrow(ln), sp)), Some(n)) = (&ev, borrowck::root_local(recv)) {
                    // (or a local that may hold a loan of it (`let w = &v; v.ext(w)`): what it holds is held until the
                    // `&mut` is taken)
                    let mut holds = false;
                    for a in margs.iter() {
                        // (also a block's value: `v.ext({ w })`)
                        let mut a: &Expr = strip_paren(a);
                        while let Expr::Block(None, _, b) = a {
                            match b.stmts.last() {
                                Some(crate::parse::Stmt::Expr(t, false)) => a = strip_paren(t),
                                _ => break,
                            }
                        }
                        if let Expr::Path(None, p) = a {
                            if let (1, Some((_, t))) = (p.segs.len(), self.lookup(&p.segs[0].name)) {
                                holds |= self.bk_ty(t).1;
                            }
                        }
                    }
                    if holds || margs.iter().any(|a| borrowck::mentions(a, n)) {
                        let l = &self.bk.loans[*ln as usize];
                        let res = borrowck::Loan { place: l.place.clone(), mutable: false, sp: l.sp, at: l.at, quiet: l.quiet, capture: l.capture };
                        let rs = self.bk.loans.len() as u32;
                        self.bk.loans.push(res);
                        self.bk.reserved.push(rs);
                        self.bk_push(Some(borrowck::Ev::Acc(place.clone(), borrowck::Acc::Borrow(rs), *sp)));
                        self.bk_call_temp(&[borrowck::Src::Loan(rs)], true);
                        bk_op_temp = true;
                        res_tid = self.bk.call_temps.last().copied();
                    }
                }
                // (the `&mut` loan is taken at the call, while the arguments' values are live (call_fn): one that
                // holds a loan of the receiver (`v.extend(&v)`) conflicts with it (E0502))
                match (res_tid, ev) {
                    (Some(tid), Some(ev)) => bk_act = Some((ev, tid)),
                    (_, ev) => bk_late = ev,
                }
            } else {
                // (an operator's `&mut self` lasts until the call: `x += x` moves x while it is borrowed)
                if let (true, Some(borrowck::Ev::Acc(_, borrowck::Acc::Borrow(ln), _))) = (matches!(bk_recv_ty, Ty::Ref(true, _)), &ev) {
                    let ln = *ln;
                    self.bk_push(ev);
                    self.bk_call_temp(&[borrowck::Src::Loan(ln)], true);
                    bk_op_temp = true;
                } else {
                    // (a shared receiver an argument names (`v.get({ v.push(2); 0 })`): its loan lasts while the
                    // arguments are evaluated)
                    let hold = match (&ev, borrowck::root_local(recv)) {
                        (Some(borrowck::Ev::Acc(_, borrowck::Acc::Borrow(ln), _)), Some(n)) if margs.iter().any(|a| borrowck::mentions(a, n)) => Some(*ln),
                        _ => None,
                    };
                    self.bk_push(ev);
                    if let Some(ln) = hold {
                        self.bk_call_temp(&[borrowck::Src::Loan(ln)], true);
                        bk_op_temp = true;
                    }
                }
            }
        }
        // (a method of what a user's `Deref` gives (`x.get()` with get on the target): `deref(&self)` borrows the
        // receiver shared, and a result that borrows from self borrows from it)
        if self.bk_on() && via_deref && is_place_expr(recv) {
            let flows = self.bk_ret_from(id, 0);
            let ev = self.bk_recv_ev(recv, Ty::Ref(false, intern(st)), flows);
            // (an argument that names the receiver (`w.read(drop(w))`): the loan lasts while the arguments are
            // evaluated, as a receiver's that is not reached through `Deref`)
            let hold = match (&ev, borrowck::root_local(recv)) {
                (Some(borrowck::Ev::Acc(_, borrowck::Acc::Borrow(ln), _)), Some(n)) if margs.iter().any(|a| borrowck::mentions(a, n)) => Some(*ln),
                _ => None,
            };
            self.bk_push(ev);
            if let Some(ln) = hold {
                self.bk_call_temp(&[borrowck::Src::Loan(ln)], true);
                bk_op_temp = true;
            }
        }
        // `self` を値で受ける: 受け手からムーブ
        // (a temporary receiver's holder ends after the call, which takes it after the arguments)
        let mut consumed_holder = None;
        if !matches!(recv_ty, Ty::Ref(..)) && self.needs_drop(st) {
            match recv_temp {
                Some(f) => self.clear_flag(f),
                None => self.mark_moved(recv),
            }
            consumed_holder = recv_holder.filter(|_| !through_ref);
        }
        let args: Vec<&Expr> = margs.iter().collect();
        // (a `&mut self` method of a value that holds references stores what its arguments borrow:
        // `v.push(&mut x)`)
        // (also a `&self` one whose self type is invariant in a lifetime an argument is borrowed for, or outlives:
        // `c.push(&mut y)` of `fn push(&self, r: &'c mut i32)` in `impl<'c> C<'c>` with C holding a
        // `RefCell<Vec<&'c mut i32>>`: the argument's borrow lasts as long as c's 'c)
        let shared_holder = self.bk_on() && !via_deref && matches!(recv_ty, Ty::Ref(false, _)) && self.bk_ty(st).1;
        let mut stores = if shared_holder { self.shared_recv_stores(id) } else { vec![] };
        // (and the prelude's cells that store their argument through `&self` (`c.set(Some(&x))`, `c.replace(v)`): what
        // it borrows, the cell holds, as rustc's `Cell<T>` is invariant in T)
        // (also one reached through a deref (`c.set(Some(&x))` with c: `Rc<Cell<..>>`): the local the receiver is
        // rooted at holds it)
        let cell_deref = self.bk_on() && via_deref && matches!(recv_ty, Ty::Ref(false, _)) && self.bk_ty(st).1 && self.cell_store(id, st);
        if (shared_holder && stores.is_empty() && self.cell_store(id, st)) || cell_deref {
            stores = vec![1];
        }
        let into_recv = self.bk_on() && (!via_deref || cell_deref) && self.bk_ty(st).1 && (matches!(recv_ty, Ty::Ref(true, _)) || !stores.is_empty());
        // (the arguments stored: their loans flow out of the call, as a result's that borrows them)
        let stores0 = std::mem::replace(&mut self.bk.store_args, stores);
        if into_recv {
            self.bk_frame();
        }
        self.check_unsafe_call(id, msp.start)?;
        // (a receiver borrowed for a lifetime the signature names (`fn insert(&'a mut self)` in `impl<'a> Foo<'a>`):
        // checked from here on, as the borrow may last as long as the value)
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && matches!(recv_ty, Ty::Ref(..)) {
            // (or an argument borrowed for a lifetime of the impl: `fn set(&mut self, x: &'a mut u8)` stores it)
            let named = self.prog.sigs[id as usize].regions.as_deref().is_some_and(|r| {
                let n = r.slots.len().saturating_sub(1);
                r.slots.first().and_then(|s| s.first()).is_some_and(|&c| c < crate::types::R_STATIC) || r.slots[..n].iter().skip(1).flatten().any(|&c| c < r.outer)
            });
            if named {
                self.rg_late_on();
            }
        }
        if !via_deref {
            // (the receiver's borrow is followed for the program's methods: the prelude's results are tied to self
            // as an approximation, `Iterator::next`'s too)
            let user = self.prog.item_of.get(id as usize).copied().flatten().is_some_and(|i| i < self.prog.prelude_from);
            self.rg_recv(recv, recv_ty, sid, user.then_some(e));
        }
        // (the region check reports at the method call: call_fn is given no span for it; also when it may begin there)
        if self.rg_on() || self.rg.eligible {
            self.rg.call_sp = borrowck::pos(e);
        }
        if bk_act.is_some() {
            self.bk.activate = bk_act;
        }
        let r = self.call_fn(id, Some((recv_v, recv_ty)), &args, tf, expected, SP);
        // (used there, by the callee's drop of it, so its loans are live while the arguments are made)
        if let Some(h) = consumed_holder {
            self.bk_drop_held(&[h], SP);
            self.bk_ev(borrowck::Ev::Def(h, vec![]));
        }
        self.bk.store_args = stores0;
        self.rg_took_call(e);
        if into_recv {
            let srcs = self.bk_frame_end();
            // (a `&self` receiver is used at the call, after the arguments: what it holds is live while they are
            // made, as rustc's reborrow of it is (a `&mut self` one's two-phase borrow is activated there))
            if matches!(recv_ty, Ty::Ref(false, _)) && is_place_expr(recv) {
                self.bk_acc(recv, borrowck::Acc::Read);
            }
            self.bk_flow_into(recv, &srcs);
        }
        // (the arguments' and the reservation's loans end at the call, before a `&mut self` borrow is taken)
        if bk_op_temp {
            self.bk_call_temps_end(t0);
        }
        self.bk_push(bk_late);
        r
    }

    /// `thread::spawn(|| ..)`: the closure must own what it uses (E0373) and be Send (see not_send)
    /// Whether `x.m()` with x: t takes `&mut self` (a method of t's, or a builtin one of a slice / str that changes it)
    /// `w.m()` with w (through references and boxes) of the program's struct with a `DerefMut` and no method m of its
    /// own, where the method m of what it derefs to (through further derefs) takes `&mut self`: the references and boxes
    /// to go through, and the deref's target
    pub(crate) fn deref_mut_recv(&self, t: Ty, m: &str) -> Option<(usize, Ty)> {
        let (mut t, mut refs) = (t, 0);
        while let Ty::Ref(_, i) | Ty::Box(i) = t {
            t = *i;
            refs += 1;
        }
        let Ty::Struct(sid) = t else { return None };
        // (the cheap test first: most structs have no `DerefMut`)
        if !self.prog.deref_mut_of.contains(&self.prog.base_of(sid)) || self.prog.method(sid, "deref_mut").is_none() {
            return None;
        }
        let base = self.prog.base_of(sid);
        if !self.prog.struct_item.get(base as usize).is_some_and(|&i| i < self.prog.prelude_from) || self.prog.is_placeholder(sid) {
            return None;
        }
        if self.prog.self_method(sid, m).is_some() || self.prog.self_default_method(sid, m).is_some() {
            return None;
        }
        // (a method of an impl for a reference to it (`impl RefTouch for &mut W`) with a reference receiver: the
        // probe reaches it by autoref before it derefs)
        if refs > 0 && self.prog.method(sid, &format!("__ref_{m}")).is_some() {
            return None;
        }
        let deref_target = |sid: u32| -> Option<Ty> {
            let did = self.prog.method(sid, "deref")?;
            let Ty::Ref(_, i) = self.prog.sigs[did as usize].ret else { return None };
            let args: Vec<Option<Ty>> = self.prog.structs[sid as usize].args.iter().map(|&a| Some(a)).collect();
            Some(self.prog.partial_subst(*i, &args))
        };
        let target = deref_target(sid)?;
        // (not to an unsized one: a slice's or a str's, which the rewrite to `t.deref_mut().m()` above takes)
        if self.prog.has_param(target) || matches!(target, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) {
            return None;
        }
        // (the type m is found on: through the target's own derefs. Not a type the derefs came through already: a
        // struct whose `deref` gives itself (E0055 when m is on none of them))
        let mut at = target;
        let mut seen = vec![t];
        for _ in 0..8 {
            if seen.contains(&at) {
                return None;
            }
            seen.push(at);
            match at {
                Ty::Struct(s) if self.prog.self_method(s, m).is_none() && self.prog.self_default_method(s, m).is_none() && self.guard_of(at).is_none() => match deref_target(s) {
                    Some(n) => at = n,
                    None => break,
                },
                _ => break,
            }
        }
        // (a program's struct without m: none of the prelude's `&mut self` methods takes_mut_self knows by name is its)
        if let Ty::Struct(s) = at {
            let user = self.prog.struct_item.get(self.prog.base_of(s) as usize).is_some_and(|&i| i < self.prog.prelude_from);
            if user && self.prog.self_method(s, m).is_none() && self.prog.self_default_method(s, m).is_none() {
                return None;
            }
        }
        self.takes_mut_self(at, m).then_some((refs, target))
    }

    pub(crate) fn takes_mut_self(&self, t: Ty, m: &str) -> bool {
        let t = self.strip_refs(t);
        if let Ty::Struct(ts) = t {
            if let Some(id) = self.prog.self_method_of(ts, m).or_else(|| self.prog.self_default_method(ts, m)) {
                return matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(true, _)));
            }
        }
        // (a trait's method of a primitive type's (`impl Inc for i32 { fn inc(&mut self) }`): through `&mut *g`,
        // or it changes a copy)
        if let Some(id) = self.prog.prim_self_method(t, m) {
            if matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(true, _))) {
                return true;
            }
        }
        matches!(
            m,
            "sort" | "sort_unstable" | "sort_by" | "sort_by_key" | "sort_unstable_by" | "sort_unstable_by_key" | "sort_by_cached_key"
                | "reverse" | "swap" | "fill" | "iter_mut" | "copy_from_slice" | "clone_from_slice" | "rotate_left" | "rotate_right"
                | "as_mut_ptr" | "as_mut_slice" | "as_mut_str" | "first_mut" | "last_mut" | "get_mut" | "split_at_mut" | "chunks_mut"
                | "chunks_exact_mut" | "make_ascii_uppercase" | "make_ascii_lowercase"
        )
    }

    /// The method `name` a placeholder's bounds' traits declare (Program::placeholder)
    pub(crate) fn placeholder_method(&self, sid: u32, name: &str) -> Option<u32> {
        if !self.prog.is_placeholder(sid) {
            return None;
        }
        let bs = self.prog.structs[sid as usize].placeholder.clone()?;
        // (a trait of a module is known by its path: `m::Tr` for a bound written `Tr`)
        let decls = |tr: &str| -> Option<&Vec<u32>> {
            self.prog.traits.get(tr).or_else(|| {
                let tail = format!("::{tr}");
                self.prog.traits.iter().find(|(k, _)| k.ends_with(&tail)).map(|(_, v)| v)
            })
        };
        bs.iter().find_map(|b| {
            let tr = b.split('<').next().unwrap_or(b).trim();
            decls(tr)?.iter().copied().find(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(name))
        })
    }

    /// The type arguments the bound of placeholder sid on trait tr writes (`FnLike<&'a isize, u8>`: `&isize`, `u8`),
    /// each None where it is not a type this function knows
    pub(crate) fn placeholder_bound_targs(&self, sid: u32, tr: &str) -> Option<Vec<Option<Ty>>> {
        let bs = self.prog.structs[sid as usize].placeholder.as_ref()?;
        let short = tr.split('<').next().unwrap_or(tr).trim();
        let short = short.rsplit("::").next().unwrap_or(short);
        let mut of = bs.iter().filter(|b| {
            let n = b.split('<').next().unwrap_or(b).trim();
            n == short || n.rsplit("::").next() == Some(short)
        });
        let b = of.next()?;
        // (two bounds of the trait: which one a call takes is its arguments', not known here)
        if of.next().is_some() || !b.contains('<') {
            return None;
        }
        let f = crate::parse::parse_file(&format!("type __B = dyn {b};")).ok()?;
        let Some(crate::parse::ItemKind::Type(_, _, _, Some(crate::parse::Ty::Dyn(dbs)))) = f.items.first().map(|i| &i.kind) else { return None };
        let Some(crate::parse::Bound::Trait(_, p)) = dbs.first() else { return None };
        let seg = p.segs.last()?;
        Some(seg.args.iter().filter_map(|a| match a {
            crate::parse::GArg::Ty(t) => Some(self.parse_ty(t).ok()),
            _ => None,
        }).collect())
    }

    /// `thread::spawn`, or the prelude's `Builder::spawn` (not a program's own `Builder`'s): the closure it is given
    /// runs on another thread, so its captures must be Send and held by value, and its value Send
    fn spawns(&self, id: u32) -> bool {
        match self.prog.names.get(id as usize).map(|n| n.as_str()) {
            Some("__core_spawn") => true,
            Some("Builder::spawn" | "__p_Builder::spawn") => self.prog.item_of.get(id as usize).copied().flatten().is_some_and(|i| i >= self.prog.prelude_from),
            _ => false,
        }
    }

    /// A spawned closure given as a value, not written in the call (`spawn(f)`, `spawn({ move || .. })`,
    /// `spawn(if c { f } else { g })`): checked by its type t once the call's arguments are evaluated. Its captures (the
    /// fields of its type) run on the other thread just the same: one held by reference may be outlived (E0373), and
    /// each must be Send (E0277)
    fn check_spawn_value(&self, c: &Expr, t: Ty) -> R<()> {
        if let Ty::Struct(sid) = t {
            let cd = self.prog.closure_of_struct.get(&self.prog.base_of(sid)).and_then(|f| self.prog.closures.get(f));
            if let Some(cd) = cd {
                if let Some(k) = cd.moved.iter().position(|m| !m) {
                    let n = &cd.captures[k];
                    return Err(self.at(value_pos(c), format!("closure may outlive the current function, but it borrows `{n}`, which is owned by the current function")));
                }
            }
        }
        if not_send(self.prog, t) {
            let bad = match t {
                Ty::Struct(sid) => self.prog.structs[sid as usize].fields.iter().map(|f| f.1).find(|&f| not_send(self.prog, f)),
                _ => None,
            };
            return Err(self.at(value_pos(c), format!("`{}` cannot be sent between threads safely", self.prog.tn(bad.unwrap_or(t)))));
        }
        Ok(())
    }

    /// A closure written in a spawn call: its free variables, before the call is evaluated (one given as a value is
    /// check_spawn_value's, after)
    fn check_spawn_closure(&self, c: &Expr) -> R<()> {
        let Expr::Closure(mv, _, _, _, cpos) = strip_paren(c) else {
            return Ok(());
        };
        for n in crate::ast_closure::free_vars(strip_paren(c)) {
            let Some((_, t)) = self.lookup(&n) else { continue };
            if not_send(self.prog, t) {
                return Err(self.at(*cpos, format!("`{}` cannot be sent between threads safely", self.prog.tn(t))));
            }
            if !*mv && !crate::ast_closure::moves_capture_by(strip_paren(c), &[n.clone()], &|p, _| borrowck::pat_moves(self.prog, p, t, 0)) {
                return Err(self.at(*cpos, format!("closure may outlive the current function, but it borrows `{n}`, which is owned by the current function")));
            }
        }
        Ok(())
    }

    /// std's `AsMut<str>` of `recv.as_mut()`: `recv.as_mut_str()` of a String, `&mut *recv` of a str (through `&mut`
    /// references only; not a program's own type named String)
    pub(crate) fn as_mut_of_text(&self, recv: &Expr) -> Option<Expr> {
        let mut t = self.peek(recv)?;
        let mut derefs = 0;
        while let Ty::Ref(m, i) = t {
            if !m {
                return None;
            }
            derefs += 1;
            t = *i;
        }
        // (a program's own `as_mut` of the type, `impl Len for str { fn as_mut(&self) -> usize }`: on the place itself,
        // `(*s).as_mut()` / `t.as_mut()`, rustc's lookup takes it (its `&self` before AsMut's `&mut self`); through
        // `&mut`, AsMut's `self: &mut str` comes first)
        if derefs == 0 {
            let own = match t {
                Ty::Struct(sid) => self.prog.method(sid, "as_mut"),
                Ty::Str => self.prog.prim_method(Ty::Str, "as_mut"),
                _ => None,
            };
            if own.is_some() {
                return None;
            }
        }
        match t {
            Ty::Struct(sid) if self.std_struct_name(sid) == "String" => Some(mcall(paren(recv.clone()), "as_mut_str", vec![])),
            Ty::Str => {
                let mut e = paren(recv.clone());
                for _ in 0..derefs {
                    e = paren(Expr::Unary("*".into(), Box::new(e), crate::parse::Span::default()));
                }
                Some(Expr::Ref(true, Box::new(e)))
            }
            _ => None,
        }
    }

    /// The name of std's (the prelude's) struct sid's base, "" for a program's own type (`struct String` of its own)
    pub(crate) fn std_struct_name(&self, sid: u32) -> &str {
        let b = self.prog.base_of(sid);
        if self.prog.struct_item.get(b as usize).is_some_and(|&i| i >= self.prog.prelude_from) {
            self.prog.structs[b as usize].name.as_str()
        } else {
            ""
        }
    }

    /// 組み込みの AsRef (`s.as_ref()` → `s.as_bytes()` / `&s[..]`)。受け手が as_ref を持つ構造体 (Option など) なら None
    pub(crate) fn as_ref_rewrite(&self, recv: &Expr, expected: Option<Ty>) -> Option<Expr> {
        let (target, is_string, seq) = self.as_ref_target(recv, expected)?;
        let r = paren(recv.clone());
        // (a `&&str` (an `AsRef<str>` instance at `&str`, through `iter()`): the `&str` it points to, `*x`)
        let mut refs = 0;
        let mut t = self.peek(recv)?;
        while let Ty::Ref(_, i) = t {
            refs += 1;
            t = *i;
        }
        if target == "str" && t == Ty::Str && refs >= 2 {
            let mut x = r;
            for _ in 1..refs {
                x = paren(Expr::Unary("*".into(), Box::new(x), Default::default()));
            }
            return Some(x);
        }
        Some(if target == "str" {
            if is_string { mcall(r, "as_str", vec![]) } else { Expr::Ref(false, Box::new(Expr::Index(Box::new(r), Box::new(Expr::Range(None, None, false)), Default::default()))) }
        } else if !seq {
            mcall(r, "as_bytes", vec![])
        } else {
            Expr::Ref(false, Box::new(Expr::Index(Box::new(r), Box::new(Expr::Range(None, None, false)), Default::default())))
        })
    }

    /// Which of a Vec's two built-in AsMut impls `recv.as_mut()` is (the receiver a Vec, through references):
    /// `(true, T)` for AsMut<[T]>, `(false, Vec<T>)` for AsMut<Vec<T>>. The expected type tells (`&mut [T]` /
    /// `&mut Vec<T>`; another `&mut X` is neither), or else the bounds of the function on the receiver's type
    /// parameter (`S: AsMut<[u8]>`), when they name one of the two. None otherwise: no method, as before (rustc's
    /// ambiguity is not told apart)
    pub(crate) fn as_mut_of_vec(&self, recv: &Expr, expected: Option<Ty>) -> Option<(bool, Ty)> {
        let t = self.peek(recv)?;
        let base = self.strip_refs(t);
        let Ty::Struct(sid) = base else { return None };
        if self.prog.structs[self.prog.base_of(sid) as usize].name != "Vec" {
            return None;
        }
        let elem = self.vec_fields(sid)?.2;
        match expected {
            Some(Ty::Ref(true, Ty::Slice(e))) if **e == elem => return Some((true, elem)),
            Some(Ty::Ref(true, x)) if *x == base => return Some((false, base)),
            Some(Ty::Ref(true, _)) => return None,
            _ => {}
        }
        let (mut slice, mut itself) = (false, false);
        for (pn, bs) in &self.prog.sigs[self.root as usize].generics {
            let pt = self.env.params.iter().find(|(n, _)| n == pn).map(|p| p.1);
            if !pt.is_some_and(|p| p == t || self.strip_refs(p) == base) {
                continue;
            }
            for b in bs {
                let b: String = b.chars().filter(|c| !c.is_whitespace()).collect();
                if let Some(x) = b.strip_prefix("AsMut<") {
                    slice |= x.starts_with('[');
                    itself |= x.starts_with("Vec<");
                }
            }
        }
        match (slice, itself) {
            (true, false) => Some((true, elem)),
            (false, true) => Some((false, base)),
            _ => None,
        }
    }

    /// 組み込みの AsRef の結果の型
    pub(crate) fn as_ref_ty(&self, recv: &Expr, expected: Option<Ty>) -> Option<Ty> {
        let (target, _, seq) = self.as_ref_target(recv, expected)?;
        if target == "str" {
            return Some(Ty::Ref(false, intern(Ty::Str)));
        }
        if !seq {
            return Some(Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8)))));
        }
        let elem = match self.strip_refs(self.peek(recv)?) {
            Ty::Array(e, _) | Ty::Slice(e) => *e,
            Ty::Struct(sid) => self.vec_fields(sid)?.2,
            _ => return None,
        };
        Some(Ty::Ref(false, intern(Ty::Slice(intern(elem)))))
    }

    /// (先 ("str" / "[u8]" / "[T]"), 受け手が String か, 受け手が Vec・配列・スライスか)
    fn as_ref_target(&self, recv: &Expr, expected: Option<Ty>) -> Option<(String, bool, bool)> {
        let t = self.peek(recv)?;
        let base = self.strip_refs(t);
        let is_string = matches!(base, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == "String");
        let seq = matches!(base, Ty::Array(..) | Ty::Slice(_))
            || matches!(base, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == "Vec");
        if !(base == Ty::Str || is_string || seq) {
            return None;
        }
        // 先: "str" か "[..]"
        let mut target: Option<String> = match expected {
            Some(Ty::Ref(_, Ty::Str)) => Some("str".into()),
            Some(Ty::Ref(_, Ty::Slice(e))) if **e == Ty::U8 => Some("[u8]".into()),
            Some(Ty::Ref(_, Ty::Slice(_))) => Some("[T]".into()),
            _ => None,
        };
        if target.is_none() {
            let sig = &self.prog.sigs[self.root as usize];
            let mut cands: Vec<(Option<Ty>, String)> = vec![];
            for (pn, bs) in &sig.generics {
                for b in bs {
                    let b: String = b.chars().filter(|c| !c.is_whitespace()).collect();
                    if let Some(x) = b.strip_prefix("AsRef<").and_then(|x| x.strip_suffix('>')) {
                        let pt = self.env.params.iter().find(|(n, _)| n == pn).map(|p| p.1);
                        cands.push((pt, x.to_string()));
                    }
                }
            }
            target = cands
                .iter()
                .find(|(pt, _)| pt.is_some_and(|p| p == t || self.strip_refs(p) == base))
                .or(if cands.len() == 1 { cands.first() } else { None })
                .map(|c| c.1.clone());
        }
        let target = target.unwrap_or_else(|| if seq { "[T]".into() } else { "str".into() });
        Some((target, is_string, seq))
    }

    /// プリミティブ型への trait 実装のメソッド (`impl Tr for u32` の `x.m()`)
    pub(crate) fn prim_trait_method(&mut self, e: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::MethodCall(recv, method, gargs, margs, msp) = e else { unreachable!() };
        let recv: &Expr = recv;
        let tf: Option<&[GArg]> = (!gargs.is_empty()).then_some(&gargs[..]);
        // (接尾辞のない整数リテラル `5.d()` は i32)
        let lit_int = matches!(strip_paren(recv), Expr::Lit(t, _) if matches!(lit::lit(t), Some(Lit::Int(i)) if i.suffix().is_empty()));
        let Some(mut t) = self.peek(recv).or(lit_int.then_some(Ty::I32)) else {
            return Ok(None);
        };
        // (`4.f()` with `impl U for isize`: the literal takes the one integer type that has the method)
        if lit_int && t.is_int() && self.prog.prim_self_method(t, method).is_none() {
            let ints = [Ty::I8, Ty::I16, Ty::I32, Ty::I64, Ty::I128, Ty::Isize, Ty::U8, Ty::U16, Ty::U32, Ty::U64, Ty::U128, Ty::Usize];
            let with: Vec<Ty> = ints.into_iter().filter(|&i| self.prog.prim_self_method(i, method).is_some()).collect();
            if let [only] = with[..] {
                t = only;
            }
        }
        // `impl Tr for &[T]` のメソッド (`__slice::__ref_m`、self は &[T]): 受け手の型が &[T] のとき
        if let Ty::Ref(false, Ty::Slice(_)) = t {
            let f = format!("__slice::__ref_{method}");
            if let Some(&id) = self.prog.index.get(&f) {
                let a0 = match self.prog.sigs[id as usize].params.first() {
                    Some(Ty::Ref(m, Ty::Ref(..))) => Expr::Ref(*m, Box::new(recv.clone())),
                    _ => recv.clone(),
                };
                let mut args = vec![a0];
                args.extend(margs.iter().cloned());
                let c = Expr::Call(Box::new(with_targs(&f, gargs)), args);
                return self.expr(&c, expected).map(Some);
            }
        }
        // (`impl Tr for &&str` / `for &&S`: `T::__ref___ref_m`, for a receiver of type &&T, as rustc probes: a method whose
        // self is the receiver as it is comes first (`impl Tr for &str`'s `&self` one), then one that takes it by
        // reference)
        if let Ty::Ref(_, Ty::Ref(_, inner2)) = t {
            let lookup = |pre: &str| -> Option<u32> {
                match **inner2 {
                    Ty::Struct(sid) => self.prog.method(sid, &format!("{pre}{method}")),
                    it if it.is_int() || it.is_float() || matches!(it, Ty::Bool | Ty::Char | Ty::Str) => {
                        self.prog.index.get(&format!("{}::{pre}{method}", self.prog.tn(it))).copied()
                    }
                    _ => None,
                }
            };
            let self_is = |id: u32, want: Ty| self.prog.sigs[id as usize].params.first() == Some(&want);
            let by_ref = |id: u32| matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(_, p)) if **p == t);
            // (`Second::m(&rr)`: the impl of the trait the call named, among those of one name. None when that impl
            // leaves the method out: the trait's default, as the call without this probe finds it)
            let named = self.explicit_trait.as_ref().filter(|(m, _)| m == method).map(|(_, tr)| tr.clone());
            let of_named = |id: u32| match &named {
                Some(tr) => self.trait_candidate(&self.prog.names[id as usize].clone(), tr),
                None => Some(id),
            };
            let outer = lookup("__ref___ref_").and_then(of_named);
            // (the method's own type arguments, `rr.size::<u16>()`, go with the call)
            let callee = |f: &str| with_targs(f, gargs);
            // (`impl Tr for &str`'s `&self` method: its self is the receiver as it is)
            // (with a trait named, only that trait's)
            let shadow = lookup("__ref_")
                .and_then(of_named)
                .filter(|&o| self_is(o, t) && !outer.is_some_and(|id| self_is(id, t)))
                .filter(|&o| named.as_ref().is_none_or(|tr| self.prog.sigs[o as usize].impl_trait.as_deref().is_some_and(|it| it.split('<').next().unwrap_or(it).trim() == tr.as_str())));
            if let Some(o) = shadow.filter(|_| outer.is_some()) {
                let f = self.prog.names[o as usize].clone();
                let mut args = vec![recv.clone()];
                args.extend(margs.iter().cloned());
                let c = Expr::Call(Box::new(callee(&f)), args);
                self.keep_overload.set(Some(o));
                let r = self.expr(&c, expected).map(Some);
                self.keep_overload.set(None);
                return r;
            }
            if let Some(id) = outer {
                let shadowed = shadow.is_some();
                if (self_is(id, t) || by_ref(id)) && !shadowed {
                    let f = self.prog.names[id as usize].clone();
                    let a0 = if by_ref(id) {
                        let m = matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(true, _)));
                        Expr::Ref(m, Box::new(recv.clone()))
                    } else {
                        recv.clone()
                    };
                    let mut args = vec![a0];
                    args.extend(margs.iter().cloned());
                    let c = Expr::Call(Box::new(callee(&f)), args);
                    // (the overload chosen: the call of its name resolves to it, not to the group's first)
                    self.keep_overload.set(Some(id));
                    let r = self.expr(&c, expected).map(Some);
                    self.keep_overload.set(None);
                    return r;
                }
            }
        }
        // (`impl Tr for &i32` / `impl Tr for &S`: `T::__ref_m` with self `&&T`, for a receiver of type &T)
        if let Ty::Ref(_, inner) = t {
            let f = match *inner {
                Ty::Struct(sid) => self.prog.method(sid, &format!("__ref_{method}")).map(|id| self.prog.names[id as usize].clone()),
                it if it.is_int() || it.is_float() || matches!(it, Ty::Bool | Ty::Char | Ty::Str) => {
                    let f = format!("{}::__ref_{method}", self.prog.tn(it));
                    self.prog.index.contains_key(&f).then_some(f)
                }
                _ => None,
            };
            if let Some(f) = f {
                if let Some(&id0) = self.prog.index.get(&f) {
                    // (a named trait's impl among the overloads: `impl Other for &X` and `impl PartialEq for &X`)
                    // (that trait's impl for this receiver: `impl PartialEq for &TX<u8>` and `.. for &TX<u16>` share the
                    // group)
                    let fits = |c: u32| {
                        let sig = &self.prog.sigs[c as usize];
                        let Some(&Ty::Ref(_, Ty::Ref(_, s))) = sig.params.first() else { return true };
                        self.prog.match_ty(**s, *inner, &mut vec![None; sig.generics.len()])
                    };
                    let id = match &self.explicit_trait {
                        Some((m, tr)) if m == method => self.trait_candidate_where(&f, tr, fits).or_else(|| self.trait_candidate(&f, tr)).unwrap_or(id0),
                        _ => id0,
                    };
                    let f = self.prog.names[id as usize].clone();
                    let a0 = match self.prog.sigs[id as usize].params.first() {
                        Some(Ty::Ref(m, Ty::Ref(..))) => Expr::Ref(*m, Box::new(recv.clone())),
                        _ => recv.clone(),
                    };
                    let mut args = vec![a0];
                    args.extend(margs.iter().cloned());
                    let c = Expr::Call(Box::new(with_targs(&f, gargs)), args);
                    if id != id0 {
                        self.keep_overload.set(Some(id));
                    }
                    let r = self.expr(&c, expected).map(Some);
                    self.keep_overload.set(None);
                    return r;
                }
            }
        }
        while let Ty::Ref(_, i) = t {
            t = *i;
        }
        let name = method.clone();
        // str / スライス / 配列への trait 実装 (`impl Hash for str` / `for [T]`): 受け手は &str / &[T] (fat) か &[T; N]
        if matches!(t, Ty::Str | Ty::Slice(_) | Ty::Array(..)) {
            let Some(id) = self.prog.prim_self_method(t, &name) else {
                return Ok(None);
            };
            if !matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(..))) {
                return Ok(None);
            }
            // (an array the method borrows (`&self` / `&mut self`): its place, not a move out of it (`a.touch()` of
            // `a: [String; 1]`))
            let (v, vt) = if is_place_expr(recv) && matches!(self.peek(recv), Some(Ty::Array(..))) { self.read_no_move(recv)? } else { self.expr(recv, None)? };
            let Some(mut v) = v else { return Ok(Some((None, Ty::Never))) };
            let mut vt = vt;
            // (参照の参照は 1 段ずつ外し、&str / &[T] / &[T; N] で止める)
            while let Ty::Ref(_, i) = vt {
                if matches!(i, Ty::Str | Ty::Slice(_) | Ty::Array(..)) {
                    break;
                }
                // (`&&str` の値は `&str` (fat) の置き場所のアドレスで、`&str` の表現と同じ: 読まない)
                if !i.is_fat() {
                    v = self.b.ins().load(PTR, mf(), v, 0);
                }
                vt = *i;
            }
            if matches!(vt, Ty::Array(..)) {
                // (a `&mut self` method (`impl Touch for [i32; 2]`'s `fn touch(&mut self)`): a mutable borrow of the
                // array (E0596 when it is not `mut`; it needs the `mut`, for unused_mut))
                let mutable = matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(true, _)));
                // (a `&self` one: a shared borrow of the array, which a result that borrows from self holds
                // (`let r = a.head(); drop(a);` is E0505))
                if is_place_expr(recv) && matches!(self.peek(recv), Some(Ty::Array(..))) {
                    self.bk_borrow(recv, mutable, err_pos(recv));
                    if mutable {
                        self.bk_mut_use(recv);
                    }
                }
                vt = Ty::Ref(mutable, intern(vt));
            }
            let args: Vec<&Expr> = margs.iter().collect();
            // (an `unsafe fn` of the trait: E0133 outside `unsafe`, as for the other method calls)
            self.check_unsafe_call(id, msp.start)?;
            return self.call_fn(id, Some((v, vt)), &args, tf, expected, SP).map(Some);
        }
        // (a vector of core::arch too: `impl Vector for __m128i`'s `v.movemask()`)
        if !(t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Box(_) | Ty::Vec(_))) {
            return Ok(None);
        }
        // (`b.inc()` with `impl Inc for i32` and b: Box<i32>: the number's method, when the Box has none)
        // (also `Box<Box<i32>>`: the number at the bottom of the Boxes)
        let leaf = |mut t: Ty| {
            while let Ty::Box(i) = t {
                t = *i;
            }
            t
        };
        let boxed_num = |t: Ty| matches!(t, Ty::Box(_)) && { let i = leaf(t); i.is_int() || i.is_float() || matches!(i, Ty::Bool | Ty::Char) };
        // (the trait the call names for its lookup (method_pick's, or `Tr::m(x)`'s): its impl's method for the number,
        // else its default, before whatever method of the name the number has: `x.foo()` of A's default `foo(self)`
        // beside B's `foo(&self)`)
        let named = self.explicit_trait.as_ref().filter(|(m, _)| *m == name).and_then(|(_, tr)| {
            if !(t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char)) {
                return None;
            }
            let user = |d: u32| self.prog.item_of.get(d as usize).copied().flatten().is_some_and(|x| x < self.prog.prelude_from);
            let key = format!("{}::{name}", self.prog.tn(t));
            match self.trait_candidate(&key, tr) {
                Some(c) => user(c).then_some(c),
                None => self.trait_default(tr, &name, t).filter(|&d| user(d)),
            }
        });
        let Some(id) = named.or_else(|| self.prog.prim_self_method(t, &name)).or_else(|| match t {
            Ty::Box(_) if boxed_num(t) => self.prog.prim_self_method(leaf(t), &name),
            _ => None,
        }) else {
            return Ok(None);
        };
        // (an `unsafe fn` of the impl: E0133 outside `unsafe`, as for any method. For a vector's overload group that mixes
        // safe and unsafe methods, only when every overload is unsafe: a safe one the overload choice may take is checked there)
        let group = self.prog.overloads.get(&self.prog.names[id as usize]).cloned().unwrap_or_else(|| vec![id]);
        if !matches!(t, Ty::Vec(_)) || group.iter().all(|&i| self.prog.sigs[i as usize].is_unsafe) {
            self.check_unsafe_call(id, msp.start)?;
        }
        // (`impl Tr for Box<S>`: only when the impl's self type is this Box; else the method is S's, through deref)
        if let Ty::Box(_) = t {
            let st = self.prog.sigs[id as usize].self_ty;
            let mut sol = vec![None; self.prog.sigs[id as usize].generics.len()];
            if !st.is_some_and(|st| matches!(st, Ty::Box(_)) && self.prog.match_ty(st, t, &mut sol)) {
                // (a blanket impl's method on `Box<i32>`: the number's, through the deref (`(*b).m()`))
                // (the Box's value is the address of the number: it is the `&self` / `&mut self` receiver itself)
                if let Ty::Box(mut inner) = t {
                    let nested = matches!(inner, Ty::Box(_));
                    // (`x.m()` with x: `Box<Box<i32>>` and `self: Box<Self>`: the inner Box moves out of the outer ones, whose
                    // memory is freed)
                    if nested && matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Box(_))) {
                        if !boxed_num(t) {
                            return Ok(None);
                        }
                        let (v, mut vt) = self.expr(recv, Some(t))?;
                        let Some(mut v) = v else { return Ok(Some((None, Ty::Never))) };
                        while let Ty::Box(Ty::Box(i)) = vt {
                            let next = self.b.ins().load(PTR, mf(), v, 0);
                            self.free(v);
                            v = next;
                            vt = Ty::Box(i);
                        }
                        let args: Vec<&Expr> = margs.iter().collect();
                        return self.call_fn(id, Some((v, vt)), &args, tf, expected, SP).map(Some);
                    }
                    while let Ty::Box(i) = inner {
                        inner = i;
                    }
                    if inner.is_int() || inner.is_float() || matches!(*inner, Ty::Bool | Ty::Char) {
                        // (`self: Box<Self>`: the Box itself, moved)
                        if matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Box(_))) {
                            let (v, vt) = self.expr(recv, Some(t))?;
                            let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                            let args: Vec<&Expr> = margs.iter().collect();
                            return self.call_fn(id, Some((v, vt)), &args, tf, expected, SP).map(Some);
                        }
                        // (the receiver is borrowed, not moved: the Box stays the caller's)
                        let (v, vt) = self.read_no_move(recv)?;
                        let Some(mut v) = v else { return Ok(Some((None, Ty::Never))) };
                        let mut vt = vt;
                        while let Ty::Ref(_, i) = vt {
                            v = self.b.ins().load(PTR, mf(), v, 0);
                            vt = *i;
                        }
                        // (the Box's value is the address of what it holds: one load per inner Box)
                        while let Ty::Box(Ty::Box(i)) = vt {
                            v = self.b.ins().load(PTR, mf(), v, 0);
                            vt = Ty::Box(i);
                        }
                        let recv_arg = match self.prog.sigs[id as usize].params.first() {
                            Some(Ty::Ref(m, _)) => (v, Ty::Ref(*m, intern(*inner))),
                            _ => (self.b.ins().load(clif_ty(*inner).unwrap(), mf(), v, 0), *inner),
                        };
                        let args: Vec<&Expr> = margs.iter().collect();
                        return self.call_fn(id, Some(recv_arg), &args, tf, expected, SP).map(Some);
                    }
                }
                return Ok(None);
            }
        }
        // (`x.inc()` with `fn inc(&mut self)`: the address of x itself (through its references), not of a copy)
        if matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(true, _))) && (is_place_expr(recv) || matches!(self.peek(recv), Some(Ty::Ref(true, _)))) {
            let mut rt = self.peek(recv);
            let r = if matches!(rt, Some(Ty::Ref(true, _))) {
                // (a `&mut i32` (or `&mut &mut i32`): its value is the address; one load per extra reference)
                let (v, mut vt) = self.expr(recv, None)?;
                let mut v = v.ok_or("internal: receiver without a value")?;
                while let Ty::Ref(_, i @ Ty::Ref(..)) = vt {
                    v = self.b.ins().load(PTR, mf(), v, 0);
                    vt = *i;
                }
                rt = Some(vt);
                v
            } else {
                let re = self.keep_expr(Expr::Ref(true, Box::new(recv.clone())));
                let (v, vt) = self.expr(re, None)?;
                rt = Some(vt);
                v.ok_or("internal: receiver without an address")?
            };
            if rt == Some(Ty::Ref(true, intern(t))) {
                let args: Vec<&Expr> = margs.iter().collect();
                return self.call_fn(id, Some((r, Ty::Ref(true, intern(t)))), &args, tf, expected, SP).map(Some);
            }
        }
        // (`a.loan()` / `id(&a).loan()` with `fn loan(&self) -> &Self`: the address of a itself, not of a copy, so the
        // result is `a` (`ptr::eq(r, &a)`))
        if matches!(self.prog.sigs[id as usize].params.first(), Some(Ty::Ref(false, _))) && !lit_int && (is_place_expr(recv) || matches!(self.peek(recv), Some(Ty::Ref(..)))) {
            let (r, rt) = if matches!(self.peek(recv), Some(Ty::Ref(..))) {
                let (v, mut vt) = self.expr(recv, None)?;
                let mut v = v.ok_or("internal: receiver without a value")?;
                while let Ty::Ref(_, i @ Ty::Ref(..)) = vt {
                    v = self.b.ins().load(PTR, mf(), v, 0);
                    vt = *i;
                }
                (v, vt)
            } else {
                let re = self.keep_expr(Expr::Ref(false, Box::new(recv.clone())));
                let (v, vt) = self.expr(re, None)?;
                (v.ok_or("internal: receiver without an address")?, vt)
            };
            if matches!(rt, Ty::Ref(_, i) if *i == t) {
                let args: Vec<&Expr> = margs.iter().collect();
                return self.call_fn(id, Some((r, Ty::Ref(false, intern(t)))), &args, tf, expected, SP).map(Some);
            }
            // (another shape: through a copy, below; the receiver is evaluated again only when it is a place)
            if !is_place_expr(recv) {
                let mut v = r;
                let mut vt = rt;
                while let Ty::Ref(_, i) = vt {
                    v = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
                    vt = *i;
                }
                let a = self.temp(t);
                self.b.ins().store(mf(), v, a, 0);
                let args: Vec<&Expr> = margs.iter().collect();
                return self.call_fn(id, Some((a, Ty::Ref(false, intern(t)))), &args, tf, expected, SP).map(Some);
            }
        }
        // 受け手: 値ならそのまま、&self ならメモリに置いてそのアドレス
        let (v, vt) = self.expr(recv, lit_int.then_some(t))?;
        let mut v = v.unwrap();
        let mut vt = vt;
        // (a receiver that is a reference (`id(&v).loan()`): dropping it frees nothing)
        let recv_is_ref = matches!(vt, Ty::Ref(..));
        while let Ty::Ref(_, i) = vt {
            v = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
            vt = *i;
        }
        let (recv_v, recv_ty) = match self.prog.sigs[id as usize].params.first() {
            Some(Ty::Ref(m, _)) => {
                let a = self.temp(t);
                self.b.ins().store(mf(), v, a, 0);
                // (a receiver a call made, borrowed for a result that borrows it: the temporary is freed at the end
                // of the statement (E0716 when the result is used after: `let r = mk().loan(); *r`), as for a struct's)
                if self.bk_on() && !is_place_expr(recv) && !recv_is_ref && self.bk_ret_from(id, 0) {
                    self.bk_temp_borrow(recv, *m, t);
                }
                (a, Ty::Ref(*m, intern(t)))
            }
            _ => (v, t),
        };
        let args: Vec<&Expr> = margs.iter().collect();
        self.call_fn(id, Some((recv_v, recv_ty)), &args, tf, expected, SP).map(Some)
    }

    /// 型引数を解いて、呼ぶべき関数 (インスタンス / trait 実装) とそのシグネチャを決める
    /// turbofish (`f::<A, B>`) で決まる型パラメータ
    /// the kind of a const argument as written (`true` / `{ false }`: bool, `'c'`: char, `3`: an integer, `N`: a named
    /// const's own type), for a parameter's declared type: (kind, its name in the message)
    pub(crate) fn const_arg_kind(&self, a: &GArg) -> Option<(&'static str, &'static str)> {
        // (`{true}`: a block of one boolean is a boolean too)
        let braced_bool = |e: &Expr| match e {
            Expr::Block(None, false, b) if b.stmts.len() == 1 => {
                matches!(&b.stmts[0], crate::parse::Stmt::Expr(Expr::Lit(s, _), false) if s == "true" || s == "false")
                    || matches!(&b.stmts[0], crate::parse::Stmt::Expr(Expr::Path(None, p), false) if p.segs.len() == 1 && matches!(p.segs[0].name.as_str(), "true" | "false"))
            }
            _ => false,
        };
        // (`flag::<N>()` with `const N: u8`: a named const's own type)
        let named_const = |n: &str| self.lookup(n).is_none().then(|| self.prog.const_index.get(n).map(|&ci| self.prog.consts[ci as usize].ty)).flatten();
        let kind_of = |t: Ty| match t {
            Ty::Bool => Some(("bool", "bool")),
            Ty::Char => Some(("char", "char")),
            t if t.is_int() => Some(("int", "integer")),
            _ => None,
        };
        match a {
            GArg::Ty(crate::parse::Ty::Path(None, q)) if q.segs.len() == 1 && q.segs[0].args.is_empty() && named_const(&q.segs[0].name).is_some() => {
                kind_of(named_const(&q.segs[0].name).unwrap())
            }
            GArg::Ty(t) if crate::ast_types::bool_arg(t).is_some() => Some(("bool", "bool")),
            GArg::Const(e) if braced_bool(e) => Some(("bool", "bool")),
            GArg::Const(Expr::Lit(s, _)) if s.starts_with('\'') => Some(("char", "char")),
            GArg::Const(Expr::Lit(s, _)) if s.starts_with(|c: char| c.is_ascii_digit()) => Some(("int", "integer")),
            _ => None,
        }
    }

    /// a turbofish argument against its parameter's kind: E0747 for a boolean where a type goes, "mismatched types" for
    /// a const argument of another kind than the parameter's declared type (out of line: the common calls do not reach
    /// it)
    #[inline(never)]
    fn turbofish_arg_kind(&self, sig: &FnSig, slot: usize, a: &GArg) -> R<()> {
        // (`get_flag::<false, 0xFF>()` with `const C: char`: a const argument of another kind than its parameter's
        // type)
        let const_slot = sig.generics.get(slot).filter(|(_, bs)| bs.iter().any(|b| b == crate::types::CONST_GENERIC));
        // (`identity::<true>(..)` with `fn identity<T>`: a boolean where a type goes)
        if const_slot.is_none() && sig.generics.get(slot).is_some() && matches!(a, GArg::Ty(t) if crate::ast_types::bool_arg(t).is_some()) {
            return err(SP, "constant provided when a type was expected (E0747)");
        }
        if let Some((_, bs)) = const_slot {
            let decl = bs.iter().find_map(|b| b.strip_prefix(crate::types::CONST_GENERIC)?.strip_prefix(':')).unwrap_or("usize");
            let decl = &*self.prog.const_decl(decl);
            let found = self.const_arg_kind(a);
            let kind = match decl {
                "bool" | "char" => decl,
                _ => "int",
            };
            if let Some((_, shown)) = found.filter(|(k, _)| *k != kind) {
                return err(SP, format!("mismatched types: expected `{decl}`, found {shown} (const argument)"));
            }
        }
        Ok(())
    }

    pub(crate) fn turbofish_sol(&self, sig: &FnSig, turbofish: Option<&[GArg]>) -> R<Vec<Option<Ty>>> {
        let mut sol: Vec<Option<Ty>> = vec![None; sig.generics.len()];
        if let (Some(t), Some(st)) = (self.seg_self, sig.self_ty) {
            self.prog.match_ty(st, t, &mut sol);
        } else if let (Some(t), None, Some(_)) = (self.seg_self, sig.self_ty, &sig.trait_of) {
            // (a trait's default fn: Self is its first type parameter)
            if let Some(s0) = sol.first_mut() {
                *s0 = Some(t);
            }
        }
        // trait メソッドは先頭が Self なので、turbofish はその後ろの型パラメータに当たる
        // (the turbofish names the fn's own parameters, the last ones: an impl's come first, `impl<const N: usize>
        // T<N> for S { fn l<const M: bool>() }` with `<S as T<123>>::l::<true>()`; a hidden `__impl0` of an
        // `impl Trait` argument is not named)
        let first = sig.trait_of.is_some() as usize;
        if let Some(tf) = turbofish {
            // (the summary names an `impl Trait` argument's parameter `__impl` and its index: a user's `__implT` is not one)
            // (an impl's member only: a free fn's parameters are all its own, the prelude's `__exr_*` put the named
            // ones first; the many calls to those skip the list)
            let member = sig.self_ty.is_some() || sig.impl_of.is_some();
            let named: Vec<usize> = if member { (0..sig.generics.len()).filter(|&k| !hidden_param(&sig.generics[k].0)).collect() } else { vec![] };
            let ntf = tf.iter().filter(|a| !matches!(a, GArg::Lifetime(_))).count();
            // (E0107: a free fn's turbofish names only its explicit parameters, not an `impl Trait` argument's hidden one:
            // `foo::<str, String>` of `fn foo<T: ?Sized>(_f: impl AsRef<T>)`)
            if !member && sig.trait_of.is_none() && sig.generics.iter().any(|g| hidden_param(&g.0)) {
                let n = sig.generics.iter().filter(|g| !hidden_param(&g.0)).count();
                if ntf > n {
                    let w = |k: usize| if k == 1 { "argument" } else { "arguments" };
                    return err(SP, format!("function takes {n} generic {} but {ntf} generic {} supplied (E0107)", w(n), if ntf == 1 { "argument was" } else { "arguments were" }));
                }
            }
            let own0 = if member { named.len().saturating_sub(ntf).max(first.min(named.len())) } else { first };
            let slot_of = |i: usize| if member { named.get(own0 + i).copied().unwrap_or(usize::MAX) } else { first + i };
            for (i, a) in tf.iter().enumerate() {
                let first = slot_of(i).wrapping_sub(i);
                self.turbofish_arg_kind(sig, first + i, a)?;
                let ty = match a {
                    // (`f::<true>()`: a bool const argument, which the parser reads as a type path)
                    GArg::Ty(t) if crate::ast_types::bool_arg(t).is_some() => self.prog.resolve_env(&crate::ast_types::bool_arg(t).unwrap(), &self.env).or_else(|er| err(SP, er))?,
                    GArg::Ty(t) => self.parse_ty(t)?,
                    // `f::<3>()`: const ジェネリクスの引数
                    GArg::Const(e) => {
                        let ts = crate::ast_types::const_arg_syn(e).or_else(|er| err(SP, er))?;
                        self.prog.resolve_env(&ts, &self.env).or_else(|er| err(SP, er))?
                    }
                    _ => return err(SP, "only type arguments are supported"),
                };
                if let Some(slot) = sol.get_mut(first + i) {
                    // `_` は推論に任せる。`Vec<_>` のように一部だけ `_` なら穴として持つ
                    *slot = (ty != Ty::Param(crate::program::HOLE)).then_some(ty);
                }
            }
        }
        Ok(sol)
    }

    /// `transmute::<T, U>`: E0512 for types of different sizes, and `&T` to `&mut T` (deny lint)
    fn check_transmute(&self, from: Ty, to: Ty, span: Sp) -> R<()> {
        if self.prog.has_param(from) || self.prog.has_param(to) {
            return Ok(());
        }
        if matches!((from, to), (Ty::Ref(false, _), Ty::Ref(true, _))) {
            return err(span, "transmuting &T to &mut T is undefined behavior, even if the reference is unused");
        }
        let (a, b) = (self.prog.size_align(from).0, self.prog.size_align(to).0);
        if a != b {
            return err(span, format!(
                "cannot transmute between types of different sizes, or dependently-sized types (E0512): `{}` ({} bits) to `{}` ({} bits)",
                self.prog.tn(from), a * 8, self.prog.tn(to), b * 8));
        }
        Ok(())
    }

    /// A default method of a generic trait (`trait Getter<T> { fn do_get2(&self) -> (T, T) { .. } }`) on a type with
    /// one impl of the trait (`impl Getter<isize> for isize`): the trait's parameters (after Self) are that impl's
    /// arguments, when nothing else gave them. Once Self is known, before the arguments are lowered, so a literal
    /// argument takes that type (`p.translate(3)` with `impl Movable<isize> for Point`)
    pub(crate) fn trait_args_from_impl(&self, sig: &FnSig, sol: &mut [Option<Ty>]) {
        let (None, Some(tr), Some(Some(st))) = (sig.self_ty, sig.trait_of.as_deref(), sol.first().copied()) else { return };
        if !sol.iter().skip(1).any(|x| x.is_none()) {
            return;
        }
        let Some(imps) = self.prog.impl_targs.get(tr) else { return };
        let fits: Vec<Vec<Ty>> = imps
            .iter()
            .filter_map(|(ist, args, ng)| {
                let mut isol = vec![None; *ng];
                if !self.prog.match_ty(*ist, st, &mut isol) {
                    return None;
                }
                let a: Vec<Ty> = args.iter().map(|&a| self.prog.partial_subst(a, &isol)).collect();
                a.iter().all(|&t| !self.prog.has_param(t)).then_some(a)
            })
            .collect();
        if let [a] = fits.as_slice() {
            for (i, &t) in a.iter().enumerate() {
                if let Some(x @ None) = sol.get_mut(1 + i) {
                    *x = Some(t);
                }
            }
        }
    }

    pub(crate) fn resolve_call(&mut self, id: u32, arg_tys: &[Ty], mut sol: Vec<Option<Ty>>, ret_hint: Option<Ty>, span: Sp) -> R<(u32, FnSig)> {
        let sig = self.prog.sigs[id as usize].clone();
        if !sig.is_generic() {
            return Ok((id, sig));
        }
        for (&pt, &at) in sig.params.iter().zip(arg_tys) {
            if !self.prog.match_ty(pt, at, &mut sol) {
                return err(span, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(pt), self.prog.tn(at)));
            }
        }
        // 引数から決まらない型パラメータは期待される戻り値型から (`let s: S<i64> = S::new()`)
        if let Some(rt) = ret_hint.filter(|_| sol.iter().any(|s| s.is_none())) {
            let mut s2 = sol.clone();
            if self.prog.match_ty(sig.ret, rt, &mut s2) {
                sol = s2;
            }
        }
        self.solve_assoc_bounds(&sig, &mut sol);
        self.trait_args_from_impl(&sig, &mut sol);
        // (a default method of a generic trait: `trait Getter<T> { fn do_get2(&self) -> (T, T) { .. } }` on a type
        // with `impl Getter<isize> for isize`: the trait's parameters (after Self) are that impl's arguments, when nothing else gave them)
        if let (None, Some(tr), Some(Some(st))) = (sig.self_ty, sig.trait_of.as_deref(), sol.first().copied()) {
            if sol.iter().skip(1).any(|x| x.is_none()) {
                if let Some(imps) = self.prog.impl_targs.get(tr) {
                    let fits: Vec<Vec<Ty>> = imps
                        .iter()
                        .filter_map(|(ist, args, ng)| {
                            let mut isol = vec![None; *ng];
                            if !self.prog.match_ty(*ist, st, &mut isol) {
                                return None;
                            }
                            let a: Vec<Ty> = args.iter().map(|&a| self.prog.partial_subst(a, &isol)).collect();
                            a.iter().all(|&t| !self.prog.has_param(t)).then_some(a)
                        })
                        .collect();
                    if let [a] = fits.as_slice() {
                        for (i, &t) in a.iter().enumerate() {
                            if let Some(x @ None) = sol.get_mut(1 + i) {
                                *x = Some(t);
                            }
                        }
                    }
                }
            }
        }
        // (a method of a placeholder's bound's trait (zustc check of a generic fn nothing calls): the trait's parameters
        // the arguments did not give are the bound's arguments (`F: FnLike<&'a isize, &'a isize>` gives `R` of
        // `fn call(&self, arg: A) -> R`))
        if let (Some(tr), Some(Some(Ty::Struct(ps)))) = (sig.trait_of.as_deref(), sol.first().copied()) {
            if self.prog.is_placeholder(ps) && sol.iter().skip(1).any(|x| x.is_none()) {
                if let Some(targs) = self.placeholder_bound_targs(ps, tr) {
                    for (i, t) in targs.into_iter().enumerate() {
                        if let (Some(x @ None), Some(t)) = (sol.get_mut(1 + i), t) {
                            *x = Some(t);
                        }
                    }
                }
            }
        }
        // prelude の helper のダミーの型引数 `G` (呼ばれた分だけインスタンスにするためのもの) は ()
        for (i, g) in sig.generics.iter().enumerate() {
            if g.0 == "G" && sol[i].is_none() {
                sol[i] = Some(Ty::Unit);
            }
        }
        if let Some(i) = sol.iter().position(|s| s.is_some_and(|t| has_hole(self.prog, t))) {
            return err(span, format!("cannot infer type parameter `{}`; annotate the type", sig.generics[i].0));
        }
        let Some(args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
            let i = sol.iter().position(|s| s.is_none()).unwrap();
            return err(span, format!("cannot infer type parameter `{}`", sig.generics[i].0));
        };
        // trait の関数を `Tr::m(&x)` で呼んだら、実装があればそちらへ静的ディスパッチ
        if sig.trait_of.is_some() {
            let m = self.prog.names[id as usize].rsplit("::").next().unwrap().to_string();
            let tr0 = sig.trait_of.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
            let imp = match args[0] {
                // (the impl of the trait the call named: through `T: A`, S's `foo` from `impl B for S` is not A's,
                // and A's default is the one when S's `impl A` does not override it)
                // (an impl for the receiver's instance: `impl A for W<u8>`'s `foo` is not W<u16>'s)
                Ty::Struct(sid) => self
                    .trait_candidate_where(&format!("{}::{m}", self.prog.structs[self.prog.base_of(sid) as usize].name), tr0.unwrap_or(""), |i| self.self_fits(i, args[0]))
                    // (an inherent method answers it only for the prelude's types, which write std's traits' methods so:
                    // never the program's own, `C::baz(&w)` beside `impl<T> W<T> { fn baz(&self) }` is C's)
                    .or_else(|| self.prog.method(sid, &m).filter(|&i| {
                        let it = self.prog.sigs[i as usize].impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
                        let user = self.prog.item_of.get(i as usize).copied().flatten().is_some_and(|x| x < self.prog.prelude_from);
                        (it == tr0 && self.self_fits(i, args[0])) || (it.is_none() && !user)
                    })),
                // (only an impl of the trait the call named: `A::m(&0.0f32)` is not B's `f32::m`)
                t if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char) => self.prog.index.get(&format!("{}::{m}", self.prog.tn(t))).copied().filter(|&i| {
                    let it = self.prog.sigs[i as usize].impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
                    it.is_none() || it == sig.trait_of.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim())
                }),
                // (an impl of the trait for a reference type (`impl<'a> Foo for &'a isize`): its method, as for a
                // `dyn Foo` made of a `&isize`)
                // (named `__ref_m` of the referent's type: the impl of the trait the call names among them, for this
                // reference (`impl A for &S` and `impl B for &S` share the name))
                t @ Ty::Ref(_, inner) => {
                    let owner = match *inner {
                        Ty::Struct(sid) => self.prog.structs[self.prog.base_of(sid) as usize].name.clone(),
                        it => self.prog.tn(it),
                    };
                    let f = format!("{owner}::__ref_{m}");
                    self.trait_candidate_where(&f, tr0.unwrap_or(""), |i| self.self_fits(i, t)).filter(|&i| {
                        let it = self.prog.sigs[i as usize].impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
                        it == tr0
                    })
                }
                _ => None,
            };
            // (else a blanket impl of the trait whose bounds hold of Self: `impl<A> Foo for A`'s `get` for a `&i32`
            // made a `dyn Foo`, before the trait's default, which is the one only when no such impl overrides it)
            // (blanket impls that are not told apart: neither is taken, and neither is the trait's default, which one
            // of them may override)
            let ambiguous = std::cell::Cell::new(false);
            // (the blanket impl's own parameters, solved from its header against the call's Self and trait arguments,
            // and how many trait arguments come before the method's own generics)
            let header: std::cell::RefCell<Option<(Vec<Option<Ty>>, usize)>> = std::cell::RefCell::new(None);
            let imp = imp.or_else(|| {
                // (the trait's own: `Program::blanket` keeps one method of each name, of whichever trait came first, so
                // the methods of the name are looked through)
                let tr = tr0?;
                let prog = self.prog;
                let mut bl = prog.fns_named(&m)?.iter().copied().filter(|&c| {
                    let bsig = &prog.sigs[c as usize];
                    matches!(bsig.self_ty, Some(Ty::Param(_))) && bsig.impl_trait.as_deref().is_some_and(|t| t.split('<').next().unwrap_or(t).trim() == tr)
                });
                // (one only: two blanket impls of a generic trait (`impl<T> A<u8> for T`, `impl<T> A<u16> for T`) are
                // told apart by the trait's arguments, which are not kept for each method)
                let bid = bl.next()?;
                if bl.next().is_some() {
                    ambiguous.set(true);
                    return None;
                }
                // (the trait's arguments the call has (after Self) are those of the blanket impl's header)
                if let Some(es) = self.prog.impl_targs.get(tr) {
                    let mut bes = es.iter().filter(|e| matches!(e.0, Ty::Param(_)));
                    if let Some((st, targs, ng)) = bes.next() {
                        if bes.next().is_some() {
                            ambiguous.set(true);
                            return None;
                        }
                        let mut isol = vec![None; *ng];
                        if !self.prog.match_ty(*st, args[0], &mut isol) {
                            return None;
                        }
                        for (k, &ta) in targs.iter().enumerate() {
                            if !args.get(1 + k).is_some_and(|&want| self.prog.match_ty(ta, want, &mut isol)) {
                                return None;
                            }
                        }
                        *header.borrow_mut() = Some((isol, targs.len()));
                    }
                }
                let bounds = self.prog.blanket_bounds.get(tr);
                bounds.is_none_or(|bs| blanket_impls_hold(self.prog, args[0], bs, &mut vec![], &mut vec![])).then_some(bid)
            });
            if imp.is_none() && ambiguous.get() {
                return err(span, format!("`{}` has no implementation for `{}` that is told apart from the trait's other blanket impls", self.prog.names[id as usize], self.prog.tn(args[0])));
            }
            // (impls of the trait with other arguments for the same type (`impl Conv<u64> for u8` and `impl Conv<bool>
            // for u8`): the one whose method has the trait's signature under the trait's arguments the call has (Self,
            // then the trait's own: `T = bool`), as the solver selects the impl whose head matches the goal)
                let imp = imp.map(|imp| {
                let Some(group) = self.prog.overloads.get(self.prog.names[imp as usize].split('#').next().unwrap_or("")) else { return imp };
                let want_ret = self.prog.subst(sig.ret, &args);
                let want_params: Vec<Ty> = sig.params.iter().map(|&t| self.prog.subst(t, &args)).collect();
                if self.prog.has_param(want_ret) || want_params.iter().any(|&t| self.prog.has_param(t)) {
                    return imp;
                }
                let fit: Vec<u32> = group.iter().copied().filter(|&c| {
                    let cs = &self.prog.sigs[c as usize];
                    // (an impl for the call's Self itself: `impl Conv<u8> for W<u16>` is not one for `W<u8>`)
                    !cs.is_generic() && cs.self_ty == args.first().copied() && cs.impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim()) == tr0 && cs.ret == want_ret && cs.params.len() == want_params.len()
                        && cs.params.iter().zip(&want_params).skip(1).all(|(a, b)| a == b)
                }).collect();
                if fit.len() == 1 { fit[0] } else { imp }
            });
            if let Some(imp) = imp {
                let isig = &self.prog.sigs[imp as usize];
                if !isig.is_generic() {
                    return Ok((imp, isig.clone()));
                }
                // ジェネリックな impl (`impl<T: D> D for Vec<T>`): Self との照合で impl の型引数を、
                // メソッド自身の型引数は trait 側の解 (Self の後ろ) から
                let mut s2: Vec<Option<Ty>> = vec![None; isig.generics.len()];
                if let Some(st) = isig.self_ty {
                    self.prog.match_ty(st, args[0], &mut s2);
                }
                // (a blanket impl's parameters as its header solved them (`impl<T, U> A<[U; 2]> for T` with
                // `A<[u16; 2]>`: U is u16), and the method's own generics after the trait's arguments)
                let (own_from, k0) = match header.borrow_mut().take() {
                    Some((isol, ntargs)) => {
                        for (slot, x) in s2.iter_mut().zip(&isol) {
                            if slot.is_none() {
                                *slot = *x;
                            }
                        }
                        (1 + ntargs, isol.len())
                    }
                    None => {
                        let own = args.len() - 1;
                        (1, s2.len().saturating_sub(own))
                    }
                };
                for (j, &a) in args.get(own_from..).unwrap_or(&[]).iter().enumerate() {
                    if let Some(slot) = s2.get_mut(k0 + j) {
                        if slot.is_none() {
                            *slot = Some(a);
                        }
                    }
                }
                let Some(iargs) = s2.into_iter().collect::<Option<Vec<Ty>>>() else {
                    return err(span, format!("cannot infer the type parameters of `{}`", self.prog.names[imp as usize]));
                };
                let mut g = self.insts.lock().unwrap();
                let iid = g.request(self.prog, imp, iargs).or_else(|e| err(span, e))?;
                self.requested.push(iid);
                return Ok((iid, g.sig(self.prog, iid).clone()));
            }
            // (Self a type parameter's placeholder: the trait's method itself, with the trait's signature; its
            // instance is not compiled)
            let placeholder = matches!(args[0], Ty::Struct(s) if self.prog.is_placeholder(s));
            if !sig.has_body && !placeholder {
                return err(span, format!("`{}` has no implementation for `{}`", self.prog.names[id as usize], self.prog.tn(args[0])));
            }
        }
        let mut g = self.insts.lock().unwrap();
        let iid = g.request(self.prog, id, args).or_else(|e| err(span, e))?;
        self.requested.push(iid);
        Ok((iid, g.sig(self.prog, iid).clone()))
    }

    /// 同じ名前の trait impl の関数 (`P::from` が `From<&str>` と `From<String>`) から、最初の引数の型で選ぶ
    pub(crate) fn pick_overload(&self, id: u32, recv: Option<Ty>, args: &[&Expr], expected: Option<Ty>) -> u32 {
        if self.keep_overload.get() == Some(id) {
            self.keep_overload.set(None);
            return id;
        }
        // (by the group's name: a candidate's own may carry an overload suffix, `S::from#1`)
        let gname = &self.prog.names[id as usize];
        let Some(cands) = self.prog.overloads.get(gname).or_else(|| self.prog.overloads.get(gname.split('#').next().unwrap_or(gname))) else {
            return id;
        };
        // (a function without `self` in impls for other self types (`impl L for [u8; 4]` / `[u8; 2]`, `fn n<T>(x: T)`):
        // not an overload of this one. The caller chose it by the self type, and its first argument says nothing of
        // that. Methods with `self` are told apart by the receiver below)
        let own = &self.prog.sigs[id as usize];
        let same_self: Vec<u32>;
        let has_self = |c: &u32| self.prog.sigs[*c as usize].has_self;
        let cands: &Vec<u32> = if own.has_self && recv.is_some() && !cands.iter().all(has_self) {
            // (a method call of a method with `self`: a function without it is no candidate, `s.f()` with `impl S { fn
            // f(x: &S) }` beside `impl Tr for S { fn f(&self) }` is Tr's (self_method, #207). Nor one that takes its
            // receiver another way: self_method_at chose by the receiver's step, and the receiver is passed as that one
            // takes it, `(&mut s).f()` with `Take::f(&mut self)` beside `S::f(&self)` is Take's)
            let kind = |c: u32| match self.prog.sigs[c as usize].params.first() {
                Some(Ty::Ref(m, _)) => Some(*m),
                _ => None,
            };
            same_self = cands.iter().copied().filter(|c| has_self(c) && kind(*c) == kind(id)).collect();
            &same_self
        } else if own.has_self || cands.iter().all(|&c| self.prog.sigs[c as usize].self_ty == own.self_ty) {
            cands
        } else {
            same_self = cands.iter().copied().filter(|&c| self.prog.sigs[c as usize].self_ty == own.self_ty).collect();
            if same_self.len() <= 1 {
                return id;
            }
            &same_self
        };
        // (a function without `self` or arguments in impls of one trait with other arguments for one type (typenum's
        // `impl ToInt<i8> for Z0 { fn to_int() -> i8 }` beside `ToInt<i16>`'s): the one whose result is the type
        // expected, from which rustc infers the trait's argument)
        if recv.is_none() && args.is_empty() && !own.has_self && own.impl_trait.is_some() {
            if let Some(want) = expected.filter(|&w| !self.prog.has_param(w)) {
                let by_ret: Vec<u32> = cands.iter().copied().filter(|&c| self.prog.sigs[c as usize].impl_trait.is_some() && self.prog.sigs[c as usize].ret == want).collect();
                if let [c] = by_ret[..] {
                    return c;
                }
            }
        }
        let Some(at) = recv.or_else(|| args.first().and_then(|a| self.peek(a))) else {
            return id;
        };
        // (an inherent method of an impl for one instance of the type (`impl<T> W<&T>` beside `impl<T> W<&mut T>`):
        // the receiver's references inside the type arguments are its own too)
        let fits = |c: u32, at: Ty| {
            let sig = &self.prog.sigs[c as usize];
            let mut sol = vec![None; sig.generics.len()];
            sig.params.first().is_some_and(|&pt| self.prog.match_ty(pt, at, &mut sol) && (sig.impl_trait.is_some() || self.prog.arg_refs_agree(pt, at)))
        };
        // (impls of two traits of one name, `a::M` and `b::M` for u32: the one whose trait is in scope here)
        let base = |c: u32| self.prog.sigs[c as usize].impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
        // (`&String` は `&str` の候補に: deref の強制変換)
        let deref = match at {
            Ty::Ref(m, Ty::Struct(sid)) if self.prog.structs[*sid as usize].name == "String" => Some(Ty::Ref(m, intern(Ty::Str))),
            Ty::Ref(_, Ty::Ref(m, i)) => Some(Ty::Ref(*m, i)),
            _ => None,
        };
        // (impls of one trait with different arguments for one type (`impl Conv<u64> for u8`, `impl Conv<bool> for
        // u8`): the receiver fits both, the trait's argument tells them apart, as the trait solver's matching of the
        // impls' heads does. Here the argument is what the call's result must be: the one whose result is that type)
        if let Some(want) = expected.filter(|&w| !self.prog.has_param(w)) {
            // (the receiver by value or by reference: the impls' self type)
            let mut bare = at;
            while let Ty::Ref(_, i) = bare {
                bare = *i;
            }
            // (an inherent method that fits comes first, whatever the result: `s.f()` with `impl S { fn f() }`)
            let inherent = self.explicit_trait.is_none()
                && cands.iter().any(|&c| self.prog.sigs[c as usize].impl_trait.is_none() && (fits(c, at) || self.prog.sigs[c as usize].self_ty == Some(bare)));
            let mut fit: Vec<u32> = cands.iter().copied().filter(|&c| self.prog.sigs[c as usize].impl_trait.is_some() && (fits(c, at) || self.prog.sigs[c as usize].self_ty == Some(bare))).collect();
            // (only impls of one trait are told apart this way: of the trait the call names (`A::f(&s)`), or the one
            // trait all candidates are of; two traits' methods of one name are not chosen by their results)
            if let Some((_, tr)) = &self.explicit_trait {
                let tr = tr.rsplit("::").next().unwrap_or(tr);
                fit.retain(|&c| base(c).map(|b| b.rsplit("::").next().unwrap_or(b)) == Some(tr));
            }
            let one_trait = fit.windows(2).all(|w| base(w[0]) == base(w[1]));
            if fit.len() > 1 && one_trait && !inherent {
                let by_ret: Vec<u32> = fit.iter().copied().filter(|&c| self.prog.sigs[c as usize].ret == want).collect();
                if by_ret.len() == 1 {
                    return by_ret[0];
                }
            }
        }
        // (`a::M::m(&x)`: the impl of the trait the call named, also through the deref coercion; never an inherent
        // method of the same name)
        if let Some((m, tr)) = &self.explicit_trait {
            let name = &self.prog.names[id as usize];
            // (an impl for a reference, `impl PartialEq for &X`: its methods are `X::__ref_eq`)
            let last = name.rsplit("::").next().map(|l| l.split('#').next().unwrap_or(l));
            // (an impl for `&&T` too: every `__ref_` off)
            let bare = last.map(|mut l| {
                while let Some(b) = l.strip_prefix("__ref_") {
                    l = b;
                }
                l
            });
            if last == Some(m.as_str()) || bare == Some(m.as_str()) {
                let of_tr = |c: u32| base(c) == Some(tr.as_str());
                // (one for the receiver's own type first: `&[T]`'s impl, not `&mut [T]`'s)
                let exact = |c: u32| {
                    let sig = &self.prog.sigs[c as usize];
                    sig.params.first().is_some_and(|&pt| self.prog.match_ty_exact(pt, at, &mut vec![None; sig.generics.len()]))
                };
                if let Some(&c) = cands.iter().find(|&&c| of_tr(c) && exact(c)).or_else(|| cands.iter().find(|&&c| of_tr(c) && fits(c, at))) {
                    return c;
                }
                if let Some(c) = deref.and_then(|d| cands.iter().copied().find(|&c| of_tr(c) && fits(c, d))) {
                    return c;
                }
                if let Some(&c) = cands.iter().find(|&&c| of_tr(c)) {
                    return c;
                }
            }
        }
        // (inherent methods of one name in impls for disjoint instances, `W<&T>::copied` / `W<&mut T>::copied`: the one
        // whose self type is the receiver's own, no coercion)
        let inherent_only = cands.iter().all(|&c| self.prog.sigs[c as usize].impl_trait.is_none() && self.prog.sigs[c as usize].has_self);
        if inherent_only {
            let exact = |c: u32, at: Ty| {
                let sig = &self.prog.sigs[c as usize];
                let mut sol = vec![None; sig.generics.len()];
                sig.params.first().is_some_and(|&pt| self.prog.match_ty_exact(pt, at, &mut sol))
            };
            if let Some(&c) = cands.iter().find(|&&c| exact(c, at)) {
                return c;
            }
        }
        // (of the impls that fit, one whose parameters' bounds hold for what they matched: `impl<T: Into<J>, E> Tr for
        // Result<T, E>` is not `Result<(), E>`'s when `(): Into<J>` does not hold, `impl<E> Tr for Result<(), E>` is)
        let bounds_hold = |c: u32, at: Ty| {
            let sig = &self.prog.sigs[c as usize];
            let mut sol = vec![None; sig.generics.len()];
            sig.params.first().is_some_and(|&pt| self.prog.match_ty(pt, at, &mut sol))
                && sig.generics.iter().zip(&sol).all(|((_, bs), a)| match a {
                    Some(a) if !self.prog.has_param(*a) => bs.iter().all(|b| self.prog.implements(*a, b)),
                    _ => true,
                })
        };
        // (and whose other parameters take the other arguments' types: `impl Add<A> for S` / `impl Add<B> for S`, the
        // right-hand side's)
        let rest: &[&Expr] = if recv.is_some() { args } else { args.get(1..).unwrap_or(&[]) };
        let args_fit = |c: u32| {
            let sig = &self.prog.sigs[c as usize];
            let mut sol = vec![None; sig.generics.len()];
            sig.params.first().is_some_and(|&pt| self.prog.match_ty(pt, at, &mut sol))
                && sig.params.iter().skip(1).zip(rest).all(|(&pt, a)| self.peek(a).is_none_or(|t| self.prog.has_param(t) || self.prog.match_ty(pt, t, &mut sol)))
        };
        let first_fit = |pick: &dyn Fn(u32) -> bool| {
            let mut fit = cands.iter().copied().filter(|&c| pick(c) && fits(c, at));
            let first = fit.next()?;
            match fit.next() {
                None => Some(first),
                Some(second) => {
                    let mut all: Vec<u32> = [first, second].into_iter().chain(fit).collect();
                    // (an inherent method that fits before the traits' methods, also when a trait's takes the arguments'
                    // types and the inherent one does not: `s.run(7u16)` is `S::run(&self, u8)`'s E0308, as in rustc's
                    // method lookup)
                    let inherent = |c: &u32| self.prog.sigs[*c as usize].impl_trait.is_none();
                    if all.iter().any(inherent) && !all.iter().all(inherent) {
                        all.retain(inherent);
                        if let [one] = all[..] {
                            return Some(one);
                        }
                    }
                    let by = |f: &dyn Fn(u32) -> bool| all.iter().copied().find(|&c| f(c));
                    // (a receiver of the parameter's own type before a coercion: `&[T]`'s impl, not `&mut [T]`'s)
                    let exact = |c: u32| {
                        let sig = &self.prog.sigs[c as usize];
                        sig.params.first().is_some_and(|&pt| self.prog.match_ty_exact(pt, at, &mut vec![None; sig.generics.len()]))
                    };
                    by(&|c| exact(c) && args_fit(c) && bounds_hold(c, at))
                        .or_else(|| by(&|c| args_fit(c) && bounds_hold(c, at)))
                        .or_else(|| by(&|c| args_fit(c)))
                        .or_else(|| by(&|c| bounds_hold(c, at)))
                        .or(Some(first))
                }
            }
        };
        // (an inherent method that fits comes first, as in rustc's method lookup: then nothing changes here)
        let inherent = cands.iter().any(|&c| self.prog.sigs[c as usize].impl_trait.is_none() && fits(c, at));
        if let Some(vis) = self.traits_in_scope().filter(|_| !inherent) {
            if let Some(c) = first_fit(&|c| base(c).is_some_and(|t| vis.iter().any(|v| v == t))) {
                return c;
            }
        }
        if let Some(c) = first_fit(&|_| true) {
            return c;
        }
        deref.and_then(|d| cands.iter().copied().find(|&c| fits(c, d))).unwrap_or(id)
    }

    /// A placeholder's type parameter is `'static` by its function's bounds (`T: 'static`, or a trait whose `Self` must
    /// be `'static`)
    /// A placeholder in t (itself, or in its type arguments, under references, boxes, slices and arrays) that may not
    /// be 'static (placeholder_static)
    fn non_static_placeholder(&self, t: Ty, depth: u32) -> Option<u32> {
        if depth > 12 {
            return None;
        }
        match t {
            Ty::Struct(s) if self.prog.is_placeholder(s) => (!self.placeholder_static(s)).then_some(s),
            Ty::Struct(s) => self.prog.structs[s as usize].args.iter().find_map(|&a| self.non_static_placeholder(a, depth + 1)),
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Slice(i) | Ty::Array(i, _) => self.non_static_placeholder(*i, depth + 1),
            _ => None,
        }
    }

    pub(crate) fn placeholder_static(&self, s: u32) -> bool {
        let Some((id, k)) = self.prog.placeholder_of(s) else { return true };
        let sig = &self.prog.sigs[id as usize];
        let Some((name, bounds)) = sig.generics.get(k as usize) else { return true };
        // (`T: 'a` with `'a` reaching 'static through the outlives bounds (`'a: 'static`))
        let by_outlives = sig.regions.as_deref().is_some_and(|r| {
            r.ty_outlives.iter().any(|(t, c)| {
                if t != name {
                    return false;
                }
                let mut seen = vec![*c];
                let mut k = 0;
                while k < seen.len() {
                    if seen[k] == crate::types::R_STATIC {
                        return true;
                    }
                    for &(a, b) in &r.outlives {
                        if a == seen[k] && !seen.contains(&b) {
                            seen.push(b);
                        }
                    }
                    k += 1;
                }
                false
            })
        });
        // (a trait whose `Self` must be 'static, also through its supertraits (`trait Derived: Base`, `trait Base:
        // 'static`))
        let short = |t: &str| t.split('<').next().unwrap_or(t).trim().rsplit("::").next().unwrap_or("").to_string();
        let by_trait = bounds.iter().any(|b| {
            let mut todo = vec![short(b)];
            let mut seen: Vec<String> = vec![];
            while let Some(n) = todo.pop() {
                if seen.contains(&n) {
                    continue;
                }
                if self.prog.trait_self_outlives.iter().any(|(t, so)| short(t) == n && so.iter().any(|x| x == "static")) {
                    return true;
                }
                for (t, sups) in &self.prog.trait_supers {
                    if short(t) == n {
                        todo.extend(sups.iter().map(|x| short(x)));
                    }
                }
                seen.push(n);
            }
            false
        });
        // (or implied by a parameter's type: `_: &'static T` makes T 'static)
        fn mentions(prog: &crate::program::Program, t: Ty, k: u32, depth: u32) -> bool {
            depth < 12
                && match t {
                    Ty::Param(i) => i as u32 == k,
                    Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Slice(i) | Ty::Array(i, _) => mentions(prog, *i, k, depth + 1),
                    Ty::Struct(s) => prog.structs[s as usize].args.iter().any(|&a| mentions(prog, a, k, depth + 1)),
                    _ => false,
                }
        }
        let by_param = sig.regions.as_deref().is_some_and(|r| {
            let n = r.slots.len().saturating_sub(1);
            sig.params.iter().zip(&r.slots[..n]).any(|(&t, s)| matches!(t, Ty::Ref(_, i) if mentions(self.prog, *i, k, 0)) && s.first() == Some(&crate::types::R_STATIC))
        });
        // (or implied by a type of the signature whose struct or enum bounds its parameter by 'static: `fn foo<T>() ->
        // IfaceToken<T>` with `struct IfaceToken<T: 'static>` (T: 'static holds in the body, in its closures too,
        // borrowck/nested-closure-with-generic-ice.rs))
        fn implies(prog: &crate::program::Program, t: Ty, k: u32, depth: u32) -> bool {
            if depth > 12 {
                return false;
            }
            match t {
                Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Slice(i) | Ty::Array(i, _) => implies(prog, *i, k, depth + 1),
                Ty::Struct(s) => {
                    let b = prog.base_of(s);
                    let args = &prog.structs[s as usize].args;
                    // (also within the argument: `St<Option<T>>` and `St<&'a T>` need T: 'static too; not within a fn
                    // pointer's types (`mentions` does not look there))
                    let own = prog.adt_regions.get(&b).is_some_and(|ar| {
                        args.iter().enumerate().any(|(j, &a)| {
                            mentions(prog, a, k, 0) && prog.structs[b as usize].generics.get(j).is_some_and(|gn| ar.ty_outlives.iter().any(|x| x.0 == *gn && x.1 == crate::types::R_STATIC))
                        })
                    });
                    own || args.iter().any(|&a| implies(prog, a, k, depth + 1))
                }
                _ => false,
            }
        }
        let by_implied = !self.prog.adt_regions.is_empty() && sig.params.iter().chain(std::iter::once(&sig.ret)).any(|&t| implies(self.prog, t, k, 0));
        by_outlives || by_trait || by_param || by_implied || bounds.iter().any(|b| b.trim() == "'static")
    }

    /// Program::trait_scopes at this function: the traits of a colliding name in scope where it is (the innermost
    /// scope around the source line of its item; a closure is in its fn's). None for the prelude, or when no trait
    /// name collides
    pub(crate) fn traits_in_scope(&self) -> Option<&[String]> {
        if self.prog.trait_scopes.is_empty() || !self.user_code {
            return None;
        }
        let ii = self.prog.item_of.get(self.root as usize).copied().flatten()?;
        self.prog.traits_at_item(ii)
    }

    /// E0133: a call written in the source (pos.line != 0) to an `unsafe fn` outside `unsafe`
    pub(crate) fn check_unsafe_call(&self, id: u32, pos: Sp) -> R<()> {
        if self.unsafe_ok == 0 && pos.line != 0 && self.prog.sigs[id as usize].is_unsafe {
            let n = self.prog.names[id as usize].rsplit("::").next().unwrap_or("").split('#').next().unwrap_or("");
            // (`__core_ptr_swap` is `ptr::swap`, kept apart from `mem::swap`'s `__core_swap`)
            let shown = match n.strip_prefix("__core_").unwrap_or(n) {
                "ptr_swap" => "ptr::swap",
                x => x,
            };
            return err(pos, format!("call to unsafe function `{shown}` is unsafe and requires unsafe block (E0133)"));
        }
        Ok(())
    }

    /// E0133 for an unsafe std function codegen lowers itself instead of calling the prelude's `unsafe fn`
    /// (`p.add(n)` / `p.read()` on raw pointers, `slice::from_raw_parts`, `str::from_utf8_unchecked`,
    /// `get_unchecked`, `mem::zeroed`): such a path never reaches check_unsafe_call, so it checks here
    pub(crate) fn check_unsafe_op(&self, name: &str, pos: Sp) -> R<()> {
        if self.unsafe_ok == 0 && pos.line != 0 && self.user_code {
            return err(pos, format!("call to unsafe function `{name}` is unsafe and requires unsafe block (E0133)"));
        }
        Ok(())
    }

    pub(crate) fn call_fn(&mut self, id: u32, recv: Option<(Value, Ty)>, args: &[&Expr], turbofish: Option<&[GArg]>, expected: Option<Ty>, span: Sp) -> R<V> {
        // (a compound assignment's: taken here, so that a call among its arguments does not)
        let act = self.bk.activate.take();
        let t0 = self.bk.call_temps.len();
        self.rg.depth += 1;
        let ha = std::mem::replace(&mut self.bk.hold_args, act.is_some());
        let aa0 = std::mem::take(&mut self.bk.arg_acts);
        let r = self.call_fn_inner(id, recv, args, turbofish, expected, span);
        let arg_acts = std::mem::replace(&mut self.bk.arg_acts, aa0);
        self.bk.hold_args = ha;
        // (the arguments are evaluated and their loans still live: the reservation ends, the `&mut` loan is taken)
        if let Some((ev, tid)) = act {
            if self.bk_on() {
                self.bk_ev(borrowck::Ev::Acc(borrowck::BPlace { local: tid, proj: vec![] }, borrowck::Acc::Read, SP));
                self.bk_ev(borrowck::Ev::Dead(tid, SP));
                if let Some(t) = self.bk.call_temps.iter_mut().rev().find(|t| **t == tid) {
                    *t = u32::MAX;
                }
            }
            self.bk_push(Some(ev));
        }
        // (and the arguments' implicit `&mut` reborrows, the same way)
        for (ev, tid) in arg_acts {
            if self.bk_on() {
                self.bk_ev(borrowck::Ev::Acc(borrowck::BPlace { local: tid, proj: vec![] }, borrowck::Acc::Read, SP));
                self.bk_ev(borrowck::Ev::Dead(tid, SP));
                if let Some(t) = self.bk.call_temps.iter_mut().rev().find(|t| **t == tid) {
                    *t = u32::MAX;
                }
            }
            self.bk_push(Some(ev));
        }
        self.rg.depth -= 1;
        self.bk_call_temps_end(t0);
        r
    }

    /// `__exr_atomic_{load,store,xchg,xadd,cas,fence}(p, ..)` as one instruction (None: not a type it has one for)
    fn atomic_intrinsic(&mut self, op: &str, vals: &[(Value, Ty, Sp)]) -> Option<V> {
        if op == "fence" {
            self.b.ins().fence();
            return Some((None, Ty::Unit));
        }
        let t = match vals.first()?.1 {
            Ty::Ptr(_, t) => *t,
            _ => return None,
        };
        let ct = clif_ty(t)?;
        if !matches!(ct, types::I8 | types::I16 | types::I32 | types::I64) {
            return None;
        }
        let p = vals[0].0;
        let v = match (op, vals.len()) {
            ("load", 1) => self.b.ins().atomic_load(ct, p),
            ("store", 2) => {
                self.b.ins().atomic_store(vals[1].0, p);
                return Some((None, Ty::Unit));
            }
            ("xchg", 2) => self.b.ins().atomic_rmw(crate::ir::AtomicOp::Xchg, p, vals[1].0),
            ("xadd", 2) => self.b.ins().atomic_rmw(crate::ir::AtomicOp::Add, p, vals[1].0),
            ("cas", 3) => self.b.ins().atomic_cas(p, vals[1].0, vals[2].0),
            _ => return None,
        };
        Some((Some(v), t))
    }

    fn call_fn_inner(&mut self, id: u32, recv: Option<(Value, Ty)>, args: &[&Expr], turbofish: Option<&[GArg]>, expected: Option<Ty>, span: Sp) -> R<V> {
        // (this call's: not the calls in its arguments')
        let store_args = std::mem::take(&mut self.bk.store_args);
        // (the receiver's region tree is this call's: the calls in the arguments must not take it)
        let rg_recv = if recv.is_some() { self.rg.recv.take() } else { None };
        let rg_sp = std::mem::take(&mut self.rg.call_sp);
        // (the closure: spawn's argument, or Builder::spawn's after its receiver, which a path call
        // (`Builder::spawn(b, f)`) passes as the first argument)
        if self.spawns(id) {
            let at = usize::from(recv.is_none() && self.prog.names[id as usize] != "__core_spawn");
            if let Some(c) = args.get(at) {
                self.check_spawn_closure(c)?;
            }
        }
        // (戻り値を書く先: 引数の中の呼び出しには渡さない)
        let hint = self.dst_hint.take();
        let id0 = id;
        let id = self.pick_overload(id, recv.map(|r| r.1), args, expected);
        // (E0133 was checked on the fn found before the overload was chosen: an `unsafe fn` chosen in its place, by
        // the traits in scope, needs the same check)
        // (also when the group mixes safe and unsafe methods (a vector's `m` of two traits): the check before the choice
        // is left to here, on the one chosen, even when it is the first)
        let mixed = || {
            let g = self.prog.overloads.get(&self.prog.names[id0 as usize]);
            g.is_some_and(|g| g.iter().any(|&i| self.prog.sigs[i as usize].is_unsafe) && g.iter().any(|&i| !self.prog.sigs[i as usize].is_unsafe))
        };
        if self.prog.sigs[id as usize].is_unsafe && ((id != id0 && !self.prog.sigs[id0 as usize].is_unsafe) || mixed()) && self.unsafe_ok == 0 && self.user_code {
            let n = self.prog.names[id as usize].rsplit("::").next().unwrap_or("").split('#').next().unwrap_or("");
            return err(span, format!("call to unsafe function `{n}` is unsafe and requires unsafe block (E0133)"));
        }
        let sig0 = self.prog.sigs[id as usize].clone();
        // (a callee that requires 'static (`fn f<T: 'static>`, `&'static T`): checked from its arguments on
        // (`f(&g())`: the temporary))
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && (sig0.regions.as_deref().is_some_and(Self::callee_needs_static) || sig0.params.iter().any(|&p| Self::static_object(p))) {
            self.rg_late_on();
        }
        let skip = recv.is_some() as usize;
        if sig0.params.len() != args.len() + skip {
            return err(span, format!("expected {} arguments, found {}", sig0.params.len().saturating_sub(skip), args.len()));
        }
        // 実引数を先に評価する (ジェネリックなら型を見てから呼び先を決める)。
        // 型パラメータは turbofish → 前の引数の順に分かった分だけ、後の引数の期待型に使う
        let mut sol = self.turbofish_sol(&sig0, turbofish)?;
        // 期待型が戻り値と同じ型の (具体的な) インスタンスなら、そこから型パラメータを解いておく
        // (`let r: RefCell<Vec<String>> = RefCell::new(vec![]);` の空の vec![] の要素型)
        if let (Some(Ty::Struct(es)), Ty::Struct(rs)) = (expected, sig0.ret) {
            if !self.prog.has_param(Ty::Struct(es)) && self.prog.has_param(sig0.ret) && self.prog.base_of(es) == self.prog.base_of(rs) {
                self.prog.match_ty(sig0.ret, Ty::Struct(es), &mut sol);
            }
        }
        // (a parameter that is a fn pointer over the type parameters, `f: fn(T)`, and an argument of a fn type (a fn
        // item that is not generic, a fn pointer): it decides them before the arguments are lowered in order, so an
        // earlier literal (`spawn(100, f)` with `fn f(i: isize)`) takes their type, not i32)
        for (a, &pt) in args.iter().zip(&sig0.params[skip.min(sig0.params.len())..]) {
            if !matches!(pt, Ty::Fn(_)) || !self.prog.has_param(pt) {
                continue;
            }
            let ft = match self.fn_item(a) {
                Some(fid) if self.prog.sigs[fid as usize].generics.is_empty() => {
                    let fs = &self.prog.sigs[fid as usize];
                    Some(Ty::Fn(crate::types::intern_fn(crate::types::FnTy { params: fs.params.clone(), ret: fs.ret })))
                }
                Some(_) => None,
                None => self.peek(a).filter(|t| matches!(t, Ty::Fn(_)) && !self.prog.has_param(*t)),
            };
            if let Some(ft) = ft {
                self.prog.match_ty(pt, ft, &mut sol);
            }
        }
        let mut vals: Vec<(Value, Ty, Sp)> = vec![];
        if let Some((r, rt)) = recv {
            self.prog.match_ty(sig0.params[0], rt, &mut sol);
            vals.push((r, rt, span));
            self.trait_args_from_impl(&sig0, &mut sol);
        }
        // `Iterator` を受ける引数に Vec / &Vec / スライス / 配列を渡したら (IntoIterator 相当) イテレータにする
        let conv: Vec<Option<Expr>> = args
            .iter()
            .zip(&sig0.params[skip..])
            .map(|(a, &pt)| {
                let Ty::Param(g) = pt else { return None };
                let iter_bound = sig0.generics.get(g as usize).is_some_and(|gg| gg.1.iter().any(|b| b.starts_with("Iterator")));
                if !iter_bound {
                    return None;
                }
                // (型の記録がない配列リテラル `[(&a, &b)]` も配列として)
                if self.peek(a).is_none() && matches!(strip_paren(a), Expr::Array(_) | Expr::Repeat(..)) {
                    return Some(mcall(paren((*a).clone()), "into_iter", vec![]));
                }
                let t = self.peek(a)?;
                if self.is_iterator_ty(t) {
                    return None;
                }
                let mut base = t;
                while let Ty::Ref(_, i) = base {
                    if base.is_fat() {
                        break;
                    }
                    base = *i;
                }
                let is_vec = matches!(base, Ty::Struct(sid) if self.vec_fields(sid).is_some());
                // (Option も 0 個か 1 個のイテレータ: `it.chain(Some(x))`)
                let is_opt = matches!(base, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == "Option")
                    && self.prog.index.contains_key("Option::into_iter");
                let seq = is_vec || is_opt || matches!(base, Ty::Array(..) | Ty::Ref(_, Ty::Slice(_)));
                if !seq {
                    return None;
                }
                // (a `&mut` one, `zip(dst)` with dst: &mut [u32]: its iter_mut, as IntoIterator for &mut [T] gives)
                let it = if matches!(t, Ty::Ref(true, _)) { "iter_mut" } else { "iter" };
                Some(match (t, strip_paren(a)) {
                    (Ty::Ref(..), Expr::Ref(_, inner)) => mcall(paren((**inner).clone()), it, vec![]),
                    (Ty::Ref(..), _) => mcall(paren((*a).clone()), it, vec![]),
                    _ => mcall(paren((*a).clone()), "into_iter", vec![]),
                })
            })
            .collect();
        let conv_refs: Vec<&Expr> = args.iter().zip(&conv).map(|(a, c)| c.as_ref().unwrap_or(*a)).collect();
        let args: &[&Expr] = &conv_refs;
        // (`f(&[], &[1, 2])` with `fn f<T>(a: &[T], b: &[T])`: an empty array (or `None`) tells nothing of T, so the other
        // arguments' recorded types solve it first)
        let empty_arr = |e: &Expr| match strip_paren(e) {
            Expr::Array(xs) => xs.is_empty(),
            Expr::Ref(_, i) => matches!(strip_paren(i), Expr::Array(xs) if xs.is_empty()),
            // (`None` likewise)
            Expr::Path(None, p) => p.segs.len() == 1 && p.segs[0].name == "None" && p.segs[0].args.is_empty(),
            _ => false,
        };
        if args.iter().zip(&sig0.params[skip..]).any(|(a, &pt)| empty_arr(a) && self.prog.has_param(self.prog.partial_subst(pt, &sol))) {
            // (only the type parameters the empty arguments' types mention, and not from closures: their types are
            // the closures' own)
            let mut wanted = vec![false; sol.len()];
            for (a, &pt) in args.iter().zip(&sig0.params[skip..]) {
                if empty_arr(a) {
                    for (g, w) in wanted.iter_mut().enumerate() {
                        // (pt mentions parameter g when substituting g alone changes it)
                        let mut only = vec![None; sol.len()];
                        only[g] = Some(Ty::Never);
                        *w |= self.prog.partial_subst(pt, &only) != pt;
                    }
                }
            }
            for (a, &pt) in args.iter().zip(&sig0.params[skip..]) {
                if !empty_arr(a) && !matches!(strip_paren(a), Expr::Closure(..)) {
                    if let Some(t) = self.peek(a).filter(|&t| !has_hole(self.prog, t)) {
                        let mut s2 = sol.clone();
                        // (`&[1, 2]` for a `&[T]` parameter: the array coerces to the slice)
                        let t2 = match t {
                            Ty::Ref(m, Ty::Array(e, _)) if matches!(pt, Ty::Ref(_, Ty::Slice(_))) => Ty::Ref(m, intern(Ty::Slice(e))),
                            t => t,
                        };
                        if self.prog.match_ty(pt, t2, &mut s2) {
                            for ((x, y), w) in sol.iter_mut().zip(s2).zip(&wanted) {
                                if x.is_none() && *w {
                                    *x = y;
                                }
                            }
                        }
                    }
                }
            }
        }
        // (each argument's expected type with a `dyn` type's parameters by their names, where it has them)
        let mut unmarked_exp: Vec<Option<Ty>> = vec![None; args.len()];
        for (i, (a, &pt)) in args.iter().zip(&sig0.params[skip..]).enumerate() {
            // (a `dyn` type's parameters not known yet: by their names, as written (`Box<dyn Fn(T) -> T>`))
            let given = self.prog.partial_subst(pt, &sol);
            let um = self.prog.dyn_unmarked(given);
            if um != given {
                unmarked_exp[i] = Some(um);
            }
            let mut exp = Some(um).filter(|&t| !self.prog.has_param(t));
            // (an argument for a type parameter not known yet (`fn g<T>(x: T)`): no coercion site, so a `&mut` local
            // passed is moved, not reborrowed (E0382 when it is used after))
            let to_param = exp.is_none() && matches!(pt, Ty::Param(_));
            // (an integer literal for `T: Foo`: the one integer type that implements every bound, as rustc picks it)
            if exp.is_none() {
                if let (Ty::Param(g), true) = (pt, is_int_lit(a)) {
                    if let Some((_, bounds)) = sig0.generics.get(g as usize) {
                        let traits: Vec<&str> = bounds.iter().map(|b| b.split('<').next().unwrap_or(b).trim()).filter(|b| self.prog.traits.contains_key(*b)).collect();
                        if !traits.is_empty() && !traits.iter().all(|tr| self.prog.implements(Ty::I32, tr)) {
                            let ints = [Ty::U8, Ty::U16, Ty::U32, Ty::U64, Ty::Usize, Ty::I8, Ty::I16, Ty::I64, Ty::Isize, Ty::U128, Ty::I128];
                            let ok: Vec<Ty> = ints.into_iter().filter(|&t| traits.iter().all(|tr| self.prog.implements(t, tr))).collect();
                            if let [t] = ok.as_slice() {
                                exp = Some(*t);
                                sol[g as usize] = Some(*t);
                            }
                        }
                    }
                }
                // (a float literal likewise: f32 when it implements every bound and f64 does not)
                if let (Ty::Param(g), true) = (pt, is_float_lit(a)) {
                    if let Some((_, bounds)) = sig0.generics.get(g as usize) {
                        let traits: Vec<&str> = bounds.iter().map(|b| b.split('<').next().unwrap_or(b).trim()).filter(|b| self.prog.traits.contains_key(*b)).collect();
                        if !traits.is_empty() && !traits.iter().all(|tr| self.prog.implements(Ty::F64, tr)) && traits.iter().all(|tr| self.prog.implements(Ty::F32, tr)) {
                            exp = Some(Ty::F32);
                            sol[g as usize] = Some(Ty::F32);
                        }
                    }
                }
            }
            // (`[i64; N]` の N がまだ分からなくても、配列のリテラルには要素の型を渡す。長さはリテラルから決まる)
            if exp.is_none() {
                if let Ty::Array(e, n) = self.prog.partial_subst(pt, &sol) {
                    if n >= crate::types::CONST_LEN_BASE && !self.prog.has_param(*e) {
                        if let Expr::Array(al) = strip_paren(a) {
                            exp = Some(Ty::Array(e, al.len() as u32));
                        }
                    }
                }
            }
            // `F: Fn(A) -> B` の引数にクロージャを渡すなら、A / B をクロージャの型の手がかりにする
            let fbt = self.fn_bound_with_text(&sig0, pt);
            let fb = fbt.as_ref().map(|x| (x.0.clone(), x.1));
            // (a closure made where nothing was expected, passed by name where a bound takes a reference (for any
            // region: `FnOnce(&u32)`): its parameter of one region is not general enough. Not the prelude's bounds,
            // which write no lifetimes)
            if let (Some(x), Expr::Path(None, ap), false) = (&fbt, strip_paren(a), self.hr_lost.is_empty()) {
                let prelude = self.prog.item_of.get(id as usize).copied().flatten().is_none_or(|i| i >= self.prog.prelude_from);
                if !prelude && !x.2.contains('\'') && ap.segs.len() == 1 {
                    if let Some((_, Ty::Struct(csid))) = self.lookup(&ap.segs[0].name) {
                        let base = self.prog.base_of(csid);
                        if let Some((_, refs)) = self.hr_lost.iter().find(|h| h.0 == base) {
                            if refs.iter().any(|&i| matches!(x.0.get(i), Some(Ty::Ref(..)))) {
                                let k = x.2.split('(').next().unwrap_or("FnOnce").trim().to_string();
                                return Err(self.at(super::borrowck::pos(a), format!("implementation of `{k}` is not general enough (the closure's parameter has one lifetime: write its type, `|x: &T|`)")));
                            }
                        }
                    }
                }
            }
            // (a fn item whose result borrows from a parameter the bound gives for any region (`FnMut(&String) -> T`),
            // where the bound's result is a type parameter: one type for every region, which the fn's is not)
            if let (Some(x), Expr::Path(..)) = (&fbt, strip_paren(a)) {
                if let (Ty::Param(_), Some(fid), false) = (x.1, self.fn_item(a), x.2.contains('\'')) {
                    let user = |i: u32| self.prog.item_of.get(i as usize).copied().flatten().is_some_and(|i| i < self.prog.prelude_from);
                    let fs = &self.prog.sigs[fid as usize];
                    let rf = fs.ret_from;
                    let borrows = |i: usize| rf >> 63 & 1 == 1 && rf >> i & 1 == 1;
                    if user(id) && user(fid) && matches!(fs.ret, Ty::Ref(..)) && (0..x.0.len().min(62)).any(|i| matches!(x.0[i], Ty::Ref(..)) && borrows(i)) {
                        let k = x.2.split('(').next().unwrap_or("FnOnce").trim().to_string();
                        return Err(self.at(super::borrowck::pos(a), format!("implementation of `{k}` is not general enough (the fn's result borrows from its parameter, the bound's result is one type for every lifetime)")));
                    }
                }
            }
            // (a closure passed by name where a bound wants `Fn` / `FnMut`: one that mutates its captures is only `FnMut`,
            // one that moves them out only `FnOnce` (E0525, at the closure). Passed by reference, `&c` is an `Fn..` only
            // where c is an `Fn`, `&mut c` only where c is an `FnMut`. Not where the parameter's type is the reference,
            // `_: &F`: F is the closure's type there)
            let by_ref = match strip_paren(a) {
                Expr::Ref(m, inner) if matches!(strip_paren(inner), Expr::Path(None, _)) => Some((*m, &**inner)),
                _ => None,
            };
            let named: Option<&Expr> = match (strip_paren(a), by_ref) {
                (e @ Expr::Path(None, _), _) => Some(e),
                (_, Some((_, inner))) => Some(inner),
                _ => None,
            };
            let by_ref = by_ref.filter(|_| !matches!(pt, Ty::Ref(..)));
            if let (Some(x), Some(a)) = (&fbt, named) {
                let want = x.2.trim_start();
                let is_fn_bound = ["Fn", "FnMut", "FnOnce"].iter().any(|k| want.strip_prefix(k).is_some_and(|r| r.trim_start().starts_with('(')));
                let kind = |k: &str| match by_ref {
                    Some((false, _)) => k == "Fn" && is_fn_bound,
                    Some((true, _)) => k == "FnMut" && is_fn_bound && !want.starts_with("Fn(") && !want.starts_with("Fn ("),
                    None => want.strip_prefix(k).is_some_and(|r| r.trim_start().starts_with('(')),
                };
                if let Some(t @ Ty::Struct(_)) = self.peek(a) {
                    let has = if self.bk.fnonce.contains(&t) && (kind("Fn") || kind("FnMut")) {
                        Some("FnOnce")
                    } else if self.bk.fnmut.contains(&t) && kind("Fn") {
                        Some("FnMut")
                    } else {
                        None
                    };
                    if let Some(has) = has {
                        let w = if kind("Fn") { "Fn" } else { "FnMut" };
                        let at = self.bk.clo_at.iter().find(|c| c.0 == t).map(|c| c.1).unwrap_or(super::borrowck::pos(a));
                        return Err(self.at(at, format!("expected a closure that implements the `{w}` trait, but this closure only implements `{has}` (E0525)")));
                    }
                }
            }
            // 関数アイテム (`or_insert_with(Vec::new)`) なら、境界を関数ポインタの期待型にして型引数を決める
            if let (Some((bps, bret)), None, Expr::Path(..)) = (&fb, exp, strip_paren(a)) {
                let ft = crate::types::FnTy {
                    params: bps.iter().map(|&t| self.prog.partial_subst(t, &sol)).collect(),
                    ret: self.prog.partial_subst(*bret, &sol),
                };
                // (the return type may be still open (`map(drop)`: B): the parameters decide the instance)
                if !ft.params.iter().any(|&t| self.prog.has_param(t)) {
                    exp = Some(Ty::Fn(crate::types::intern_fn(ft)));
                }
            }
            if let (Some((bps, bret)), Expr::Closure(..)) = (&fb, strip_paren(a)) {
                let ps = bps.iter().map(|&t| self.prog.partial_subst(t, &sol)).collect();
                let r = Some(self.prog.partial_subst(*bret, &sol));
                self.closure_hint = Some((ps, r));
                // (the bound for the closure's region check: not one of the prelude's, which writes no lifetimes
                // (`is_sorted_by_key`'s is `FnMut(&'a T)` in std, not higher-ranked))
                let prelude = self.prog.item_of.get(id as usize).copied().flatten().is_none_or(|i| i >= self.prog.prelude_from);
                // (those of the prelude whose bound is higher-ranked in std as written (`FnMut(&T) -> K`), K one type
                // for every region: `v.sort_by_key(|x| &x.name)` is an error)
                let by_key = prelude && self.prog.names.get(id as usize).is_some_and(|n| ["::sort_by_key", "::sort_unstable_by_key", "::sort_by_cached_key", "::max_by_key", "::min_by_key"].iter().any(|k| n.ends_with(k)));
                let prelude = prelude && !by_key;
                self.closure_bound = None;
                self.closure_prelude = prelude;
                self.closure_fn_only = fbt.as_ref().is_some_and(|x| x.2.trim_start().strip_prefix("Fn").is_some_and(|r| r.trim_start().starts_with('(')));
                if !prelude && self.user_code && self.bk.level >= 2 && regionck::enabled() {
                    if let Some(x) = &fbt {
                        let lts = sig0.regions.as_deref().map(|r| r.lts.clone()).unwrap_or_default();
                        self.closure_bound = Some(Box::new((x.2.to_string(), lts, sig0.generics.iter().map(|g| g.0.clone()).collect())));
                    }
                }
            }
            let top_ref = self.bk_on() && matches!(strip_paren(a), Expr::Ref(..));
            // (an argument `&x` the result does not borrow from: its loan ends with the call)
            let flows = !(top_ref && matches!(pt, Ty::Ref(..)) && !self.bk_ret_from(id, i + skip)) || store_args.contains(&(i + skip));
            // (the call's `&mut self` is taken after its arguments: what one that holds references borrows is held
            // until then)
            let hold = self.bk_on() && self.bk.hold_args;
            // (a call's result passed for a reference: the loans it holds live until the call too, as a `&x`
            // argument's: `f(x, id(&*x))` conflicts when the first argument's reborrow is taken)
            let ref_result = self.bk_on()
                && !top_ref
                && !hold
                && matches!(strip_paren(a), Expr::Call(..) | Expr::MethodCall(..))
                && matches!((pt, self.prog.partial_subst(pt, &sol)), (Ty::Ref(..), _) | (_, Ty::Ref(..)));
            if top_ref || hold || ref_result {
                self.bk_frame();
            }
            let r = self.expr(a, exp);
            if hold && !top_ref {
                let srcs = self.bk_frame_end();
                if r.as_ref().is_ok_and(|x| self.bk_ty(x.1).1) {
                    self.bk_call_temp_all(&srcs);
                }
            }
            if ref_result {
                let srcs = self.bk_frame_end();
                self.bk_call_temp(&srcs, true);
            }
            // (also where a type parameter the call already knows is a `&mut` (`v.push(c.raw)` with v: `Vec<&mut T>`),
            // as rustc's coercion to the expected type)
            // (and where a `&` is: a shared reborrow, `pick(x, &b)` is `pick(&*x, &b)`)
            // (without the region check, a result the signature says borrows from the argument holds the reborrow:
            // `let r = pick(x, &b); *x = 1; use(r)` is E0506)
            match (pt, self.prog.partial_subst(pt, &sol)) {
                (Ty::Ref(m, _), _) | (_, Ty::Ref(m, _)) => {
                    if let Some(ln) = self.bk_reborrow_arg(a, m, true) {
                        if !self.rg.on && self.bk_ret_from(id, i + skip) {
                            self.bk.src(borrowck::Src::Loan(ln));
                        }
                    }
                }
                _ => {}
            }
            // (a closure written where a bound wants `Fn` / `FnMut`: it is that, so its body moving out what it captures
            // by value (`|| { let s = val; }`) is E0507, at the move. Passed by name, it is E0525 (above))
            // (also `Box::new(|| ..)` where a `Box<dyn Fn()>` is expected: the coercion wants it `Fn`)
            if let (Some(x), true) = (&fbt, self.bk_on()) {
                let want = x.2.to_string();
                self.fn_kind_move_check(a, &want)?;
            }
            // (`take(m::BAR)`: a static moved out by its module's path, E0507; by its name alone, the read of the
            // place checks it. A path is no place expression, so it is read as a value elsewhere too (a method's
            // receiver `m::KEY.with(..)`): only an argument passed by value is surely moved)
            if self.bk_on() && matches!(strip_paren(a), Expr::Path(None, p) if p.segs.len() >= 2) && self.static_of(strip_paren(a)).is_some() {
                if let Ok(t) = r.as_ref().map(|x| x.1) {
                    if !self.bk_ty(t).0 {
                        self.drop_move_check(a)?;
                    }
                }
            }
            if to_param && self.bk_on() && matches!(strip_paren(a), Expr::Path(..) | Expr::Field(..)) && matches!(r.as_ref().map(|x| x.1), Ok(Ty::Ref(true, _))) {
                // (`consume(d.r)` with d's type implementing `Drop`: E0509, as for any field moved out of it)
                self.drop_move_check(a)?;
                self.bk_acc(a, borrowck::Acc::Move);
            }
            if top_ref {
                // (the loan of an argument `&x` / `&mut x` lasts until the call: `f(&mut x, &x)` conflicts)
                let srcs = self.bk_frame_end_quiet();
                for &s in &srcs {
                    if flows || matches!(s, borrowck::Src::Local(_)) {
                        self.bk.src(s);
                    }
                }
                // (a `&x` argument also lives until the call: `add(&*a, rewrite(&mut a))` conflicts)
                if hold {
                    self.bk_call_temp_all(&srcs);
                } else {
                    self.bk_call_temp(&srcs, matches!(strip_paren(a), Expr::Ref(..)));
                }
            }
            let (v, mut t) = r?;
            self.closure_hint = None;
            self.closure_bound = None;
            self.closure_prelude = false;
            self.closure_fn_only = false;
            // クロージャの実際の引数・戻り値の型から境界の型パラメータを解く (`map<U, F: FnOnce(T) -> U>`)
            if let Some((bps, bret)) = &fb {
                let mut ct = t;
                while let Ty::Ref(_, i) = ct {
                    ct = *i;
                }
                if let Some((cps, cret)) = self.closure_sig(ct) {
                    for (&b, &c) in bps.iter().zip(&cps) {
                        self.prog.match_ty(b, c, &mut sol);
                    }
                    self.prog.match_ty(*bret, cret, &mut sol);
                }
            }
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            // ジェネリックな `&[T]` に `&[U; N]` / `&Vec<U>` を渡したら、型を解く前にスライスにする
            let mut v = v;
            if let (Ty::Ref(pm, Ty::Slice(_) | Ty::Str), Ty::Ref(am, Ty::Struct(sid)), Some(val)) = (pt, t, v) {
                if am || !pm {
                    if let Some((fat, ft)) = self.deref_to_slice(val, *sid, pm) {
                        let want_str = matches!(pt, Ty::Ref(_, Ty::Str));
                        if want_str == matches!(ft, Ty::Ref(_, Ty::Str)) {
                            v = Some(fat);
                            t = ft;
                        }
                    }
                }
            }
            if let (Ty::Ref(pm, wi @ Ty::Struct(_)), Ty::Ref(am, gi), Some(val)) = (pt, t, v) {
                if am || !pm {
                    if let Some((m, u)) = self.unsize_tail_sol(*wi, *gi, &mut sol) {
                        v = Some(self.make_unsized(val, m, expr_start(a))?);
                        t = Ty::Ref(pm, intern(u));
                    }
                }
            }
            if let (Ty::Ref(pm, Ty::Slice(_)), Ty::Ref(am, Ty::Array(e, n)), Some(val)) = (pt, t, v) {
                if am || !pm {
                    v = Some(self.make_fat(val, *n as i64));
                    t = Ty::Ref(pm, intern(Ty::Slice(e)));
                }
            }
            // 自動 deref 強制: `&&T` を `&T` の引数に渡す
            let want = self.prog.partial_subst(pt, &sol);
            // `m.get("k")` (キーが &str / String の map: std の Borrow 相当): &str を &&str / &String にする
            if let (Ty::Ref(_, wi), Ty::Ref(_, Ty::Str), Some(val)) = (want, t, v) {
                if matches!(wi, Ty::Ref(_, Ty::Str)) {
                    let tmp = self.temp(*wi);
                    self.copy(tmp, val, *wi);
                    v = Some(tmp);
                    t = want;
                } else if let Ty::Struct(ws) = wi {
                    if self.prog.structs[*ws as usize].base.is_none()
                        && self.prog.structs[*ws as usize].name == "String"
                        && self.prog.index.contains_key("String::from")
                    {
                        let sref = Ty::Ref(false, intern(Ty::Str));
                        let (sv, st) = self.call_named("String::from", vec![], &[(val, sref)], SP)?;
                        if let Some(sv) = sv {
                            self.own_temp(sv, st);
                            v = Some(sv);
                            t = Ty::Ref(false, intern(st));
                        }
                    }
                }
            }
            // `set.get(&t)` (要素が `&T` の HashSet / HashMap: std の Borrow 相当): `&T` を `&&T` にする
            if let (Ty::Ref(_, Ty::Ref(wm, wx)), Ty::Ref(tm, tx), Some(val)) = (want, t, v) {
                if **wx == *tx && !tx.is_fat() && !self.prog.has_param(want) && (!*wm || tm) {
                    let tmp = self.temp(Ty::Ref(*wm, wx));
                    self.b.ins().store(mf(), val, tmp, 0);
                    v = Some(tmp);
                    t = want;
                }
            }
            // (`&Box<T>` も `&T` に)
            while let (Ty::Ref(_, _), Ty::Ref(m, inner @ (Ty::Ref(..) | Ty::Box(_))), Some(val)) = (want, t, v) {
                if t == want || self.prog.has_param(want) {
                    break;
                }
                // fat な参照 (`&str` / `&[T]`) はメモリ上の 16 byte のアドレスが値なのでそのまま
                v = Some(if inner.is_fat() { val } else { self.b.ins().load(PTR, mf(), val, 0) });
                t = match inner {
                    Ty::Box(x) => Ty::Ref(m, x),
                    i => *i,
                };
            }
            if let (Ty::Ref(false, wi), Ty::Ref(true, gi)) = (want, t) {
                if wi == gi {
                    t = want;
                }
            }
            self.prog.match_ty(pt, t, &mut sol);
            // (`()` の引数は値を持たない: 渡すときに飛ばす)
            let v = match v {
                Some(v) => v,
                None => self.b.ins().iconst(types::I8, 0),
            };
            // (a block argument's type is its tail's: rustc points there, `f({ String::new() })`)
            vals.push((v, t, value_pos(a)));
        }
        let arg_tys: Vec<Ty> = vals.iter().map(|v| v.1).collect();
        // 引数の型から改めて解く。引数に現れない型パラメータ (クロージャの戻り値など) は sol から
        let mut sol0 = self.turbofish_sol(&sig0, turbofish)?;
        for (a, b) in sol0.iter_mut().zip(&sol) {
            if a.is_none() {
                *a = *b;
            }
        }
        // (E0277: a type parameter of the program's fn that is not `?Sized` takes a sized type; a placeholder of a
        // caller's `?Sized` parameter may be unsized (`fn f1<X: ?Sized>(x: &X) { f2::<X>(x) }`))
        if let Some(ks) = self.prog.fn_sized_gens.get(&id) {
            for &k in ks {
                if let Some(t) = sol0.get(k).copied().flatten() {
                    if self.prog.maybe_unsized(t) {
                        let n = crate::traits::display_ty(self.prog, t);
                        return Err(self.at(span, format!("the size for values of type `{n}` cannot be known at compilation time {}", crate::traits::SOLVED_NO)));
                    }
                }
            }
        }
        // (the prelude's atomic intrinsics, on an integer, bool, char or pointer of 1, 2, 4 or 8 bytes: one atomic
        // instruction each, not a call. Their bodies are only what a value of another type would do)
        if let Some(op) = self.prog.names[id as usize].strip_prefix("__exr_atomic_") {
            if let Some(r) = self.atomic_intrinsic(op, &vals) {
                return Ok(r);
            }
        }
        // (drop の要らない型の `__exr_drop_in_place::<T>` / `drop(x)` は何もしない: インスタンスも作らない)
        if matches!(self.prog.names[id as usize].as_str(), "__exr_drop_in_place" | "__core_drop") && arg_tys.len() == 1 {
            let t = match arg_tys[0] {
                Ty::Ptr(_, t) if self.prog.names[id as usize] == "__exr_drop_in_place" => Some(*t),
                t if self.prog.names[id as usize] == "__core_drop" => Some(t),
                _ => None,
            };
            if t.is_some_and(|t| !self.prog.has_param(t) && !self.needs_drop(t)) {
                return Ok((None, Ty::Unit));
            }
        }
        // (a turbofish names the pointee: it must be the argument's (`size_of_val::<u8>(s)` with s: &[u8] is E0308))
        let tf_ok = |t: Ty, me: &Self| match turbofish {
            Some(tf) => match tf.first() {
                Some(GArg::Ty(pt)) => me.parse_ty(pt).is_ok_and(|w| w == t),
                _ => true,
            },
            None => true,
        };
        // (`drop_in_place(p)` of a pointer to a struct with an unsized tail: no value of it can be read out, the drop
        // glue drops it in place through the pair)
        if self.prog.names[id as usize] == "__core_drop_in_place" && vals.len() == 1 {
            // (`*mut` only: `drop_in_place(p)` of a `*const` is E0308)
            if let Ty::Ptr(true, t) = vals[0].1 {
                if t.is_dst_struct() && tf_ok(*t, self) {
                    let Some(&g) = self.prog.index.get(crate::prelude::DROP_GLUE) else { return err(span, "internal: no drop glue") };
                    let iid = {
                        let mut ins = self.insts.lock().unwrap();
                        ins.request(self.prog, g, vec![*t]).or_else(|e| err(span, e))?
                    };
                    self.requested.push(iid);
                    let fs = self.insts.lock().unwrap().sig(self.prog, iid).clone();
                    let f = self.fref(iid, &fs);
                    self.b.ins().call(f, &[vals[0].0]);
                    return Ok((None, Ty::Unit));
                }
            }
        }
        // (`size_of_val(x)` / `align_of_val(x)` of a pointer to an unsized type: from the pair's second word)
        if matches!(self.prog.names[id as usize].as_str(), "__core_size_of_val" | "__core_align_of_val") && vals.len() == 1 {
            // (a reference only: `size_of_val(p)` of a raw pointer is E0308)
            if let Ty::Ref(_, t) = vals[0].1 {
                if t.is_unsized() && tf_ok(*t, self) {
                    let meta = self.b.ins().load(types::I64, mf(), vals[0].0, 8);
                    let (size, align) = self.unsized_size_align(meta, *t);
                    let v = if self.prog.names[id as usize] == "__core_size_of_val" { size } else { align };
                    return Ok((Some(v), Ty::Usize));
                }
            }
        }
        // (a callee that requires 'static (`fn f<T: 'static>`, `&'static T`): from here on, checked)
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && self.prog.sigs[id as usize].regions.as_deref().is_some_and(Self::callee_needs_static) {
            self.rg_late_on();
        }
        // (in a closure's body, whose region check has no placeholders for its function's type parameters: a callee
        // that requires `T: 'static` of the function's T, which none of the function's bounds gives (E0310))
        if self.user_code && self.inline_depth == 0 && self.prog.closures.contains_key(&self.rg.base) {
            if let Some(r) = self.prog.sigs[id as usize].regions.as_deref() {
                for (tp, c) in &r.ty_outlives {
                    if *c != crate::types::R_STATIC {
                        continue;
                    }
                    let Some(g) = sig0.generics.iter().position(|x| x.0 == *tp) else { continue };
                    // (also one in the type argument: `needs_static::<Option<T>>()`)
                    if let Some(s) = sol0.get(g).copied().flatten().and_then(|t| self.non_static_placeholder(t, 0)) {
                        let n = self.prog.structs[s as usize].name.trim_end_matches('#').to_string();
                        return Err(self.at(if span.line == 0 { rg_sp } else { span }, format!("the parameter type `{n}` may not live long enough (it must outlive `'static`) (E0310)")));
                    }
                }
            }
        }
        let sol_rg = if self.rg_on() { Some(sol0.clone()) } else { None };
        let explicit = turbofish.is_some_and(|t| t.iter().any(|a| matches!(a, GArg::Ty(_))));
        let prev = crate::traits::EXPLICIT_ARGS.with(|e| e.replace(explicit));
        let r = self.resolve_call(id, &arg_tys, sol0, expected, span);
        crate::traits::EXPLICIT_ARGS.with(|e| e.set(prev));
        let (target, sig) = r?;
        // (a closure given as a value: by its type, now evaluated (vals holds the receiver or the builder first))
        if self.spawns(id) && self.user_code {
            let (ai, vi) = match (self.prog.names[id as usize] == "__core_spawn", recv.is_some()) {
                (true, _) => (0, 0),
                (false, true) => (0, 1),
                (false, false) => (1, 1),
            };
            if let (Some(c), Some(v)) = (args.get(ai), vals.get(vi)) {
                if !matches!(strip_paren(c), Expr::Closure(..)) {
                    self.check_spawn_value(c, v.1)?;
                }
            }
        }
        // (spawn's T goes to the thread that joins: it must be Send, as std's `T: Send`. Builder::spawn's comes in a
        // Result)
        if self.spawns(id) && self.user_code {
            let mut ret = sig.ret;
            if let Ty::Struct(s) = ret {
                if self.prog.structs[self.prog.base_of(s) as usize].name == "Result" {
                    ret = self.prog.structs[s as usize].args.first().copied().unwrap_or(ret);
                }
            }
            if let Ty::Struct(s) = ret {
                if let Some(&t) = self.prog.structs[s as usize].args.first() {
                    if not_send(self.prog, t) {
                        return Err(self.at(span, format!("`{}` cannot be sent between threads safely", self.prog.tn(t))));
                    }
                }
            }
        }
        if let Some(sol) = sol_rg {
            self.rg_call(id, &sol, turbofish, args, &sig.params, sig.ret, rg_recv, skip == 1, if span.line == 0 { rg_sp } else { span });
        }
        if self.prog.names[id as usize] == "__core_transmute" && sig.params.len() == 1 {
            self.check_transmute(sig.params[0], sig.ret, span)?;
        }
        // (`size_of_val(s)` / `align_of_val(s)` of a slice or str: the length times the element's size, the element's
        //  alignment; the prelude's `size_of::<T>()` has no length)
        if matches!(self.prog.names[id as usize].as_str(), "__core_size_of_val" | "__core_align_of_val") && vals.len() == 1 {
            let el = match sig.params[0] {
                Ty::Ref(_, Ty::Slice(e)) => Some(**e),
                Ty::Ref(_, Ty::Str) => Some(Ty::U8),
                _ => None,
            };
            if let Some(e) = el {
                let (es, ea) = self.prog.size_align(e);
                if self.prog.names[id as usize] == "__core_align_of_val" {
                    return Ok((Some(self.b.ins().iconst(types::I64, ea as i64)), Ty::Usize));
                }
                let fat = vals[0].0;
                let n = self.b.ins().load(types::I64, mf(), fat, 8);
                let k = self.b.ins().iconst(types::I64, es as i64);
                return Ok((Some(self.b.ins().imul(n, k)), Ty::Usize));
            }
        }
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = match hint {
                Some(d) if sig.ret == self.ret => d,
                _ => self.temp_fresh(sig.ret),
            };
            cv.push(a);
            Some(a)
        } else {
            None
        };
        for (k, ((v, t, sp), &pt)) in vals.into_iter().zip(&sig.params).enumerate() {
            // (a value coerced to a `dyn` type with the names of the parameters not known then (dyn_unmarked): the
            // instance's type, as the parameters only name the trait's arguments)
            let coerced = || k.checked_sub(skip).and_then(|i| unmarked_exp.get(i).copied().flatten()) == Some(t) || sig0.params.get(k).is_some_and(|&g| self.prog.has_param(g) && self.prog.dyn_unmarked(g) == t);
            let t = if t != pt && coerced() { pt } else { t };
            let (v, t) = if matches!(pt, Ty::Ref(false, Ty::Str)) {
                let (v2, t2) = self.deref_coerce(Some(v), t, pt);
                (v2.unwrap_or(v), t2)
            } else {
                (v, t)
            };
            // 参照の参照を参照に (deref の強制変換: `t.starts_with(k)` で k: &&str)
            let (v, t) = match (t, pt) {
                (Ty::Ref(_, a), Ty::Ref(_, pi)) if matches!(*a, Ty::Ref(_, i) if *i == *pi) => {
                    if pt.is_fat() {
                        // (&&str は fat pointer の組を指すアドレス: それがそのまま &str の値)
                        (v, pt)
                    } else {
                        (self.b.ins().load(PTR, mf(), v, 0), pt)
                    }
                }
                // (`&mut x` for a `*mut T` parameter)
                (Ty::Ref(m1, a), Ty::Ptr(m2, b)) if a == b && (m1 || !m2) && !a.is_fat() => (v, pt),
                // (a `&mut [T]` for a generic `&T`, `size_of_val(iv_dst)`: the same value, thin or fat)
                (Ty::Ref(true, a), Ty::Ref(false, b)) if a == b => (v, pt),
                _ => (v, t),
            };
            self.same(sp, t, pt)?;
            self.push_arg(&mut cv, v, pt);
        }
        // (a foreign fn that takes or returns a struct by value: the C calling convention)
        if sig.is_extern && (sig.ret.is_agg() || sig.params.iter().any(|p| p.is_agg())) {
            return self.c_call(target, &sig, cv, sret, span);
        }
        // (-O: 小さい prelude の関数は呼ばずに本体を展開する)
        if let Some(r) = self.try_inline(target, &sig, &cv)? {
            if let Some(a) = sret {
                return Ok((Some(a), sig.ret));
            }
            return Ok((r, sig.ret));
        }
        // (the location of the method call that reaches target, written before the receiver and the arguments
        // were evaluated, may have been overwritten by a call among them)
        if let Some((m, loc)) = self.caller_locs.last() {
            if self.prog.names.get(id as usize).is_some_and(|n| n.rsplit("::").next() == Some(m.as_str())) {
                if let Some(&ci) = self.prog.const_index.get("__EXR_CALLER") {
                    let v = self.b.ins().iconst(types::I64, *loc);
                    let a = self.static_sym(ci);
                    self.b.ins().store(mf(), v, a, 0);
                }
            }
        }
        let f = self.fref(target, &sig);
        let call = self.b.ins().call(f, &cv);
        if sig.ret == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(self.dead());
        }
        if let Some(a) = sret {
            return Ok((Some(a), sig.ret));
        }
        let r = self.b.inst_results(call).first().copied();
        Ok((r, sig.ret))
    }

    /// The scalars at the leaves of a C struct: (offset, float, size). None: a type C cannot take by value (an enum ..)
    fn c_leaves(&self, t: Ty, off: u32, out: &mut Vec<(u32, bool, u32)>) -> Option<()> {
        match t {
            Ty::Struct(sid) => {
                let sd = &self.prog.structs[sid as usize];
                if sd.is_enum {
                    return None;
                }
                for &(_, ft, fo) in &sd.fields {
                    self.c_leaves(ft, off + fo, out)?;
                }
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                for i in 0..n {
                    self.c_leaves(*e, off + i * es, out)?;
                }
            }
            t if t.is_fat() => return None,
            t => {
                let ct = clif_ty(t)?;
                if ct == types::I128 {
                    return None;
                }
                out.push((off, t.is_float(), self.prog.size_align(t).0));
            }
        }
        Some(())
    }

    /// System V x86-64: how a struct of at most 16 bytes goes in registers (one scalar per eightbyte: SSE when every
    /// scalar in it is a float, else INTEGER). None: in memory (larger, or not C-shaped)
    fn c_classes(&self, t: Ty) -> Option<Vec<Type>> {
        let (size, _) = self.prog.size_align(t);
        if size > 16 {
            return None;
        }
        let mut leaves = vec![];
        self.c_leaves(t, 0, &mut leaves)?;
        Some(
            (0..size.div_ceil(8))
                .map(|k| {
                    let lv: Vec<_> = leaves.iter().filter(|l| l.0 / 8 == k).collect();
                    if !lv.is_empty() && lv.iter().all(|l| l.1) {
                        if lv.len() == 1 && lv[0].2 == 4 { types::F32 } else { types::F64 }
                    } else {
                        types::I64
                    }
                })
                .collect(),
        )
    }

    /// A call of a foreign fn with structs by value: a struct of at most 16 bytes is passed as its eightbytes and
    /// returned in rax / rdx / xmm0 / xmm1; a larger return comes through the hidden pointer (our sret, as in C)
    fn c_call(&mut self, target: u32, sig: &FnSig, cv: Vec<Value>, sret: Option<Value>, span: Sp) -> R<V> {
        let mut it = cv.into_iter();
        let mut args = vec![];
        let ret_regs = match sret {
            Some(_) => {
                let p = it.next().unwrap();
                match self.c_classes(sig.ret) {
                    Some(c) => Some(c),
                    None => {
                        args.push(p);
                        None
                    }
                }
            }
            None => None,
        };
        let pair = Ty::Array(intern(Ty::U64), 2);
        for &pt in &sig.params {
            if !pt.is_agg() && clif_ty(pt).is_none() {
                continue;
            }
            let v = it.next().unwrap();
            if !pt.is_agg() {
                args.push(v);
                continue;
            }
            let Some(cls) = self.c_classes(pt) else {
                return err(span, "passing this struct by value to an extern fn is not supported (only C structs of up to 16 bytes are)");
            };
            if cls.is_empty() {
                continue;
            }
            let tmp = self.temp(pair);
            self.copy(tmp, v, pt);
            for (k, &ct) in cls.iter().enumerate() {
                args.push(self.b.ins().load(ct, mf(), tmp, 8 * k as i32));
            }
        }
        let rets: Vec<Type> = match &ret_regs {
            Some(c) => c.clone(),
            None if !sig.ret.is_agg() => clif_ty(sig.ret).into_iter().collect(),
            None => vec![],
        };
        let f = self.b.import_function(&sym_name('f', target), rets);
        let call = self.b.ins().call(f, &args);
        if sig.ret == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(self.dead());
        }
        if let (Some(a), Some(c)) = (sret, &ret_regs) {
            let rs: Vec<Value> = self.b.inst_results(call).to_vec();
            if !c.is_empty() {
                let tmp = self.temp(pair);
                for (k, r) in rs.into_iter().enumerate() {
                    self.b.ins().store(mf(), r, tmp, 8 * k as i32);
                }
                self.copy(a, tmp, sig.ret);
            }
            return Ok((Some(a), sig.ret));
        }
        if let Some(a) = sret {
            return Ok((Some(a), sig.ret));
        }
        Ok((self.b.inst_results(call).first().copied(), sig.ret))
    }

    pub(crate) fn push_arg(&mut self, vals: &mut Vec<Value>, v: Value, pt: Ty) {
        if !pt.is_agg() && clif_ty(pt).is_none() {
            return;
        }
        if pt.is_agg() {
            // (呼び出しの戻り値の一時領域はそのまま渡す)
            if self.take_fresh(v).is_some() {
                vals.push(v);
                return;
            }
            // 値渡し: 呼び出し側でコピーしてから渡す
            let tmp = self.temp(pt);
            self.copy(tmp, v, pt);
            vals.push(tmp);
        } else {
            vals.push(v);
        }
    }

    pub(crate) fn convert(&mut self, v: Value, from: Ty, to: Ty) -> Value {
        let (fb, tb) = (from.bits(), to.bits());
        let tt = clif_ty(to).unwrap();
        if fb == tb {
            v
        } else if fb > tb {
            self.b.ins().ireduce(tt, v)
        } else if from.signed() {
            self.b.ins().sextend(tt, v)
        } else {
            self.b.ins().uextend(tt, v)
        }
    }

    /// リテラル (e は `Expr::Lit` 全体)
    /// overflowing_literals: the decimal digits (`-` first when negative) past the integer type t
    #[cold]
    #[inline(never)]
    fn lit_range(&self, t: Ty, d: &str) -> R<()> {
        let (neg, d) = match d.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, d),
        };
        if lit_fits(t, d, neg) {
            return Ok(());
        }
        err(SP, format!("literal out of range for `{}`", self.prog.tn(t)))
    }

    pub(crate) fn lit(&mut self, text: &str, expected: Option<Ty>) -> R<V> {
        let Some(l) = lit::lit(text) else { return err(SP, "unsupported literal") };
        match &l {
            // 浮動小数点: `1.5` / `2e3` / `1.0f32` / `1f64` (と、浮動小数点が期待される整数リテラル)
            Lit::Float(_) | Lit::Int(_)
                if matches!(l, Lit::Float(_))
                    || matches!(&l, Lit::Int(i) if matches!(i.suffix(), "f32" | "f64"))
                    || (matches!(&l, Lit::Int(i) if i.suffix().is_empty()) && expected.is_some_and(|t| t.is_float())) =>
            {
                // (an integer literal is never a float: `let y: f64 = 2;` is rustc's E0308, in the program's own code)
                if let (Lit::Int(i), Some(t), true) = (&l, expected, self.user_code) {
                    if i.suffix().is_empty() {
                        self.int_as_float = text.as_ptr() as usize;
                        return err(SP, format!("mismatched types: expected `{}`, found integer", self.prog.tn(t)));
                    }
                }
                let (digits, suffix) = match &l {
                    Lit::Float(f) => (f.base10_digits().to_string(), f.suffix().to_string()),
                    Lit::Int(i) => (i.base10_digits().to_string(), i.suffix().to_string()),
                    _ => unreachable!(),
                };
                let ty = match suffix.as_str() {
                    "f32" => Ty::F32,
                    "f64" => Ty::F64,
                    _ => expected.filter(|t| t.is_float()).unwrap_or(Ty::F64),
                };
                // (an f32 literal is rounded once, from its digits: through f64 it can round twice)
                let v = if ty == Ty::F32 {
                    let x: f32 = digits.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?;
                    self.b.ins().f32const(x)
                } else {
                    let x: f64 = digits.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?;
                    self.b.ins().f64const(x)
                };
                Ok((Some(v), ty))
            }
            Lit::Int(i) => {
                let fixed = Ty::from_suffix(i.suffix()).or(expected.filter(|t| t.is_int()));
                // (overflowing_literals, deny by default: a literal of a type its suffix or the expected one fixes, past
                // that type: `256u8`, `let x: u8 = 256;`)
                // (a literal of up to 2 digits fits every integer type: most literals stop here)
                if let Some(t) = fixed.filter(|_| i.base10_digits().len() > 2) {
                    if self.user_code && !self.prog.allow_overflowing_literals {
                        self.lit_range(t, i.base10_digits())?;
                    }
                }
                let ty = fixed.unwrap_or(Ty::I32);
                // 負のリテラル (パターンの `-1` など) も来るので i128 で読む
                let n = i
                    .base10_digits()
                    .parse::<i128>()
                    .or_else(|_| i.base10_digits().parse::<u128>().map(|v| v as i128))
                    .map_err(|e| e.to_string())?;
                let v = self.int_const(ty, n);
                Ok((Some(v), ty))
            }
            Lit::Bool(b) => Ok((Some(self.b.ins().iconst(types::I8, *b as i64)), Ty::Bool)),
            Lit::Byte(b) => Ok((Some(self.b.ins().iconst(types::I8, *b as i64)), Ty::U8)),
            Lit::Char(c) => Ok((Some(self.b.ins().iconst(types::I32, *c as i64)), Ty::Char)),
            // "..." は &'static str (fat pointer)、b"..." は &'static [u8; N]
            Lit::Str(st) => {
                let bytes = st.clone().into_bytes();
                let n = bytes.len() as i64;
                let a = self.data_addr(bytes);
                Ok((Some(self.make_fat(a, n)), Ty::Ref(false, intern(Ty::Str))))
            }
            Lit::ByteStr(bs) => {
                let bytes = bs.clone();
                let n = bytes.len() as u32;
                let a = self.data_addr(bytes);
                Ok((Some(a), Ty::Ref(false, intern(Ty::Array(intern(Ty::U8), n)))))
            }
            // (`c"hello"`: the prelude's `&'static CStr` of its bytes and their 0)
            Lit::CStr(bs) => {
                let mut text = String::from("b\"");
                for &c in bs.iter().chain(std::iter::once(&0u8)) {
                    text.push_str(&format!("\\x{c:02x}"));
                }
                text.push('"');
                let call = Expr::Call(Box::new(path_expr("__exr_cstr_lit")), vec![Expr::Lit(text, crate::parse::Pos::default())]);
                let e: &Expr = self.keep_expr(call);
                self.expr(e, expected)
            }
            _ => err(SP, "unsupported literal"),
        }
    }

    /// of なら None、そうでなければ Some(v)
    pub(crate) fn option_if(&mut self, v: Value, t: Ty, of: Value) -> R<V> {
        let ot = {
            let ob = *self.prog.struct_index.get("Option").ok_or("Option is not available")?;
            Ty::Struct(self.prog.instantiate(ob, vec![t])?)
        };
        let Ty::Struct(oi) = ot else { unreachable!() };
        let out = self.temp(ot);
        let some = self.prog.structs[oi as usize].variants.iter().position(|v| v.name == "Some").unwrap();
        let none = self.prog.structs[oi as usize].variants.iter().position(|v| v.name == "None").unwrap();
        let sb = self.b.create_block();
        let nb = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(of, nb, &[], sb, &[]);
        self.b.switch_to_block(sb);
        self.store_tag(out, oi, some);
        let off = self.prog.structs[oi as usize].variants[some].fields[0].2;
        self.b.ins().store(mf(), v, out, off as i32);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(nb);
        self.store_tag(out, oi, none);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        Ok((Some(out), ot))
    }

    /// `(v, flag)` のタプル
    pub(crate) fn pair_with_bool(&mut self, v: Value, t: Ty, flag: Value) -> R<V> {
        let sid = self.prog.instantiate(self.prog.tuple_base(2), vec![t, Ty::Bool])?;
        let tt = Ty::Struct(sid);
        let out = self.temp(tt);
        let (o0, o1) = {
            let f = &self.prog.structs[sid as usize].fields;
            (f[0].2, f[1].2)
        };
        self.b.ins().store(mf(), v, out, o0 as i32);
        self.b.ins().store(mf(), flag, out, o1 as i32);
        Ok((Some(out), tt))
    }

    /// clz / ctz などの結果 (受け手の幅) を u32 に
    pub(crate) fn to_u32(&mut self, v: Value, ct: Type) -> Value {
        match ct.bits() {
            32 => v,
            b if b > 32 => self.b.ins().ireduce(types::I32, v),
            _ => self.b.ins().uextend(types::I32, v),
        }
    }

    /// 128 bit の除算・剰余 (prelude の関数)
    pub(crate) fn div128(&mut self, l: Value, r: Value, t: Ty, rem: bool, span: Sp) -> R<Value> {
        let f = if t.signed() { "__exr_sdiv128" } else { "__exr_udiv128" };
        let rv = self.b.ins().iconst(types::I8, rem as i64);
        let (v, _) = self.call_named(f, vec![Ty::Unit], &[(l, t), (r, t), (rv, Ty::Bool)], span)?;
        Ok(v.unwrap())
    }

    /// iconst (I128 は 64 bit から符号拡張する。Cranelift の iconst は 64 bit まで)
    pub(crate) fn iconst_t(&mut self, ct: Type, x: i64) -> Value {
        if ct == types::I128 {
            let v = self.b.ins().iconst(types::I64, x);
            self.b.ins().sextend(types::I128, v)
        } else {
            self.b.ins().iconst(ct, x)
        }
    }

    /// 整数の定数 (128 bit は 64 bit の 2 つをつなぐ)
    pub(crate) fn int_const(&mut self, ty: Ty, n: i128) -> Value {
        if ty.bits() == 128 {
            let lo = self.b.ins().iconst(types::I64, n as u64 as i64);
            let hi = self.b.ins().iconst(types::I64, (n >> 64) as u64 as i64);
            self.b.ins().iconcat(lo, hi)
        } else {
            self.b.ins().iconst(clif_ty(ty).unwrap(), n as u64 as i64)
        }
    }

    /// `i32::pow(2, 10)` / `Trait::m(&x, a)` / `Ord::cmp(&a, &b)` / `HashMap::from([..])` などを
    /// メソッド呼び出しなどに書き換えたもの (front.rs の Program::prim_method_call。e は `Expr::Call` 全体)
    /// The rewrite of `x.m(..)` when x's type is a primitive that a type parameter of this fn stands for, and a trait
    /// of that parameter's bounds has an impl of m for it: `<T as Tr>::m(&x, ..)` (or `x` for a `self` method)
    pub(crate) fn bound_method_call(&self, recv: &Expr, method: &str, gargs: &[GArg], margs: &[Expr]) -> Option<Expr> {
        if self.env.params.is_empty() || self.generic_args.is_empty() {
            return None;
        }
        // (only a value of the type parameter: a parameter declared `x: T` / `x: &T`, not another value that happens to
        // be of the type T stands for (`let n: i32` beside `T = i32`))
        let Expr::Path(None, rp) = strip_paren(recv) else { return None };
        let rn = ident_of(rp)?;
        let declared = self.generic_args.iter().find(|a| a.0 == rn).map(|a| a.1.clone())?;
        // (the parameter itself: its binding is the fn's outermost one, not a `let x = ..` shadowing it)
        if self.scopes.iter().rposition(|s| s.iter().any(|b| b.0 == rn)) != Some(0) {
            return None;
        }
        let t = self.peek(recv)?;
        let (t, by_ref) = match t {
            Ty::Ref(_, i) => (*i, true),
            t => (t, false),
        };
        // (a primitive; or a struct when two traits of the program's have the method, so that the bound decides which:
        // `s.scaled()` of `S: S1` with S's instance implementing S0 and S1)
        let multi = || matches!(t, Ty::Struct(_)) && self.prog.multi_trait_methods.contains(method);
        if !(t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char) || multi()) {
            return None;
        }
        let gens = &self.prog.sigs.get(self.root as usize)?.generics;
        for (g, bounds) in gens {
            if *g != declared || self.env.params.iter().find(|p| &p.0 == g).map(|p| p.1) != Some(t) {
                continue;
            }
            for b in bounds {
                let tr = b.split('<').next().unwrap_or(b).trim();
                // (the impl's method, or the trait's default when the impl leaves it out: `<u16 as S0>::scaled`)
                let Some(id) = self.trait_candidate(&format!("{}::{method}", self.ty_key(t)), tr).or_else(|| self.trait_default(tr, method, t)) else { continue };
                let first = self.prog.sigs[id as usize].params.first().copied();
                let takes_ref = matches!(first, Some(Ty::Ref(..)));
                let takes_mut = matches!(first, Some(Ty::Ref(true, _)));
                let Ok(Expr::Path(q, mut p)) = crate::parse::parse_expr(&format!("<{} as {tr}>::{method}", self.prog.tn(t))) else { continue };
                // (the call's own type arguments: `x.m::<u8>()`)
                if let Some(last) = p.segs.last_mut() {
                    last.args = gargs.to_vec();
                }
                let r = paren(recv.clone());
                // (`&self` / `&mut self` of a value: borrowed as the method takes it; `self` of a `&T`: the value)
                let a0 = match (takes_ref, by_ref) {
                    (true, false) => Expr::Ref(takes_mut, Box::new(r)),
                    (false, true) => Expr::Unary("*".to_string(), Box::new(r), Default::default()),
                    _ => r,
                };
                let mut args = vec![a0];
                args.extend(margs.iter().cloned());
                return Some(Expr::Call(Box::new(Expr::Path(q, p)), args));
            }
        }
        None
    }

    pub(crate) fn prim_method_call(&self, e: &Expr) -> Option<Expr> {
        let Expr::Call(func, cargs) = e else { return None };
        let Expr::Path(qself, p) = &**func else { return None };
        if cargs.is_empty() || qself.is_some() {
            return None;
        }
        // (`std::fmt::Write::write_str(&mut s, ..)`: as `Write::write_str(&mut s, ..)`, the trait's last two segments,
        // as the type inference reads it)
        if let Some(k) = self.prog.std_trait_path_skip(p) {
            let tail = crate::parse::Path { global: false, segs: p.segs[k..].to_vec(), pos: p.pos };
            return self.prim_method_call(&Expr::Call(Box::new(Expr::Path(None, tail)), cargs.clone()));
        }
        // `Trait::m(&x, a)` / `crate::m::Trait::m(..)` (self を取るユーザ trait のメソッド): `x.m(a)` に
        {
            let pp = PPath::of(p);
            let q = self.prog.strip_mods_p(&pp);
            let q = q.as_ref().unwrap_or(&pp);
            if q.segs.len() == 2 && q.segs.iter().all(|s| !s.args) {
                let tr = q.segs[0].name.clone();
                let ms = q.segs[1].name.clone();
                if let Some(ds) = self.prog.traits.get(&tr) {
                    let selfm = ds
                        .iter()
                        .any(|&d| self.prog.sigs[d as usize].has_self && self.prog.names[d as usize].rsplit("::").next() == Some(ms.as_str()));
                    // (not an operator's, whose method takes self by value: `BitAnd::bitand(&a, b)` is `(&a) & (b)`,
                    // the impl for `&A`, not `a.bitand(b)`)
                    let op_trait = matches!(tr.as_str(), "Add" | "Sub" | "Mul" | "Div" | "Rem" | "BitAnd" | "BitOr" | "BitXor" | "Shl" | "Shr" | "Neg" | "Not");
                    if selfm && !op_trait && !matches!(tr.as_str(), "Ord" | "PartialOrd" | "PartialEq" | "Eq") {
                        let mut args = cargs.iter();
                        let a0 = args.next()?;
                        let rest: Vec<Expr> = args.cloned().collect();
                        // (std's Pointer, which the compiler writes itself, `fmt::Pointer::fmt(&r, f)`: the method call with the
                        // trait named (radix_fmt_method); its receiver keeps its `&`, as `Pointer::fmt(&rr, f)` of a `&&u8`
                        // writes rr itself, not *rr)
                        let std_fmt = ms == "fmt" && tr == if self.prog.traits.contains_key("__p_Pointer") { "__p_Pointer" } else { "Pointer" };
                        let recv = match a0 {
                            Expr::Ref(_, e) if !std_fmt => paren((**e).clone()),
                            _ => paren(a0.clone()),
                        };
                        // (a primitive receiver with an impl of this trait: `<u32 as Tr>::m(&x)`, which a builtin
                        // method of the same name (`to_string`) cannot take over as `x.m()` would let it)
                        if let Some(t) = self.peek(&recv).map(|t| self.strip_refs(t)).filter(|_| !std_fmt) {
                            if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char) {
                                let key = format!("{}::{ms}", self.ty_key(t));
                                if self.trait_candidate(&key, &tr).is_some() {
                                    if let Ok(Expr::Path(q, qp)) = crate::parse::parse_expr(&format!("<{} as {tr}>::{ms}", self.prog.tn(t))) {
                                        return Some(Expr::Call(Box::new(Expr::Path(q, qp)), cargs.clone()));
                                    }
                                }
                                // (`A::m(&0.0f32)` taking A's default, beside another trait's `f32::m`: not `x.m()`, which
                                // would reach that one; the path call keeps the trait. Not Display / Debug's `fmt` of a number,
                                // which the compiler writes itself, beside the prelude's `impl LowerHex for u32`: the method
                                // call with the trait named, as fmt_method takes it)
                                if !(ms == "fmt" && matches!(tr.as_str(), "Display" | "Debug"))
                                    && self.prog.index.get(&key).is_some_and(|&i| self.prog.sigs[i as usize].impl_trait.as_deref().is_some_and(|it| it.split('<').next().unwrap_or(it).trim() != tr)) {
                                    return None;
                                }
                            }
                        }
                        return Some(Expr::MethodCall(Box::new(recv), ms, vec![], rest, crate::parse::Span::default()));
                    }
                }
            }
        }
        if p.segs.len() != 2 {
            return None;
        }
        let ty = p.segs[0].name.as_str();
        // `HashMap::from([(1, 2), ..])`: 配列を vec![..] にして `from(Vec)` を呼ぶ
        if p.segs[1].name == "from"
            && matches!(ty, "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque" | "BinaryHeap")
            && cargs.len() == 1
        {
            if let Expr::Array(es) = &cargs[0] {
                return Some(Expr::Call(func.clone(), vec![vec_of(es)]));
            }
        }
        // `Ord::cmp(&a, &b)` / `PartialOrd::partial_cmp(..)` / `PartialEq::eq(..)`: メソッド呼び出しの形に
        if matches!(ty, "Ord" | "PartialOrd" | "PartialEq" | "Eq") && cargs.len() == 2 {
            let m = &p.segs[1].name;
            return Some(Expr::MethodCall(Box::new(paren(cargs[0].clone())), m.clone(), vec![], vec![cargs[1].clone()], crate::parse::Span::default()));
        }
        // (`U::from(x)` in a generic fn with `U: From<u8>`: U's instance, when it is a number and x a number std
        // converts to it losslessly; a program's own `impl From<Code> for u32` is called as written)
        if Ty::prim(ty).is_none() && p.segs[1].name == "from" && cargs.len() == 1 {
            if let Some(&(_, t)) = self.env.params.iter().rev().find(|(n, _)| n == ty) {
                if (t.is_int() || t.is_float()) && self.peek(&cargs[0]).is_some_and(|a| crate::program::prim_from(a, t)) {
                    return Some(paren(Expr::Cast(Box::new(paren(cargs[0].clone())), ty_named(&self.prog.tn(t)))));
                }
            }
        }
        // (`Self::is_nan(self)` in an impl for a number type: std's inherent method only, as below. Not in a trait's
        // default method, whose `Self` is any type of the trait's: `Self::count_ones(self)` there is the trait's)
        let self_prim = ty == "Self";
        // (`root`: the fn being lowered, the one a closure is in, and an inlined callee's own while it is expanded)
        if self_prim && self.prog.sigs.get(self.root as usize).is_some_and(|s| s.trait_of.is_some()) {
            return None;
        }
        let t = if self_prim { self.env.self_ty.filter(|t| t.is_int() || t.is_float())? } else { Ty::prim(ty)? };
        if t == Ty::Char || t == Ty::Bool {
            return None;
        }
        let ms = p.segs[1].name.as_str();
        if self_prim && !crate::acg::p8_misc::prim_builtin(t, ms) {
            return None;
        }
        let pp = PPath::of(p);
        // `i64::from(7u32)` / `u32::from('a')`: 数値への変換はキャスト (広げる変換だけが書ける)
        // (also beside the program's `impl From<J> for f64`, for a number: a program cannot implement From of a
        // number for a number, E0117)
        let num_arg = || {
            cargs.len() == 1
                && self.peek(&cargs[0]).is_some_and(|a| a.is_int() || a.is_float() || matches!(a, Ty::Bool | Ty::Char))
                && self.trait_candidate(&format!("{ty}::from"), "From").is_some()
        };
        if ms == "from" && cargs.len() == 1 && (self.prog.fn_by_p(&pp, self.self_sid).is_none() || num_arg()) {
            let a0 = &cargs[0];
            return Some(paren(Expr::Cast(Box::new(paren(a0.clone())), ty_named(ty))));
        }
        if ms.starts_with("from") || ms.starts_with("try_from") || ms == "default" || ms == "new" {
            return None;
        }
        // (the program's own `T::m` stays as written, unless it is a trait impl's method and std has an inherent one of
        // the name, which a path reaches first: `impl Float for f32 { fn classify(self) -> FpCategory {
        // Self::classify(self) } }` (num-traits) calls f32's)
        if let Some(id) = self.prog.fn_by_p(&pp, self.self_sid) {
            if !(self.prog.sigs[id as usize].impl_trait.is_some() && crate::acg::p8_misc::prim_builtin(t, ms)) {
                return None;
            }
        }
        let mut args = cargs.iter();
        let a0 = args.next()?;
        let rest: Vec<Expr> = args.cloned().collect();
        // (リテラルは型を付ける: `i32::pow(2, 10)` の 2 は i32)
        let recv = if matches!(a0, Expr::Lit(_, _)) { paren(Expr::Cast(Box::new(a0.clone()), ty_named(&self.prog.tn(t)))) } else { paren(a0.clone()) };
        Some(Expr::MethodCall(Box::new(recv), ms.to_string(), vec![], rest, crate::parse::Span::default()))
    }
}

/// The literal of decimal digits (negated when neg) is in the integer type t's range
pub(crate) fn lit_fits(t: Ty, digits: &str, neg: bool) -> bool {
    // (the digits that fit any value of the width, as most literals do, need no parse: up to 2 for 8 bits ..)
    let (bits, short) = match t {
        Ty::I8 | Ty::U8 => (8, 2),
        Ty::I16 | Ty::U16 => (16, 4),
        Ty::I32 | Ty::U32 => (32, 9),
        Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize => (64, 18),
        Ty::I128 | Ty::U128 => (128, 38),
        _ => return true,
    };
    if digits.len() <= short {
        return true;
    }
    // (u128's range is what parses)
    let Ok(n) = digits.parse::<u128>() else { return false };
    if t.signed() {
        let max = (1u128 << (bits - 1)) - 1;
        if neg { n <= max + 1 } else { n <= max }
    } else {
        // (a negated unsigned literal is another error, E0600)
        neg || bits == 128 || n <= (1u128 << bits) - 1
    }
}

/// An integer literal without a suffix (`3`, `-3`)
pub(crate) fn is_int_lit(e: &Expr) -> bool {
    match strip_paren(e) {
        Expr::Lit(t, _) => t.starts_with(|c: char| c.is_ascii_digit()) && !t.contains('.') && !t.contains(['i', 'u']) && !t.contains('e'),
        Expr::Unary(op, x, _) => op == "-" && is_int_lit(x),
        _ => false,
    }
}

/// An unsuffixed float literal (`5.0`, `1e3`, `-2.5`)
pub(crate) fn is_float_lit(e: &Expr) -> bool {
    match strip_paren(e) {
        Expr::Lit(t, _) => {
            t.starts_with(|c: char| c.is_ascii_digit()) && !t.starts_with("0x") && !t.starts_with("0b") && !t.starts_with("0o") && (t.contains('.') || t.contains(['e', 'E'])) && !t.contains("f32") && !t.contains("f64")
        }
        Expr::Unary(op, x, _) => op == "-" && is_float_lit(x),
        _ => false,
    }
}

/// `Box::new(..)` (also `std::boxed::Box::new(..)`), in parentheses or not
fn is_box_new(e: &Expr) -> bool {
    match e {
        Expr::Paren(x) => is_box_new(x),
        Expr::Call(f, _) => matches!(&**f, Expr::Path(None, p) if p.segs.len() >= 2 && p.segs[p.segs.len() - 2].name == "Box" && p.segs[p.segs.len() - 1].name == "new"),
        _ => false,
    }
}

/// The name of the generic struct sid was made from, and whether the prelude declares it
fn base_name(prog: &Program, sid: u32) -> (&str, bool) {
    let b = prog.base_of(sid);
    let prelude = prog.struct_item.get(b as usize).is_some_and(|&i| i >= prog.prelude_from);
    (prog.structs[b as usize].name.as_str(), prelude)
}

/// A type whose values may not move to another thread (not Send): an `Rc`, a raw pointer, a `MutexGuard` or an
/// RwLock guard, a `&T` of a T that is not Sync, an `Arc<T>` of a T that is not Send and Sync, or a type holding one.
/// A type of the program's with `unsafe impl Send` is Send. The prelude's own types are judged by their arguments (a
/// Vec holds a raw pointer, and is Send when its elements are). A type parameter is taken as Send. A `dyn Trait` is
/// Send only when it is written with `+ Send` or its trait has Send as a supertrait (tests/security/send_dyn_thread.rs).
/// The part of t that makes it not Send (sync: not Sync), as rustc names it: through a closure's captures and the
/// fields of the program's own structs, down to the type that is not (`Rc<i32>`, not the closure that captures it, nor
/// the raw pointer inside the Rc), else t
pub fn auto_culprit(prog: &Program, t: Ty, sync: bool) -> Ty {
    let fails = |x: Ty| if sync { not_sync(prog, x) } else { not_send(prog, x) };
    let mut t = t;
    for _ in 0..16 {
        let Ty::Struct(sid) = t else { break };
        let base = prog.base_of(sid);
        let own = prog.struct_item.get(base as usize).is_some_and(|&i| i < prog.prelude_from);
        if !own || prog.impls.contains(&(base, if sync { "Sync" } else { "Send" }.to_string())) {
            break;
        }
        let next = prog.structs[sid as usize].fields.iter().enumerate().map(|(i, f)| match f.1 {
            Ty::Ptr(_, inner) if prog.ref_capture(sid, i).is_some() => *inner,
            x => x,
        }).find(|&f| fails(f));
        match next {
            Some(n) => t = n,
            None => break,
        }
    }
    t
}

pub fn not_send(prog: &Program, t: Ty) -> bool {
    auto_trait_fails(prog, t, false, &mut vec![], 0)
}

/// A type whose values may not be shared between threads (not Sync): `Cell`, `RefCell`, `UnsafeCell`, `Rc`, a
/// channel's receiver, a raw pointer, `Mutex<T>` of a T that is not Send, `RwLock<T>` / `Arc<T>` / `OnceLock<T>` of a
/// T that is not Send and Sync, or a type holding one. A type of the program's with `unsafe impl Sync` is Sync
pub fn not_sync(prog: &Program, t: Ty) -> bool {
    auto_trait_fails(prog, t, true, &mut vec![], 0)
}

/// not_send (sync = false) / not_sync (sync = true). `seen` holds the types of the program's being looked into: one
/// met again is taken to hold (as rustc's auto traits do for recursive types), so a recursive type is looked into
/// once. A nesting past the limit counts as failing, never as Send / Sync
fn auto_trait_fails(prog: &Program, t: Ty, sync: bool, seen: &mut Vec<(u32, bool)>, d: u32) -> bool {
    if d > 64 {
        return true;
    }
    let send = |t: Ty, seen: &mut Vec<(u32, bool)>| auto_trait_fails(prog, t, false, seen, d + 1);
    let sync_ = |t: Ty, seen: &mut Vec<(u32, bool)>| auto_trait_fails(prog, t, true, seen, d + 1);
    match t {
        Ty::Ptr(..) => true,
        // (a trait object: only by an auto trait it is written with (`dyn Tr + Send`) or its trait's supertraits)
        Ty::Dyn(d) => {
            let (core, send, sync_) = crate::types::dyn_autos(d);
            !(if sync { sync_ } else { send }) && prog.tdb.gives_auto(prog, core, if sync { "Sync" } else { "Send" }) != Some(true)
        }
        // (&T is Send if T is Sync; &mut T if T is Send; both are Sync if T is Sync)
        Ty::Ref(false, i) => sync_(*i, seen),
        Ty::Ref(true, i) if !sync => send(*i, seen),
        Ty::Ref(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => auto_trait_fails(prog, *i, sync, seen, d + 1),
        Ty::Struct(sid) => {
            let (name, prelude) = base_name(prog, sid);
            let sd = &prog.structs[sid as usize];
            if prelude {
                let args = &sd.args;
                return match (name, sync) {
                    // (std's atomics, AtomicPtr<T> too, are Send and Sync whatever they point to)
                    (n, _) if n.starts_with("Atomic") => false,
                    ("Rc", _) | ("MutexGuard" | "RwLockReadGuard" | "RwLockWriteGuard" | "StdoutLock", false) => true,
                    // (a RefCell's guards share the cell's non-atomic borrow flag: neither Send nor Sync, as in std)
                    ("Ref" | "RefMut", _) => true,
                    ("Cell" | "RefCell" | "UnsafeCell" | "LazyCell" | "OnceCell" | "__Rcvr__", true) => true,
                    ("Mutex", true) => args.iter().any(|&a| send(a, seen)),
                    ("Arc" | "RwLock" | "OnceLock", true) | ("Arc", false) => args.iter().any(|&a| send(a, seen) || sync_(a, seen)),
                    _ => args.iter().any(|&a| auto_trait_fails(prog, a, sync, seen, d + 1)),
                };
            }
            let base = prog.base_of(sid);
            if prog.impls.contains(&(base, if sync { "Sync" } else { "Send" }.to_string())) {
                return false;
            }
            if seen.contains(&(sid, sync)) {
                return false;
            }
            seen.push((sid, sync));
            // (a closure's capture held by reference is a raw pointer: the reference it stands for. Sync needs
            // `T: Sync`; Send of a `&mut T` (one the body mutates) `T: Send`, and of one that may be either kind both.
            // A closure is judged by its captures alone: its type arguments are the parent's parameters and the
            // captures' types, raw pointers among them)
            let field = |i: usize, t: Ty, seen: &mut Vec<(u32, bool)>| match (t, prog.ref_capture(sid, i)) {
                (Ty::Ptr(_, inner), Some(true)) => auto_trait_fails(prog, *inner, sync, seen, d + 1),
                (Ty::Ptr(_, inner), Some(false)) => sync_(*inner, seen) || (!sync && send(*inner, seen)),
                _ => auto_trait_fails(prog, t, sync, seen, d + 1),
            };
            // (a struct of the program's with fields is judged by them (`Maker<Rc<u32>>` of a `fn() -> T` field is
            // Send); one without (a placeholder instance) by its arguments)
            let closure = prog.closure_of_struct.contains_key(&base);
            let laid_out = !sd.fields.is_empty() || !sd.variants.is_empty();
            (!closure && !laid_out && sd.args.iter().any(|&a| auto_trait_fails(prog, a, sync, seen, d + 1)))
                || sd.fields.iter().enumerate().any(|(i, f)| field(i, f.1, seen))
                || sd.variants.iter().any(|v| v.fields.iter().any(|f| auto_trait_fails(prog, f.1, sync, seen, d + 1)))
        }
        _ => false,
    }
}

/// E0277: a `static` (not `static mut`, not an extern block's) of the program's whose type is not Sync: every thread
/// can reach it at once (`static C: Cell<u32>`). Returns (the static's index, its type's name)
pub fn static_not_sync(prog: &Program) -> Option<(usize, String)> {
    prog.consts.iter().enumerate().find_map(|(ci, c)| {
        let user = c.item < prog.prelude_from;
        let ext = c.expr.starts_with(crate::parse::EXTERN_STATIC_SYM);
        (c.stat == Some(false) && user && !ext && not_sync(prog, c.ty)).then(|| (ci, prog.tn(c.ty)))
    })
}

/// `__impl0`: the parameter the summary makes of an `impl Trait` argument (its index after the name), not a user's
/// `__implT`
pub(crate) fn hidden_param(n: &str) -> bool {
    n.starts_with("__impl") && n.len() > 6 && n.as_bytes()[6..].iter().all(|c| c.is_ascii_digit())
}

/// The bounds of some one of a trait's blanket impls (`impl<T: A> Tr<u8> for T`, `impl<T: B> Tr<u16> for T`) hold of t
pub(crate) fn blanket_impls_hold(prog: &crate::program::Program, t: Ty, impls: &[Vec<String>], path: &mut Vec<String>, seen: &mut Vec<(String, bool)>) -> bool {
    impls.iter().any(|bs| blanket_bounds_hold(prog, t, bs, path, seen))
}

/// The bounds of a blanket impl's self type hold of t (`?Sized` and lifetimes aside). A bound that is a trait with a
/// blanket impl of its own holds when that impl's bounds do (`impl<T: Marker> Other for T`)
pub(crate) fn blanket_bounds_hold(prog: &crate::program::Program, t: Ty, bs: &[String], path: &mut Vec<String>, seen: &mut Vec<(String, bool)>) -> bool {
    bs.iter().all(|b| {
        if b.starts_with('?') || b.starts_with('\'') {
            return true;
        }
        // (an impl for t itself (`impl Other for ()`), or the blanket impl's bounds. A trait met again on the way
        // (a cycle of blanket impls), or a chain past rustc's recursion limit (128), does not hold: rustc reports
        // E0275 there. Each trait is decided once (seen): its answer for t is one, over all its blanket impls)
        match prog.blanket_bounds.get(b.as_str()) {
            Some(inner) if prog.blanket_traits.contains(b.as_str()) => {
                if prog.implements_concrete(t, b) {
                    return true;
                }
                if let Some(&(_, r)) = seen.iter().find(|(n, _)| n == b) {
                    return r;
                }
                if path.len() >= 128 || path.iter().any(|n| n == b) {
                    return false;
                }
                path.push(b.clone());
                let r = blanket_impls_hold(prog, t, inner, path, seen);
                path.pop();
                seen.push((b.clone(), r));
                r
            }
            _ => prog.implements(t, b),
        }
    })
}

/// t names the type parameter 0 (a trait's `Self`, or a blanket impl's `T`)
fn mentions_param0(prog: &crate::program::Program, t: Ty, depth: u32) -> bool {
    match t {
        Ty::Param(0) => true,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) if depth < 8 => mentions_param0(prog, *i, depth + 1),
        Ty::Struct(s) if depth < 8 => prog.structs[s as usize].args.iter().any(|&a| mentions_param0(prog, a, depth + 1)),
        _ => false,
    }
}

/// The methods of a slice (and so of an array) that take `&mut self`: an array's call of one needs the array `mut`
/// (unused_mut)
const MUT_SLICE_METHODS: &[&str] = &[
    "sort", "sort_unstable", "sort_by", "sort_by_key", "sort_unstable_by", "sort_unstable_by_key", "sort_by_cached_key", "swap", "reverse",
    "fill", "fill_with", "rotate_left", "rotate_right", "copy_from_slice", "clone_from_slice", "copy_within", "swap_with_slice", "iter_mut",
    "as_mut_slice", "as_mut_ptr", "as_mut_ptr_range", "get_mut", "get_unchecked_mut", "first_mut", "last_mut", "split_at_mut",
    "split_first_mut", "split_last_mut", "chunks_mut", "chunks_exact_mut", "rchunks_mut", "rchunks_exact_mut", "split_mut", "splitn_mut",
    "rsplit_mut", "rsplitn_mut", "select_nth_unstable", "select_nth_unstable_by", "select_nth_unstable_by_key", "each_mut",
    "make_ascii_uppercase", "make_ascii_lowercase", "partition_dedup",
];

/// A call's path to the function f with the method call's own type arguments (`r.size::<u16>()` as `T::__ref_size::<u16>`)
fn with_targs(f: &str, gargs: &[crate::parse::GArg]) -> Expr {
    let mut p = crate::acg::path_of(f);
    p.segs[0].args = gargs.to_vec();
    Expr::Path(None, p)
}

impl Cx<'_> {
    /// A closure written where an `Fn` / `FnMut` is wanted (a bound's text, a `Box<dyn Fn()>`'s): it is that, so its
    /// body moving out what it captures by value (`|| { let s = val; }`) is E0507, at the move
    pub(crate) fn fn_kind_move_check(&mut self, a: &Expr, want: &str) -> R<()> {
        let c = strip_paren(a);
        if !matches!(c, Expr::Closure(..)) {
            return Ok(());
        }
        let want = want.trim_start();
        let kind = |k: &str| want.strip_prefix(k).is_some_and(|r| r.trim_start().starts_with('('));
        if (kind("Fn") || kind("FnMut")) && self.bk.fnonce_sure.contains(&(c as *const Expr as usize)) {
            let caps: Vec<String> = crate::ast_closure::free_vars(c).into_iter().filter(|n| self.lookup(n).is_some_and(|(_, t)| !self.bk_ty(t).0)).collect();
            let prog = self.prog;
            let types: Vec<(String, Ty)> = caps.iter().filter_map(|n| self.lookup(n).map(|(_, t)| (n.clone(), t))).collect();
            let by = |p: &crate::parse::Pat, n: &str| types.iter().find(|x| x.0 == n).is_none_or(|x| borrowck::pat_moves(prog, p, x.1, 0));
            if let Some((n, at)) = crate::ast_closure::move_site_by(c, &caps, &by) {
                let w = if kind("Fn") { "Fn" } else { "FnMut" };
                return Err(self.at(at, format!("cannot move out of `{n}`, a captured variable in an `{w}` closure (E0507)")));
            }
        }
        Ok(())
    }
}

impl Cx<'_> {
    /// A `&self` method whose self type is invariant in a lifetime another parameter's type names, or one that
    /// outlives such a lifetime (`'f: 'c`): what that argument borrows is held by the receiver (rustc: its region is
    /// the receiver's, which lives as long as the receiver is used)
    /// id is `set` or `replace` of the prelude's Cell, RefCell or OnceCell, whose value of type st it stores its
    /// argument in
    fn cell_store(&self, id: u32, st: Ty) -> bool {
        let Ty::Struct(sid) = st else { return false };
        let b = self.prog.base_of(sid);
        let prelude = self.prog.struct_item.get(b as usize).is_some_and(|&i| i >= self.prog.prelude_from);
        let m = self.prog.names[id as usize].rsplit("::").next().unwrap_or("").split('#').next().unwrap_or("");
        // (the prelude's own method, not a program's trait method of the name)
        let own = self.prog.item_of.get(id as usize).copied().flatten().is_some_and(|i| i >= self.prog.prelude_from);
        prelude && own && matches!(self.prog.structs[b as usize].name.as_str(), "Cell" | "RefCell" | "OnceCell") && matches!(m, "set" | "replace")
    }

    pub(crate) fn shared_recv_stores(&mut self, id: u32) -> Vec<usize> {
        let prog = self.prog;
        let sig = &prog.sigs[id as usize];
        let Some(rs) = sig.regions.as_deref() else { return vec![] };
        let (Some(&p0), Some(s0)) = (sig.params.first(), rs.slots.first()) else { return vec![] };
        if !matches!(p0, Ty::Ref(false, _)) || sig.params.len() < 2 {
            return vec![];
        }
        let n = rs.lts.len();
        let Some(vs) = self.rg.code_variances(prog, p0, s0, n, sig.generics.len()) else { return vec![] };
        let mut inv: Vec<u8> = (0..n).filter(|&k| vs[k] == super::regionck::Var::Inv).map(|k| k as u8).collect();
        if inv.is_empty() {
            return vec![];
        }
        // (to a fixpoint: `'a: 'b, 'b: 'c` in either order)
        loop {
            let n0 = inv.len();
            for &(a, b) in &rs.outlives {
                if inv.contains(&b) && !inv.contains(&a) {
                    inv.push(a);
                }
            }
            if inv.len() == n0 {
                break;
            }
        }
        (1..sig.params.len()).filter(|&j| rs.slots.get(j).is_some_and(|s| s.iter().any(|c| inv.contains(c)))).collect()
    }
}

impl Cx<'_> {
    /// A place expression evaluated again without effects: a variable, its fields, derefs of references, boxes and
    /// pointers, an index of an array, a slice or a Vec by a literal or a variable (`refs[i].x`; not `refs[next()]`,
    /// nor a type's own `Index` or `Deref`, whose code would run again)
    pub(crate) fn pure_place(&self, e: &Expr) -> bool {
        match e {
            Expr::Path(None, _) => true,
            Expr::Field(b, _, _) | Expr::Paren(b) => self.pure_place(b),
            Expr::Unary(op, b, _) => op == "*" && matches!(self.peek(b), Some(Ty::Ref(..) | Ty::Box(_) | Ty::Ptr(..))) && self.pure_place(b),
            Expr::Index(b, i, _) => {
                let builtin = match self.peek(b).map(|t| self.strip_refs(t)) {
                    Some(Ty::Array(..) | Ty::Slice(_)) => true,
                    // (the prelude's Vec, not a type of the program's of that name, whose `Index` would run again)
                    Some(Ty::Struct(sid)) => {
                        let b = self.prog.base_of(sid) as usize;
                        self.prog.structs[b].name == "Vec" && self.prog.struct_item.get(b).is_some_and(|&i| i >= self.prog.prelude_from)
                    }
                    _ => false,
                };
                builtin && self.pure_place(b) && (matches!(&**i, Expr::Lit(..)) || self.pure_place(i))
            }
            _ => false,
        }
    }
}
