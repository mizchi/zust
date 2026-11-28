use super::*;
use crate::parse::{Block, Expr, GArg, Pat, Path, Stmt};
use crate::program::{FnSig, PPath, TyEnv};
use crate::types::{Ty, intern};
use crate::acg::clif::{TrapCode, types};
use crate::ir::{IntCC, Value};

/// A label's name (without the leading `'`, as syn's `l.ident`)
pub(crate) fn label_name(l: &str) -> String {
    l.trim_start_matches('\'').to_string()
}

/// Makes a closure body an ordinary fn: `fn call(__env: (), a: _, __p1: _) { let (x, y) = __p1; body }`.
/// (as closure::synth. Given to the body's type inference (infer_ctx) to find the open argument and return types)
pub(crate) fn closure_synth(c: &Expr) -> ItemFn {
    let Expr::Closure(_, inputs, _, body, _) = c else { unreachable!() };
    let mut params: Vec<(Pat, crate::parse::Ty)> = vec![(Pat::Ident(false, false, "__env".into(), None), crate::parse::Ty::Tuple(vec![]))];
    let mut stmts: Vec<Stmt> = vec![];
    for (i, (pat, ty)) in inputs.iter().enumerate() {
        let simple = matches!(pat, Pat::Ident(false, _, _, None));
        let pat_in = if simple {
            pat.clone()
        } else {
            let id = format!("__p{i}");
            stmts.push(Stmt::Let(pat.clone(), None, Some(path_expr(&id)), None, Default::default()));
            Pat::Ident(false, false, id, None)
        };
        params.push((pat_in, ty.clone().unwrap_or(crate::parse::Ty::Infer)));
    }
    match &**body {
        Expr::Block(None, false, b) => stmts.extend(b.stmts.iter().cloned()),
        e => stmts.push(Stmt::Expr(e.clone(), false)),
    }
    ItemFn {
        attrs: vec![],
        sig: crate::parse::FnSig {
            name: "call".into(),
            generics: crate::parse::Generics { params: vec![], preds: vec![] },
            recv: None,
            recv_ty: None,
            params,
            ret: None,
            quals: vec![],
        },
        block: Block { stmts, spos: Default::default() },
    }
}

impl Cx<'_> {
    /// `S::<'static>(..)` with `struct S<'a, 'b>`: E0107 for another number of lifetime arguments than the type has
    /// (none given is fine: they are inferred)
    pub(crate) fn lifetime_arity(&self, sid: u32, p: &crate::parse::Path, what: &str) -> R<()> {
        // (on any segment: `E::<'static>::V::<u8>(..)`)
        let k: usize = p.segs.iter().map(|s| s.args.iter().filter(|a| matches!(a, GArg::Lifetime(_))).count()).sum();
        if k == 0 {
            return Ok(());
        }
        let n = self.prog.adt_regions.get(&self.prog.base_of(sid)).map_or(0, |r| r.lts.len());
        if k != n {
            let (ns, ks) = (if n == 1 { "" } else { "s" }, if k == 1 { " was" } else { "s were" });
            return err(SP, format!("{what} takes {n} lifetime argument{ns} but {k} lifetime argument{ks} supplied (E0107)"));
        }
        Ok(())
    }

    /// lifetime_arity of the program's own structs and enums a signature's type t names (not in fn pointer, `dyn` and
    /// `impl` types, nor a type parameter tps names). Not the prelude's, some of which leave out lifetimes std's have
    /// (`Cow<'a, B>`)
    /// E0616: a field of a struct or union of another module, not `pub`, where the access is written outside the
    /// module it is visible within (`s.x` with `mod a { pub struct S { x: u8 } }`). Known for the program's own
    /// structs by Program::field_priv; a position of no line (a rewrite's) is not read
    pub(crate) fn field_privacy(&self, sid: u32, fname: &str, at: Sp) -> R<()> {
        let Some(fp) = self.prog.field_priv.as_deref() else { return Ok(()) };
        // (the root crate's own code: line_base is the item's line before its first, a dependency's is tagged above
        // 1 << 20)
        if !self.user_code || at.line == 0 || self.line_base >= 1 << 20 {
            return Ok(());
        }
        let base = self.prog.structs[sid as usize].base.unwrap_or(sid);
        let name = &self.prog.structs[base as usize].name;
        let Some((what, fs)) = fp.fields.get(name.as_str()) else { return Ok(()) };
        let Some(&(_, scope)) = fs.iter().find(|f| f.0 == fname) else { return Ok(()) };
        match fp.module_of(self.line_base + at.line) {
            Some(site) if !fp.within(site, scope) => Err(self.at(at, format!("field `{fname}` of {what} `{name}` is private (E0616)"))),
            _ => Ok(()),
        }
    }

    /// E0624: a method call `x.f()` of a fn of an inherent impl of another module, not `pub`, written outside the
    /// module it is visible within (Program::field_priv)
    pub(crate) fn method_privacy(&self, sid: u32, mname: &str, at: Sp) -> R<()> {
        let Some(fp) = self.prog.field_priv.as_deref() else { return Ok(()) };
        if !self.user_code || at.line == 0 || self.line_base >= 1 << 20 {
            return Ok(());
        }
        let base = self.prog.structs[sid as usize].base.unwrap_or(sid);
        let Some(ms) = fp.methods.get(self.prog.structs[base as usize].name.as_str()) else { return Ok(()) };
        let Some(&(_, scope)) = ms.iter().find(|f| f.0 == mname) else { return Ok(()) };
        match fp.module_of(self.line_base + at.line) {
            Some(site) if !fp.within(site, scope) => Err(self.at(at, format!("method `{mname}` is private (E0624)"))),
            _ => Ok(()),
        }
    }

    /// The type of a place `let _ = place;` names (a variable and its fields), when known: E0609 for a field its type
    /// does not have (through references and boxes; not through a `Deref` of the program's, which is not followed)
    pub(crate) fn wild_place_ty(&self, e: &Expr) -> R<Option<Ty>> {
        match e {
            Expr::Paren(x) => self.wild_place_ty(x),
            Expr::Path(None, p) if p.segs.len() == 1 => Ok(self.lookup(&p.segs[0].name).map(|x| x.1)),
            Expr::Field(x, f, sp) => {
                let Some(t) = self.wild_place_ty(x)? else { return Ok(None) };
                let mut t = t;
                while let Ty::Ref(_, i) | Ty::Box(i) = t {
                    t = *i;
                }
                let Ty::Struct(sid) = t else { return Ok(None) };
                match self.prog.field(sid, f) {
                    Some((ft, _)) => {
                        self.field_privacy(sid, f, sp.at)?;
                        Ok(Some(ft))
                    }
                    None if self.prog.method(sid, "deref").is_some() || self.prog.is_placeholder(sid) => Ok(None),
                    None => Err(self.at(sp.at, format!("no field `{f}` on type `{}` (E0609)", self.prog.tn(t)))),
                }
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn sig_lt_arity(&self, t: &crate::parse::Ty, tps: &[&str], depth: u32) -> R<()> {
        use crate::parse::Ty as PTy;
        if depth > 16 {
            return Ok(());
        }
        match t {
            PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => self.sig_lt_arity(i, tps, depth + 1),
            PTy::Tuple(ts) => ts.iter().try_for_each(|t| self.sig_lt_arity(t, tps, depth + 1)),
            PTy::Path(None, p) => {
                // (only a path that writes lifetimes may write too many or too few: most write none, and are not
                // looked up)
                if let [s] = p.segs.as_slice() {
                    if s.fn_args.is_none() && s.args.iter().any(|a| matches!(a, GArg::Lifetime(_))) && !tps.contains(&s.name.as_str()) {
                        if let Some(&sid) = self.prog.struct_index.get(&s.name) {
                            let b = self.prog.base_of(sid);
                            if self.prog.struct_item.get(b as usize).is_some_and(|&i| i < self.prog.prelude_from) {
                                let what = if self.prog.structs[sid as usize].is_enum { "enum" } else { "struct" };
                                self.lifetime_arity(sid, p, what)?;
                            }
                        }
                    }
                }
                p.segs.iter().flat_map(|s| s.args.iter()).try_for_each(|a| match a {
                    GArg::Ty(t) => self.sig_lt_arity(t, tps, depth + 1),
                    _ => Ok(()),
                })
            }
            _ => Ok(()),
        }
    }

    /// `Enum::<()>::V::<()>(..)` / `Alias::V::<()>(..)` / `Self::V::<()>(..)`: E0109 for type arguments on a variant
    /// when its type has them already, or is an alias or `Self` (`Enum::V::<()>` alone is fine)
    pub(crate) fn variant_targs_allowed(&self, p: &crate::parse::Path, kind: &str) -> R<()> {
        let n = p.segs.len();
        let has_ty = |s: &crate::parse::Seg| s.args.iter().any(|a| matches!(a, GArg::Ty(_) | GArg::Const(_)));
        if n < 2 || !has_ty(&p.segs[n - 1]) {
            return Ok(());
        }
        let ty_seg = &p.segs[n - 2];
        if ty_seg.name == "Self" || self.prog.aliases.contains_key(&ty_seg.name) {
            return err(SP, "type arguments are not allowed on this type (E0109)");
        }
        // (`E::<'static>::V::<u8>(..)`: the enum's arguments are written, lifetimes too)
        if !ty_seg.args.is_empty() {
            return err(SP, format!("type arguments are not allowed on {kind} `{}` (E0109)", p.segs[n - 1].name));
        }
        Ok(())
    }

    /// t implements `Copy` (borrowck's is_copy counts a `&mut` as Copy, for the reborrows of passing it)
    pub(crate) fn strict_copy(&self, t: Ty) -> bool {
        match t {
            Ty::Ref(true, _) => false,
            Ty::Array(e, _) => self.strict_copy(*e),
            Ty::Struct(s) if self.prog.is_tuple(s) => self.prog.structs[s as usize].args.iter().all(|&a| self.strict_copy(a)),
            t => borrowck::is_copy(self.prog, t, 0),
        }
    }

    /// A struct's turbofish (ts: its arguments, None for `_`) against the expected instance of it: each argument
    /// written is the expected one (`let _: S<u8, u8> = S::<u16, _> { .. }` is a mismatch; `S::<u8, _>` is not)
    pub(crate) fn turbofish_vs_expected(&self, sid: u32, ts: &[Option<Ty>], expected: Option<Ty>) -> R<()> {
        let Some(Ty::Struct(es)) = expected else { return Ok(()) };
        let base = self.prog.base_of(sid);
        if self.prog.base_of(es) != base || self.prog.has_param(Ty::Struct(es)) {
            return Ok(());
        }
        let eargs = &self.prog.structs[es as usize].args;
        if ts.iter().zip(eargs).any(|(w, e)| w.is_some_and(|w| w != *e)) {
            let shown: Vec<String> = ts.iter().map(|w| w.map_or("_".to_string(), |w| self.prog.tn(w))).collect();
            let name = &self.prog.structs[base as usize].name;
            return err(SP, format!("mismatched types: expected `{}`, found `{name}<{}>`", self.prog.tn(Ty::Struct(es)), shown.join(", ")));
        }
        Ok(())
    }

    /// `U::<u8>` / `Foo::<3>` / `Flag::<true>`: the path without the last segment's turbofish, and the type arguments
    /// it gives (None for `_`); the path itself when it has none
    /// The func `<X>::m` of a path call whose path names the type X before m: `T::A::m` (A an associated type of the type
    /// parameter T), `<T as Tr>::A::m`, or `<X as From<_>>::from` (std's conversions, whose impls for the primitives
    /// are builtins); X known (no parameter left). None otherwise
    #[cold]
    #[inline(never)]
    pub(crate) fn assoc_path_func(&self, q: Option<&crate::parse::Ty>, p: &crate::parse::Path) -> Option<Expr> {
        let (m, pre) = p.segs.split_last()?;
        let ty = match q {
            None if pre.len() == 2 && pre.iter().all(|s| s.args.is_empty()) && self.env.params.iter().any(|(n, _)| *n == pre[0].name) => {
                crate::parse::Ty::Path(None, crate::parse::Path { global: false, segs: pre.to_vec(), pos: p.pos })
            }
            Some(q) if pre.len() >= 2 => crate::parse::Ty::Path(Some(Box::new(q.clone())), crate::parse::Path { global: p.global, segs: pre.to_vec(), pos: p.pos }),
            Some(q) if pre.len() == 1 && matches!((pre[0].name.as_str(), m.name.as_str()), ("From", "from") | ("TryFrom", "try_from")) => q.clone(),
            _ => return None,
        };
        let t = self.parse_ty(&ty).ok().filter(|&t| !self.prog.has_param(t) && !has_hole(self.prog, t))?;
        // (`<u16>::from` parses as `u16::from`)
        let Ok(Expr::Path(qt, mut qp)) = crate::parse::parse_expr(&format!("<{}>::{}", self.prog.tn(t), m.name)) else { return None };
        // (the method's own arguments: `<T as Foo>::U::gm::<f64>()`)
        if let Some(l) = qp.segs.last_mut() {
            l.args = m.args.clone();
        }
        qp.pos = p.pos;
        Some(Expr::Path(qt, qp))
    }

    /// The arguments of `<T as From<S>>::from(x)` as the call on T gets them: x of the source S written (E0308 for
    /// another type; an unsuffixed literal of S's kind is S's, `<u16 as From<u8>>::from(6)`). Other calls' as they are
    fn from_source_args(&self, p: &crate::parse::Path, cargs: &[Expr]) -> R<Vec<Expr>> {
        if let ([tr, m], [x]) = (p.segs.as_slice(), cargs) {
            if matches!((tr.name.as_str(), m.name.as_str()), ("From", "from") | ("TryFrom", "try_from")) {
                if let [GArg::Ty(sty)] = tr.args.as_slice() {
                    if let Some(s) = self.parse_ty(sty).ok().filter(|&s| !self.prog.has_param(s) && !has_hole(self.prog, s)) {
                        // (a literal of the source's kind takes it as its suffix (`6` as `6u8`, out of range an error as for
                        // `300u8`); one of the other kind is a mismatch (`from(3.5)` for a u8), as rustc says)
                        // (the literal's parsed kind, only without a suffix: `1f32` and `1E3` are floats, `5u8` keeps its own)
                        fn unsuffixed(e: &Expr) -> Option<bool> {
                            match e {
                                Expr::Paren(i) => unsuffixed(i),
                                Expr::Unary(op, i, _) if op == "-" => unsuffixed(i),
                                Expr::Lit(t, _) => match super::lit::lit(t)? {
                                    super::lit::Lit::Int(i) if i.suffix().is_empty() => Some(true),
                                    super::lit::Lit::Float(f) if f.suffix().is_empty() => Some(false),
                                    _ => None,
                                },
                                _ => None,
                            }
                        }
                        let kind = unsuffixed(x);
                        let (int, float) = (kind == Some(true), kind == Some(false));
                        if int || float {
                            if (int && !s.is_int()) || (float && !s.is_float()) {
                                let found = if int { "integer" } else { "floating-point number" };
                                return err(expr_start(x), format!("mismatched types: expected `{}`, found {found}", self.prog.tn(s)));
                            }
                            fn suffixed(e: &Expr, sfx: &str) -> Expr {
                                match e {
                                    Expr::Paren(i) => suffixed(i, sfx),
                                    Expr::Lit(t, sp) => Expr::Lit(format!("{t}{sfx}"), *sp),
                                    Expr::Unary(op, i, sp) => Expr::Unary(op.clone(), Box::new(suffixed(i, sfx)), *sp),
                                    e => e.clone(),
                                }
                            }
                            return Ok(vec![suffixed(x, &self.prog.tn(s))]);
                        }
                        if let Some(a) = self.peek(x).filter(|&a| a != s && !self.prog.has_param(a)) {
                            return err(expr_start(x), format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(s), self.prog.tn(a)));
                        }
                    }
                }
            }
        }
        Ok(cargs.to_vec())
    }

    pub(crate) fn turbofish_split<'a>(&self, p: &'a crate::parse::Path) -> (PPath, Option<&'a [GArg]>) {
        let mut pp = PPath::of(p);
        let Some(last) = p.segs.last().filter(|s| !s.args.is_empty() && s.fn_args.is_none()) else { return (pp, None) };
        if let Some(l) = pp.segs.last_mut() {
            l.args = false;
        }
        (pp, Some(&last.args))
    }

    /// a unit struct pattern's instance (`G::<5, _>` against a `G<5, 7>`: its `_` from the value's type), after its
    /// lifetime and argument counts are checked; None when it is not one
    pub(crate) fn pat_turbofish_ty(&self, path: &crate::parse::Path, value: Ty) -> R<Option<Ty>> {
        let (pp, targs) = self.turbofish_split(path);
        let (Some(sid), Some(args)) = (self.prog.struct_by_p(&pp, self.self_sid), targs) else { return Ok(None) };
        let sd = &self.prog.structs[sid as usize];
        if !sd.fields.is_empty() || sd.is_enum {
            return Ok(None);
        }
        self.lifetime_arity(sid, path, "struct")?;
        let base = self.prog.base_of(sid);
        let mut ts = self.turbofish_targs(sid, args)?;
        // (`matches!(0u8, G::<_>)`: a value of another type than the struct is a mismatch, `_` or not)
        if !matches!(value, Ty::Struct(v) if self.prog.base_of(v) == base) && !self.prog.has_param(value) {
            return err(SP, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(value), self.prog.structs[base as usize].name));
        }
        if let Ty::Struct(v) = value {
            if self.prog.base_of(v) == base {
                for (k, t) in ts.iter_mut().enumerate() {
                    if t.is_none() {
                        *t = self.prog.structs[v as usize].args.get(k).copied();
                    }
                }
            }
        }
        match ts.into_iter().collect::<Option<Vec<Ty>>>() {
            Some(ts) => Ok(Some(Ty::Struct(self.prog.instantiate(base, ts)?))),
            None => Ok(None),
        }
    }

    /// the type arguments a struct's turbofish gives (None for `_`; lifetimes left out), checked against the struct's
    /// number of generic parameters (E0107); a type that does not resolve is that error
    pub(crate) fn turbofish_targs(&self, sid: u32, args: &[GArg]) -> R<Vec<Option<Ty>>> {
        let mut ts = vec![];
        // (`Flag::<1>` with `struct Flag<const B: bool>`: a const argument of another kind than the parameter's type)
        let decls = self.prog.const_param_tys.get(&self.prog.base_of(sid));
        for a in args {
            if let Some(ds) = decls {
                let k = ts.len();
                if let Some((_, decl)) = ds.iter().find(|(i, _)| *i == k) {
                    let decl = &*self.prog.const_decl(decl);
                    let kind = match decl {
                        "bool" | "char" => decl,
                        _ => "int",
                    };
                    if let Some((_, shown)) = self.const_arg_kind(a).filter(|(f, _)| *f != kind) {
                        return err(SP, format!("mismatched types: expected `{decl}`, found {shown} (const argument)"));
                    }
                }
            }
            ts.push(match a {
                GArg::Ty(crate::parse::Ty::Infer) => None,
                GArg::Ty(t) if crate::ast_types::bool_arg(t).is_some() => Some(self.prog.resolve_env(&crate::ast_types::bool_arg(t).unwrap(), &self.env).or_else(|e| err(SP, e))?),
                GArg::Ty(t) => Some(self.parse_ty(t)?).filter(|&t| !has_hole(self.prog, t)),
                GArg::Const(e) => {
                    let syn = crate::ast_types::const_arg_syn(e).or_else(|e| err(SP, e))?;
                    Some(self.prog.resolve_env(&syn, &self.env).or_else(|e| err(SP, e))?)
                }
                _ => continue,
            });
        }
        let n = self.prog.structs[self.prog.base_of(sid) as usize].generics.len();
        if ts.len() != n {
            let (ns, ks) = (if n == 1 { "" } else { "s" }, if ts.len() == 1 { " was" } else { "s were" });
            return err(SP, format!("struct takes {n} generic argument{ns} but {} generic argument{ks} supplied (E0107)", ts.len()));
        }
        Ok(ts)
    }

    // --- place (lvalue) ---------------------------------------------------

    pub(crate) fn place(&mut self, e: &Expr) -> R<Place> {
        let outer = self.bk.place_depth == 0;
        self.bk.place_depth += 1;
        let r = self.place_inner(e).map_err(|m| self.at(err_pos(e), m));
        self.bk.place_depth -= 1;
        if outer && r.is_ok() {
            self.bk_acc(e, borrowck::Acc::Read);
        }
        r
    }

    pub(crate) fn place_inner(&mut self, e: &Expr) -> R<Place> {
        match e {
            Expr::Paren(p) => self.place(p),
            Expr::Path(..) if self.const_generic(e).is_some() => {
                let n = self.const_generic(e).unwrap();
                let t = self.const_generic_ty(e).unwrap_or(Ty::Usize);
                let v = self.int_const(t, n as i128);
                let var = self.b.declare_var(clif_ty(t).unwrap_or(types::I64));
                self.b.def_var(var, v);
                Ok(Place::Var(var, t))
            }
            // A const (`LIMIT` / `S::N`) used as a place (`println!("{}", LIMIT)` / `&LIMIT`): put in a temporary
            Expr::Path(_, p) if self.const_of(e).is_some() && ident_of(p).is_none_or(|id| self.lookup(id).is_none()) => {
                let ci = self.const_of(e).unwrap();
                let ci = self.pick_const_expr(e, ci, None);
                let (v, t) = self.const_value(e, ci)?;
                match v {
                    Some(a) if t.is_agg() => Ok(Place::Mem(a, 0, t)),
                    Some(v) => {
                        let var = self.b.declare_var(clif_ty(t).unwrap_or(PTR));
                        self.b.def_var(var, v);
                        Ok(Place::Var(var, t))
                    }
                    None => Ok(Place::Zst(t)),
                }
            }
            Expr::Path(_, p) => {
                // (`m::S`: the static by its index; a local of the same name, `S`, is not it)
                if let Some(ci) = self.static_of(e) {
                    if let Some(m) = self.unsafe_static_read(ci).filter(|_| self.unsafe_ok == 0 && p.pos.line != 0 && self.raw_ref == 0) {
                        return err(p.pos, m);
                    }
                    let t = self.prog.consts[ci as usize].ty;
                    let a = self.static_sym(ci);
                    return Ok(Place::Mem(a, 0, t));
                }
                // (`Generic::<5>` / `PhantomData::<u8>`: a generic unit struct with the turbofish's type arguments)
                if p.segs.last().is_some_and(|s| !s.args.is_empty()) {
                    let (pp, targs) = self.turbofish_split(p);
                    if let (Some(sid), Some(args)) = (self.prog.struct_by_p(&pp, self.self_sid), targs) {
                        let sd = &self.prog.structs[sid as usize];
                        if sd.fields.is_empty() && !sd.is_enum {
                            if let Some(ts) = self.turbofish_targs(sid, args)?.into_iter().collect::<Option<Vec<Ty>>>() {
                                let t = Ty::Struct(self.prog.instantiate(self.prog.base_of(sid), ts)?);
                                return Ok(Place::Mem(self.temp(t), 0, t));
                            }
                        }
                    }
                }
                let Some(id) = ident_of(p) else {
                    return err(SP, "unsupported path");
                };
                match self.lookup(id) {
                    Some((Loc::Var(v), t)) => Ok(Place::Var(v, t)),
                    Some((Loc::Zst(_), t)) => Ok(Place::Zst(t)),
                    Some((l, t)) => {
                        let a = self.loc_addr(l).unwrap();
                        Ok(Place::Mem(a, 0, t))
                    }
                    None => match self.prog.const_index.get(id) {
                        // A static is its data's address
                        Some(&ci) if self.prog.consts[ci as usize].stat.is_some() => {
                            if let Some(m) = self.unsafe_static_read(ci).filter(|_| self.unsafe_ok == 0 && p.pos.line != 0 && self.raw_ref == 0) {
                                return err(p.pos, m);
                            }
                            let t = self.prog.consts[ci as usize].ty;
                            let a = self.static_sym(ci);
                            Ok(Place::Mem(a, 0, t))
                        }
                        // A unit struct's value (`U` of `struct U;`)
                        _ => match self.prog.struct_by_p(&PPath::of(p), self.self_sid) {
                            Some(sid)
                                if self.prog.structs[sid as usize].fields.is_empty()
                                    && !self.prog.structs[sid as usize].is_enum
                                    && self.prog.structs[sid as usize].generics.is_empty() =>
                            {
                                let t = Ty::Struct(sid);
                                Ok(Place::Mem(self.temp(t), 0, t))
                            }
                            // (a generic unit struct (`PhantomData`): the instance type inference chose, or `()`s)
                            Some(sid) if self.prog.structs[sid as usize].fields.is_empty() && !self.prog.structs[sid as usize].is_enum => {
                                let base = self.prog.base_of(sid);
                                let t = match self.lets.get(&(e as *const Expr as usize)).copied() {
                                    Some(Ty::Struct(x)) if self.prog.base_of(x) == base && !self.prog.has_param(Ty::Struct(x)) => Ty::Struct(x),
                                    _ => {
                                        let n = self.prog.structs[base as usize].generics.len();
                                        Ty::Struct(self.prog.instantiate(base, vec![Ty::Unit; n])?)
                                    }
                                };
                                Ok(Place::Mem(self.temp(t), 0, t))
                            }
                            // `PI` after `use std::f64::consts::PI;`
                            _ => match self.float_const_name(e) {
                                Some(x) => {
                                    let v = self.b.ins().f64const(x);
                                    let var = self.b.declare_var(types::F64);
                                    self.b.def_var(var, v);
                                    Ok(Place::Var(var, Ty::F64))
                                }
                                None => err(SP, format!("cannot find value `{id}`")),
                            },
                        },
                    },
                }
            }
            Expr::Field(base, fname, fsp) => {
                // (a type without the field that implements Deref (`self.prog` with self: &Inf): `(*self.deref()).prog`)
                if let Some(Ty::Struct(sid)) = self.peek(base).map(|t| self.strip_refs(t)) {
                    if self.prog.field(sid, fname).is_none() {
                        // (a Deref chain that does not reach a field of that name within rustc's default recursion
                        // limit (128): a cycle (`impl Deref for Foo { type Target = Foo }`), or a chain that long,
                        // through generic targets too (`W<W<..>>` with `Target = T`). The borrow check follows a
                        // place's derefs as far, the same way (bk_place, bk_overloaded_shr))
                        let mut t = sid;
                        let mut over = true;
                        for _ in 0..128 {
                            let Some(d) = self.prog.method(t, "deref") else {
                                over = false;
                                break;
                            };
                            let Ty::Ref(_, i) = self.prog.sigs[d as usize].ret else {
                                over = false;
                                break;
                            };
                            let args: Vec<Option<Ty>> = self.prog.structs[t as usize].args.iter().map(|&a| Some(a)).collect();
                            // (a target that is a reference (`Target = &'static S`) is followed through it)
                            let mut target = self.prog.partial_subst(*i, &args);
                            while let Ty::Ref(_, x) = target {
                                target = *x;
                            }
                            let Ty::Struct(n) = target else {
                                over = false;
                                break;
                            };
                            t = n;
                            if self.prog.field(t, fname).is_some() {
                                over = false;
                                break;
                            }
                        }
                        // (the rewrites below lower the access again from the target: counted across them)
                        if over || self.deref_depth >= 128 {
                            let sname = self.prog.structs[sid as usize].name.clone();
                            return err(SP, format!("reached the recursion limit while auto-dereferencing `{sname}` (E0055)"));
                        }
                        if let Some(d) = self.user_deref_sid(base, sid) {
                            let f2 = self.keep_expr(Expr::Field(Box::new(Expr::Paren(Box::new(d))), fname.clone(), crate::parse::Span::default()));
                            if let Expr::Field(p, _, _) = f2 {
                                if let Expr::Paren(u) = &**p {
                                    self.deref_shared(u);
                                }
                            }
                            self.deref_depth += 1;
                            let r = self.place(f2);
                            self.deref_depth -= 1;
                            return r;
                        }
                    }
                }
                let (a, o, sid) = self.struct_base(base)?;
                match self.prog.field(sid, fname) {
                    Some((ft, foff)) => {
                        // (not a read `..base` makes of the fields a literal leaves out, whose span starts at its
                        // field: rustc's privacy pass reports that one, E0451)
                        if fsp.start != fsp.at {
                            self.field_privacy(sid, fname, fsp.at)?;
                        }
                        // (a struct with an unsized tail is a place of its (data, length) pair: a field is at the data
                        // address, the tail is a pair of its own (its address, the same length))
                        if Ty::Struct(sid).is_dst_struct() {
                            Ok(self.dst_field(a, o, ft, foff))
                        } else {
                            Ok(Place::Mem(a, o + foff as i32, ft))
                        }
                    }
                    None => err(SP, format!("no field `{fname}` on type `{}`", self.prog.structs[sid as usize].name)),
                }
            }
            Expr::Unary(op, x, sp) if op == "*" => {
                // `*g` (g: a type that implements Deref, such as MutexGuard): `*g.deref_mut()` / `*g.deref()`
                if let Some(d) = self.user_deref(x) {
                    let d = self.keep_expr(d);
                    // (the temporaries the borrow check follows in x (`*RefCell::new(1).borrow_mut()`'s receiver) are
                    // followed in its copy too)
                    if let Expr::Unary(_, c, _) = d {
                        if let Expr::MethodCall(r, ..) = &**c {
                            if let Expr::Paren(xc) = &**r {
                                if !self.bk.direct.is_empty() {
                                    borrowck::copy_marks(x, xc, &mut self.bk.direct, 0);
                                }
                            }
                        }
                    }
                    self.deref_shared(d);
                    let r = self.place(d);
                    // (the loan the deref's call takes of x is the loan of `*x`: `(&mut *w).push(w.len())` holds it
                    // while the arguments are evaluated)
                    if let (true, Expr::Unary(_, c, _)) = (self.bk_on(), d) {
                        if let Some(&ln) = self.bk.loan_of.get(&(&**c as *const Expr as usize)) {
                            self.bk.loan_of.insert(e as *const Expr as usize, ln);
                        }
                    }
                    return r;
                }
                // `*b` does not move b
                let (v, t) = self.read_no_move(x)?;
                match (v, t) {
                    // (`match *x { [a, b] => .. }` with x: &[T]: the slice itself, as a place that holds its fat pointer;
                    // patterns on it bind by value)
                    // (a fat pointer's value is the address of its two words, which is what a slice place holds)
                    (Some(fat), Ty::Ref(_, Ty::Slice(e)) | Ty::Box(Ty::Slice(e))) => Ok(Place::Mem(fat, 0, Ty::Slice(e))),
                    (Some(fat), Ty::Ref(_, i) | Ty::Box(i)) if i.is_dst_struct() => Ok(Place::Mem(fat, 0, *i)),
                    // (`*b` with b: `Box<dyn Tr>` / `&str`: a place of the pair too; reading it as a value is E0277)
                    (Some(fat), Ty::Ref(_, i @ (Ty::Str | Ty::Dyn(_))) | Ty::Box(i @ (Ty::Str | Ty::Dyn(_)))) => Ok(Place::Mem(fat, 0, *i)),
                    (Some(_), Ty::Ptr(..)) if self.unsafe_ok == 0 && sp.start.line != 0 => {
                        err(sp.start, "dereference of raw pointer is unsafe and requires unsafe block (E0133)")
                    }
                    // (a fat raw pointer is the address of its two words, as a fat reference is)
                    (Some(fat), Ty::Ptr(_, Ty::Slice(e))) => Ok(Place::Mem(fat, 0, Ty::Slice(e))),
                    (Some(fat), Ty::Ptr(_, i)) if i.is_dst_struct() => Ok(Place::Mem(fat, 0, *i)),
                    (Some(fat), Ty::Ptr(_, i @ (Ty::Str | Ty::Dyn(_)))) => Ok(Place::Mem(fat, 0, *i)),
                    (Some(_), t) if t.is_fat() => err(SP, "dereferencing a slice is not supported (index it instead)"),
                    (Some(_), Ty::Ptr(..)) if self.unsafe_ok == 0 && sp.start.line != 0 => {
                        err(sp.start, "dereference of raw pointer is unsafe and requires unsafe block (E0133)")
                    }
                    (Some(p), Ty::Ref(_, inner) | Ty::Ptr(_, inner) | Ty::Box(inner)) => Ok(Place::Mem(p, 0, *inner)),
                    // (a Deref type the type records did not name (`*guard.value` in a generic instance): through
                    // its deref, as above; x is a place, so reaching it again reads nothing new)
                    (_, Ty::Struct(sid)) if is_place_expr(x) && self.user_deref_sid(x, sid).is_some() => {
                        let d = self.user_deref_sid(x, sid).unwrap();
                        let d = self.keep_expr(d);
                        self.deref_shared(d);
                        self.place(d)
                    }
                    // (a Deref value that is not a place (`*(*rc).borrow()`, a guard returned by a call the type records
                    // did not name): its deref on the value already computed, which lives to the end of the statement)
                    (Some(a), Ty::Struct(sid)) if self.user_deref_sid(x, sid).is_some() => {
                        let m = if self.prog.method(sid, "deref_mut").is_some() { "deref_mut" } else { "deref" };
                        let did = self.prog.method(sid, m).unwrap();
                        self.own_temp(a, t);
                        let (pv, pt) = self.call_fn_vals(did, &[(a, Ty::Ref(m == "deref_mut", intern(t)))], SP)?;
                        match (pv, pt) {
                            (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                            _ => err(SP, format!("type `{}` cannot be dereferenced", self.prog.tn(t))),
                        }
                    }
                    _ => err(SP, format!("type `{}` cannot be dereferenced", self.prog.tn(t))),
                }
            }
            // `map[&k]`: what get(&k).unwrap() points to
            Expr::Index(b, k, _)
                if matches!(self.peek(b).map(|t| self.strip_refs(t)),
                    Some(Ty::Struct(sid)) if self.prog.structs[self.prog.base_of(sid) as usize].name == "HashMap") =>
            {
                let e = self.keep_expr(mcall(mcall(Expr::Paren(b.clone()), "get", vec![(**k).clone()]), "unwrap", vec![]));
                // (the map and key are values, not the place `&raw` takes: reading a static in them still needs unsafe)
                let raw = std::mem::take(&mut self.raw_ref);
                let r = self.expr(e, None);
                self.raw_ref = raw;
                let (v, t) = r?;
                match (v, t) {
                    (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(SP, "internal: bad map index"),
                }
            }
            // `d[i]` / `m[&k]` (impl Index / IndexMut、VecDeque / BTreeMap): `*x.index_mut(i)` / `*x.index(i)`
            Expr::Index(b, k, _)
                if matches!(self.peek_or_static(b).map(|t| self.strip_refs(t)),
                    Some(Ty::Struct(sid)) if self.vec_fields(sid).is_none()
                        && (self.prog.method(sid, "index").is_some() || self.prog.method(sid, "index_mut").is_some())) =>
            {
                let Some(Ty::Struct(sid)) = self.peek_or_static(b).map(|t| self.strip_refs(t)) else { unreachable!() };
                let m = if self.prog.method(sid, "index_mut").is_some() { "index_mut" } else { "index" };
                let e = self.keep_expr(mcall(Expr::Paren(b.clone()), m, vec![(**k).clone()]));
                // (index_mut is used for reads too: the borrow checker sees a shared borrow; a write is checked on the
                // place itself)
                self.bk.shared_recv = e as *const Expr as usize;
                // (the call takes `&x` and its index by value: neither is the place `&raw` takes, so reading a static
                // in them still needs unsafe)
                let raw = std::mem::take(&mut self.raw_ref);
                let r = self.expr(e, None);
                self.raw_ref = raw;
                let (v, t) = r?;
                match (v, t) {
                    (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(SP, "`index` must return a reference"),
                }
            }
            Expr::Index(b, index, isp) if !matches!(&**index, Expr::Range(..)) => {
                let isp = *isp;
                let (base, len, elem) = self.indexable(b)?;
                // (a place written through `IndexMut` (`v[v.len() - 1] = 1` of a Vec): the `&mut v` is taken before
                // the index is evaluated, with no two-phase borrow)
                let t0 = self.bk.call_temps.len();
                let held = self.bk_on()
                    && self.bk.place_mut
                    && borrowck::root_local(b).is_some_and(|n| borrowck::mentions(index, n))
                    && matches!(self.peek(b).map(|t| self.strip_refs(t)), Some(Ty::Struct(_)))
                    && self.bk_index_mut(b);
                // (the index is a value, not the place `&raw` takes: reading a static in it still needs unsafe)
                let raw = std::mem::take(&mut self.raw_ref);
                let r = self.expr(index, Some(Ty::Usize));
                self.raw_ref = raw;
                if held {
                    self.bk_call_temps_end(t0);
                }
                let (i, it) = r?;
                let Some(i) = i else {
                    return err(SP, "index diverges");
                };
                self.same(SP, it, Ty::Usize)?;
                // (indexing an array of length 3 or more with the k of `for k in 0..3` is known to be in range)
                let in_range = match (&**index, self.iconst_of(len)) {
                    (Expr::Path(_, p), Some(n)) => ident_of(p)
                        .and_then(|id| self.lookup(id))
                        .is_some_and(|(l, _)| self.loop_bounds.iter().any(|&(bl, h)| bl == l && h <= n as u64)),
                    _ => false,
                };
                if in_range {
                    let addr = self.elem_addr(base, i, elem);
                    return Ok(Place::Mem(addr, 0, elem));
                }
                // Out of range panics (bounds checked, as in rustc)
                let ok = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                // (rustc's place: the `[` for an Index impl such as Vec's, the expression's start for arrays and slices)
                let via_impl = matches!(self.peek(b).map(|t| self.strip_refs(t)), Some(Ty::Struct(_)));
                let sp = if via_impl { isp.at } else { isp.start };
                self.index_check(ok, len, i, sp);
                let addr = self.elem_addr(base, i, elem);
                Ok(Place::Mem(addr, 0, elem))
            }
            _ => err(SP, "expression is not assignable"),
        }
    }

    /// The base of a field access: finds the struct's address with auto-deref
    pub(crate) fn struct_base(&mut self, base: &Expr) -> R<(Value, i32, u32)> {
        // (`BAR.0` with a `const BAR` that needs dropping: the value is a temporary, dropped after the statement)
        let const_tmp = matches!(base, Expr::Path(..))
            && self.const_of(base).is_some_and(|c| {
                let c = &self.prog.consts[c as usize];
                c.stat.is_none() && self.needs_drop(c.ty)
            });
        let mut p = if is_place_expr(base) && !const_tmp {
            self.place(base)?
        } else {
            // (a temporary whose drop may use what it borrows (`D(&*v).0` with `impl Drop for D`): it holds its loans
            // until it is dropped, at the end of the statement (a loop's: after the loop, a `while let`'s scrutinee's:
            // with the iteration), E0502 for `for x in D(&*v).0.iter() { v.push(*x); }`)
            let watch = self.bk_on() && self.bk.level >= 2 && self.user_code;
            if watch {
                self.bk_frame();
            }
            let r = self.expr(base, None);
            let srcs = if watch { self.bk_frame_end() } else { vec![] };
            let r = r?;
            if !srcs.is_empty() && self.dropck(r.1, 0) {
                let _ = self.bk_drop_temp(r.1, srcs);
            }
            if let (Some(v), t) = r {
                self.own_temp(v, t);
            }
            match r {
                (Some(a), t) if t.is_agg() => Place::Mem(a, 0, t),
                (Some(p), Ty::Ref(_, i) | Ty::Box(i)) => Place::Mem(p, 0, *i),
                (_, t) => {
                    return err(SP, format!("type `{}` has no fields", self.prog.tn(t)));
                }
            }
        };
        loop {
            p = match p {
                Place::Mem(a, o, Ty::Struct(sid)) => return Ok((a, o, sid)),
                // (a reference to a struct with an unsized tail: its two words are the place's (data, length) pair)
                Place::Mem(a, o, Ty::Ref(_, i) | Ty::Box(i) | Ty::Ptr(_, i)) if i.is_dst_struct() => Place::Mem(self.addr_plus(a, o), 0, *i),
                Place::Var(v, Ty::Ref(_, i) | Ty::Box(i) | Ty::Ptr(_, i)) if i.is_dst_struct() => Place::Mem(self.b.use_var(v), 0, *i),
                Place::Mem(a, o, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.ins().load(PTR, mf(), a, o);
                    Place::Mem(ptr, 0, *i)
                }
                Place::Var(v, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.use_var(v);
                    Place::Mem(ptr, 0, *i)
                }
                p => {
                    return err(SP, format!("type `{}` has no fields", self.prog.tn(p.ty())));
                }
            }
        }
    }

    // --- expressions -------------------------------------------------------------

    /// Evaluates an expression and applies the implicit conversion to the expected type (`&[T; N]` → `&[T]`)
    /// The deny-by-default lint dangerous_implicit_autorefs, where rustc 1.100 fires it: `(*p).len()` /
    /// `(*p).is_empty()` and `(*p)[a..b]` with p a raw pointer to a slice or str (an implicit `&*p`)
    fn implicit_autoref(&self, e: &Expr) -> R<()> {
        let raw_deref = |r: &Expr| match strip_paren(r) {
            Expr::Unary(op, x, _) if op == "*" => matches!(self.peek(x), Some(Ty::Ptr(_, Ty::Slice(_) | Ty::Str))),
            _ => false,
        };
        let hit = match e {
            Expr::MethodCall(recv, m, _, args, _) => args.is_empty() && matches!(m.as_str(), "len" | "is_empty") && raw_deref(recv),
            Expr::Index(b, k, _) => matches!(&**k, Expr::Range(..)) && raw_deref(b),
            _ => false,
        };
        if hit && !self.prog.allow_implicit_autorefs {
            return err(expr_start(e), "implicit autoref creates a reference to the dereference of a raw pointer (dangerous_implicit_autorefs)");
        }
        Ok(())
    }

    pub(crate) fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // (no value has an unsized type: `&S { .. }` where `&S<[T]>` is expected makes an `S<[T; N]>`, then coerced)
        let expected = expected.filter(|t| !t.is_dst_struct());
        if self.user_code && matches!(e, Expr::MethodCall(..) | Expr::Index(..)) {
            self.implicit_autoref(e)?;
        }
        // (a returned position: what is borrowed and read in it is returned (Bk::ret_leaves))
        let returned = !self.bk.ret_leaves.is_empty() && self.bk.ret_leaves.contains(&(e as *const Expr as usize));
        if returned {
            self.bk.in_ret += 1;
        }
        let r = self.expr_at(e, expected);
        if returned {
            self.bk.in_ret -= 1;
        }
        let r = r?;
        // (a tuple struct's or an enum's value built by its constructor or named by its variant's path (`St(Some(x))`,
        // `En::A(x)`, `En::<T>::B`), of a type that bounds a type parameter by 'static: the type is well formed)
        if self.rg.on && self.inline_depth == 0 && matches!(e, Expr::Path(..) | Expr::Call(..)) {
            self.rg_ctor_wf(e, r.1);
        }
        Ok(r)
    }

    fn expr_at(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // (the type arguments a path writes are well formed: `None::<Static<T>>` with `struct Static<T: 'static>(T)`)
        if self.rg.on && self.inline_depth == 0 {
            let p = match e {
                Expr::Path(_, p) => Some(p),
                Expr::Call(f, ..) => match &**f {
                    Expr::Path(_, p) => Some(p),
                    _ => None,
                },
                _ => None,
            };
            if let Some(p) = p.filter(|p| p.segs.iter().any(|s| s.args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(_))))) {
                for seg in &p.segs {
                    self.rg_targs_wf(&seg.args, super::borrowck::pos(e));
                }
            }
            // (and a method call's: `r.use_ty::<Static<T>>()`)
            if let Expr::MethodCall(_, _, targs, ..) = e {
                if targs.iter().any(|a| matches!(a, crate::parse::GArg::Ty(_))) {
                    self.rg_targs_wf(targs, super::borrowck::pos(e));
                }
            }
        }
        // (the places inside a sub-expression are accesses of their own)
        let pd = std::mem::take(&mut self.bk.place_depth);
        let hint_only = std::mem::take(&mut self.bk.move_mut_hint);
        self.bk.exp_none = std::mem::take(&mut self.bk.move_mut_next) && (expected.is_none() || hint_only);
        // (a block's tail's receiver that is a temporary: what its value holds (bk_watch_tail))
        let watched = !self.bk.watch.is_empty() && self.bk.watch.contains(&(e as *const Expr as usize));
        // (or a copy of one: a method call at a watched position)
        let at = if !self.bk.watch_at.is_empty() && matches!(e, Expr::MethodCall(..)) {
            let p = super::err_pos(e);
            self.bk.watch_at.iter().position(|w| w.0 == p)
        } else {
            None
        };
        if watched || at.is_some() {
            self.bk_frame();
        }
        // (a place read or a call's result of a type that holds no reference (`*d` of `d: &mut bool`, `p.0` of a
        // number, `mem::replace(d, false)`) carries none of the loans it is reached through or made from: `(*d, log)`
        // holds only what log holds)
        let read = self.bk.on && (matches!(e, Expr::Unary(op, ..) if op == "*") || matches!(e, Expr::Field(..) | Expr::Index(..) | Expr::Call(..) | Expr::MethodCall(..)));
        let mark = if read { self.bk.frames.last().map(|f| (self.bk.frames.len(), f.len())) } else { None };
        // (a local `s: &&U` where a `&U` is expected: the deref coercion copies `*s` out, so the value holds what that
        // holds, not the loan s is (bk_copy_out_of))
        let coerced = match (expected, strip_paren(e)) {
            (Some(Ty::Ref(_, w)), Expr::Path(None, p)) if self.bk.on && self.bk.level >= 2 && !matches!(w, Ty::Ref(..) | Ty::Param(_)) => {
                match ident_of(p).and_then(|n| self.bk_local_of(n)).map(|l| (l, self.bk.locals[l as usize].ty)) {
                    Some((l, Ty::Ref(_, u @ Ty::Ref(false, i)))) if *i == w => self.bk.frames.last().map(|f| (self.bk.frames.len(), f.len(), l, *u)),
                    _ => None,
                }
            }
            _ => None,
        };
        let r = self.expr_inner(e, expected);
        if let (Some((n, k, l, u)), true) = (coerced, self.bk_on()) {
            if self.bk.frames.len() == n && self.bk.frames.last().is_some_and(|f| f[k..].contains(&borrowck::Src::Local(l))) {
                let c = self.bk_copy_out_of(l, u);
                if let Some(f) = self.bk.frames.last_mut() {
                    for s in f[k..].iter_mut() {
                        if *s == borrowck::Src::Local(l) {
                            *s = borrowck::Src::Local(c);
                        }
                    }
                }
            }
        }
        if let (Some((n, k)), Ok((_, t))) = (mark, &r) {
            // (not a call whose result `impl Trait` captures its arguments' borrows: it holds them, whatever its hidden type)
            if self.bk.frames.len() == n && *t != Ty::Never && self.bk_on() && !self.bk_ty(*t).1 && !self.bk_opaque_call(e) {
                if let Some(f) = self.bk.frames.last_mut() {
                    f.truncate(k);
                }
            }
        }
        if watched || at.is_some() {
            let s = self.bk_frame_end();
            if let Some(i) = at {
                for x in &s {
                    if !self.bk.watch_at[i].1.contains(x) {
                        self.bk.watch_at[i].1.push(*x);
                    }
                }
            }
            if watched {
                self.bk.watched.insert(e as *const Expr as usize, s);
            }
        }
        self.bk.place_depth = pd;
        let (v, t) = r.map_err(|m| {
            // (the error gets the start line:col of the innermost positioned expression; ACG_DEBUG_AT=1 adds the expression)
            let m = if super::debug_at() && !m.contains(" (at `") {
                let d = format!("{e:?}");
                format!("{m} (at `{}`)", d.chars().take(400).collect::<String>())
            } else {
                m
            };
            self.at(err_pos(e), m)
        })?;
        // `&Box<T>` → `&T` (deref coercion: `e = x;` with x: &Box<Expr>)
        if let (Some(want @ Ty::Ref(m2, we)), Some(val), Ty::Ref(m1, Ty::Box(bi))) = (expected, v, t) {
            if *we == **bi && !bi.is_unsized() && (m1 || !m2) {
                let p = self.b.ins().load(PTR, mf(), val, 0);
                return Ok((Some(p), want));
            }
        }
        // (a value of a closure type with no captures where a fn pointer is expected: its fn-pointer instance)
        if let (Some(want @ Ty::Fn(_)), Ty::Struct(sid), true) = (expected, t, self.lub_fnptr > 0 && matches!(e, Expr::Path(None, p) if p.segs.len() == 1)) {
            let b = self.prog.base_of(sid);
            if let Some(&cid) = self.prog.closure_of_struct.get(&b) {
                if self.prog.structs[sid as usize].fields.is_empty() && self.closure_sig(t).is_some_and(|s| matches!(want, Ty::Fn(f) if f.params == s.0 && f.ret == s.1)) {
                    let args = self.prog.structs[sid as usize].args.clone();
                    let (iid, sig) = {
                        let mut g = self.insts.lock().unwrap();
                        let iid = g.request_fnptr(self.prog, cid, args).or_else(|e| err(SP, e))?;
                        (iid, g.sig(self.prog, iid).clone())
                    };
                    self.requested.push(iid);
                    let fr = self.fref(iid, &sig);
                    let v = self.b.ins().func_addr(PTR, fr);
                    return Ok((Some(v), want));
                }
            }
        }
        // `&T` / `&mut T` → `*const T` / `*mut T`
        if let (Some(want @ Ty::Ptr(m2, pe)), Ty::Ref(m1, re)) = (expected, t) {
            if pe == re && (m1 || !m2) {
                return Ok((v, want));
            }
            // (`&[T; N]` → `*const [T]`: the fat pointer (address, N), as the cast makes it; `&[T]` is one already)
            match (pe, re, v) {
                (Ty::Slice(we), Ty::Array(ge, n), Some(v)) if we == ge && (m1 || !m2) => {
                    let fat = self.make_fat(v, *n as i64);
                    return Ok((Some(fat), want));
                }
                (Ty::Slice(we), Ty::Slice(ge), _) if we == ge && (m1 || !m2) => return Ok((v, want)),
                _ => {}
            }
        }
        if let (Some(want @ Ty::Ref(m2, Ty::Slice(we))), Some(v), Ty::Ref(m1, Ty::Array(ge, n))) = (expected, v, t) {
            if we == ge && (m1 || !m2) {
                let fat = self.make_fat(v, *n as i64);
                return Ok((Some(fat), want));
            }
        }
        // `&S<[T; N]>` → `&S<[T]>` (and to `*const S<[T]>`, `Box<S<[T; N]>>` → `Box<S<[T]>>`): the address and N
        if let (Some(want @ (Ty::Ref(m2, wi) | Ty::Ptr(m2, wi))), Some(v), Ty::Ref(m1, gi) | Ty::Ptr(m1, gi)) = (expected, v, t) {
            if (m1 || !m2) && !(matches!(want, Ty::Ref(..)) && matches!(t, Ty::Ptr(..))) {
                if let Some(m) = self.unsize_tail(*wi, *gi) {
                    let fat = self.make_unsized(v, m, expr_start(e))?;
                    return Ok((Some(fat), want));
                }
            }
        }
        if let (Some(want @ Ty::Box(wi)), Some(v), Ty::Box(gi)) = (expected, v, t) {
            if let Some(m) = self.unsize_tail(*wi, *gi) {
                let fat = self.make_unsized(v, m, expr_start(e))?;
                return Ok((Some(fat), want));
            }
        }
        // `&Vec<T>` → `&[T]`、`&String` → `&str`
        if let (Some(want @ Ty::Ref(m2, Ty::Slice(_) | Ty::Str)), Some(v), Ty::Ref(m1, Ty::Struct(sid))) = (expected, v, t) {
            if m1 || !m2 {
                if let Some((fat, ft)) = self.deref_to_slice(v, *sid, m2) {
                    let ok = match (want, ft) {
                        (Ty::Ref(_, Ty::Str), Ty::Ref(_, Ty::Str)) => true,
                        (Ty::Ref(_, Ty::Slice(we)), Ty::Ref(_, Ty::Slice(fe))) => fe == we,
                        _ => false,
                    };
                    if ok {
                        return Ok((Some(fat), want));
                    }
                }
            }
        }
        // `&mut T` → `&T` (`Box<T>` is a `&mut T` too, so it becomes `&T` here)
        if let (Some(want @ Ty::Ref(false, wi)), Ty::Ref(true, gi)) = (expected, t) {
            if wi == gi {
                return Ok((v, want));
            }
        }
        // (`strategy: PhantomData` for a `PhantomData<fn(S) -> S>`: a generic unit struct, with no fields, is the
        // instance it is expected to be)
        if let (Some(want @ Ty::Struct(x)), Ty::Struct(y)) = (expected, t) {
            if x != y && self.prog.base_of(x) == self.prog.base_of(y) && !self.prog.structs[y as usize].is_enum && self.prog.structs[y as usize].fields.is_empty() && self.prog.structs[x as usize].fields.is_empty() && !matches!(e, Expr::Call(..) | Expr::MethodCall(..)) {
                return Ok((v, want));
            }
        }
        // `&Box<[T]>` → `&[T]` (what the reference points to is the (pointer, length) pair, a `&[T]`'s value)
        if let (Some(want @ Ty::Ref(m2, Ty::Slice(we))), Some(v), Ty::Ref(m1, Ty::Box(Ty::Slice(ge)))) = (expected, v, t) {
            if we == ge && (m1 || !m2) {
                return Ok((Some(v), want));
            }
        }
        // (`&Box<Foo<[T]>>` → `&Foo<[T]>`: the same)
        if let (Some(want @ Ty::Ref(m2, wi)), Some(v), Ty::Ref(m1, Ty::Box(gi))) = (expected, v, t) {
            if *wi == **gi && wi.is_dst_struct() && (m1 || !m2) {
                return Ok((Some(v), want));
            }
        }
        // `Box<[T; N]>` → `Box<[T]>` (the pointer and N)
        if let (Some(want @ Ty::Box(Ty::Slice(we))), Some(v), Ty::Box(Ty::Array(ge, n))) = (expected, v, t) {
            if we == ge {
                let fat = self.make_fat(v, *n as i64);
                return Ok((Some(fat), want));
            }
        }
        // (`Box<dyn Tr + Send>` → `Box<dyn Tr>`, also behind `&` / `*const`: an object that drops auto traits is the
        // same value)
        if let (Some(want), Some(v)) = (expected, v) {
            let drops = match (want, t) {
                (Ty::Box(Ty::Dyn(w)), Ty::Box(Ty::Dyn(g))) => w != g && self.dyn_coerces(g, w),
                (Ty::Ref(m2, Ty::Dyn(w)), Ty::Ref(m1, Ty::Dyn(g))) | (Ty::Ptr(m2, Ty::Dyn(w)), Ty::Ptr(m1, Ty::Dyn(g))) => w != g && (m1 || !m2) && self.dyn_coerces(g, w),
                _ => false,
            };
            if drops {
                return Ok((Some(v), want));
            }
        }
        // `Box<T>` → `Box<dyn Trait>`
        if let (Some(want @ Ty::Box(Ty::Dyn(tr))), Some(v), Ty::Box(inner)) = (expected, v, t) {
            if !matches!(inner, Ty::Dyn(_)) {
                let fat = self.to_dyn(v, *inner, tr.as_str(), SP)?;
                return Ok((Some(fat), want));
            }
        }
        // `&T` → `&dyn Trait` (data pointer + vtable)
        if let (Some(want @ Ty::Ref(m2, Ty::Dyn(tr))), Some(v), Ty::Ref(m1, inner)) = (expected, v, t) {
            if !matches!(inner, Ty::Dyn(_)) && (m1 || !m2) {
                let fat = self.to_dyn(v, *inner, tr.as_str(), SP)?;
                return Ok((Some(fat), want));
            }
        }
        // `&W` → `&Target` through W's Deref (`deref_mut` for a `&mut`), as many as reach the type wanted (deref
        // coercion: `is_str(&value)` of an `Either<String, &str>`, whose Deref gives the str through String's). Last,
        // after the coercions above, and never to a `dyn`, which `&W` itself coerces to
        if let (Some(want @ Ty::Ref(m2, wi)), Some(v0), Ty::Ref(m1, Ty::Struct(sid0))) = (expected, v, t) {
            // (a program's type's Deref only: the prelude's (`PathBuf` → `Path`, `Rc<T>` → `T`) are not taken here, as
            // `ToolA::from(&path_buf)` with impls of `From<&Path>` and `From<&str>` is rustc's E0277, no coercion)
            let user = self.prog.struct_item.get(self.prog.base_of(*sid0) as usize).is_some_and(|&i| i < self.prog.prelude_from);
            if user && (m1 || !m2) && !matches!(wi, Ty::Dyn(_)) && *wi != Ty::Struct(*sid0) && !self.prog.has_param(*wi) && self.prog.method(*sid0, "deref").is_some() {
                if let Some(r) = self.deref_coerce_user(v0, *sid0, *wi, m2)? {
                    return Ok((Some(r), want));
                }
            }
        }
        // `&T` → `*const dyn Trait` (the same fat value as `&dyn Trait`)
        if let (Some(want @ Ty::Ptr(m2, Ty::Dyn(tr))), Some(v), Ty::Ref(m1, inner)) = (expected, v, t) {
            if m1 || !m2 {
                if matches!(inner, Ty::Dyn(t2) if t2 == tr || self.dyn_coerces(t2, tr)) {
                    return Ok((Some(v), want));
                }
                if !matches!(inner, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str) {
                    let fat = self.to_dyn(v, *inner, tr.as_str(), SP)?;
                    return Ok((Some(fat), want));
                }
            }
        }
        Ok((v, t))
    }

    /// The address of what `&W`'s derefs reach where a `&want` is wanted: W's `deref` (`deref_mut` for a `&mut`), then the
    /// target's, up to 8, and a String's / Vec's str / slice last. None where they do not reach it
    fn deref_coerce_user(&mut self, v: Value, sid: u32, want: Ty, m: bool) -> R<Option<Value>> {
        let (mut cur, mut s) = (v, sid);
        for _ in 0..8 {
            let name = if m && self.prog.method(s, "deref_mut").is_some() { "deref_mut" } else { "deref" };
            if m && name == "deref" {
                return Ok(None);
            }
            let Some(did) = self.prog.method(s, name) else { return Ok(None) };
            let (r, rt) = self.call_fn_vals(did, &[(cur, Ty::Ref(m, intern(Ty::Struct(s))))], SP)?;
            let (Some(r), Ty::Ref(_, inner)) = (r, rt) else { return Ok(None) };
            if *inner == want {
                return Ok(Some(r));
            }
            let Ty::Struct(s2) = *inner else { return Ok(None) };
            if matches!(want, Ty::Str | Ty::Slice(_)) {
                if let Some((fat, ft)) = self.deref_to_slice(r, s2, m) {
                    if matches!(ft, Ty::Ref(_, i) if *i == want) {
                        return Ok(Some(fat));
                    }
                }
            }
            if self.prog.method(s2, "deref").is_none() {
                return Ok(None);
            }
            cur = r;
            s = s2;
        }
        Ok(None)
    }

    pub(crate) fn expr_inner(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // Without an expected type, use the type that type inference chose from later uses
        // (the syn version re-looked up pos_key under in_fmt_args; our own syntax tree has no pos_key, so it is skipped)
        let expected = match expected {
            Some(t) => Some(t),
            None if matches!(e, Expr::Lit(_, _) | Expr::Struct(..) | Expr::Call(..)) => self.peek(e),
            None if matches!(e, Expr::Path(..)) => self.lets.get(&(e as *const Expr as usize)).copied(),
            None => None,
        };
        match e {
            Expr::Call(..) | Expr::MethodCall(..) | Expr::Struct(..) if self.ret_call == Some(e as *const Expr as usize) => {
                self.ret_call = None;
                self.dst_hint = self.sret;
                let r = match e {
                    Expr::Call(..) => self.call(e, expected),
                    Expr::MethodCall(..) => self.method_call(e, expected),
                    _ => self.struct_lit(e, expected),
                };
                self.dst_hint = None;
                r
            }
            Expr::Lit(l, _) => self.lit(l, expected),
            Expr::Paren(p) => self.expr(p, expected),
            Expr::Block(label, uns, b) => {
                self.unsafe_ok += *uns as u32;
                let r = match label {
                    Some(l) => self.labeled_block(label_name(l), b, expected),
                    None => self.block(b, expected),
                };
                self.unsafe_ok -= *uns as u32;
                // (a value that holds what the block's locals hold (bk_block_value), used here, after them: a match's
                // scrutinee, a receiver, an argument. Not one a statement discards)
                let key = b as *const crate::parse::Block as usize;
                if self.bk_on() && self.bk.stmt_block != key {
                    if let Some(&t) = self.bk.block_val.get(&key) {
                        self.bk_ev(borrowck::Ev::Acc(borrowck::BPlace { local: t, proj: vec![] }, borrowck::Acc::Read, borrowck::pos(e)));
                        self.bk.src(borrowck::Src::Local(t));
                    }
                }
                r
            }
            // (`[a, a]` after `let a = foo;` (foo generic): the fn foo)
            Expr::Path(None, p) if !self.fn_aliases.is_empty() && ident_of(p).is_some_and(|n| self.fn_aliases.iter().any(|a| a.0 == n)) => {
                let n = ident_of(p).unwrap();
                let target = self.fn_aliases.iter().rev().find(|a| a.0 == n).unwrap().1;
                self.expr(target, expected)
            }
            Expr::Path(_, p) if self.is_variant_path(p) || p.segs.len() >= 2 => {
                let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(p), self.self_sid) else {
                    return self.path_value(e, expected);
                };
                if self.prog.structs[sid as usize].variants[vi].kind != crate::types::VariantKind::Unit {
                    return err(SP, "this variant has fields: use `E::V(..)` or `E::V { .. }`");
                }
                // `None::<u32>`: a variant with written type arguments
                let expected = expected.or_else(|| {
                    let args = p.segs.iter().rev().find_map(|s| if s.args.is_empty() { None } else { Some(&s.args) })?;
                    let mut ts = vec![];
                    for a in args {
                        if let GArg::Ty(t) = a {
                            ts.push(self.parse_ty(t).ok()?);
                        }
                    }
                    self.prog.instantiate(self.prog.base_of(sid), ts).ok().map(Ty::Struct)
                });
                self.rg_written_on(p);
                let r = self.variant_value(sid, vi, &[], expected);
                self.rg_took_call(e);
                self.rg_written_ctor(e, p);
                r
            }
            Expr::Match(..) => self.match_(e, expected),
            // N of const generics: the length in the instance's type argument
            Expr::Path(..) if self.const_generic(e).is_some() => {
                let n = self.const_generic(e).unwrap();
                // (`const B: bool` / `const N: u8`: the parameter's declared type; a usize one takes an expected integer
                // type)
                let t = match (self.const_generic_ty(e), expected) {
                    (Some(t), _) => t,
                    (None, Some(t)) if t.is_int() => t,
                    _ => Ty::Usize,
                };
                Ok((Some(self.int_const(t, n as i128)), t))
            }
            // A const embeds the expression itself
            Expr::Path(..) if self.const_of(e).is_some() => {
                let ci = self.const_of(e).unwrap();
                // (`<&S>::C`: of consts of one name in more impls, the one of the impl it names)
                let ci = self.pick_const_expr(e, ci, expected);
                self.const_hint = expected;
                self.const_value(e, ci)
            }
            Expr::Path(..) if self.fn_item(e).is_some() => {
                let id = self.fn_item(e).unwrap();
                self.fn_value(e, id, expected)
            }
            Expr::Path(..) | Expr::Field(..) => {
                // (reading a union field needs `unsafe`, E0133; writing one does not)
                if let Expr::Field(b, _, sp) = e {
                    if self.unsafe_ok == 0 && sp.start.line != 0 {
                        if let Some(Ty::Struct(sid)) = self.peek(b).map(|t| self.strip_refs(t)) {
                            if self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name) {
                                return err(sp.start, "access to union field is unsafe and requires unsafe block (E0133)");
                            }
                        }
                    }
                }
                // (`&mut T` read where no reference type is expected is moved, not reborrowed: `{src}` / `let b = a;`)
                let exp_none = self.bk.exp_none;
                let p = self.place(e)?;
                // (a field of an unsized type read as a value (`let t = r.tail;` with r: `&S<[u8]>`): its size is not
                // known, as for `*slice`; a borrow of it (`&r.tail`) is a place, not here)
                if p.ty().is_unsized() {
                    return err(expr_start(e), format!("the size for values of type `{}` cannot be known at compilation time (E0277)", self.prog.tn(p.ty())));
                }
                let r = self.read(p);
                if self.bk_on() && (!self.bk_ty(r.1).0 || exp_none && matches!(r.1, Ty::Ref(true, _))) {
                    if !self.bk_ty(r.1).0 {
                        self.drop_move_check(e)?;
                    }
                    self.bk_acc(e, borrowck::Acc::Move);
                } else if self.bk_on() && !self.prog.static_copy.is_empty() {
                    self.bk_copy(e, r.1);
                }
                // Reading a value that needs drop as a value moves it
                if self.needs_drop(r.1) {
                    let mut root = e;
                    while let Expr::Field(b, _, _) = strip_paren(root) {
                        root = b;
                    }
                    if let Expr::Path(_, rp) = strip_paren(root) {
                        if let Some(id) = ident_of(rp) {
                            if let Some(c) = self.caps.iter().find(|c| c.0 == id) {
                                // Taking out of a capture held by value (FnOnce): save it and make the environment's side "empty" (0)
                                // (the environment is still dropped, but a 0 Vec / String / Box frees nothing). Also a field of it
                                // (`let t = s.1;`, the 2021 edition's capture of `s.1` by value): that field is emptied, and dropping the
                                // rest of the capture frees nothing of it
                                if let (false, Expr::Path(..) | Expr::Field(..), Place::Mem(a, o, t), Some(v)) = (c.2, strip_paren(e), p, r.0) {
                                    if self.zeroable(t) {
                                        let v = if t.is_agg() {
                                            let tmp = self.temp(t);
                                            self.copy(tmp, v, t);
                                            tmp
                                        } else {
                                            v
                                        };
                                        let dst = self.addr_plus(a, o);
                                        self.zero(dst, t);
                                        return Ok((Some(v), t));
                                    }
                                }
                                return err(SP, "moving a captured value out of a closure is not supported");
                            }
                        }
                    }
                    // A field moved out of a temporary (`f().0`): no drop of the whole temporary; only the other fields drop here
                    // (fields cannot move out of a type that implements Drop, so dropping field by field is fine)
                    if let (Expr::Field(..), Place::Mem(a, o, t), Some(v)) = (strip_paren(e), p, r.0) {
                        if let Some(tm) = self.temps.iter().rev().find(|tm| tm.loc == Loc::Ptr(a)).map(|tm| (tm.flag, tm.ty)) {
                            if let Ty::Struct(sid) = tm.1 {
                                let fields = self.prog.structs[sid as usize].fields.clone();
                                if !self.prog.structs[sid as usize].is_enum && fields.iter().any(|f| f.2 as i32 == o) {
                                    let v = if t.is_agg() {
                                        let tmp = self.temp(t);
                                        self.copy(tmp, v, t);
                                        tmp
                                    } else {
                                        v
                                    };
                                    self.clear_flag(tm.0);
                                    for (_, ft, off) in fields {
                                        if off as i32 != o && self.needs_drop(ft) {
                                            let fa = self.addr_plus(a, off as i32);
                                            self.drop_in_place(fa, ft);
                                        }
                                    }
                                    return Ok((Some(v), t));
                                }
                            }
                        }
                    }
                    if let (Expr::Field(..), Place::Mem(a, o, t), Some(v)) = (strip_paren(e), p, r.0) {
                        if self.zeroable(t) {
                            // Moving only a field: save the value and make the original "empty" (0).
                            // The owner is still dropped (a 0 Box frees nothing, a 0 Option is None)
                            let v = if t.is_agg() {
                                let tmp = self.temp(t);
                                self.copy(tmp, v, t);
                                tmp
                            } else {
                                v
                            };
                            let dst = self.addr_plus(a, o);
                            self.zero(dst, t);
                            return Ok((Some(v), t));
                        }
                    }
                    self.mark_moved(e);
                }
                Ok(r)
            }
            Expr::Unary(op, x, _) if op == "*" => {
                // (a reference copied out of what a call's result points into (`*names.first().unwrap()` with
                // `names: Vec<&str>`): it does not hold the borrow of names that reached it (bk_copy_out))
                let copy_out = self.bk_on() && !is_place_expr(x) && matches!(self.peek(x), Some(Ty::Ref(_, i)) if self.bk_ty(*i) == (true, true));
                // (`*r` of a local `r: &U`: the U copied out holds what r's referent holds, not the loan r is
                // (bk_copy_out_of): `let r = &a; t = *r;` holds what a holds)
                let local_copy = match (copy_out || !self.bk_on() || self.bk.level < 2, strip_paren(x)) {
                    (false, Expr::Path(None, p)) => match ident_of(p).and_then(|n| self.bk_local_of(n)).map(|l| (l, self.bk.locals[l as usize].ty)) {
                        Some((l, Ty::Ref(_, u))) if self.bk_ty(*u) == (true, true) => Some((l, *u)),
                        _ => None,
                    },
                    _ => None,
                };
                if copy_out || local_copy.is_some() {
                    self.bk_frame();
                }
                let p = self.place(e);
                if copy_out {
                    let srcs = self.bk.frames.pop().unwrap_or_default();
                    if let Some(Ty::Ref(_, u)) = self.peek(x) {
                        self.bk_copy_out(&srcs, *u);
                    }
                }
                if let Some((l, u)) = local_copy {
                    let srcs = self.bk.frames.pop().unwrap_or_default();
                    let c = srcs.contains(&borrowck::Src::Local(l)).then(|| self.bk_copy_out_of(l, u));
                    for s in srcs {
                        match (s, c) {
                            (borrowck::Src::Local(x), Some(c)) if x == l => self.bk.src(borrowck::Src::Local(c)),
                            _ => self.bk.src(s),
                        }
                    }
                }
                let p = p?;
                // (`*slice` as a value: its size is not known; only a place (a pattern, `&*s`) can be one)
                if p.ty().is_unsized() {
                    return err(expr_start(e), format!("the size for values of type `{}` cannot be known at compilation time", self.prog.tn(p.ty())));
                }
                // (a `&mut` a `let` with no type written is initialized from (`let y = unsafe { *p };`): moved, not
                // reborrowed)
                let mut_moved = self.bk.plain_init == e as *const Expr as usize && self.bk.level >= 2 && matches!(p.ty(), Ty::Ref(true, _));
                if self.bk_on() && (!self.bk_ty(p.ty()).0 || mut_moved) {
                    // (`*x` with x a raw pointer: what it points to is not owned, E0507; also with x a reference that
                    // is no place (`*&x`, `*v.first().unwrap()`))
                    if self.bk.level >= 1 && (matches!(self.peek(x), Some(Ty::Ptr(..))) || !is_place_expr(x)) {
                        self.drop_move_check(e)?;
                    }
                    // (`*rc` with rc an Rc / a Deref type of the program's: its deref gives a reference, which cannot
                    // be moved out of (Box is the one type whose `*` moves))
                    if let Some(Ty::Struct(sid)) = self.peek(x).map(|t| self.strip_refs(t)) {
                        if self.user_deref_sid(x, sid).is_some() && self.bk.level >= 1 {
                            let n = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
                            let what = if matches!(n.as_str(), "Rc" | "Arc") { format!("an `{n}`") } else { format!("dereference of `{}`", self.prog.tn(Ty::Struct(sid))) };
                            return err(expr_start(e), format!("cannot move out of {what} (E0507)"));
                        }
                    }
                    self.bk_acc(e, borrowck::Acc::Move);
                }
                // `*b` (b: Box<T>, T needs drop): moves the contents. b is not dropped
                let boxed = matches!(self.peek(x), Some(Ty::Box(_)));
                if boxed && self.needs_drop(p.ty()) {
                    self.mark_moved(e);
                }
                Ok(self.read(p))
            }
            Expr::Unary(..) => self.unary(e, expected),
            // (`&*s` / `&**r` with s: String or Vec<T> (through Deref): `s.as_str()` / `&s[..]`)
            Expr::Ref(m, x)
                if matches!(strip_paren(x), Expr::Unary(op, _, _) if op == "*")
                    && matches!(strip_paren(x), Expr::Unary(_, y, _) if matches!(self.peek(y), Some(Ty::Struct(sid))
                        if matches!(self.prog.structs[self.prog.base_of(sid) as usize].name.as_str(), "String" | "Vec"))) =>
            {
                let Expr::Unary(_, y, _) = strip_paren(x) else { unreachable!() };
                let Some(Ty::Struct(sid)) = self.peek(y) else { unreachable!() };
                let yb = Expr::Paren(Box::new((**y).clone()));
                let ne = if self.prog.structs[self.prog.base_of(sid) as usize].name == "String" {
                    mcall(yb, if *m { "as_mut_str" } else { "as_str" }, vec![])
                } else {
                    Expr::Ref(*m, Box::new(Expr::Index(Box::new(yb), Box::new(Expr::Range(None, None, false)), crate::parse::Span::default())))
                };
                let ne: &Expr = self.keep_expr(ne);
                self.expr(ne, expected)
            }
            // (deref coercion of a guard: `&g` where a `&T` (or `&[U]` / `&str` of a Vec / String T) is expected, with
            // g a RefCell's Ref / RefMut or a MutexGuard: `&*g`)
            Expr::Ref(m, x) if self.guard_coercion(x, *m, expected).is_some() => {
                let ne = self.guard_coercion(x, *m, expected).unwrap();
                let ne: &Expr = self.keep_expr(ne);
                self.expr(ne, expected)
            }
            Expr::Ref(..) => self.reference(e, expected),
            // `&raw const p` / `&raw mut p`: the address of the place, as a raw pointer (also of a `static mut`)
            Expr::RawRef(m, x) => {
                let r = self.keep_expr(Expr::Ref(*m, x.clone()));
                self.raw_ref += 1;
                let v = self.reference(r, None);
                self.raw_ref -= 1;
                match v? {
                    (v, Ty::Ref(m, i)) => Ok((v, Ty::Ptr(m, i))),
                    r => Ok(r),
                }
            }
            Expr::Struct(..) => self.struct_lit(e, expected),
            Expr::Binary(op, l, r, _) if self.rg.on => {
                let v = self.binary(e, expected);
                if v.is_ok() && matches!(op.as_str(), "==" | "!=" | "<" | "<=" | ">" | ">=") {
                    self.rg_cmp(l, r, e);
                }
                v
            }
            Expr::Binary(..) => self.binary(e, expected),
            Expr::AssignOp(_, left, _, _) => {
                if let Some(m) = self.prog.any_immutable_static.then(|| self.immutable_static(left)).flatten() {
                    return err(expr_start(left), m);
                }
                if let Some(m) = self.const_ptr_write(left).filter(|_| self.bk.level >= 1) {
                    return err(expr_start(left), m);
                }
                // (an overloaded `+=` is a call of `add_assign(&mut self, ..)`: no access of its own)
                let builtin = !matches!(self.peek(left), Some(Ty::Struct(_) | Ty::Ref(_, Ty::Struct(_))));
                let r = self.binary(e, expected)?;
                if builtin {
                    self.bk_acc(left, borrowck::Acc::Modify);
                }
                Ok(r)
            }
            Expr::Assign(left, right) => {
                if let Some(m) = self.prog.any_immutable_static.then(|| self.immutable_static(left)).flatten() {
                    return err(expr_start(left), m);
                }
                if let Some(m) = self.const_ptr_write(left).filter(|_| self.bk.level >= 1) {
                    return err(expr_start(left), m);
                }
                // (`map[&k] = v`: a map's index is not a place to assign to (E0594); `insert` or `get_mut`)
                if let Expr::Index(b, _, _) = strip_paren(left) {
                    if let Some(Ty::Struct(sid)) = self.peek(b).map(|t| self.strip_refs(t)) {
                        let n = self.prog.structs[self.prog.base_of(sid) as usize].name.as_str();
                        if matches!(n, "HashMap" | "BTreeMap") {
                            return err(expr_start(left), format!("cannot assign to data in an index of `{n}` (E0594)"));
                        }
                    }
                }
                // (`*{ x = z; &mut *y } = x`: a place with a block in it runs after the right side, which reads the
                // old x. The value goes to a temporary first, an aggregate copied out of where it is)
                if place_has_block(left) {
                    if let Some(t) = self.peek(left) {
                        let (v, rt) = self.expr(right, Some(t))?;
                        if rt == Ty::Never {
                            return Ok((None, Ty::Never));
                        }
                        self.same(SP, rt, t)?;
                        let v = match v {
                            Some(a) if t.is_agg() => {
                                let tmp = self.temp(t);
                                self.copy(tmp, a, t);
                                Some(tmp)
                            }
                            v => v,
                        };
                        let p = self.place(left)?;
                        let refill = if self.needs_drop(t) { self.drop_before_assign(left, p) } else { None };
                        self.write(p, v);
                        if let Some(f) = refill {
                            let one = self.b.ins().iconst(types::I8, 1);
                            self.b.ins().stack_store(one, f, 0);
                        }
                        return Ok((None, Ty::Unit));
                    }
                }
                let (blk0, ev0) = (self.b.live_block(), self.bk.evs.len());
                self.bk.place_depth += 1;
                let pm = std::mem::replace(&mut self.bk.place_mut, true);
                let p = self.place(left);
                self.bk.place_mut = pm;
                self.bk.place_depth -= 1;
                let p = p?;
                // (the right side is evaluated first: `*{ drop(x); &mut *y } = S { ..*x }`)
                let lhs_evs = if self.b.live_block() == blk0 && blk0.is_some() { self.bk.evs.split_off(ev0) } else { vec![] };
                let t = p.ty();
                self.bk_frame();
                let r = self.expr(right, Some(t));
                // (a place of a `&mut` assigned where a reference is: reborrowed, as at a `let` (`x = y;` is `x = &mut
                // *y`; `*y = 1` while x lives is E0506))
                if let (true, Ty::Ref(..)) = (r.is_ok(), t) {
                    self.bk_reborrow_let(right, t);
                }
                let srcs = self.bk_frame_end();
                let cur = self.b.live_block();
                for (b, e) in lhs_evs {
                    self.bk.evs.push((cur.unwrap_or(b), e));
                }
                let (v, rt) = r?;
                if rt == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                // (a value of a type that holds no references carries no loans: `self.i = I::new(buf);`)
                let srcs = if self.bk_on() && !self.bk_ty(t).1 { vec![] } else { srcs };
                self.bk_assign(left, &srcs);
                self.rg_assign(left, right);
                // (`p = ptr::null_mut()` into a `*const T`: `*mut T` coerces to `*const T`)
                let rt = match (rt, t) {
                    (Ty::Ptr(true, a), Ty::Ptr(false, b)) if a == b => t,
                    _ => rt,
                };
                self.same(SP, rt, t)?;
                let refill = if self.needs_drop(t) { self.drop_before_assign(left, p) } else { None };
                self.write(p, v);
                if let Some(f) = refill {
                    let one = self.b.ins().iconst(types::I8, 1);
                    self.b.ins().stack_store(one, f, 0);
                }
                Ok((None, Ty::Unit))
            }
            Expr::If(..) => self.if_(e, expected),
            Expr::While(_, cond, _) if matches!(&**cond, Expr::Let(..)) => self.while_let(e),
            Expr::While(label, cond, wbody) => {
                self.pending_label = label.as_deref().map(label_name);
                let header = self.b.create_block();
                let body = self.b.create_block();
                let exit = self.b.create_block();
                // -O: index receivers that the condition and body do not change are read before the loop
                let nh = self.hoisted.len();
                if self.prog.opt {
                    self.hoist_index_bases(wbody, None, Some(cond))?;
                }
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let t0 = self.temps.len();
                // (`'a: while break 'a {}`: the label is a target of `break` already in the condition (an unlabeled
                // `break` or a `continue` there is not))
                let label = self.pending_label.take();
                if label.is_some() {
                    self.loops.push(Loop { cont: header, brk: exit, breaks: 0, label: label.clone(), val: None, is_block: true, depth: self.scopes.len(), temps: t0, no_value: Some("while") });
                }
                let bk0 = self.bk.stmt_temps.len();
                let (c, ct) = self.expr(cond, Some(Ty::Bool))?;
                if label.is_some() {
                    self.loops.pop();
                }
                self.same(SP, ct, Ty::Bool)?;
                // (the condition's temporaries are dropped before the body: `while borrow().ok() { .. }`)
                if c.is_some() {
                    self.drop_temps(t0);
                    self.bk_temps_end(bk0);
                }
                match c {
                    Some(c) => {
                        self.b.ins().brif(c, body, &[], exit, &[]);
                    }
                    None => {
                        self.b.ins().trap(TrapCode::unwrap_user(1));
                    }
                }
                self.b.switch_to_block(body);
                self.loops.push(Loop {
                    cont: header,
                    brk: exit,
                    breaks: 0,
                    label,
                    val: None,
                    is_block: false,
                    depth: self.scopes.len(),
                    temps: t0,
                    no_value: Some("while"),
                });
                let bk_t0 = self.bk.stmt_temps.len();
                let (_, bt) = self.block(wbody, Some(Ty::Unit))?;
                self.loops.pop();
                if bt != Ty::Never {
                    // (a body with a value: at its tail, as rustc points)
                    self.same(block_end_pos(wbody), bt, Ty::Unit)?;
                    self.drop_for_exit(usize::MAX, t0);
                    self.bk_iteration_end(bk_t0);
                    self.b.ins().jump(header, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                self.drop_temps(t0);
                self.hoisted.truncate(nh);
                Ok((None, Ty::Unit))
            }
            Expr::Loop(label, lbody) => {
                self.pending_label = label.as_deref().map(label_name);
                let body = self.b.create_block();
                let exit = self.b.create_block();
                self.b.ins().jump(body, &[]);
                self.b.switch_to_block(body);
                self.loops.push(Loop {
                    cont: body,
                    brk: exit,
                    breaks: 0,
                    label: self.pending_label.take(),
                    val: None,
                    is_block: false,
                    depth: self.scopes.len(),
                    temps: self.temps.len(),
                    no_value: None,
                });
                let bk_t0 = self.bk.stmt_temps.len();
                let (_, bt) = self.block(lbody, Some(Ty::Unit))?;
                let lp = self.loops.pop().unwrap();
                if bt != Ty::Never {
                    // (a body with a value: at its tail, as rustc points)
                    self.same(block_end_pos(lbody), bt, Ty::Unit)?;
                    self.drop_for_exit(usize::MAX, lp.temps);
                    self.bk_iteration_end(bk_t0);
                    self.b.ins().jump(body, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                if lp.breaks == 0 {
                    // A loop without break returns no value (its type is !)
                    return Ok((None, Ty::Never));
                }
                // The value of `break v` is exit's argument
                if let Some(t) = lp.val {
                    let v = self.b.block_params(exit).first().copied();
                    return Ok((v, t));
                }
                Ok((None, Ty::Unit))
            }
            Expr::Break(label, value, pos) => {
                let label = label.as_deref().map(label_name);
                // (the error at the keyword or label itself, also when a value follows on another line)
                let Some(li) = self.loop_index(label.as_deref(), true) else {
                    return Err(self.at(*pos, "`break` outside of a loop (or unknown label)".to_string()));
                };
                // (E0571: `break 'a 1` out of a `while` / `for` loop, or a while's condition)
                if let (Some(_), Some(kind)) = (value, self.loops[li].no_value) {
                    return Err(self.at(*pos, format!("`break` with value from a `{kind}` loop (E0571)")));
                }
                // `break v`: evaluate the value, then leave (the first break decides the type)
                let mut args = vec![];
                if let Some(e) = value {
                    let want = self.loops[li].val;
                    let (v, t) = self.expr(e, want)?;
                    if t == Ty::Never {
                        return Ok((None, Ty::Never));
                    }
                    match want {
                        Some(w) => self.same(SP, t, w)?,
                        None => {
                            let brk = self.loops[li].brk;
                            let ct = if t.is_agg() || t.is_fat() { PTR } else { clif_ty(t).unwrap_or(types::I8) };
                            if t != Ty::Unit {
                                self.b.append_block_param(brk, ct);
                            }
                            self.loops[li].val = Some(t);
                        }
                    }
                    if let Some(v) = v {
                        args.push(v);
                    }
                }
                let lp = &mut self.loops[li];
                lp.breaks += 1;
                let (t, d, tt) = (lp.brk, lp.depth, lp.temps);
                self.drop_for_exit(d, tt);
                // (the locals of the scopes left go out of scope here: `loop { let x = 1; r = &x; break; }`)
                self.bk_dead_exit(d, *pos);
                let args: Vec<Value> = args;
                self.b.ins().jump(t, &args);
                Ok(self.dead())
            }
            Expr::Continue(label, pos) => {
                let label = label.as_deref().map(label_name);
                let Some(li) = self.loop_index(label.as_deref(), false) else {
                    return Err(self.at(*pos, "`continue` outside of a loop (or unknown label)".to_string()));
                };
                let lp = &self.loops[li];
                let (t, d, tt) = (lp.cont, lp.depth, lp.temps);
                self.drop_for_exit(d, tt);
                self.bk_dead_exit(d, *pos);
                self.b.ins().jump(t, &[]);
                Ok(self.dead())
            }
            Expr::Return(value, pos) => {
                let ret = self.ret;
                let v = match value {
                    Some(e) => {
                        self.mark_ret_call(e);
                        self.bk_ret_leaves(e);
                        self.bk_frame();
                        let r = self.expr(e, Some(ret));
                        let srcs = self.bk_frame_end();
                        let (v, t) = r?;
                        if t == Ty::Never {
                            return Ok((None, Ty::Never));
                        }
                        if self.bk_ty(ret).1 {
                            { let _ = &srcs; self.bk_ret(e); }
                        }
                        self.rg_ret(e);
                        let (v, t) = if t != ret { self.deref_coerce(v, t, ret) } else { (v, t) };
                        self.same(expr_start(e), t, ret)?;
                        v
                    }
                    None => {
                        // (E0069: `return;` in a fn of another return type, `!` too; at the keyword, also where it
                        // has no statement of its own: `0 => return,`)
                        if self.same(SP, Ty::Unit, ret).is_err() {
                            return Err(self.at(*pos, "`return;` in a function whose return type is not `()` (E0069)".to_string()));
                        }
                        None
                    }
                };
                // (a `Copy` aggregate read out of a place that what the function drops may own (`return v[0];`):
                // copied out before the drops)
                let v = match v {
                    Some(x) if ret.is_agg() && !self.fresh.contains_key(&x.0) && !self.b.is_stack_addr(x) && (!self.temps.is_empty() || !self.owned.is_empty()) => {
                        let a = self.temp(ret);
                        self.copy(a, x, ret);
                        Some(a)
                    }
                    v => v,
                };
                // (a closure's exit: what its captures held by value hold escapes it)
                self.bk_cap_escape();
                self.drop_for_exit(0, 0);
                self.emit_return(v);
                Ok(self.dead())
            }
            Expr::Call(..) => self.call(e, expected),
            Expr::Closure(..) => self.closure_value(e, expected),
            Expr::Range(..) => self.range_value(e, expected),
            Expr::Tuple(t) if t.is_empty() => Ok((None, Ty::Unit)),
            Expr::Tuple(_) => {
                let r = self.tuple_lit(e, expected);
                if let Ok((_, t)) = &r {
                    self.rg_tuple(e, *t);
                }
                r
            }
            Expr::Array(_) => {
                let r = self.array_lit(e, expected);
                if let Ok((_, t)) = &r {
                    self.rg_array(e, *t);
                }
                r
            }
            Expr::Repeat(..) => self.repeat_lit(e, expected),
            Expr::Index(_, ix, _) if matches!(&**ix, Expr::Range(..)) => err(SP, "range indexing needs a reference: `&a[lo..hi]`"),
            Expr::Index(b, _, _) => {
                // (a reference copied out of what an `index` call gives (`m[&1]` with `m: HashMap<u8, &str>`): it does
                // not hold the borrow of m that reached it (bk_copy_out). A Vec's or an array's element is a place of
                // the local, which holds only what the local holds)
                let copy_out = self.bk_on() && matches!(self.peek(b).map(|t| self.strip_refs(t)), Some(Ty::Struct(sid)) if self.vec_fields(sid).is_none());
                if copy_out {
                    self.bk_frame();
                }
                let p = self.place(e);
                if copy_out {
                    let srcs = self.bk.frames.pop().unwrap_or_default();
                    match &p {
                        Ok(p) if self.bk_ty(p.ty()) == (true, true) => self.bk_copy_out(&srcs, p.ty()),
                        _ => {
                            for s in srcs {
                                self.bk.src(s);
                            }
                        }
                    }
                }
                let p = p?;
                // (a `&mut` element a `let` with no type written is initialized from (`let b = a[0];`): moved, not
                // reborrowed (E0508))
                let mut_moved = self.bk.plain_init == e as *const Expr as usize && self.bk.level >= 2 && matches!(p.ty(), Ty::Ref(true, _));
                if self.bk_on() && (!self.bk_ty(p.ty()).0 || mut_moved) {
                    // (out of an array that is no local's place (a constant's, a temporary: `C[0]`, `[S][0]`): E0508,
                    // as out of a local's, which the borrow check's events report)
                    if let Some(at @ Ty::Array(..)) = self.peek(b) {
                        if self.bk.level >= 1 && self.bk_place(e).is_none() {
                            return err(expr_start(e), format!("cannot move out of type `{}`, a non-copy array (E0508)", self.prog.tn(at)));
                        }
                    }
                    self.bk_acc(e, borrowck::Acc::Move);
                }
                Ok(self.read(p))
            }
            Expr::For(..) => self.for_(e),
            Expr::MethodCall(..) => self.method_call(e, expected),
            Expr::Cast(x, ty) => {
                let to = self.parse_ty(ty)?;
                // `&x as &dyn Trait` is the same as evaluating with an expected type (a coercion)
                // (`&mut a as &mut [u8]` (a: an array) is a coercion too)
                if let Ty::Ref(_, Ty::Dyn(_) | Ty::Slice(_)) = to {
                    // (`#![deny(trivial_casts)]`: `&a as &[u8]` with a: [u8; N], or a cast to the type it has, is a
                    // coercion)
                    if self.user_code && self.prog.deny_trivial_casts.0 {
                        if let Some(ft) = self.peek(x) {
                            // (`&mut [u8] as &[u8]` too: the same pointee, a mutable reference to a shared one)
                            let trivial = ft == to
                                || matches!((ft, to), (Ty::Ref(m, Ty::Array(e, _)), Ty::Ref(tm, Ty::Slice(e2))) if e == e2 && (m || !tm))
                                || matches!((ft, to), (Ty::Ref(true, a), Ty::Ref(false, b)) if a == b);
                            if trivial {
                                return err(SP, format!("trivial cast: `{}` as `{}`", self.prog.tn(ft), self.prog.tn(to)));
                            }
                        }
                    }
                    let (v, t) = self.expr(x, Some(to))?;
                    self.same(SP, t, to)?;
                    return Ok((v, t));
                }
                // (`&s as *const dyn Tr`: the unsizing coercion to `&dyn Tr`, whose fat value the raw pointer shares)
                if let Ty::Ptr(pm, d @ Ty::Dyn(_)) = to {
                    if let Some(Ty::Ref(m, inner)) = self.peek(x) {
                        if !matches!(inner, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str) && (m || !pm) {
                            let want = Ty::Ref(m, d);
                            let (v, t) = self.expr(x, Some(want))?;
                            self.same(SP, t, want)?;
                            return Ok((v, to));
                        }
                    }
                }
                // (`Box::new(s) as Box<dyn Tr>` / `as Rc<dyn Tr>`: the unsizing coercion too; `Box::new(()) as _` where a
                // `Box<dyn Send>` is expected)
                let to = match (to, expected) {
                    (Ty::Param(h), Some(e @ Ty::Box(Ty::Dyn(_)))) if h == crate::program::HOLE => e,
                    _ => to,
                };
                let unsizing = match to {
                    Ty::Box(Ty::Dyn(_) | Ty::Slice(_)) => true,
                    Ty::Struct(sid) => self.prog.structs[sid as usize].args.iter().any(|a| matches!(a, Ty::Dyn(_))),
                    _ => false,
                };
                if unsizing {
                    let (v, t) = self.expr(x, Some(to))?;
                    self.same(SP, t, to)?;
                    return Ok((v, t));
                }
                // (`|x| x + 1 as fn(i32) -> i32` / `f as fn()` with a closure: the closure's fn pointer)
                if matches!(to, Ty::Fn(_)) && matches!(strip_paren(x), Expr::Closure(..)) {
                    let (v, t) = self.expr(x, Some(to))?;
                    self.same(SP, t, to)?;
                    // (its annotated parameters against the regions the type binds, as in a `let`: `(|_: &'static u32|
                    // {}) as fn(&u32)` is "lifetime may not live long enough")
                    if self.rg_on() {
                        self.rg_closure_fnptr(ty, x);
                    }
                    return Ok((v, t));
                }
                // (`c as fn(&str)` with c a closure value that captures nothing: its fn-pointer instance)
                if let (Ty::Fn(_), Some(Ty::Struct(sid))) = (to, self.peek(x)) {
                    let base = self.prog.base_of(sid);
                    if let Some(&cid) = self.prog.closure_of_struct.get(&base) {
                        // (its annotated parameters against the regions the type binds, as for a closure written there:
                        // `let c = |_: &'static u32| {}; c as fn(&u32)`)
                        if let Some(mut ce) = self.clo_ann.iter().find(|x| x.0 == base).map(|x| x.1.clone()) {
                            // (reported at the cast's value, as rustc does)
                            if let Expr::Closure(_, _, _, _, p) = &mut ce {
                                *p = borrowck::pos(x);
                            }
                            if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && regionck::enabled() {
                                self.rg_late_on();
                            }
                            if self.rg_on() {
                                self.rg_closure_fnptr(ty, &ce);
                            }
                        }
                        if self.prog.structs[sid as usize].fields.is_empty() {
                            // (the value carries nothing; evaluated for its effects)
                            self.expr(x, None)?;
                            let args = self.prog.structs[sid as usize].args.clone();
                            let prog = self.prog;
                            let (iid, sig) = {
                                let mut g = self.insts.lock().unwrap();
                                let iid = g.request_fnptr(prog, cid, args).or_else(|e| err(SP, e))?;
                                (iid, g.sig(prog, iid).clone())
                            };
                            self.requested.push(iid);
                            let fr = self.fref(iid, &sig);
                            let v = self.b.ins().func_addr(PTR, fr);
                            let t = Ty::Fn(crate::types::intern_fn(crate::types::FnTy { params: sig.params.clone(), ret: sig.ret }));
                            self.same(SP, t, to)?;
                            return Ok((Some(v), t));
                        }
                    }
                }
                let int_lit = matches!(strip_paren(x), Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Int(i)) if i.suffix().is_empty()));
                let hint = if int_lit && matches!(to, Ty::Ptr(..)) {
                    // (`0 as *mut u8`: an address)
                    Some(Ty::Usize)
                } else if int_lit && to == Ty::Char {
                    // (`0x51 as char`: the literal is a `u8`; a larger one is the deny-by-default `overflowing_literals`)
                    if let Expr::Lit(l, _) = strip_paren(x) {
                        if let Some(Lit::Int(i)) = lit::lit(l) {
                            let n: u64 = i.base10_digits().parse().unwrap_or(u64::MAX);
                            if n > 0xFF {
                                let m = if (0xD800..=0xDFFF).contains(&n) {
                                    "surrogate values are not valid for `char`"
                                } else if n > 0x10FFFF {
                                    "value exceeds maximum `char` value"
                                } else {
                                    "only `u8` can be cast into `char`"
                                };
                                return err(SP, m.to_string());
                            }
                        }
                    }
                    Some(Ty::U8)
                } else {
                    self.peek(x).or(match &**x {
                        Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Float(_))) => Some(Ty::F64),
                        _ => Some(Ty::I32),
                    })
                };
                let f0 = self.bk.frames.last().map(|f| f.len());
                let (v, from) = self.expr(x, hint)?;
                // (a cast to a number holds nothing of what x borrows: `r as *mut _ as usize` does not keep r's loans)
                if to.is_int() || to.is_float() || matches!(to, Ty::Bool | Ty::Char) {
                    if let (Some(n), Some(f)) = (f0, self.bk.frames.last_mut()) {
                        f.truncate(n);
                    }
                }
                // `x as _` (the expected type, else the same type) / `&x as *const _` (the pointee of the source)
                let to = match to {
                    Ty::Param(h) if h == crate::program::HOLE => expected.filter(|&e| !has_hole(self.prog, e)).unwrap_or(from),
                    Ty::Ptr(m, i) if has_hole(self.prog, *i) => match from {
                        Ty::Ref(_, a) | Ty::Ptr(_, a) if matches!(*i, Ty::Param(_)) => Ty::Ptr(m, a),
                        _ => match expected {
                            Some(e @ Ty::Ptr(..)) if !has_hole(self.prog, e) => e,
                            _ => to,
                        },
                    },
                    t => t,
                };
                if matches!(to, Ty::Ptr(..)) && has_hole(self.prog, to) {
                    return err(SP, "cannot cast to a pointer of an unknown kind (E0641)");
                }
                // (a cast to the same type: `f as fn()` with f: fn(); under `#![deny(trivial_casts)]` a reference's is
                // one, as `r as &u8` with r: &u8)
                if from == to && !from.is_int() && !from.is_float() {
                    if self.user_code && self.prog.deny_trivial_casts.0 && matches!(from, Ty::Ref(..)) {
                        let n = self.prog.tn(from);
                        return err(SP, format!("trivial cast: `{n}` as `{n}`"));
                    }
                    return Ok((v, to));
                }
                if from == Ty::Never {
                    return Ok((None, Ty::Never));
                }
                // (`#![deny(trivial_casts)]`: `&T as *const T`, `&mut T as *mut T`, `&[T; N] as *const [T]` are
                // coercions; `#![deny(trivial_numeric_casts)]`: a number cast to its own type)
                if self.user_code {
                    let (tc, tn) = self.prog.deny_trivial_casts;
                    if tn && from == to && (from.is_int() || from.is_float()) {
                        let n = self.prog.tn(from);
                        return err(SP, format!("trivial numeric cast: `{n}` as `{n}`"));
                    }
                    let trivial = match (from, to) {
                        (Ty::Ref(m, a), Ty::Ptr(pm, b)) => (m || !pm) && (a == b || matches!((*a, *b), (Ty::Array(e, _), Ty::Slice(e2)) if e == e2)),
                        _ => false,
                    };
                    if tc && trivial {
                        return err(SP, format!("trivial cast: `{}` as `{}`", self.prog.tn(from), self.prog.tn(to)));
                    }
                }
                // Reference → raw pointer, raw pointer ⇔ raw pointer / usize
                match (from, to) {
                    // (`&[T; N]` / `*const [T; N]` as `*const [T]`: the fat pointer (address, N). Before the arm of
                    // pointer to pointer, which keeps the thin value; `&T` gives no `*mut`)
                    (Ty::Ref(false, Ty::Array(..)), Ty::Ptr(true, Ty::Slice(_))) | (Ty::Ref(false, Ty::Slice(_)), Ty::Ptr(true, Ty::Slice(_))) => {
                        return err(SP, format!("casting `{}` as `{}` is invalid (E0606)", self.prog.tn(from), self.prog.tn(to)));
                    }
                    (Ty::Ref(_, Ty::Array(e, n)) | Ty::Ptr(_, Ty::Array(e, n)), Ty::Ptr(_, Ty::Slice(e2))) if e == e2 => {
                        let Some(a) = v else { return Ok((None, to)) };
                        let len = self.b.ins().iconst(types::I64, *n as i64);
                        return Ok((Some(self.make_fat_v(a, len)), to));
                    }
                    // (`&[T]` as `*const [T]` / `&dyn Tr` as `*const dyn Tr`: the same fat pointer)
                    (Ty::Ref(_, Ty::Slice(e)), Ty::Ptr(_, Ty::Slice(e2))) if e == e2 => return Ok((v, to)),
                    (Ty::Ref(m, Ty::Dyn(a)), Ty::Ptr(pm, Ty::Dyn(b))) if (a == b || self.dyn_coerces(a, b)) && (m || !pm) => return Ok((v, to)),
                    (Ty::Ref(_, a), Ty::Ptr(_, b)) if a == b || matches!(*a, Ty::Array(e, _) if e == b) => {
                        return Ok((v, to));
                    }
                    // (a raw slice pointer is the address of its (data, length) pair here: as a thin pointer, the data
                    // word)
                    // (a raw dyn pointer, the address of its (data, vtable) pair: likewise the data word)
                    (Ty::Ptr(_, a), Ty::Ptr(_, b)) if a.is_unsized() && !b.is_unsized() => {
                        return Ok((v.map(|a| self.b.ins().load(PTR, mf(), a, 0)), to));
                    }
                    // (`p as usize` of a fat raw pointer: its address, the data word)
                    (Ty::Ptr(_, a), t) if a.is_unsized() && matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v.map(|a| self.b.ins().load(PTR, mf(), a, 0)), to));
                    }
                    // (`p as *const S<[T]>` with p: `*const S<[T; N]>` / `*const S<T>` to `*const S<dyn Tr>`: the
                    // unsizing of a struct's tail, the address and N or the vtable)
                    (Ty::Ptr(m, gi), Ty::Ptr(pm, wi)) if wi.is_dst_struct() && (m || !pm) && self.unsize_tail(*wi, *gi).is_some() => {
                        let Some(a) = v else { return Ok((None, to)) };
                        let meta = self.unsize_tail(*wi, *gi).unwrap();
                        return Ok((Some(self.make_unsized(a, meta, SP)?), to));
                    }
                    // (`p as *const dyn Tr` with p: `*const S`: the unsizing to a (data, vtable) pair)
                    (Ty::Ptr(m, inner), Ty::Ptr(pm, Ty::Dyn(tr))) if !matches!(inner, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str) && (m || !pm) => {
                        let Some(a) = v else { return Ok((None, to)) };
                        // (a trait the program does not define (`dyn PartialEq<u8>`): no vtable to call through, a null
                        // one; the data word still casts back)
                        let fat = match self.to_dyn(a, *inner, tr.as_str(), SP) {
                            Ok(f) => f,
                            Err(e) if e.to_string().contains("cannot find trait") => {
                                let z = self.b.ins().iconst(PTR, 0);
                                self.make_fat_v(a, z)
                            }
                            Err(e) => return Err(e),
                        };
                        return Ok((Some(fat), to));
                    }
                    // (a cast may drop an object's auto traits, not add them (E0804): the value behind the pointer may not
                    // have them. As rustc's, only an object with a principal trait: one of auto traits alone (`dyn Sync`
                    // as `dyn Send + Sync`) has no methods to call)
                    (Ty::Ptr(_, Ty::Dyn(g)), Ty::Ptr(_, Ty::Dyn(w))) if g != w && dyn_norm(g).0.is_some() && dyn_norm(g).0 == dyn_norm(w).0 && !self.dyn_coerces(g, w) => {
                        let ((_, gs, _), (_, ws, _)) = (dyn_norm(g), dyn_norm(w));
                        let added = if ws && !gs { "Send" } else { "Sync" };
                        return err(SP, format!("cannot add auto trait `{added}` to dyn bound via pointer cast (E0804)"));
                    }
                    (Ty::Ptr(..), Ty::Ptr(..)) => return Ok((v, to)),
                    // (a fn pointer as a raw pointer: the same address)
                    (Ty::Fn(_), Ty::Ptr(..)) => return Ok((v, to)),
                    (Ty::Ptr(..), t) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    // A function pointer's address (`f as usize`)
                    (Ty::Fn(_), t) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    (t, Ty::Ptr(..)) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    // (a narrower or wider integer and a raw pointer: through usize, as `as` does)
                    (t, Ty::Ptr(_, i)) if t.is_int() && t.bits() <= 64 && !i.is_unsized() => {
                        return Ok((v.map(|x| self.convert(x, t, Ty::Usize)), to));
                    }
                    (Ty::Ptr(_, i), t) if t.is_int() && t.bits() <= 64 && !i.is_unsized() => {
                        return Ok((v.map(|x| self.convert(x, Ty::Usize, t)), to));
                    }
                    _ => {}
                }
                // Fieldless enum → integer: read the tag (= the discriminant)
                if let Ty::Struct(sid) = from {
                    let sd = &self.prog.structs[sid as usize];
                    if to.is_int() && sd.is_enum && sd.variants.iter().all(|v| v.fields.is_empty()) {
                        // (one variant: no tag is stored, the value is its discriminant)
                        if sd.variants.len() == 1 {
                            let d = sd.variants[0].discr;
                            return Ok((Some(self.int_const(to, d as i128)), to));
                        }
                        let tag = sd.tag;
                        let t = self.b.ins().load(clif_ty(tag).unwrap(), mf(), v.unwrap(), 0);
                        return Ok((Some(self.convert(t, tag, to)), to));
                    }
                }
                // Casts involving floats (as with Rust's `as`, float → integer saturates and NaN is 0)
                if from.is_float() || to.is_float() {
                    let v = v.unwrap();
                    let span = SP;
                    // 128-bit integer ⇔ float go through prelude functions (Cranelift lacks them)
                    if from.bits() == 128 && to.is_float() {
                        let sg = self.b.ins().iconst(types::I8, from.signed() as i64);
                        let single = self.b.ins().iconst(types::I8, (to == Ty::F32) as i64);
                        let (r, _) =
                            self.call_named("__exr_u128_to_f", vec![Ty::Unit], &[(v, Ty::U128), (sg, Ty::Bool), (single, Ty::Bool)], span)?;
                        let r = r.unwrap();
                        let r = if to == Ty::F32 { self.b.ins().fdemote(types::F32, r) } else { r };
                        return Ok((Some(r), to));
                    }
                    if from.is_float() && to.bits() == 128 {
                        let w = if from == Ty::F32 { self.b.ins().fpromote(types::F64, v) } else { v };
                        let sg = self.b.ins().iconst(types::I8, to.signed() as i64);
                        let (r, _) = self.call_named("__exr_f_to_u128", vec![Ty::Unit], &[(w, Ty::F64), (sg, Ty::Bool)], span)?;
                        return Ok((r, to));
                    }
                    let r = match (from.is_float(), to.is_float()) {
                        (true, true) if from == to => v,
                        (true, true) if to == Ty::F64 => self.b.ins().fpromote(types::F64, v),
                        (true, true) => self.b.ins().fdemote(types::F32, v),
                        (false, true) if from.is_int() => {
                            let w = if from.bits() < 32 { self.convert(v, from, if from.signed() { Ty::I32 } else { Ty::U32 }) } else { v };
                            let ft = clif_ty(to).unwrap();
                            if from.signed() { self.b.ins().fcvt_from_sint(ft, w) } else { self.b.ins().fcvt_from_uint(ft, w) }
                        }
                        (true, false) if to.is_int() => {
                            // Saturate to 64 bit, then clamp a narrower type to its range
                            let wide =
                                if to.signed() { self.b.ins().fcvt_to_sint_sat(types::I64, v) } else { self.b.ins().fcvt_to_uint_sat(types::I64, v) };
                            if to.bits() == 64 {
                                wide
                            } else {
                                let (lo, hi): (i64, i64) = match to {
                                    Ty::I8 => (i8::MIN as i64, i8::MAX as i64),
                                    Ty::I16 => (i16::MIN as i64, i16::MAX as i64),
                                    Ty::I32 => (i32::MIN as i64, i32::MAX as i64),
                                    Ty::U8 => (0, u8::MAX as i64),
                                    Ty::U16 => (0, u16::MAX as i64),
                                    _ => (0, u32::MAX as i64),
                                };
                                let c = if to.signed() {
                                    let a = self.b.ins().iconst(types::I64, lo);
                                    let m = self.b.ins().smax(wide, a);
                                    let b2 = self.b.ins().iconst(types::I64, hi);
                                    self.b.ins().smin(m, b2)
                                } else {
                                    let b2 = self.b.ins().iconst(types::I64, hi);
                                    self.b.ins().umin(wide, b2)
                                };
                                self.b.ins().ireduce(clif_ty(to).unwrap(), c)
                            }
                        }
                        _ => {
                            return err(SP, format!("unsupported cast `{}` as `{}`", self.prog.tn(from), self.prog.tn(to)));
                        }
                    };
                    return Ok((Some(r), to));
                }
                // Integer ⇔ char / bool → integer (to char only from u8)
                let ok = (to.is_int() && (from.is_int() || matches!(from, Ty::Bool | Ty::Char))) || (to == Ty::Char && matches!(from, Ty::U8 | Ty::Char));
                if !ok && to == Ty::Char && from.is_int() {
                    return err(SP, format!("only `u8` can be cast as `char`, not `{}` (E0604)", self.prog.tn(from)));
                }
                if !ok {
                    return err(SP, format!("unsupported cast `{}` as `{}`", self.prog.tn(from), self.prog.tn(to)));
                }
                Ok((Some(self.convert(v.unwrap(), from, to)), to))
            }
            Expr::Macro(p, g, sp) => {
                let saved = std::mem::replace(&mut self.mbase, sp.at);
                let saved_s = std::mem::replace(&mut self.mstart, sp.start);
                let r = self.macro_(p, g, expected);
                self.mbase = saved;
                self.mstart = saved_s;
                r
            }
            Expr::Try(_) => self.try_(e),
            // `const { .. }` (an inline const): its value, computed where it is (the same value as at compile time)
            Expr::Const(b) => {
                // (E0435: an inline const reads no local of the code around it: `if let Some(v) = .. { const { v } }`)
                if let Some(at) = self.const_reads_local(e) {
                    return Err(self.at(at, "attempt to use a non-constant value in a constant (E0435)".into()));
                }
                self.block(b, expected)
            }
            e => {
                let k = format!("{e:?}");
                let k = k.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_string();
                err(SP, format!("unsupported expression ({k})"))
            }
        }
    }

    /// whether a value of t can change behind a shared reference (a Cell, an atomic ..): such a constant is not
    /// promoted (its static data is read-only, and each `&C` is a value of its own)
    fn interior_mut(&self, t: Ty, depth: u32) -> bool {
        match t {
            Ty::Struct(sid) if depth < 16 => {
                let sd = &self.prog.structs[sid as usize];
                let base = &self.prog.structs[self.prog.base_of(sid) as usize].name;
                matches!(base.as_str(), "UnsafeCell" | "Cell" | "RefCell" | "Mutex" | "OnceLock" | "OnceCell")
                    || base.starts_with("Atomic")
                    || sd.fields.iter().any(|f| self.interior_mut(f.1, depth + 1))
                    || sd.variants.iter().any(|v| v.fields.iter().any(|f| self.interior_mut(f.1, depth + 1)))
            }
            Ty::Array(e, _) => self.interior_mut(*e, depth + 1),
            _ => false,
        }
    }

    /// whether the constant expression e is made of parts the constant evaluator reads exactly (literals, struct /
    /// tuple / array / variant constructors, and constants that are such too): only those are promoted
    fn const_pure(&self, e: &Expr, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        match e {
            Expr::Lit(..) => is_promotable(self, e),
            Expr::Paren(x) => self.const_pure(x, depth),
            Expr::Unary(op, x, _) => op == "-" && self.const_pure(x, depth),
            Expr::Struct(_, fs, rest) => rest.is_none() && fs.iter().all(|f| self.const_pure(&f.1, depth)),
            Expr::Tuple(t) | Expr::Array(t) => t.iter().all(|x| self.const_pure(x, depth)),
            Expr::Call(_, args) => is_promotable(self, e) && args.iter().all(|x| self.const_pure(x, depth)),
            Expr::Path(..) => match self.const_of(e) {
                Some(ci) => {
                    let c = &self.prog.consts[ci as usize];
                    c.stat.is_none() && crate::parse::parse_expr(&c.expr).is_ok_and(|x| self.const_pure(&x, depth + 1))
                }
                None => is_promotable(self, e),
            },
            _ => false,
        }
    }

    /// a constant expression's value as static data: a reference to it
    fn promote(&mut self, rexpr: &Expr, inner_exp: Option<Ty>) -> Option<V> {
        // (`&1u16`: a suffixed literal is of its suffix's type, whatever is expected or inferred around it)
        let own = match strip_paren(rexpr) {
            Expr::Lit(l, _) => match lit::lit(l) {
                Some(Lit::Int(i)) => Ty::from_suffix(i.suffix()).or(match i.suffix() { "f32" => Some(Ty::F32), "f64" => Some(Ty::F64), _ => None }),
                Some(Lit::Float(f)) => match f.suffix() { "f32" => Some(Ty::F32), "f64" => Some(Ty::F64), _ => None },
                _ => None,
            },
            _ => None,
        };
        let t = own.or_else(|| inner_exp.filter(|t| !matches!(t, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str))).or_else(|| self.peek(rexpr));
        let t = t.filter(|&t| !self.needs_drop(t) && !self.prog.has_param(t) && !self.interior_mut(t, 0))?;
        let dv = crate::ast_consteval::eval(self.prog, rexpr, t).ok()?;
        if !dv.relocs.is_empty() || dv.bytes.is_empty() {
            return None;
        }
        let a = self.data_addr(dv.bytes);
        Some((Some(a), Ty::Ref(false, intern(t))))
    }

    /// A borrow guard (`Ref<T>` / `RefMut<T>` / `MutexGuard<T>`): (its struct, T, whether it derefs mutably)
    pub(crate) fn guard_of(&self, t: Ty) -> Option<(u32, Ty, bool)> {
        let Ty::Struct(sid) = t else { return None };
        if !matches!(self.prog.structs[self.prog.base_of(sid) as usize].name.as_str(), "Ref" | "RefMut" | "MutexGuard" | "PeekMut") {
            return None;
        }
        let target = *self.prog.structs[sid as usize].args.first()?;
        Some((sid, target, self.prog.method(sid, "deref_mut").is_some()))
    }

    /// A RefCell, or a reference / Box / Rc / Arc to one (what `c.borrow()` reaches by auto-deref)
    pub(crate) fn is_refcell(&self, t: Ty) -> bool {
        let mut t = t;
        for _ in 0..8 {
            t = match t {
                Ty::Ref(_, i) | Ty::Box(i) => *i,
                Ty::Struct(sid) => match self.prog.structs[self.prog.base_of(sid) as usize].name.as_str() {
                    "RefCell" => return true,
                    "Rc" | "Arc" => match self.prog.structs[sid as usize].args.first() {
                        Some(&a) => a,
                        None => return false,
                    },
                    _ => return false,
                },
                _ => return false,
            };
        }
        false
    }

    /// `&g` (g: Ref<T> / RefMut<T> / MutexGuard<T>) where a reference to T, or the slice / str of a Vec / String T, is
    /// expected: the expression to use instead (`&*g`, `&(*g)[..]`, `(*g).as_str()`). None when no coercion applies
    /// (a `&dyn Trait` is expected, or a reference to the guard itself)
    pub(crate) fn guard_coercion(&self, x: &Expr, written_mut: bool, expected: Option<Ty>) -> Option<Expr> {
        let Some(Ty::Ref(wm, want)) = expected else { return None };
        // (`&g` where a `&mut T` is expected: no coercion makes a shared borrow of the guard mutable (E0308))
        if wm && !written_mut {
            return None;
        }
        let (sid, target, _) = self.guard_of(self.peek(x)?)?;
        if *want == Ty::Struct(sid) || matches!(want, Ty::Dyn(_)) || self.prog.has_param(*want) {
            return None;
        }
        let inner = Expr::Paren(Box::new(Expr::Unary("*".into(), Box::new(x.clone()), Default::default())));
        let tname = match target {
            Ty::Struct(t) => self.prog.structs[self.prog.base_of(t) as usize].name.clone(),
            _ => String::new(),
        };
        Some(match want {
            w if *w == target => Expr::Ref(wm, Box::new(inner)),
            Ty::Slice(_) if tname == "Vec" => Expr::Ref(wm, Box::new(Expr::Index(Box::new(inner), Box::new(Expr::Range(None, None, false)), Default::default()))),
            Ty::Str if tname == "String" => mcall(inner, if wm { "as_mut_str" } else { "as_str" }, vec![]),
            _ => return None,
        })
    }

    pub(crate) fn reference(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Ref(m, rexpr) = e else { return err(SP, "internal: expected a reference") };
        let m = *m;
        // (`&(42,).0` / `&NUMS[i]`: a field or an element of a constant is borrowed out of the constant promoted to
        // static data, `&(*&B).f` / `&(*&B)[i]`, so the reference outlives the function)
        // (`&CONST`: the constant's own static data, not a copy in this frame)
        if !m && matches!(strip_paren(rexpr), Expr::Path(..)) && self.const_of(strip_paren(rexpr)).is_some() && self.const_pure(strip_paren(rexpr), 0) {
            let inner_exp = match expected {
                Some(Ty::Ref(_, i)) if !matches!(i, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str) => Some(*i),
                _ => None,
            };
            if let Some(r) = self.promote(strip_paren(rexpr), inner_exp) {
                return Ok(r);
            }
        }
        let plain = |cx: &Self, b: &Expr| cx.peek(b).is_some_and(|t| !cx.needs_drop(t) && !cx.prog.has_param(t) && !cx.interior_mut(t, 0));
        if !m {
            let promoted = |b: &Expr| Box::new(Expr::Paren(Box::new(Expr::Unary("*".into(), Box::new(Expr::Ref(false, Box::new(b.clone()))), Default::default()))));
            let re = match strip_paren(rexpr) {
                Expr::Field(b, f, sp) if !matches!(strip_paren(b), Expr::Unary(..)) && self.const_pure(b, 0) && plain(self, b) => Some(Expr::Field(promoted(b), f.clone(), *sp)),
                Expr::Index(b, i, sp) if !matches!(&**i, Expr::Range(..)) && !matches!(strip_paren(b), Expr::Unary(..)) && self.const_pure(b, 0) && plain(self, b) => {
                    Some(Expr::Index(promoted(b), i.clone(), *sp))
                }
                _ => None,
            };
            if let Some(re) = re {
                let ne = self.keep_expr(Expr::Ref(false, Box::new(re)));
                return self.expr(ne, expected);
            }
        }
        // `&*x` / `&mut *x` with x a fat pointer (`&[T]`, `&str`, `Box<dyn Tr>` ..): the reborrow is x's value itself
        if let Expr::Unary(op, inner, _) = strip_paren(rexpr) {
            if op == "*" {
                if let Some(it) = self.peek(inner).filter(|t| t.is_fat() && matches!(t, Ty::Ref(..) | Ty::Box(_))) {
                    let pointee = match it {
                        Ty::Ref(_, p) | Ty::Box(p) => *p,
                        _ => unreachable!(),
                    };
                    let (v, _) = self.read_no_move(inner)?;
                    // (of a Box: a borrow of what the box owns, as `&*b` of a `Box<T>` (E0505 when b is moved while
                    // it lives, E0515 when it is returned))
                    // (a method call's auto-ref (`b.iter_mut()`) is made with no position: the box's)
                    if matches!(it, Ty::Box(_)) && !matches!(expected, Some(Ty::Ptr(..))) {
                        let sp = err_pos(rexpr);
                        self.bk_borrow(rexpr, m, if sp.line == 0 { err_pos(inner) } else { sp });
                    }
                    return Ok((v, Ty::Ref(m, intern(pointee))));
                }
            }
        }
        // (`&[][..]` where a `&[T]` is expected, form_urlencoded's `split.next().unwrap_or(&[][..])`: the empty array
        // takes the slice's element type, as `&[]` there does)
        if let (Expr::Index(b, ix, _), Some(Ty::Ref(_, Ty::Slice(_)))) = (&**rexpr, expected) {
            if matches!(strip_paren(b), Expr::Array(xs) if xs.is_empty()) && matches!(&**ix, Expr::Range(None, None, false)) {
                let ne: &Expr = self.keep_expr(Expr::Ref(m, b.clone()));
                return self.expr(ne, expected);
            }
        }
        // `&a[lo..hi]` → slice (fat pointer)
        if let Expr::Index(b, ix, _) = &**rexpr {
            if let Expr::Range(..) = &**ix {
                let r = self.slice_range(b, ix, m)?;
                // (a borrow of the elements: `&mut x[1..3]` with x: &[T] is E0596, and it conflicts as `&mut x[1]`)
                self.bk_borrow(rexpr, m, err_pos(rexpr));
                // (`&f().0[..]`: of a temporary's elements)
                if let Some(t) = self.temp_root(rexpr).and_then(|r| self.peek(r)).filter(|_| self.bk_on() && self.bk_place(rexpr).is_none()) {
                    self.bk_temp_borrow(rexpr, m, t);
                }
                return Ok(r);
            }
        }
        let fn_item = matches!(&**rexpr, Expr::Path(..) if self.fn_item(rexpr).is_some());
        // (names of variants and consts, like `&None`, are temporaries, not places)
        let not_var = |e: &Expr| {
            matches!(e, Expr::Path(_, p) if ident_of(p).is_some_and(|n| {
                self.lookup(n).is_none() && !self.prog.const_index.contains_key(n)
            }))
        };
        let inner = match &**rexpr {
            Expr::Paren(p) => &**p,
            e => e,
        };
        if is_place_expr(rexpr) && !fn_item && !not_var(inner) {
            // (`&**s` with `s: &&T`: what the reborrow holds is what bk_borrow copies out of `*s`, not all s holds,
            // which reading s gives)
            let shr_root = match strip_paren(rexpr) {
                Expr::Unary(o, x, _) if o == "*" && self.bk_on() && self.bk.level >= 2 => match strip_paren(x) {
                    Expr::Unary(o, x, _) if o == "*" => self
                        .bk_place(x)
                        .filter(|(p, _)| p.proj.is_empty() && matches!(self.bk.locals[p.local as usize].ty, Ty::Ref(_, Ty::Ref(false, _))))
                        .map(|(p, _)| p.local),
                    _ => None,
                },
                _ => None,
            };
            if shr_root.is_some() {
                self.bk_frame();
            }
            let pm = std::mem::replace(&mut self.bk.place_mut, m);
            let pl = self.place(rexpr);
            self.bk.place_mut = pm;
            if let Some(l) = shr_root {
                for s in self.bk.frames.pop().unwrap_or_default() {
                    if s != borrowck::Src::Local(l) {
                        self.bk.src(s);
                    }
                }
            }
            let pl = pl?;
            // (`&a` of a reference `a: &T` (or `&&T` ..) where a `&T` is expected (a deref coercion, also as a result): the
            // pointer a holds, a reborrow of `*a` (`**a` ..), not a borrow of the local a)
            if let (Some(Ty::Ref(_, want)), Ty::Ref(..)) = (expected, pl.ty()) {
                // (the derefs to a `&T` (`&mut T` when one is taken), as far as a few)
                let (mut t, mut n, mut muts) = (pl.ty(), 0usize, true);
                while let Ty::Ref(rm, i) = t {
                    muts &= rm;
                    if *i == *want || n > 4 {
                        break;
                    }
                    t = *i;
                    n += 1;
                }
                let thin = !matches!(want, Ty::Str | Ty::Slice(_) | Ty::Dyn(_));
                if matches!(t, Ty::Ref(_, i) if *i == *want) && thin && (muts || !m) && matches!(pl, Place::Mem(..) | Place::Var(..)) {
                    let sp = crate::parse::Span { start: expr_start(rexpr), at: expr_start(rexpr) };
                    let mut star: &Expr = rexpr;
                    for _ in 0..=n {
                        star = self.keep_expr(Expr::Unary("*".into(), Box::new(star.clone()), sp));
                    }
                    // (`&mut a` is a mutable borrow of the local a, which needs it `mut` (E0596 of `test(&mut x)` with
                    // `ref mut x`), and what it reborrows through it lives no longer than that borrow: as before)
                    if m {
                        self.bk_borrow(rexpr, true, err_pos(rexpr));
                    } else {
                        let n0 = self.bk.loans.len();
                        self.bk_borrow(star, m, err_pos(rexpr));
                        if self.bk.loans.len() > n0 {
                            self.bk.loan_of.insert(&**rexpr as *const Expr as usize, n0 as u32);
                        }
                    }
                    let mut v = match pl {
                        Place::Mem(a, o, _) => self.b.ins().load(PTR, mf(), a, o),
                        Place::Var(v, _) => self.b.use_var(v),
                        _ => unreachable!(),
                    };
                    for _ in 0..n {
                        v = self.b.ins().load(PTR, mf(), v, 0);
                    }
                    if !m && (self.rg_on() || self.rg.eligible) {
                        self.rg.coerced.push((e as *const Expr as usize, n));
                    }
                    return Ok((Some(v), Ty::Ref(m, want)));
                }
            }
            // (`&mut x as *mut _` / coerced to a raw pointer: not a loan)
            if !matches!(expected, Some(Ty::Ptr(..))) {
                self.bk_borrow(rexpr, m, err_pos(rexpr));
                // (`&*Box::new(1)`: a borrow of what a temporary owns, which goes away with it (E0716))
                if matches!(strip_paren(rexpr), Expr::Unary(op, _, _) if op == "*") && self.bk_place(rexpr).is_none() {
                    self.bk_temp_borrow(rexpr, m, pl.ty());
                } else if let Some(t) = self.temp_root(rexpr).and_then(|r| self.peek(r)).filter(|_| self.bk_on() && self.bk_place(rexpr).is_none()) {
                    // (`&f().0`, `&x.borrow().1`: a borrow of a place in a temporary, which goes away with it)
                    self.bk_temp_borrow(rexpr, m, t);
                }
            }
            return match pl {
                // `&mut v[i]` (v: Vec<Box<T>>) in a `&mut T` position: points to the Box's contents (deref coercion)
                Place::Mem(a, o, Ty::Box(x)) if matches!(expected, Some(Ty::Ref(_, e)) if *e == *x) && !x.is_unsized() => {
                    let p = self.b.ins().load(PTR, mf(), a, o);
                    Ok((Some(p), Ty::Ref(m, x)))
                }
                Place::Mem(a, o, t) => Ok((Some(self.addr_plus(a, o)), Ty::Ref(m, intern(t)))),
                Place::Zst(t) => {
                    let a = self.temp(t);
                    Ok((Some(a), Ty::Ref(m, intern(t))))
                }
                // (a value in a register: a const read as a place (`assert_eq!(B, 3)` borrows `B`) or a local the
                // address-taken scan did not see: borrow a copy in a temporary)
                Place::Var(v, t) if !matches!(rexpr.as_ref(), Expr::Path(_, p) if ident_of(p).is_some_and(|n| self.lookup(n).is_some()) && m) => {
                    let val = self.b.use_var(v);
                    let a = self.temp(t);
                    self.b.ins().store(mf(), val, a, 0);
                    Ok((Some(a), Ty::Ref(m, intern(t))))
                }
                Place::Var(..) => err(SP, "internal: borrowed local is not in memory"),
            };
        }
        // A reference to a temporary: put it in a temporary area and take its address
        let inner_exp = match expected {
            // (`f(&3)` with `f(x: *const isize)`: the pointee too, for the coercion to the raw pointer)
            // (not a pointer to a pointer: `&mut &mut x` is not `*mut *mut T`)
            Some(Ty::Ref(_, i)) => Some(*i),
            Some(Ty::Ptr(_, i)) if !matches!(i, Ty::Ptr(..) | Ty::Ref(..)) => Some(*i),
            _ => None,
        };
        // A shared reference to a const expression is promoted to static data (`fn f() -> &'static S { &S { x: 1 } }`)
        if !m && is_promotable(self, rexpr) {
            if let Some(r) = self.promote(rexpr, inner_exp) {
                return Ok(r);
            }
        }
        let (v, t) = self.expr(rexpr, inner_exp)?;
        if t == Ty::Never {
            return Ok((None, Ty::Never));
        }
        if !matches!(expected, Some(Ty::Ptr(..))) {
            self.rg_temp(rexpr, t);
        }
        let a = match (v, t) {
            (Some(a), t) if t.is_agg() => a,
            (v, t) => {
                let a = self.temp(t);
                if let Some(v) = v {
                    self.b.ins().store(mf(), v, a, 0);
                }
                a
            }
        };
        // `&temporary`: the temporary is owned until the end of the statement (for a let, of the block)
        self.own_temp_at(a, t);
        // (coerced to a raw pointer: not a loan, as for a place)
        if !matches!(expected, Some(Ty::Ptr(..))) {
            self.bk_temp_borrow(rexpr, m, t);
        }
        Ok((Some(a), Ty::Ref(m, intern(t))))
    }

    pub(crate) fn struct_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        if let Expr::Struct(path, ..) = e {
            self.rg_written_on(path);
        }
        let r = self.struct_lit_inner(e, expected);
        // (E0277: an argument of a parameter bounded by Copy that is not (`union U<T: Copy>` as `U<Rc<u32>>`))
        if let (Ok((_, Ty::Struct(sid))), true) = (&r, self.user_code && !self.prog.copy_params.is_empty()) {
            let (sid, prog) = (*sid, self.prog);
            if let Some(ks) = prog.copy_params.get(&prog.base_of(sid)) {
                for &k in ks {
                    if let Some(&a) = prog.structs[sid as usize].args.get(k) {
                        if !prog.has_param(a) && !has_hole(prog, a) && (!super::borrowck::is_copy(prog, a, 0) || super::p3_ops::copy_mut_ref(prog, a, 0)) {
                            return err(expr_start(e), format!("the trait bound `{}: Copy` is not satisfied (E0277)", prog.tn(a)));
                        }
                    }
                }
            }
        }
        if let Ok((_, t)) = &r {
            self.rg_struct_lit(e, *t);
            // (a struct-like variant (`E::V { r: x }`): its value's tree, which variant_value made last)
            if matches!(t, Ty::Struct(s) if self.prog.structs[*s as usize].is_enum) {
                self.rg_took_call(e);
            }
            if let Expr::Struct(path, ..) = e {
                self.rg_written_ctor(e, path);
            }
        }
        r
    }

    fn struct_lit_inner(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Struct(path, sfields, rest) = e else { return err(SP, "internal: expected a struct literal") };
        // (a variant that is not struct-like yields to a struct of that name: `Skip { .. }` with `use E::*`'s `Skip`)
        let variant = self.prog.variant_by_p(&PPath::of(path), self.self_sid).filter(|&(sid, vi)| {
            self.prog.structs[sid as usize].variants[vi].kind == crate::types::VariantKind::Struct || self.prog.struct_by_p(&PPath::of(path), self.self_sid).is_none()
        });
        if let Some((sid, vi)) = variant {
            let v = &self.prog.structs[sid as usize].variants[vi];
            if v.kind != crate::types::VariantKind::Struct {
                return err(SP, "this variant is not a struct-like variant");
            }
            let mut args = vec![];
            for (fid, fe) in sfields {
                // (a tuple's index `0: x` is not a name)
                if fid.parse::<u32>().is_ok() {
                    return err(SP, "expected a field name");
                }
                // (at the field, as rustc: its value's position, on its line)
                let Some(fi) = v.fields.iter().position(|f| f.0 == *fid) else {
                    return Err(self.at(err_pos(fe), format!("variant `{}` has no field named `{fid}`", v.name)));
                };
                if args.iter().any(|&(i, _)| i == fi) {
                    return Err(self.at(err_pos(fe), format!("field `{fid}` specified more than once")));
                }
                args.push((fi, fe));
            }
            if args.len() != v.fields.len() || rest.is_some() {
                return err(SP, format!("missing fields in initializer of `{}`", v.name));
            }
            return self.variant_value(sid, vi, &args, expected);
        }
        // (`Self::<u8> {}`: E0109)
        if let [s] = path.segs.as_slice() {
            if s.name == "Self" && !s.args.is_empty() {
                return err(SP, "type arguments are not allowed on self type (E0109)");
            }
        }
        // (`S::<u32> { t: 1 }`: the struct without the turbofish, which gives its type arguments)
        let (pp, targs) = self.turbofish_split(path);
        let mut written = None;
        let found = match self.prog.struct_by_p(&PPath::of(path), self.self_sid) {
            Some(s) => Some(s),
            None => match targs {
                Some(ta) => match self.prog.struct_by_p(&pp, self.self_sid) {
                    Some(s) => {
                        self.lifetime_arity(s, path, "struct")?;
                        let ts = self.turbofish_targs(s, ta)?;
                        // (`Foo::<u8> { ..base }` with base a `Foo<u16>`: the base is the mismatch, as rustc says it)
                        if let Some(base) = rest.as_deref() {
                            if let Some(bt @ Ty::Struct(b)) = self.peek(base).filter(|t| !self.prog.has_param(*t)) {
                                let bargs = &self.prog.structs[b as usize].args;
                                if self.prog.base_of(b) == self.prog.base_of(s) && ts.iter().enumerate().any(|(k, a)| a.is_some_and(|a| !self.prog.has_param(a) && bargs.get(k).is_some_and(|&ba| ba != a))) {
                                    let full: Vec<Ty> = ts.iter().enumerate().map(|(k, a)| a.or(bargs.get(k).copied()).unwrap_or(Ty::Unit)).collect();
                                    let want = self.prog.instantiate(self.prog.base_of(s), full).map(|i| self.prog.tn(Ty::Struct(i))).unwrap_or_else(|_| self.prog.structs[s as usize].name.clone());
                                    return err(expr_start(base), format!("mismatched types: expected `{want}`, found `{}`", self.prog.tn(bt)));
                                }
                            }
                        }
                        // (`let x: S<u16> = S::<u8> { t: 1 }`: the turbofish against the expected instance)
                        self.turbofish_vs_expected(s, &ts, expected)?;
                        written = Some(ts);
                        Some(s)
                    }
                    None => None,
                },
                None => None,
            },
        };
        let Some(sid) = found else {
            // (`Self::Output::Simple {}` with `type Output<T> = Bar<T>;`: the generic associated type's arguments are
            // not given, nothing else says them)
            if path.segs.len() >= 3 && path.segs[0].name == "Self" && path.segs[1].args.is_empty()
                && self.prog.assocs.iter().any(|a| a.name == path.segs[1].name && a.ngat > 0)
            {
                return err(SP, format!("type annotations needed (the arguments of the generic associated type `{}`)", path.segs[1].name));
            }
            return err(SP, "cannot find struct");
        };
        // `S { a: 1, ..base }`: puts base in a temporary variable and moves the unwritten fields out of it
        // (the base's original values of the written fields are dropped at the end of the temporary's scope)
        // (a place as the base (`..x`): the fields not written are read or moved out of it one by one, and the
        // rest of `x` stays where it is)
        if let Some(base) = rest.as_deref().filter(|b| {
            let mut r = strip_paren(b);
            while let Expr::Field(x, _, _) = r {
                r = strip_paren(x);
            }
            matches!(r, Expr::Path(..)) && is_place_expr(b) && self.const_of(r).is_none()
        }) {
            if let Some(Ty::Struct(bsid)) = self.peek(base).filter(|&t| !self.prog.has_param(t)) {
                let all: Vec<String> = self.prog.structs[bsid as usize].fields.iter().map(|f| f.0.clone()).collect();
                let mut nfields = sfields.clone();
                for f in all {
                    if !sfields.iter().any(|fv| fv.0 == f) {
                        // (a span that starts at its field: field_privacy leaves it to the privacy pass's E0451)
                        let at = expr_start(base);
                        nfields.push((f.clone(), Expr::Field(Box::new(base.clone()), f, crate::parse::Span { start: at, at })));
                    }
                }
                let ns = self.keep_expr(Expr::Struct(path.clone(), nfields, None));
                return self.struct_lit(ns, Some(Ty::Struct(bsid)));
            }
        }
        // (`S { a: f(), ..g() }`: the fields written are evaluated before the base. Fields with effects (calls,
        // blocks ..) go to locals first; literals and paths have none and keep the field's type as their hint)
        if let Some(base) = rest {
            let effects = |x: &Expr| !matches!(strip_paren(x), Expr::Lit(..) | Expr::Path(..) | Expr::Closure(..) | Expr::Ref(..) | Expr::Unary(..));
            if sfields.iter().any(|(_, x)| effects(x)) && !matches!(strip_paren(base), Expr::Path(..)) {
                let mut stmts = vec![];
                let mut nfields = vec![];
                for (i, (f, x)) in sfields.iter().enumerate() {
                    if effects(x) {
                        let n = format!("__exr_sf{i}");
                        stmts.push(crate::parse::Stmt::Let(Pat::Ident(false, false, n.clone(), None), None, Some(x.clone()), None, Default::default()));
                        nfields.push((f.clone(), path_expr(&n)));
                    } else {
                        nfields.push((f.clone(), x.clone()));
                    }
                }
                stmts.push(crate::parse::Stmt::Expr(Expr::Struct(path.clone(), nfields, Some(base.clone())), false));
                let ne: &Expr = self.keep_expr(Expr::Block(None, false, crate::parse::Block { stmts, spos: Default::default() }));
                return self.expr(ne, expected);
            }
        }
        if let Some(base) = rest {
            let st = match expected {
                Some(Ty::Struct(x)) if self.prog.base_of(x) == self.prog.base_of(sid) => Some(Ty::Struct(x)),
                _ if self.prog.structs[sid as usize].generics.is_empty() => Some(Ty::Struct(sid)),
                _ => None,
            };
            self.scopes.push(vec![]);
            // (borrow checking: the fields not written are read or moved one by one, not the whole base)
            let bkp = if self.bk_on() { self.bk_place(base).map(|p| p.0) } else { None };
            let saved_on = self.bk.on;
            if bkp.is_some() {
                self.bk.on = false;
            }
            let (blk0, ev0) = (self.b.live_block(), self.bk.evs.len());
            let r = self.expr(base, st);
            self.bk.on = saved_on;
            let (bv, bt) = r?;
            // (Rust evaluates `..base` after the fields written: keep its events for later, if it stayed in one block)
            let deferred = if self.b.live_block() == blk0 && blk0.is_some() { self.bk.evs.split_off(ev0) } else { vec![] };
            if let (Some(bp), Ty::Struct(bsid)) = (bkp, bt) {
                let fs = self.prog.structs[bsid as usize].fields.clone();
                for (f, ft, _) in fs {
                    if !sfields.iter().any(|fv| fv.0 == f) {
                        let mut p = bp.clone();
                        p.proj.push(borrowck::Proj::Field(f));
                        let a = if borrowck::is_copy(self.prog, ft, 0) { borrowck::Acc::Read } else { borrowck::Acc::Move };
                        // (moving a field out of a value whose type implements `Drop`: E0509)
                        if a == borrowck::Acc::Move && self.bk.level >= 1 {
                            if let Some(n) = self.user_drop_name(bsid) {
                                return err(err_pos(base), format!("cannot move out of type `{n}`, which implements the `Drop` trait (E0509)"));
                            }
                        }
                        self.bk_acc_place(p, a, err_pos(base));
                    }
                }
            }
            if bt == Ty::Never {
                self.scopes.pop();
                return Ok((None, Ty::Never));
            }
            self.bind("__exr_base".into(), bt, bv);
            let Ty::Struct(bsid) = bt else {
                return err(SP, "the base of `..` must be a struct");
            };
            let all: Vec<String> = self.prog.structs[bsid as usize].fields.iter().map(|f| f.0.clone()).collect();
            let mut nfields = sfields.clone();
            for f in all {
                let given = sfields.iter().any(|fv| fv.0 == f);
                if !given {
                    nfields.push((f.clone(), Expr::Field(Box::new(path_expr("__exr_base")), f, crate::parse::Span::default())));
                }
            }
            let ns = self.keep_expr(Expr::Struct(path.clone(), nfields, None));
            let r = self.struct_lit(ns, Some(bt));
            let cur = self.b.live_block();
            for (b, e) in deferred {
                self.bk.evs.push((cur.unwrap_or(b), e));
            }
            let r = r?;
            self.pop_scope();
            return Ok(r);
        }
        let fields = self.prog.structs[sid as usize].fields.clone();
        // (`Result { .. }`: an enum's name where a struct goes, E0574, at the literal)
        if self.prog.structs[self.prog.base_of(sid) as usize].is_enum {
            let n = self.prog.tn(Ty::Struct(self.prog.base_of(sid)));
            let n = n.split('<').next().unwrap_or(&n).to_string();
            return err(SP, format!("expected struct, variant or union type, found enum `{n}` (E0574)"));
        }
        let mut args = vec![];
        for (name, fe) in sfields {
            // (at the field, as rustc: its value's position, on its line)
            let Some(fi) = fields.iter().position(|f| f.0 == *name) else {
                return Err(self.at(err_pos(fe), format!("struct `{}` has no field named `{name}`", self.prog.structs[sid as usize].name)));
            };
            if args.iter().any(|&(i, _)| i == fi) {
                return Err(self.at(err_pos(fe), format!("field `{name}` specified more than once")));
            }
            args.push((fi, fe));
        }
        // (a union is written through one field)
        let union = self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name);
        if union && args.len() != 1 {
            return err(SP, "a union expression needs exactly one field");
        }
        if args.len() != fields.len() && !union {
            return err(SP, format!("missing fields in initializer of `{}`", self.prog.structs[sid as usize].name));
        }
        if written.is_some() {
            self.adt_targs = written;
        }
        let r = self.adt_value(sid, None, &args, expected, SP)?;
        // (a generic union's instance: its fields are `Copy` or `ManuallyDrop`, as the definition's bounds require
        // (`union U<T: Copy> { a: T }` with `U { a: Rc::new(0u32) }`, E0277))
        if let (true, Ty::Struct(inst)) = (union, r.1) {
            if self.prog.structs[inst as usize].base.is_some() {
                for (_, ft, _) in self.prog.structs[inst as usize].fields.clone() {
                    let md = matches!(ft, Ty::Struct(f) if self.prog.structs[self.prog.base_of(f) as usize].name == "ManuallyDrop");
                    if !md && !self.prog.has_param(ft) && !self.strict_copy(ft) {
                        return err(SP, format!("the trait bound `{}: Copy` is not satisfied (E0277)", self.prog.tn(ft)));
                    }
                }
            }
        }
        Ok(r)
    }

    /// `|..| ..`: the captures packed in a struct. Its type arguments (capture, argument, return types) are set here
    pub(crate) fn closure_value(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Closure(_, inputs, _, _, _) = c else { return err(SP, "internal: expected a closure") };
        let prog = self.prog;
        let hint = self.closure_hint.take();
        let mut bound = self.closure_bound.take();
        // (made where a trait object of a closure trait is expected (`&mut |y| x = Some(y)` for a
        // `&mut dyn FnMut(&isize)`): its text is the bound its body is checked with, its lifetimes from outside)
        if bound.is_none() && self.user_code && self.bk.level >= 2 && regionck::enabled() {
            if let Some(Ty::Dyn(d)) = expected {
                let fn_trait = ["FnOnce", "FnMut", "Fn"].iter().find_map(|k| d.trim_start().strip_prefix(k)).is_some_and(|r| r.trim_start().starts_with('('));
                if fn_trait {
                    bound = Some(Box::new((d.to_string(), self.rg.lt_names.clone(), vec![])));
                }
            }
        }
        let to_prelude = std::mem::take(&mut self.closure_prelude);
        let fn_only = self.closure_fn_only;
        self.closure_fn_only = false;
        let Some(&k) = self.closure_ix.get(&format!("@{}", c as *const Expr as usize)).or_else(|| self.closure_ix.get(&crate::ast_closure::closure_key(c))) else {
            return err(SP, "internal: unknown closure");
        };
        let Some(&cid) = prog.closure_by.get(&(self.root, k)) else {
            return err(SP, "internal: unregistered closure");
        };
        let cd = &prog.closures[&cid];
        let sig0 = &prog.sigs[cid as usize];
        let ng = sig0.generics.len();
        let mut sol: Vec<Option<Ty>> = vec![None; ng];
        for (i, s) in sol.iter_mut().enumerate().take(cd.n_parent) {
            *s = self.env.params.get(i).map(|p| p.1);
        }
        // If the expected type is this closure's own type (`impl Fn` in return position), use it with its type arguments
        let exp_args = match expected {
            Some(Ty::Struct(sid)) if prog.base_of(sid) == cd.sid => Some(prog.structs[sid as usize].args.clone()),
            _ => None,
        };
        // If the expected type is `dyn Fn(A) -> B`, it hints the argument and return types (`fn(A) -> B` too)
        let hint = hint.or_else(|| {
            if let Some(Ty::Fn(ft)) = expected {
                return Some((ft.params.clone(), Some(ft.ret)));
            }
            let s = match expected {
                Some(Ty::Dyn(s)) => s,
                Some(Ty::Ref(_, Ty::Dyn(s))) => *s,
                _ => return None,
            };
            self.dyn_fn_sig(s).ok().map(|(ps, r)| (ps, Some(r)))
        });
        // (a closure whose kind nothing expected decides, returning a mutable borrow of what it captures by
        // reference: it is `FnMut`, and the borrow is through the `&mut self` of a call)
        let own_type = matches!(expected, Some(Ty::Struct(sid)) if prog.base_of(sid) == cd.sid);
        if hint.is_none() && (expected.is_none() || own_type) && bound.is_none() && !to_prelude && self.user_code && self.bk.level >= 2 && regionck::enabled() {
            let Expr::Closure(mv, _, _, body, _) = c else { unreachable!() };
            let by_ref: Vec<&str> = cd.captures.iter().enumerate().filter(|(i, _)| !*mv && !cd.moved.get(*i).copied().unwrap_or(false)).map(|(_, n)| n.as_str()).collect();
            if let Some(sp) = regionck::fnmut_escape(body, &by_ref) {
                return Err(self.at(sp, "captured variable cannot escape `FnMut` closure body (a mutable borrow of what the closure captures is returned)".to_string()));
            }
            // (its body's borrow check finds the rest: a closure it returns that mutates what it captures)
            if !by_ref.is_empty() {
                let mut g = self.insts.lock().unwrap();
                g.kind_free.insert(cid);
                g.fx_log.push(cid);
            }
        }
        let mut caps = vec![];
        let mut copy_caps = vec![];
        for (i, n) in cd.captures.iter().enumerate() {
            let Some((loc, t)) = self.lookup(n) else {
                return err(SP, format!("cannot capture `{n}` here"));
            };
            let by_move = cd.moved.get(i).copied().unwrap_or(false);
            let ct = if by_move { t } else { Ty::Ptr(true, intern(t)) };
            sol[cd.n_parent + i] = Some(ct);
            caps.push((loc, t));
            // (a guard's checks of the arm's bindings (bk_ev) of a closure's captures: only a mutable borrow)
            let gc = std::mem::replace(&mut self.bk.guard_cap, true);
            copy_caps.push(self.bk_capture(c, n, loc, t, by_move));
            self.bk.guard_cap = gc;
        }
        // (a capture held by value (a `move` closure's) of a local not declared `mut`, which the body assigns or borrows
        // mutably: E0594 / E0596 at the mutation; one held by reference is a `&mut` loan the borrow check reports)
        if self.bk_on() && self.bk.fnmut_exprs.contains(&(c as *const Expr as usize)) {
            let Expr::Closure(_, _, _, body, _) = c else { unreachable!() };
            for (i, n) in cd.captures.iter().enumerate() {
                if !cd.moved.get(i).copied().unwrap_or(false) {
                    continue;
                }
                let immutable = self.lookup(n).and_then(|(loc, _)| self.bk.by_loc.get(&loc).copied()).is_some_and(|l| self.bk.locals[l as usize].mutable == Some(false));
                if !immutable {
                    continue;
                }
                if let Some((sp, assign)) = self.owned_mutation(body, n) {
                    let msg = if assign { format!("cannot assign to `{n}`, as it is not declared as mutable (E0594)") } else { format!("cannot borrow `{n}` as mutable, as it is not declared as mutable (E0596)") };
                    return Err(self.at(sp, msg));
                }
            }
        }
        // (a `let _ = s.f;` the body skips (a fake read, which captures nothing): its fields exist, as the lowering of
        // a read of the place would have checked (E0609))
        if !cd.fake_reads.is_empty() {
            let Expr::Closure(_, _, _, body, _) = c else { unreachable!() };
            let mut ws = vec![];
            crate::ast_closure::wild_reads(body, &mut ws);
            for w in &ws {
                if crate::ast_closure::wild_root(w).is_some_and(|n| cd.fake_reads.iter().any(|x| x == n)) {
                    self.wild_place_ty(w)?;
                }
            }
        }
        // (passed where a bound wants `Fn`: a closure that mutates what it captures is not one (E0594 / E0596))
        if fn_only && self.bk.fnmut_exprs.contains(&(c as *const Expr as usize)) {
            let Expr::Closure(_, _, _, body, csp) = c else { unreachable!() };
            let (sp, msg) = match self.fn_mutation(body, &cd.captures) {
                Some((sp, n, true)) => (sp, format!("cannot assign to `{n}`, as it is a captured variable in a `Fn` closure (E0594)")),
                Some((sp, n, false)) => (sp, format!("cannot borrow `{n}` as mutable, as it is a captured variable in a `Fn` closure (E0596)")),
                None => (*csp, "a closure that mutates what it captures is passed where `Fn` is expected (E0596)".to_string()),
            };
            return Err(self.at(sp, msg));
        }
        let mut g = cd.n_parent + cd.captures.len();
        let mut pslots = vec![];
        for (i, p) in inputs.iter().enumerate() {
            if p.1.is_some() {
                continue;
            }
            pslots.push((i, g));
            // The callee's bound first, then the parent's type inference (how it is used: `let f = |x| x; f(1u8)`)
            sol[g] = hint
                .as_ref()
                .and_then(|h| h.0.get(i).copied())
                .filter(|&t| !prog.has_param(t))
                .or_else(|| self.lets.get(&(&p.0 as *const Pat as usize)).copied());
            g += 1;
        }
        // Return type: annotation, then hint, then the body's type inference
        let ri = ng - 1;
        sol[ri] = cd
            .ret_ann
            .map(|t| prog.partial_subst(t, &sol))
            .or(hint.as_ref().and_then(|h| h.1).filter(|&t| !prog.has_param(t)))
            .or_else(|| self.lets.get(&(c as *const Expr as usize)).copied().filter(|&t| !prog.has_param(t)))
            .or(hint.as_ref().and_then(|h| h.1));
        if super::debug_clo() {
            eprintln!("closure k={k} ret_ann={:?} hint={:?} rec={:?} -> {:?}", cd.ret_ann, hint, self.lets.get(&(c as *const Expr as usize)), sol[ri]);
        }
        if let Some(a) = exp_args {
            for (s, a) in sol.iter_mut().zip(a) {
                *s = Some(a);
            }
        }
        // A partly known return type (`Result<?, u8>`) becomes a hint for type inference
        let partial_ret = sol[ri].filter(|&t| prog.has_param(t));
        if partial_ret.is_some() {
            sol[ri] = None;
        }
        // Argument and return types left open come from the body's type inference (`let f = |x| x + k;`)
        if sol.iter().any(|s| s.is_none()) {
            let f = closure_synth(c);
            let sig = FnSig { params: sig0.params.iter().map(|&t| prog.partial_subst(t, &sol)).collect(), ret: Ty::Unit, ..sig0.clone() };
            let env = TyEnv {
                self_sid: self.env.self_sid,
                params: sig0.generics.iter().zip(&sol).filter_map(|(g, s)| Some((g.0.clone(), (*s)?))).collect(),
                self_ty: self.env.self_ty,
            };
            let extra: Vec<(String, Ty)> = cd.captures.iter().zip(&caps).map(|(n, c)| (n.clone(), c.1)).collect();
            let (_, r, ps) = crate::ast_infer::infer_ctx(
                prog,
                false,
                &f.sig.params,
                &f.block,
                &sig,
                &env,
                &extra,
                sol[ri].or(partial_ret),
                Some((self.root, k + 1)),
            );
            if sol[ri].is_none() {
                sol[ri] = r;
            }
            for &(i, g) in &pslots {
                if sol[g].is_none() {
                    sol[g] = ps.get(i + 1).copied().flatten();
                }
            }
            if let Some(&(_i, _)) = pslots.iter().find(|(_, g)| sol[*g].is_none()) {
                return err(SP, "cannot infer the type of this closure parameter; add a type annotation");
            }
        }
        // (a body that ends by diverging (`panic!()`, `loop { .. }`, `return`), with no value to say otherwise: `()`,
        // rustc's fallback for `!`)
        if sol[ri].is_none() && diverging_tail(&c_body(c)) {
            sol[ri] = Some(Ty::Unit);
        }
        // (a bare `|_| panic!()` that nothing else types: rustc's fallback, `!` (in every edition since 1.100), so
        // a bound on the callee's R is checked against `!` where the call resolves (`R: Bar` fails))
        if sol[ri].is_none() && panic_tail(&c_body(c)) {
            sol[ri] = Some(Ty::Never);
        }
        // (a parameter of a size not known: `|a0: [i32]| ..`, as the constructor of `struct Test([i32]);` used as a
        // value is written (E0277))
        if let Some(u) = pslots.iter().filter_map(|&(_, g)| sol[g]).find(|t| matches!(t, Ty::Slice(_) | Ty::Str | Ty::Dyn(_))) {
            return err(expr_start(c), format!("the size for values of type `{}` cannot be known at compilation time (E0277)", self.prog.tn(u)));
        }
        let Some(args) = sol.into_iter().collect::<Option<Vec<Ty>>>() else {
            return err(SP, "cannot infer the return type of this closure; annotate it (`|..| -> T { .. }`)");
        };
        // (one with a parameter annotated with a lifetime, which a cast of a local holding it may coerce)
        if self.user_code && self.bk.level >= 2 && cd.captures.is_empty() && inputs.iter().any(|x| x.1.as_ref().is_some_and(crate::regions::names_lt)) && !self.clo_ann.iter().any(|x| x.0 == cd.sid) {
            self.clo_ann.push((cd.sid, c.clone()));
        }
        // (a closure with no captures where a fn pointer is expected: the address of its fn-pointer instance)
        if matches!(expected, Some(Ty::Fn(_))) && cd.captures.is_empty() {
            // (a parameter annotated with a lifetime, which the type may bind (`let _: fn(&u32) = |_: &'static u32|
            // {};`): checked from here on (rg_closure_fnptr))
            if self.user_code && self.bk.level >= 2 && !self.rg.on && self.rg.eligible && self.inline_depth == 0 && regionck::enabled() && inputs.iter().any(|x| x.1.as_ref().is_some_and(crate::regions::names_lt)) {
                self.rg_late_on();
            }
            let (iid, sig) = {
                let mut g = self.insts.lock().unwrap();
                let iid = g.request_fnptr(prog, cid, args).or_else(|e| err(SP, e))?;
                (iid, g.sig(prog, iid).clone())
            };
            self.requested.push(iid);
            let fr = self.fref(iid, &sig);
            let v = self.b.ins().func_addr(PTR, fr);
            let t = Ty::Fn(crate::types::intern_fn(crate::types::FnTy { params: sig.params.clone(), ret: sig.ret }));
            return Ok((Some(v), t));
        }
        let sid = prog.instantiate(cd.sid, args.clone()).or_else(|e| err(SP, e))?;
        // (the parameters its result is made of: a call's result holds what those arguments borrow)
        if self.user_code && self.bk.level >= 2 && !self.clo_ret_from.iter().any(|x| x.0 == cd.sid) {
            if let Expr::Closure(_, ins, _, body, _) = c {
                let names: Vec<(String, usize)> = ins.iter().enumerate().filter_map(|(i, (p, _))| match p {
                    Pat::Ident(false, _, n, None) => Some((n.clone(), i)),
                    _ => None,
                }).collect();
                let mut from = vec![];
                borrowck::closure_ret_params(body, &names, &mut from, 0);
                self.clo_ret_from.push((cd.sid, from));
            }
        }
        // (made where nothing was expected: a parameter with no type written that is a reference has one region,
        // not one for each call (see call_fn_inner))
        if hint.is_none() && (expected.is_none() || own_type) && self.user_code && self.bk.level >= 2 && regionck::enabled() {
            let refs: Vec<usize> = pslots.iter().filter(|&&(_, g)| matches!(args.get(g), Some(Ty::Ref(..)))).map(|&(i, _)| i).collect();
            if !refs.is_empty() {
                self.hr_lost.retain(|x| x.0 != cd.sid);
                self.hr_lost.push((cd.sid, refs));
                // (`|y| y`: its result's type is its parameter's, regions and all)
                let Expr::Closure(_, ins, _, body, _) = c else { unreachable!() };
                let ret_param = match strip_paren(body) {
                    Expr::Path(None, bp) if bp.segs.len() == 1 => ins.iter().position(|(p, _)| matches!(p, Pat::Ident(_, _, n, None) if *n == bp.segs[0].name)),
                    _ => None,
                };
                self.clo_ret_param.retain(|x| x.0 != cd.sid);
                if let Some(&(_, g)) = ret_param.and_then(|i| pslots.iter().find(|x| x.0 == i)) {
                    self.clo_ret_param.push((cd.sid, g));
                }
                // (what its body stores of its parameters in what it captures: `|input| x.0 = input`)
                let names: Vec<(String, usize)> = pslots.iter().filter_map(|&(i, g)| match &ins.get(i)?.0 {
                    Pat::Ident(_, _, n, None) => Some((n.clone(), g)),
                    _ => None,
                }).collect();
                let link = self.clo_ret_param.iter().find(|x| x.0 == cd.sid).map(|x| x.1);
                // (what its body requires of its parameters (`let y: &'static u32 = x;`): checked from here on, so
                // its calls' arguments are)
                let mut reqs = vec![];
                regionck::closure_reqs(body, &names, &mut reqs);
                if !reqs.is_empty() && !self.rg.on && self.rg.eligible && self.inline_depth == 0 {
                    self.rg_late_on();
                }
                self.rg_closure_stores(cd.sid, &args, link, body, &names, &cd.captures);
                self.rg_closure_reqs(cd.sid, &args, &reqs);
            }
        }
        // (a body with a `let` whose annotation writes `'static`: the function is checked from here on, as for its own
        // `let x: &'static T`, so what the closure captures has its trees)
        let ann_static = self.user_code && self.bk.level >= 2 && !cd.captures.is_empty() && matches!(c, Expr::Closure(_, _, _, b, _) if crate::regions::annotates_static(b));
        // (or one whose body writes a lifetime (`let y: &'static u32 = &c;`, `W::<&'a u32>`), with nothing else that
        // asks for its body to be checked: it is, with the function's regions)
        let writes_lt = !ann_static
            && self.user_code
            && self.bk.level >= 2
            && !to_prelude
            && bound.is_none()
            && !inputs.iter().any(|x| x.1.is_some())
            && regionck::enabled()
            && matches!(c, Expr::Closure(_, _, _, b, _) if crate::regions::writes_lifetime(b));
        // (or a parameter annotated with a lifetime where a bound gives the signature (`|x: &'static u32|` for
        // `F: Fn(&u32)`): rg_closure_sig relates the two)
        let ann_lt = bound.is_some() && self.user_code && self.bk.level >= 2 && !to_prelude && inputs.iter().any(|x| x.1.as_ref().is_some_and(crate::regions::names_lt));
        if (ann_static || writes_lt || ann_lt) && !self.rg.on && self.rg.eligible && self.inline_depth == 0 && regionck::enabled() {
            self.rg_late_on();
        }
        // (the region signature its body is checked with: from a bound not the prelude's, else from annotated
        // parameters)
        if self.user_code && self.bk.level >= 2 && !to_prelude && (bound.is_some() || inputs.iter().any(|x| x.1.is_some()) || (self.rg_on() && (!cd.captures.is_empty() || writes_lt))) && regionck::enabled() {
            if let Some((ps, r)) = self.closure_sig(Ty::Struct(sid)) {
                self.rg_closure_sig(c, cid, &ps, r, bound.map(|b| *b));
            }
        }
        // The body is checked and compiled even if it is never called (--gc-sections removes it when unused)
        {
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(prog, cid, args).or_else(|e| err(SP, e))?;
            drop(g);
            self.requested.push(iid);
        }
        let st = Ty::Struct(sid);
        if let Expr::Closure(.., csp) = c {
            if !self.bk.clo_at.iter().any(|x| x.0 == st) {
                self.bk.clo_at.push((st, *csp));
            }
        }
        // (a closure that moves what it captures out of itself is `FnOnce`)
        if self.bk.fnonce_sure.contains(&(c as *const Expr as usize)) && !self.bk.fnonce.contains(&st) {
            self.bk.fnonce.push(st);
        }
        // (a closure that mutates its captures is `FnMut`; one that holds a `&mut` of them is not `Copy`)
        if self.bk.fnmut_exprs.contains(&(c as *const Expr as usize)) && !self.bk.fnonce_exprs.contains(&(c as *const Expr as usize)) {
            if !self.bk.fnmut.contains(&st) {
                self.bk.fnmut.push(st);
            }
            let unique = self.bk.cap_loans.get(&(c as *const Expr as usize)).is_some_and(|ls| ls.iter().any(|&l| self.bk.loans[l as usize].mutable));
            if unique {
                self.bk.ty_info.insert(st, (false, true));
            }
        }
        self.rg_closure(c, st);
        let a = self.temp(st);
        let fields = prog.structs[sid as usize].fields.clone();
        for (i, ((loc, t), (_, ft, off))) in caps.into_iter().zip(fields).enumerate() {
            if cd.moved.get(i).copied().unwrap_or(false) {
                let p = match loc {
                    Loc::Var(v) => Place::Var(v, t),
                    Loc::Zst(_) => Place::Zst(t),
                    l => Place::Mem(self.loc_addr(l).unwrap(), 0, t),
                };
                let (v, _) = self.read(p);
                self.write(Place::Mem(a, off as i32, ft), v);
                // (a copy of which the body uses only `Copy` parts (bk_capture): its other parts are emptied, so that
                // dropping the closure frees nothing, and the variable is not moved)
                if copy_caps.get(i).copied().unwrap_or(false) {
                    let dst = self.addr_plus(a, off as i32);
                    self.zero_drop_parts(dst, t);
                    continue;
                }
                if self.needs_drop(t) {
                    // (a capture of the enclosing closure moved into this one: as for moving it out, the
                    // enclosing closure's slot becomes empty)
                    // (the cap binding is the first one of that name: a later `let` shadows it)
                    let first = self.scopes.iter().flat_map(|s| s.iter()).find(|b| b.0 == cd.captures[i]).map(|b| b.1);
                    if let Some(c) = self.caps.iter().find(|c| c.0 == cd.captures[i] && first == Some(loc)) {
                        if c.2 || !self.zeroable(t) {
                            return err(SP, "moving a captured value out of a closure is not supported");
                        }
                        if let Some(dst) = self.loc_addr(loc) {
                            self.zero(dst, t);
                        }
                    }
                    self.mark_moved_loc(loc);
                }
            } else {
                let addr = match loc {
                    Loc::Zst(_) => self.b.ins().iconst(PTR, 0),
                    Loc::Var(_) => {
                        return err(SP, "internal: captured variable is not in memory");
                    }
                    l => self.loc_addr(l).unwrap(),
                };
                self.b.ins().store(mf(), addr, a, off as i32);
            }
        }
        Ok((Some(a), st))
    }

    /// The first assignment to, `&mut` of, or `&mut self` method call of what the capture n itself holds (not through a
    /// reference it holds: `t.0.0 = x` with `t.0: &mut S` mutates no part of t): where, whether an assignment
    fn owned_mutation(&self, e: &Expr, n: &str) -> Option<(Sp, bool)> {
        let owned = |l: &Expr| self.bk_rooted_at_pub(strip_paren(l), n).is_some_and(|(p, t)| p.owned_len_pub() == p.proj.len() && !matches!(t, Some(Ty::Ref(..))));
        match e {
            Expr::Assign(l, _) | Expr::AssignOp(_, l, _, _) => {
                let mut b: &Expr = l;
                while let Expr::Index(x, _, _) = strip_paren(b) {
                    b = x;
                }
                if owned(b) {
                    return Some((borrowck::pos(e), true));
                }
            }
            Expr::Ref(true, l) => {
                if owned(l) {
                    return Some((borrowck::pos(e), false));
                }
            }
            Expr::MethodCall(r, m, ..) => {
                if owned(r) {
                    let mut_self = match self.bk_rooted_at_pub(strip_paren(r), n) {
                        Some((_, Some(Ty::Struct(sid)))) => self.prog.self_method_of(sid, m).or_else(|| self.prog.self_default_method(sid, m)).is_some_and(|id| {
                            let sg = &self.prog.sigs[id as usize];
                            sg.has_self && matches!(sg.params.first(), Some(Ty::Ref(true, _)))
                        }),
                        _ => false,
                    };
                    if mut_self {
                        return Some((borrowck::pos(e), false));
                    }
                }
            }
            _ => {}
        }
        // (a binding of the same name shadows the capture where it is in scope: `let mut x = x; x += 1;`)
        let binds = |p: &Pat| {
            let mut v = vec![];
            crate::ast_closure::pat_idents(p, &mut v);
            v.iter().any(|x| x == n)
        };
        let either = |a: Option<(Sp, bool)>, b: &mut dyn FnMut() -> Option<(Sp, bool)>| a.or_else(b);
        match e {
            Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => return self.owned_mutation_block(b, n),
            Expr::Closure(_, ps, _, body, _) => return if ps.iter().any(|(p, _)| binds(p)) { None } else { self.owned_mutation(body, n) },
            Expr::Match(x, arms) => {
                return either(self.owned_mutation(x, n), &mut || {
                    arms.iter().filter(|a| !binds(&a.pat)).find_map(|a| a.guard.as_ref().and_then(|g| self.owned_mutation(g, n)).or_else(|| self.owned_mutation(&a.body, n)))
                });
            }
            Expr::For(_, p, it, b) => return either(self.owned_mutation(it, n), &mut || if binds(p) { None } else { self.owned_mutation_block(b, n) }),
            Expr::If(c, b, _) | Expr::While(_, c, b) if matches!(&**c, Expr::Let(..)) => {
                let Expr::Let(p, x) = &**c else { unreachable!() };
                let els = if let Expr::If(_, _, els) = e { els.as_deref() } else { None };
                return either(self.owned_mutation(x, n), &mut || {
                    if binds(p) { None } else { self.owned_mutation_block(b, n) }.or_else(|| els.and_then(|x| self.owned_mutation(x, n)))
                });
            }
            _ => {}
        }
        let mut found = None;
        crate::ast_closure::each_child_pub(e, &mut |c| {
            if found.is_none() {
                found = self.owned_mutation(c, n);
            }
        });
        found
    }

    /// owned_mutation in a block's statements: a `let` of the same name ends the search
    fn owned_mutation_block(&self, b: &crate::parse::Block, n: &str) -> Option<(Sp, bool)> {
        for st in &b.stmts {
            match st {
                crate::parse::Stmt::Let(p, _, init, els, _) => {
                    if let Some(f) = init.as_ref().and_then(|x| self.owned_mutation(x, n)).or_else(|| els.as_ref().and_then(|b| self.owned_mutation_block(b, n))) {
                        return Some(f);
                    }
                    let mut v = vec![];
                    crate::ast_closure::pat_idents(p, &mut v);
                    if v.iter().any(|x| x == n) {
                        return None;
                    }
                }
                crate::parse::Stmt::Expr(x, _) => {
                    if let Some(f) = self.owned_mutation(x, n) {
                        return Some(f);
                    }
                }
                crate::parse::Stmt::Item(_) => {}
            }
        }
        None
    }

    /// The first assignment to, `&mut` of, or `&mut self` method call of a captured variable in a closure's body (a
    /// closure in it too): where, which, whether an assignment
    fn fn_mutation(&self, e: &Expr, caps: &[String]) -> Option<(Sp, String, bool)> {
        let hit = |l: &Expr| borrowck::root_local(strip_paren(l)).filter(|n| caps.iter().any(|c| c == n)).map(|n| n.to_string());
        match e {
            Expr::Assign(l, _) | Expr::AssignOp(_, l, _, _) => {
                if let Some(n) = hit(l) {
                    return Some((borrowck::pos(e), n, true));
                }
            }
            Expr::Ref(true, l) => {
                if let Some(n) = hit(l) {
                    return Some((borrowck::pos(e), n, false));
                }
            }
            Expr::MethodCall(r, m, ..) => {
                if let Some(n) = hit(r) {
                    // (of the receiver's own type, as owned_mutation: `w.0.touch()` is the field's `touch`, not w's)
                    let mut_self = match self.peek(strip_paren(r)) {
                        Some(Ty::Struct(sid)) => self.prog.self_method_of(sid, m).or_else(|| self.prog.self_default_method(sid, m)).is_some_and(|id| {
                            let sg = &self.prog.sigs[id as usize];
                            sg.has_self && matches!(sg.params.first(), Some(Ty::Ref(true, _)))
                        }),
                        _ => false,
                    };
                    if mut_self {
                        return Some((borrowck::pos(e), n, false));
                    }
                }
            }
            _ => {}
        }
        let mut found = None;
        crate::ast_closure::each_child_pub(e, &mut |c| {
            if found.is_none() {
                found = self.fn_mutation(c, caps);
            }
        });
        found
    }

    /// Makes a `&T` (address v) into a `&dyn tr` fat pointer
    /// A trait object of the text `got` may become one of `want` keeping its value: the same trait, and every auto trait
    /// `want` has is one `got` has, written or by its trait's supertraits (`trait Job: Send`: a `dyn Job` is a `dyn Job +
    /// Send`). An object of auto traits alone is the set of them (`dyn Send + Sync` is a `dyn Sync`)
    pub(crate) fn dyn_coerces(&self, got: &str, want: &str) -> bool {
        let ((gc, gs, gy), (wc, ws, wy)) = (dyn_norm(got), dyn_norm(want));
        let gives = |a: &str| gc.is_some_and(|c| self.prog.tdb.gives_auto(self.prog, c, a) == Some(true));
        gc == wc && (!ws || gs || gives("Send")) && (!wy || gy || gives("Sync"))
    }

    pub(crate) fn to_dyn(&mut self, v: Value, t: Ty, tr: &str, span: Sp) -> R<Value> {
        // (a `str` / `[T]` has no size for a vtable to give: `"test" as &dyn Any`, a `&[u8]` as a `&dyn Debug` (E0277,
        // cast/trait-object-size-error-14366.rs); a `dyn` itself is an upcast)
        if matches!(t, Ty::Str | Ty::Slice(_)) {
            return err(span, format!("the size for values of type `{}` cannot be known at compilation time (E0277)", self.prog.tn(t)));
        }
        // (`dyn Tr + Send` / `+ Sync`: the value's type must have them (E0277), as the object says it does; the vtable
        // is the trait's)
        let (tr, mut send, mut sync) = crate::types::dyn_autos(tr);
        // (`dyn Send` / `dyn Sync` alone: the auto trait is the object's trait)
        send |= tr == "Send";
        sync |= tr == "Sync";
        if self.user_code && send && super::p2_call::not_send(self.prog, t) {
            return err(span, format!("`{}` cannot be sent between threads safely (E0277)", self.prog.tn(super::p2_call::auto_culprit(self.prog, t, false))));
        }
        if self.user_code && sync && super::p2_call::not_sync(self.prog, t) {
            return err(span, format!("`{}` cannot be shared between threads safely (E0277)", self.prog.tn(super::p2_call::auto_culprit(self.prog, t, true))));
        }
        let mut fns = vec![];
        if let (Some((_, bps, _)), Ty::Fn(f)) = (crate::ast_summary::parse_fn_bound(tr), t) {
            // Function pointer: a prelude shim that calls it from the data (the place holding the function pointer)
            if bps.len() != f.params.len() {
                return err(span, format!("expected a function with {} arguments", bps.len()));
            }
            let Some(&shim) = self.prog.index.get(&format!("__exr_fnptr{}", f.params.len())) else {
                return err(span, "fn pointers with more than 3 arguments cannot be used as `dyn Fn`");
            };
            let args: Vec<Ty> = f.params.iter().copied().chain([f.ret]).collect();
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(self.prog, shim, args).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(iid);
            fns.push(iid);
        } else if crate::ast_summary::parse_fn_bound(tr).is_some() {
            let cid = match t {
                Ty::Struct(sid) => self.prog.closure_of_struct.get(&self.prog.base_of(sid)).copied(),
                _ => None,
            };
            // (a placeholder bounded by `Fn(..)` / `FnMut(..)` / `FnOnce(..)` (zustc check of a generic fn nothing
            // calls, `Box::new(f)` as a `Box<dyn FnMut(bool)>`): it has no body for the vtable's call to point at, and
            // the instance is only checked, never run. Its entry is left empty, as a method dyn cannot call is)
            let fn_placeholder = matches!(t, Ty::Struct(sid) if self.prog.is_placeholder(sid)
                && self.prog.structs[sid as usize].placeholder.as_ref().is_some_and(|bs| bs.iter().any(|b| crate::ast_summary::parse_fn_bound(b).is_some())));
            match cid {
                Some(cid) => {
                    let Ty::Struct(sid) = t else { unreachable!() };
                    let args = self.prog.structs[sid as usize].args.clone();
                    let mut g = self.insts.lock().unwrap();
                    let iid = g.request(self.prog, cid, args).or_else(|e| err(span, e))?;
                    drop(g);
                    self.requested.push(iid);
                    fns.push(iid);
                }
                None if fn_placeholder => fns.push(u32::MAX),
                None => return err(span, format!("`{}` is not a closure", self.prog.tn(t))),
            }
        } else {
            // (`dyn Foo<isize>`: the trait's own parameters are the arguments written)
            let tr0 = tr;
            let (trn, targs) = self.dyn_trait_args(tr);
            let tr = trn.as_str();
            // (`dyn Send` / `dyn Sync`: an auto trait, every type of the program's has it; no methods)
            let auto = matches!(tr, "Send" | "Sync");
            if !auto && !self.prog.traits.contains_key(tr) {
                return err(span, format!("cannot find trait `{tr}`"));
            }
            let decls = if auto { vec![] } else { self.prog.dyn_methods(tr) };
            // (a trait of the program's: by the trait solver (an impl for a reference or an array, a blanket impl with
            // where clauses); std's: implements, unless the solver knows better (`()`'s impls))
            let own = self.prog.tdb.user.contains(tr);
            let no = if own || t == Ty::Unit {
                !self.prog.has_param(t) && self.prog.tdb.holds(self.prog, t, tr) == crate::traits::Ans::No
            } else {
                !self.prog.implements(t, tr)
            };
            if !auto && no {
                return err(span, format!("the trait `{tr}` is not implemented for `{}`", self.prog.tn(t)));
            }
            // (a trait of the program's with its arguments, by the trait solver: `Struct` to `dyn Trait<isize>` needs
            // `Struct: Trait<isize>`, not some `Trait<&str>`)
            if !auto && self.prog.tdb.user.contains(tr) && !self.prog.has_param(t) && !self.prog.names_placeholder(t, 0) {
                let b = tr0.split('+').next().unwrap_or(tr0).trim();
                if self.prog.tdb.holds(self.prog, t, b) == crate::traits::Ans::No {
                    return err(span, format!("the trait bound `{}: {}` is not satisfied (E0277)", self.prog.tn(t), crate::traits::bound_text(b)));
                }
            }
            // (E0191: every associated type of the trait is bound in the dyn type: `dyn Iterator<Item = u32>`)
            {
                let full = tr0;
                let mut missing: Vec<&str> = self.prog.assoc_bounds.keys().filter(|(t, _)| t == tr).map(|(_, n)| n.as_str()).filter(|n| {
                    !full.split(|c: char| c == '<' || c == ',').any(|a| a.trim().split('=').next().is_some_and(|x| x.trim() == *n) && a.contains('='))
                }).collect();
                missing.sort();
                if let Some(n) = missing.first() {
                    return err(span, format!("the value of the associated type `{n}` in `{tr}` must be specified (E0191)"));
                }
            }
            if let Some(m) = dyn_incompatible(self.prog, tr, &decls, false) {
                return err(span, m);
            }
            for &d in &decls {
                let ds = &self.prog.sigs[d as usize];
                // (a method dyn cannot call (`self` by value, no `self`, `where Self: Sized`, generics of its own) has
                // no entry: it is an error only where it is called through dyn. `self: Box<Self>` takes the box)
                let recv = match ds.params.first() {
                    Some(Ty::Ref(m, _)) if ds.has_self => [Ty::Ref(*m, intern(t))],
                    Some(Ty::Box(_)) if ds.has_self => [Ty::Box(intern(t))],
                    _ => {
                        fns.push(u32::MAX);
                        continue;
                    }
                };
                if ds.generics.len() > 1 + targs.len() {
                    fns.push(u32::MAX);
                    continue;
                }
                let targs = targs.clone();
                // (`&0 as &dyn Display`: a type formatted by the compiler itself goes through a prelude shim that
                // formats it with `{}` / `{:?}`)
                if matches!(tr, "Display" | "Debug") {
                    let shim = if tr == "Display" { "__exr_dyn_display" } else { "__exr_dyn_debug" };
                    if let Some(&sid) = self.prog.index.get(shim) {
                        let (fid, _) = self.resolve_call(sid, &recv, vec![Some(t)], None, span)?;
                        fns.push(fid);
                        continue;
                    }
                }
                let mut sol = vec![None; ds.generics.len()];
                for (k, a) in targs.iter().enumerate() {
                    if let Some(slot) = sol.get_mut(1 + k) {
                        *slot = Some(*a);
                    }
                }
                let (mut fid, _) = self.resolve_call(d, &recv, sol, None, span)?;
                // A method of a generic impl (`impl<T> Tr for W<T>`) is instantiated further
                if (fid as usize) < self.prog.sigs.len() && self.prog.sigs[fid as usize].is_generic() {
                    let g = self.prog.sigs[fid as usize].generics.len();
                    let mut sol = vec![None; g];
                    if let Some(st) = self.prog.sigs[fid as usize].self_ty {
                        self.prog.match_ty(st, t, &mut sol);
                    }
                    fid = self.resolve_call(fid, &recv, sol, None, span)?.0;
                }
                fns.push(fid);
            }
        }
        // The last slot is the drop glue (used by the drop of `Box<dyn ..>`)
        let de = self.vtable_drop_entry(t);
        fns.push(de);
        // (then the size and the alignment: a struct with this type as its unsized tail places the tail by them,
        // size_of_val reads them)
        let (size, align) = self.prog.size_align(t);
        if size > VT_LIT_MAX {
            return err(span, format!("`{}` is too large for a trait object ({size} bytes)", self.prog.tn(t)));
        }
        fns.push(VT_LIT | size);
        fns.push(VT_LIT | align);
        let k = match self.vtables.iter().position(|x| *x == fns) {
            Some(k) => k,
            None => {
                self.vtables.push(fns);
                self.vtables.len() - 1
            }
        };
        let vt = self.vtable_sym(k as u32);
        let fat = self.temp(Ty::Ref(false, intern(Ty::Dyn(crate::types::intern_str(tr)))));
        self.b.ins().store(mf(), v, fat, 0);
        self.b.ins().store(mf(), vt, fat, 8);
        Ok(fat)
    }

    /// Calls the vtable's idx-th entry. fat is the address of the `&dyn`'s (data, vtable)
    pub(crate) fn dyn_call(&mut self, fat: Value, idx: usize, params: &[Ty], ret: Ty, args: &[&Expr], span: Sp) -> R<V> {
        self.indirect_call(Indirect::Vtable(fat, idx), params, ret, args, span)
    }

    /// A call through a function pointer / vtable
    pub(crate) fn indirect_call(&mut self, callee: Indirect, params: &[Ty], ret: Ty, args: &[&Expr], span: Sp) -> R<V> {
        if params.len() != args.len() {
            return err(span, format!("expected {} arguments, found {}", params.len(), args.len()));
        }
        let mut sig = self.prog.sigs[0].clone();
        let env = matches!(callee, Indirect::Vtable(..));
        sig.params = env.then_some(Ty::Ptr(false, intern(Ty::U8))).into_iter().chain(params.iter().copied()).collect();
        sig.ret = ret;
        sig.is_main = false;
        sig.is_extern = false;
        let data = match callee {
            Indirect::Vtable(fat, _) => Some(self.b.ins().load(PTR, mf(), fat, 0)),
            Indirect::Ptr(_) => None,
        };
        let mut vals = vec![];
        let mut reborrows = vec![];
        for (a, &pt) in args.iter().zip(params) {
            let (mut v, mut t) = self.expr(a, Some(pt))?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            // (without the region check, the reborrow by the argument's place, for a result that borrows from it
            // (Bk::fnptr_reborrows): `let r = f(x); *x = 1; use(r)` with `f: fn(&i32) -> &i32` is E0506)
            if let Ty::Ref(m, _) = pt {
                if let Some(ln) = self.bk_reborrow_arg(a, m, false) {
                    if !self.rg.on {
                        reborrows.push((vals.len(), ln));
                    }
                }
            }
            // (`&Box<T>` / `&&T` for a `&T` argument: deref coercion)
            while let (Ty::Ref(_, _), Ty::Ref(m, inner @ (Ty::Ref(..) | Ty::Box(_))), Some(val)) = (pt, t, v) {
                if t == pt || self.prog.has_param(pt) {
                    break;
                }
                v = Some(if inner.is_fat() { val } else { self.b.ins().load(PTR, mf(), val, 0) });
                t = match inner {
                    Ty::Box(x) => Ty::Ref(m, x),
                    i => *i,
                };
            }
            self.same(SP, t, pt)?;
            vals.push(v);
        }
        self.bk.fnptr_reborrows = reborrows;
        let fp = match callee {
            Indirect::Vtable(fat, idx) => {
                let vt = self.b.ins().load(PTR, mf(), fat, 8);
                self.b.ins().load(PTR, mf(), vt, 8 * idx as i32)
            }
            Indirect::Ptr(fp) => fp,
        };
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp_fresh(ret);
            cv.push(a);
            Some(a)
        } else {
            None
        };
        cv.extend(data);
        for (v, &pt) in vals.into_iter().zip(params) {
            if let Some(v) = v {
                self.push_arg(&mut cv, v, pt);
            }
        }
        let sr = self.b.import_signature(clif_sig(&sig));
        let call = self.b.ins().call_indirect(sr, fp, &cv);
        if ret == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(self.dead());
        }
        if let Some(a) = sret {
            return Ok((Some(a), ret));
        }
        let r = self.b.inst_results(call).first().copied();
        Ok((r, ret))
    }

    /// The trait of `dyn Tr<A, B, Assoc = C>` and the types of its own arguments (the bindings are left out)
    pub(crate) fn dyn_trait_args(&mut self, tr: &str) -> (String, Vec<Ty>) {
        // (the object's trait, not the auto traits it keeps: `Tr<u8> + Send`)
        let tr = crate::types::dyn_autos(tr).0;
        let Some((n, rest)) = tr.split_once('<') else { return (tr.trim().to_string(), vec![]) };
        let inner = rest.trim_end().strip_suffix('>').unwrap_or(rest);
        let mut args = vec![];
        // (the arguments split at the commas outside brackets)
        let mut parts: Vec<String> = vec![];
        let (mut d, mut cur) = (0i32, String::new());
        for c in inner.chars() {
            match c {
                '<' | '(' | '[' => d += 1,
                '>' | ')' | ']' => d -= 1,
                ',' if d == 0 => {
                    parts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(c);
        }
        parts.push(cur);
        for a in &parts {
            let a = a.trim();
            if a.is_empty() || a.contains('=') {
                continue;
            }
            if let Ok(Expr::Cast(_, pt)) = crate::parse::parse_expr(&format!("0 as {a}")) {
                if let Ok(t) = self.parse_ty(&pt) {
                    args.push(t);
                }
            }
        }
        (n.trim().to_string(), args)
    }

    /// the associated type bindings of `dyn Tr<A, Out = u32>`: (name, type)
    pub(crate) fn dyn_bindings(&mut self, tr: &str) -> Vec<(String, Ty)> {
        let tr = crate::types::dyn_autos(tr).0;
        let Some((_, rest)) = tr.split_once('<') else { return vec![] };
        let inner = rest.trim_end().strip_suffix('>').unwrap_or(rest);
        let mut out = vec![];
        let (mut d, mut cur) = (0i32, String::new());
        let mut parts = vec![];
        for c in inner.chars() {
            match c {
                '<' | '(' | '[' => d += 1,
                '>' | ')' | ']' => d -= 1,
                ',' if d == 0 => {
                    parts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(c);
        }
        parts.push(cur);
        for a in &parts {
            let Some((n, t)) = a.split_once('=') else { continue };
            if let Ok(Expr::Cast(_, pt)) = crate::parse::parse_expr(&format!("0 as {}", t.trim())) {
                if let Ok(t) = self.parse_ty(&pt) {
                    out.push((n.trim().to_string(), t));
                }
            }
        }
        out
    }

    /// A call of `&dyn Fn(A) -> B` (vtable entry 0 is the closure body)
    pub(crate) fn dyn_fn_call(&mut self, fat: Value, s: &str, args: &[&Expr], span: Sp) -> R<V> {
        let (ps, r) = self.dyn_fn_sig(s).or_else(|e| err(span, e))?;
        self.dyn_call(fat, 0, &ps, r, args, span)
    }

    /// A call through a trait object to a method of an impl for the object (`impl dyn Tr`, `impl X for dyn Tr`, or a
    /// default of a trait the object implements): id with the pair as its `&self` / `&mut self` (m), whose type st
    /// is the receiver's object. As a call through the vtable checks it: an unsafe method outside `unsafe` (E0133), a
    /// `&mut self` method through a shared reference (E0596), the receiver's borrow for the borrow check, its tree for
    /// the region check. late: the callee requires 'static, so the region check starts here; need_static: the impl
    /// is for `dyn Tr + 'static`, which the receiver's object must outlive
    #[allow(clippy::too_many_arguments)]
    fn dyn_impl_call(&mut self, id: u32, m: bool, st: Ty, mc: &Expr, recv_ty: Option<Ty>, fat: Value, late: bool, need_static: bool, expected: Option<Ty>) -> R<V> {
        let Expr::MethodCall(recv, _, tf, margs, _) = mc else { return err(SP, "internal: expected a method call") };
        self.check_unsafe_call(id, super::borrowck::pos(mc))?;
        if let (Some(rt), true, true) = (recv_ty, self.user_code, m) {
            if matches!(rt, Ty::Ref(false, _) | Ty::Ptr(false, _)) {
                return err(SP, "cannot borrow data in a `&` reference as mutable (E0596)".to_string());
            }
        }
        if late && !self.rg.on && self.rg.eligible && self.inline_depth == 0 {
            self.rg_late_on();
        }
        self.rg_dyn_recv(mc, need_static);
        // (the receiver is borrowed by `&self` / `&mut self`, and a result that borrows from self holds the loan,
        // as a vtable call's does: `b.get()` of a local `Box<dyn Tr>` returned is E0515)
        if self.bk_on() {
            let flows = self.bk_ret_from(id, 0);
            let ev = self.bk_recv_ev(recv, Ty::Ref(m, intern(Ty::Unit)), flows);
            self.bk_push(ev);
        }
        let args: Vec<&Expr> = margs.iter().collect();
        let tf = (!tf.is_empty()).then_some(tf.as_slice());
        // (the type expected of the result: a parameter only it names is solved from it (`let v: u8 = x.make()`))
        let r = self.call_fn(id, Some((fat, st)), &args, tf, expected, SP);
        // (the result's tree, for the expression: `b.get()` returned)
        self.rg_took_call(mc);
        r
    }

    /// `x.m(..)` (x: `&dyn Trait`)
    /// `recv.m(..)` through the vtable of `dyn tr`. `recv_ty` is the receiver's type as evaluated, when the caller has
    /// it: a `&mut self` method through a shared `&dyn` / `*const dyn` is E0596
    pub(crate) fn dyn_method_call(&mut self, fat: Value, tr: &str, mc: &Expr) -> R<V> {
        self.dyn_method_call_on(fat, tr, mc, None, None)
    }

    pub(crate) fn dyn_method_call_on(&mut self, fat: Value, tr: &str, mc: &Expr, recv_ty: Option<Ty>, expected: Option<Ty>) -> R<V> {
        let Expr::MethodCall(_, name, _, margs, _) = mc else { return err(SP, "internal: expected a method call") };
        let tr_full = tr;
        let (trn, targs) = self.dyn_trait_args(tr);
        let binds = if tr.contains('=') { self.dyn_bindings(tr) } else { vec![] };
        let tr = trn.as_str();
        let decls = self.prog.dyn_methods(tr);
        // (the trait `Tr::m(x)` names: not a method of the object's own trait of the same name)
        let named = self.explicit_traits.get(&(mc as *const Expr as usize)).cloned();
        let of_named = |d: u32| named.as_ref().is_none_or(|n| self.prog.names[d as usize].rsplit_once("::").is_some_and(|(t, _)| t == n));
        let Some(idx) = decls.iter().position(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(name.as_str()) && of_named(d)) else {
            // (not in the vtable: an inherent method of `impl dyn Tr { .. }`, or one of another trait's impl for the
            // object, `impl X for dyn Tr`: `__dyn_Tr::m`, called with the pair as its `&self`)
            let base = tr.rsplit("::").next().unwrap_or(tr);
            // (`E3::pick(x)` takes only E3's impl's method, not an inherent one of `impl dyn O3` (E3's default then))
            let of_trait = |t: &str, n: &str| t.split('<').next().unwrap_or(t).trim().rsplit("::").next() == n.rsplit("::").next();
            let mine = |id: u32| named.as_ref().is_none_or(|n| self.prog.sigs[id as usize].impl_trait.as_deref().is_some_and(|t| of_trait(t, n)));
            // (of one name in `impl dyn O` and in `impl Ext for dyn O`, the others are overloads: a named trait's
            // takes its impl's, an unnamed call the inherent one, as rustc's lookup does)
            let key = format!("__dyn_{base}::{name}");
            let cands: Vec<u32> = match self.prog.overloads.get(&key) {
                Some(cs) => cs.clone(),
                None => self.prog.index.get(&key).copied().into_iter().collect(),
            };
            let pick = if named.is_some() {
                cands.iter().copied().find(|&c| mine(c))
            } else {
                cands.iter().copied().find(|&c| self.prog.sigs[c as usize].impl_trait.is_none()).or(cands.first().copied())
            };
            if let Some(id) = pick {
                let s = &self.prog.sigs[id as usize];
                if let (true, Some(&Ty::Ref(m, _))) = (s.has_self, s.params.first()) {
                    // (the receiver as the object it is, so the impl's parameters are solved from its arguments:
                    // `dyn ObjectTrait<u8>` for `impl<T> MyTrait<T> for dyn ObjectTrait<T>`)
                    let st = Ty::Ref(m, intern(Ty::Dyn(crate::types::intern_str(tr_full))));
                    let late = s.regions.as_deref().is_some_and(Self::callee_needs_static);
                    return self.dyn_impl_call(id, m, st, mc, recv_ty, fat, late, false, expected);
                }
            }
            // (a default method of a trait the object implements, `impl MyTrait for dyn Tr {}`: with Self the object.
            // An impl whose object writes no lifetime is for `dyn Tr + 'static`, which the receiver's must then be)
            let obj = Ty::Dyn(crate::types::intern_str(tr_full));
            // (the trait `Tr::m(x)` names, else every trait the object implements with a default of the name: more
            // than one is ambiguous (E0034), as rustc's method lookup finds)
            let mut dflts: Vec<(String, u32)> = self
                .prog
                .dyn_impl_traits(tr_full)
                .into_iter()
                .filter(|t2| named.as_ref().is_none_or(|n| *t2 == n) && self.prog.implements(obj, t2))
                .filter_map(|t2| {
                    self.prog.traits.get(t2)?.iter()
                        .copied()
                        .find(|&d| {
                            let s = &self.prog.sigs[d as usize];
                            s.has_body && s.has_self && matches!(s.params.first(), Some(Ty::Ref(..))) && self.prog.names[d as usize].rsplit("::").next() == Some(name.as_str())
                        })
                        .map(|d| (t2.clone(), d))
                })
                .collect();
            if dflts.len() > 1 {
                return err(SP, "multiple applicable items in scope (E0034)");
            }
            if let Some((t2, d)) = dflts.pop() {
                let Some(&Ty::Ref(m, _)) = self.prog.sigs[d as usize].params.first() else { return err(SP, "internal: a default method's receiver") };
                let need_static = !self.prog.dyn_impl_writes_lt(&t2, tr_full);
                return self.dyn_impl_call(d, m, Ty::Ref(m, intern(obj)), mc, recv_ty, fat, need_static, need_static, expected);
            }
            return err(SP, format!("no method named `{name}` found for `dyn {tr}`"));
        };
        let ds = &self.prog.sigs[decls[idx] as usize];
        if let (Some(rt), true) = (recv_ty, self.user_code) {
            // (a method `where Self: Sized` is not in the vtable)
            if ds.generics.first().is_some_and(|g| g.1.iter().any(|b| b == "Sized")) {
                return err(SP, format!("the `{name}` method cannot be invoked on `{}`", self.prog.tn(rt)));
            }
            if matches!(rt, Ty::Ref(false, _) | Ty::Ptr(false, _)) && matches!(ds.params.first(), Some(Ty::Ref(true, _))) {
                return err(SP, "cannot borrow data in a `&` reference as mutable (E0596)".to_string());
            }
        }
        // (a method the vtable has no entry for: `self` by value / no `self` / generics of its own)
        let by_box = matches!(ds.params.first(), Some(Ty::Box(_)));
        if !ds.has_self || !matches!(ds.params.first(), Some(Ty::Ref(..) | Ty::Box(_))) || ds.generics.len() > 1 + targs.len() {
            return err(SP, format!("`{}` cannot be called through `dyn` (it needs `&self`)", self.prog.names[decls[idx] as usize]));
        }
        // (`self: Box<Self>` through a `Box<dyn Tr>`: the box goes to the method, which owns it now)
        if by_box {
            if let Expr::MethodCall(recv, ..) = mc {
                // (only a `Box<dyn Tr>` has it: not `&dyn Tr`)
                if let Some(rt @ Ty::Ref(..)) = self.peek(recv) {
                    return err(SP, format!("no method named `{name}` found for reference `{}` in the current scope", self.prog.tn(rt)));
                }
                if is_place_expr(recv) {
                    self.mark_moved(recv);
                }
            }
        }
        let ds = &self.prog.sigs[decls[idx] as usize];
        // (the trait's parameters are the arguments of `dyn Tr<A>`; Self appears in no argument or return (object safe))
        let mut sol: Vec<Option<Ty>> = vec![None; ds.generics.len()];
        for (k, a) in targs.iter().enumerate() {
            if let Some(slot) = sol.get_mut(1 + k) {
                *slot = Some(*a);
            }
        }
        let ps: Vec<Ty> = ds.params[1..].iter().map(|&t| self.prog.partial_subst(t, &sol)).collect();
        let ret = self.prog.partial_subst(ds.ret, &sol);
        // (`Self::Out` of `dyn Tr<Out = u32>`: the type bound in the dyn type)
        let bound = |t: Ty| match t {
            Ty::Assoc(a) if a.base == Ty::Param(0) => binds.iter().find(|b| b.0 == a.name).map(|b| b.1).unwrap_or(t),
            Ty::Ref(m, Ty::Assoc(a)) if a.base == Ty::Param(0) => binds.iter().find(|b| b.0 == a.name).map(|b| Ty::Ref(m, intern(b.1))).unwrap_or(t),
            t => t,
        };
        let ps: Vec<Ty> = ps.into_iter().map(bound).collect();
        let ret = bound(ret);
        if ps.iter().chain([&ret]).any(|&t| self.prog.has_param(t)) {
            return err(SP, format!("`{name}` cannot be called through `dyn {tr}` (it uses `Self` or generics)"));
        }
        let args: Vec<&Expr> = margs.iter().collect();
        let decl = decls[idx];
        // (the receiver is borrowed by `&self` / `&mut self`, and a result that borrows from self holds the loan:
        // `let y = x.f1(); x.f2(); y.use_ref();` with x a `&mut dyn Foo` is E0499)
        // (a `&mut self` receiver whose arguments name it (`x.set(x.get())`): two-phase, as a method of a struct's. A
        // shared reservation while the arguments are evaluated, whose loans are held to the call; the `&mut` loan then)
        let mut two_phase = None;
        if self.bk_on() && !by_box {
            if let (Expr::MethodCall(recv, ..), Some(&Ty::Ref(m, _))) = (mc, self.prog.sigs[decl as usize].params.first()) {
                let flows = self.bk_ret_from(decl, 0);
                if m && borrowck::root_local(recv).is_some_and(|n| margs.iter().any(|a| borrowck::mentions(a, n))) {
                    let t0 = self.bk.call_temps.len();
                    let rs = self.bk_recv_ev(recv, Ty::Ref(false, intern(Ty::Unit)), false);
                    let tid = match &rs {
                        Some(borrowck::Ev::Acc(_, borrowck::Acc::Borrow(l), _)) => Some(*l),
                        _ => None,
                    };
                    self.bk_push(rs);
                    if let Some(l) = tid {
                        self.bk_call_temp(&[borrowck::Src::Loan(l)], true);
                    }
                    self.bk_frame();
                    two_phase = Some((recv, flows, t0));
                } else {
                    let ev = self.bk_recv_ev(recv, Ty::Ref(m, intern(Ty::Unit)), flows);
                    self.bk_push(ev);
                }
            }
        }
        let r = self.dyn_call(fat, idx, &ps, ret, &args, SP);
        if let Some((recv, flows, t0)) = two_phase {
            // (the arguments' loans live to the call, where the `&mut` loan is taken; what they borrow may flow on)
            let srcs = self.bk_frame_end();
            self.bk_call_temp(&srcs, true);
            // (the reservation ends before the `&mut` loan is taken)
            if let Some(tid) = self.bk.call_temps.get(t0).copied().filter(|&t| t != u32::MAX) {
                self.bk_ev(borrowck::Ev::Acc(borrowck::BPlace { local: tid, proj: vec![] }, borrowck::Acc::Read, SP));
                self.bk_ev(borrowck::Ev::Dead(tid, SP));
                self.bk.call_temps[t0] = u32::MAX;
            }
            let ev = self.bk_recv_ev(recv, Ty::Ref(true, intern(Ty::Unit)), flows);
            self.bk_push(ev);
            self.bk_call_temps_end(t0);
            for s in srcs {
                self.bk.src(s);
            }
        }
        if r.is_ok() {
            self.rg_dyn_call(mc, decl, &sol, &ps, ret);
        }
        r
    }

    /// If the callee's parameter type pt is `F` / `&F` / `&mut F` with a bound `Fn(A) -> B` on F: (A, B) and
    /// the bound's text (with the callee signature's type parameters as Param)
    pub(crate) fn fn_bound_with_text<'s>(&self, sig: &'s FnSig, pt: Ty) -> Option<(Vec<Ty>, Ty, &'s str)> {
        let pt = match pt {
            Ty::Ref(_, i) => *i,
            t => t,
        };
        let Ty::Param(g) = pt else { return None };
        let (_, bounds) = sig.generics.get(g as usize)?;
        let (text, (_, ps, r)) = bounds.iter().find_map(|b| crate::ast_summary::parse_fn_bound(b).map(|x| (b.as_str(), x)))?;
        let names: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
        let ps = ps.iter().map(|t| self.prog.resolve_g(t, &names).ok()).collect::<Option<Vec<Ty>>>()?;
        Some((ps, self.prog.resolve_g(&r, &names).ok()?, text))
    }

    /// The fn pointer type that expressions es all coerce to when they are two or more distinct closures with no
    /// captures and one signature (rustc's coercion of a match's arms or an array's elements to their least upper
    /// bound). None otherwise, also when they are one closure type
    /// The element type that `&String` and `&str` elements coerce to together (`vec![&string, "abc"]`: `&str`), when
    /// both kinds are among them and nothing else
    pub(crate) fn str_refs_lub(&self, es: &[&Expr]) -> Option<Ty> {
        let string = self.prog.struct_index.get("String").copied();
        let (mut strs, mut strings) = (0, 0);
        for e in es {
            match self.peek(e)? {
                Ty::Ref(false, Ty::Str) => strs += 1,
                Ty::Ref(_, Ty::Struct(s)) if Some(self.prog.base_of(*s)) == string => strings += 1,
                _ => return None,
            }
        }
        (strs > 0 && strings > 0).then(|| Ty::Ref(false, crate::types::intern(Ty::Str)))
    }

    pub(crate) fn closures_lub(&self, es: &[&Expr]) -> Option<Ty> {
        if es.len() < 2 {
            return None;
        }
        // (each member: a closure with no captures (an expression, or a local of such a closure's type), a fn item
        // that is not generic, a fn pointer, or an arm that diverges (`unimplemented!()`), which asks nothing. The
        // signature from those whose types are known; a closure whose parameters are not written takes it when it has
        // as many)
        let mut sig: Option<(Vec<Ty>, Ty)> = None;
        let mut open_arity: Vec<usize> = vec![];
        let mut kinds: Vec<Option<Ty>> = vec![];
        let mut fn_items: Vec<u32> = vec![];
        let mut members = 0;
        let mut any_closure = false;
        let mut agree = |sig: &mut Option<(Vec<Ty>, Ty)>, s: (Vec<Ty>, Ty)| -> bool {
            if s.0.iter().chain([&s.1]).any(|&x| self.prog.has_param(x)) {
                return false;
            }
            match sig {
                Some(prev) => *prev == s,
                None => {
                    *sig = Some(s);
                    true
                }
            }
        };
        for e in es {
            let c = strip_paren(e);
            match c {
                Expr::Macro(p, _, _) if matches!(p.segs.last().map(|s| s.name.as_str()), Some("panic" | "todo" | "unimplemented" | "unreachable")) => continue,
                Expr::Closure(_, ps, ret_ann, _, _) => {
                    let k = self.closure_ix.get(&format!("@{}", c as *const Expr as usize)).or_else(|| self.closure_ix.get(&crate::ast_closure::closure_key(c)));
                    match k.and_then(|k| self.prog.closure_by.get(&(self.root, *k))) {
                        Some(cid) => {
                            if !self.prog.closures[cid].captures.is_empty() {
                                return None;
                            }
                        }
                        None => {
                            if crate::ast_closure::free_vars(c).iter().any(|n| self.lookup(n).is_some()) {
                                return None;
                            }
                        }
                    }
                    let written: Option<Vec<Ty>> = ps.iter().map(|(_, t)| t.as_ref().and_then(|t| self.parse_ty(t).ok())).collect();
                    let ret = match ret_ann {
                        Some(t) => self.parse_ty(t).ok(),
                        None => self.peek(e).filter(|t| !matches!(t, Ty::Struct(s) if self.prog.closure_of_struct.contains_key(&self.prog.base_of(*s)))),
                    };
                    match (written, ret) {
                        (Some(params), Some(ret)) => {
                            if !agree(&mut sig, (params, ret)) {
                                return None;
                            }
                        }
                        _ => open_arity.push(ps.len()),
                    }
                    kinds.push(None);
                    any_closure = true;
                }
                Expr::Path(None, p) if p.segs.len() == 1 && self.lookup(&p.segs[0].name).is_some() => {
                    let (_, t) = self.lookup(&p.segs[0].name)?;
                    match t {
                        Ty::Struct(sid) if self.prog.closure_of_struct.contains_key(&self.prog.base_of(sid)) => {
                            if !self.prog.structs[sid as usize].fields.is_empty() {
                                return None;
                            }
                            // (a parameter or result with a reference: its lifetime (`|x: &'static u32|`) is not in the
                            // fn pointer type, and the region check of an explicit cast (rg_closure_fnptr) would not run;
                            // not coerced, as before)
                            let cs = self.closure_sig(t)?;
                            if cs.0.iter().chain([&cs.1]).any(|&p| has_ref(p, 0)) {
                                return None;
                            }
                            if !agree(&mut sig, cs) {
                                return None;
                            }
                            any_closure = true;
                        }
                        Ty::Fn(f) => {
                            if !agree(&mut sig, (f.params.clone(), f.ret)) {
                                return None;
                            }
                        }
                        _ => return None,
                    }
                    kinds.push(Some(t));
                }
                _ => {
                    // (a fn item that is not generic, or a value of a fn pointer type (`foo as FnPointer`))
                    let ft = match self.fn_item(c) {
                        Some(fid) if self.prog.sigs[fid as usize].generics.is_empty() => {
                            let fs = &self.prog.sigs[fid as usize];
                            kinds.push(Some(Ty::Param(crate::program::HOLE)));
                            fn_items.push(fid);
                            (fs.params.clone(), fs.ret)
                        }
                        Some(_) => return None,
                        None => match self.peek(c)? {
                            Ty::Fn(f) => {
                                kinds.push(Some(Ty::Fn(f)));
                                (f.params.clone(), f.ret)
                            }
                            _ => return None,
                        },
                    };
                    if !agree(&mut sig, ft) {
                        return None;
                    }
                }
            }
            members += 1;
        }
        let (params, ret) = sig?;
        if members < 2 || open_arity.iter().any(|&n| n != params.len()) {
            return None;
        }
        // (one closure type alone is no coercion: `[|x| x; 2]`-like arms of the same closure, or one closure local
        // used twice (`[c, c]`), stay it. The types: each closure written is one of its own; a local's, a fn item's
        // (by the item) and a fn pointer's are as many as they are different)
        let mut types: Vec<String> = vec![];
        let mut items = fn_items.iter();
        for (i, k) in kinds.iter().enumerate() {
            let key = match k {
                None => format!("literal#{i}"),
                Some(Ty::Param(crate::program::HOLE)) => format!("item#{}", items.next().copied().unwrap_or(u32::MAX)),
                Some(t) => format!("{t:?}"),
            };
            if !types.contains(&key) {
                types.push(key);
            }
        }
        let distinct_closures = types.len() >= 2;
        if !any_closure && !kinds.iter().any(|k| matches!(k, Some(Ty::Param(_)))) {
            return None;
        }
        distinct_closures.then(|| Ty::Fn(crate::types::intern_fn(crate::types::FnTy { params, ret })))
    }

    /// If t is a closure type (a struct instance), its argument and return types
    pub(crate) fn closure_sig(&self, t: Ty) -> Option<(Vec<Ty>, Ty)> {
        if let Ty::Fn(f) = t {
            return Some((f.params.clone(), f.ret));
        }
        let Ty::Struct(sid) = t else { return None };
        let cid = *self.prog.closure_of_struct.get(&self.prog.base_of(sid))?;
        let args = &self.prog.structs[sid as usize].args;
        let s0 = &self.prog.sigs[cid as usize];
        Some((s0.params[1..].iter().map(|&p| self.prog.subst(p, args)).collect(), self.prog.subst(s0.ret, args)))
    }

    /// Solves type parameters from associated type bounds (`I: Iterator<Item = T>` /
    /// `where Self: Iterator<Item = &T>`). The `_` of `collect::<Vec<_>>()` is filled with `Self::Item`
    pub(crate) fn solve_assoc_bounds(&self, sig: &FnSig, sol: &mut [Option<Ty>]) {
        let names: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
        for _ in 0..2 {
            for (g, (_, bounds)) in sig.generics.iter().enumerate() {
                let Some(gt) = sol[g] else { continue };
                if self.prog.has_param(gt) {
                    continue;
                }
                for b in bounds {
                    let Some((name, want)) = assoc_constraint(b) else { continue };
                    let Ok(want) = self.prog.resolve_g(&want, &names) else { continue };
                    // (`I: IntoIterator<Item = A>` with a Vec or a HashMap: the prelude's collections have an `into_iter`
                    // method, not an IntoIterator impl; the Item is that of the iterator it returns)
                    let actual = self.prog.assoc_of(gt, &name).or_else(|| {
                        if name != "Item" || !b.trim_start().starts_with("IntoIterator") {
                            return None;
                        }
                        let Ty::Struct(sid) = gt else { return None };
                        let id = self.prog.method(sid, "into_iter")?;
                        // (the method is the generic one, `IntoIter<T0>`: the instance's arguments go in)
                        let args = self.prog.structs[sid as usize].args.clone();
                        let ret = self.prog.subst(self.prog.sigs[id as usize].ret, &args);
                        if self.prog.has_param(ret) {
                            return None;
                        }
                        self.prog.assoc_of(ret, "Item")
                    });
                    if let Some(actual) = actual {
                        self.prog.match_ty(want, actual, sol);
                    }
                }
            }
        }
        for g in 0..sol.len() {
            let Some(t) = sol[g] else { continue };
            if !has_hole(self.prog, t) || !sig.generics[g].1.iter().any(|b| b == "__FromIter") {
                continue;
            }
            let item = sol.first().copied().flatten().filter(|&s| !self.prog.has_param(s)).and_then(|s| self.prog.assoc_of(s, "Item"));
            if let Some(item) = item {
                let mut args: Vec<Ty> = (0..=crate::program::HOLE).map(Ty::Param).collect();
                args[crate::program::HOLE as usize] = item;
                sol[g] = Some(self.prog.subst(t, &args));
            }
        }
    }

    /// `a..b` / `a..=b` as a value: the prelude's Range / RangeInclusive
    pub(crate) fn range_value(&mut self, rg: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Range(start, end, incl) = rg else { return err(SP, "internal: expected a range") };
        let incl = *incl;
        // `(1..)`: a range without an end
        if let (Some(lo), None, false) = (start, end, incl) {
            let Some(&base) = self.prog.struct_index.get("RangeFrom") else {
                return err(SP, "ranges can only be used directly in `for` loops here");
            };
            let et = match expected {
                Some(Ty::Struct(sid)) if self.prog.base_of(sid) == base => self.prog.structs[sid as usize].args.first().copied(),
                _ => None,
            }
            .or_else(|| self.peek(lo));
            let (lv, lt) = self.expr(lo, et)?;
            let sid = self.prog.instantiate(base, vec![lt]).or_else(|e| err(SP, e))?;
            let st = Ty::Struct(sid);
            let a = self.temp(st);
            let (_, ft, off) = self.prog.structs[sid as usize].fields[0];
            self.write(Place::Mem(a, off as i32, ft), lv);
            return Ok((Some(a), st));
        }
        // `..b` / `..=b` (RangeTo / RangeToInclusive) and `..` (RangeFull), when the prelude has them
        if start.is_none() {
            let name = match (end, incl) {
                (Some(_), false) => "RangeTo",
                (Some(_), true) => "RangeToInclusive",
                (None, _) => "RangeFull",
            };
            if let Some(&base) = self.prog.struct_index.get(name) {
                let Some(hi) = end else {
                    return Ok((None, Ty::Struct(base)));
                };
                let et = match expected {
                    Some(Ty::Struct(sid)) if self.prog.base_of(sid) == base => self.prog.structs[sid as usize].args.first().copied(),
                    _ => None,
                }
                .or_else(|| self.peek(hi));
                let (hv, ht) = self.expr(hi, et)?;
                let sid = self.prog.instantiate(base, vec![ht]).or_else(|e| err(SP, e))?;
                let st = Ty::Struct(sid);
                let a = self.temp(st);
                let (_, ft, off) = self.prog.structs[sid as usize].fields[0];
                self.write(Place::Mem(a, off as i32, ft), hv);
                return Ok((Some(a), st));
            }
        }
        let name = if incl { "RangeInclusive" } else { "Range" };
        let (Some(lo), Some(hi)) = (start, end) else {
            return err(SP, "only `a..b` / `a..=b` / `a..` ranges can be used as values");
        };
        let Some(&base) = self.prog.struct_index.get(name) else {
            return err(SP, "ranges can only be used directly in `for` loops here");
        };
        let mut et = match expected {
            Some(Ty::Struct(sid)) if self.prog.base_of(sid) == base => self.prog.structs[sid as usize].args.first().copied(),
            _ => None,
        };
        if et.is_none() {
            et = self.peek(lo).or_else(|| self.peek(hi));
        }
        let (lv, lt) = self.expr(lo, et)?;
        let (hv, ht) = self.expr(hi, Some(lt))?;
        self.same(SP, ht, lt)?;
        let sid = self.prog.instantiate(base, vec![lt]).or_else(|e| err(SP, e))?;
        let st = Ty::Struct(sid);
        let a = self.temp(st);
        let fields = self.prog.structs[sid as usize].fields.clone();
        for (n, ft, off) in fields {
            let v = match n.as_str() {
                "start" => lv,
                "end" => hv,
                _ => Some(self.b.ins().iconst(types::I8, 0)),
            };
            self.write(Place::Mem(a, off as i32, ft), v);
        }
        Ok((Some(a), st))
    }

    /// Argument and return types of `Fn(A, B) -> R` (the inside of a dyn)
    /// A value of type t made of zeros (what a placeholder's call returns in a check: its bits are never used)
    pub(crate) fn zero_of(&mut self, t: Ty) -> V {
        if t == Ty::Unit || t == Ty::Never {
            return (None, t);
        }
        if t.is_agg() {
            let tmp = self.temp(t);
            self.zero(tmp, t);
            return (Some(tmp), t);
        }
        let v = clif_ty(t).map(|ct| if ct == types::I128 { self.int_const(Ty::I128, 0) } else if ct == types::F64 { self.b.ins().f64const(0.0) } else if ct == types::F32 { self.b.ins().f32const(0.0) } else { self.b.ins().iconst(ct, 0) });
        (v, t)
    }

    pub(crate) fn dyn_fn_sig(&self, s: &str) -> R<(Vec<Ty>, Ty)> {
        let s = crate::types::dyn_autos(s).0;
        let Some((_, ps, r)) = crate::ast_summary::parse_fn_bound(s) else {
            return Err(format!("`dyn {s}` is not a function type"));
        };
        let ps = ps.iter().map(|t| self.prog.resolve_env(t, &self.env)).collect::<R<Vec<Ty>>>()?;
        Ok((ps, self.prog.resolve_env(&r, &self.env)?))
    }

    /// A call `f(x)` of a value that is not a function: a closure (or a reference to one) or `&dyn Fn`
    pub(crate) fn call_value(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let t0 = self.bk.call_temps.len();
        let r = self.call_value_inner(c, expected);
        self.bk_call_temps_end(t0);
        r
    }

    fn call_value_inner(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Call(func, cargs) = c else { return err(SP, "internal: expected a call") };
        // A call does not move (Fn / FnMut are called by reference). A temporary is owned until the end of the statement
        self.bk_frame();
        let (v, mut t) = if is_place_expr(func) && !matches!(&**func, Expr::Path(..) if self.fn_item(func).is_some()) {
            let p = self.place(func)?;
            self.read(p)
        } else {
            let r = self.expr(func, None)?;
            if let (Some(v), t) = r {
                self.own_temp(v, t);
            }
            r
        };
        let callee_srcs = self.bk_frame_end();
        let Some(mut v) = v else {
            return err(SP, "expected a function");
        };
        loop {
            match t {
                Ty::Ref(_, Ty::Dyn(_)) | Ty::Box(Ty::Dyn(_)) => break,
                Ty::Ref(_, i) | Ty::Box(i) if i.is_agg() => t = *i,
                Ty::Ref(_, i) | Ty::Box(i) => {
                    v = self.b.ins().load(PTR, mf(), v, 0);
                    t = *i;
                }
                _ => break,
            }
        }
        let args: Vec<&Expr> = cargs.iter().collect();
        // (an `FnOnce` closure called by name: the call takes it (E0382 when it is called again))
        if matches!(t, Ty::Struct(_)) && self.bk.fnonce.contains(&t) && is_place_expr(func) && self.peek(func) == Some(t) {
            self.bk_acc(func, borrowck::Acc::Move);
        }
        // (an `FnMut` closure called by name: a `&mut` of it (E0596 when it is not `mut`); through a reference, a `&mut`
        // of what it points to (E0596 behind a `&`))
        if matches!(t, Ty::Struct(_)) && self.bk.fnmut.contains(&t) && !self.bk.fnonce.contains(&t) && is_place_expr(func) {
            // (through every reference to it: `&&mut c` is behind a `&`, `&mut &mut c` is not)
            let mut ft = self.peek(func);
            let mut x: &Expr = func;
            let mut n = 0;
            while let Some(Ty::Ref(_, i)) = ft {
                if n > 8 {
                    break;
                }
                let sp = crate::parse::Span { start: expr_start(func), at: expr_start(func) };
                x = self.keep_expr(Expr::Unary("*".into(), Box::new(x.clone()), sp));
                ft = Some(*i);
                n += 1;
            }
            let target: Option<&Expr> = (ft == Some(t)).then_some(x);
            if let Some(x) = target {
                self.bk.no_loan_src = true;
                self.bk_borrow(x, true, err_pos(func));
                self.bk.no_loan_src = false;
            }
        }
        // (a closure called: what it borrows is held until the call, after its arguments, as rustc's `&c` of the
        // call (`c(take(v))` with c capturing v by reference is E0505))
        if let Ty::Struct(sid) = t {
            if self.prog.closure_of_struct.contains_key(&self.prog.base_of(sid)) {
                // (and the closure itself, which the call borrows: `c(drop(c))` is E0505)
                let mut hold = callee_srcs.clone();
                if is_place_expr(func) && self.bk_on() {
                    if let Some(ln) = self.bk_loan_only(func, false) {
                        hold.push(borrowck::Src::Loan(ln));
                    }
                }
                self.bk_callee_hold(&hold);
            }
        }
        self.bk_frame();
        let r = match t {
            Ty::Struct(sid) => {
                let Some(&cid) = self.prog.closure_of_struct.get(&self.prog.base_of(sid)) else {
                    // (a type parameter's placeholder bounded by `Fn` / `FnMut` / `FnOnce` (zustc check of a generic fn
                    // nothing calls): the call is checked against the bound, its result a value of the bound's result
                    // type; `FnMut` borrows the callee mutably, `FnOnce` takes it)
                    let bound = self.prog.structs[sid as usize].placeholder.as_ref().and_then(|bs| bs.iter().find_map(|b| crate::ast_summary::parse_fn_bound(b).map(|(k, _, _)| (k, b.clone()))));
                    if let (Some((kind, b)), true) = (bound, self.prog.is_placeholder(sid)) {
                        let (ps, ret) = self.dyn_fn_sig(&b).map_err(|e| self.at(SP, e))?;
                        // (the bound's arity and its parameters of primitive types. A placeholder's instance's type errors
                        // are not reported (see compile_once): it stops being checked. A name a macro's expansion
                        // shadows (no hygiene) may be such a callee)
                        if args.len() != ps.len() {
                            return err(err_pos(func), format!("this function takes {} arguments but {} were supplied (a call through the bound `{b}`) (E0057)", ps.len(), args.len()));
                        }
                        if is_place_expr(func) && kind == "FnOnce" && self.peek(func) == Some(t) {
                            self.bk_acc(func, borrowck::Acc::Move);
                        } else if is_place_expr(func) && kind != "FnOnce" {
                            // (through every reference to it, as a closure's call: `f: &F` with `F: FnMut()` is a `&mut`
                            // of `*f`, behind a `&` (E0596))
                            let mut ft = self.peek(func);
                            let mut x: &Expr = func;
                            let mut n = 0;
                            while let Some(Ty::Ref(_, i)) = ft {
                                if n > 8 {
                                    break;
                                }
                                let sp = crate::parse::Span { start: expr_start(func), at: expr_start(func) };
                                x = self.keep_expr(Expr::Unary("*".into(), Box::new(x.clone()), sp));
                                ft = Some(*i);
                                n += 1;
                            }
                            if ft == Some(t) {
                                self.bk.no_loan_src = true;
                                self.bk_borrow(x, kind == "FnMut", err_pos(func));
                                self.bk.no_loan_src = false;
                            }
                        }
                        // (a bound that writes `'static` (`F: Fn(&'static u8)`): checked from here on, as a `dyn Fn`'s)
                        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && b.contains("'static") {
                            self.rg_late_on();
                        }
                        let mut never = false;
                        let prim = |t: Ty| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char);
                        for (a, &pt) in args.iter().zip(&ps) {
                            let (_, at) = self.expr(a, Some(pt))?;
                            never |= at == Ty::Never;
                            if at != Ty::Never && at != pt && prim(at) && prim(pt) {
                                return err(expr_start(a), format!("mismatched types: expected `{}`, found `{}` (an argument of a call through the bound `{b}`) (E0308)", self.prog.tn(pt), self.prog.tn(at)));
                            }
                        }
                        // (the arguments flow into the regions the bound's parameters write, as for a `dyn Fn`)
                        self.rg_dyn_fn_args(&b, &args);
                        let r = if never { (None, Ty::Never) } else { self.zero_of(ret) };
                        // (a result that borrows an argument by the bound's lifetimes holds the argument's loans:
                        // `let y = f(&x);` with `F: Fn(&isize) -> &isize`)
                        let from = if matches!(ret, Ty::Ref(..)) || self.bk_ty(ret).1 { crate::ast_summary::fn_bound_ret_from(&b) } else { vec![] };
                        self.bk_frame_end_call_from(&args, &from);
                        return Ok(r);
                    }
                    return err(SP, format!("expected a function, found `{}`", self.prog.tn(t)));
                };
                let recv = (v, Ty::Ptr(true, intern(t)));
                // (a closure made where nothing was expected, a parameter of it a reference: its signature's regions
                // are one for all its calls (rustc's inference variables), not each call's own)
                let base = self.prog.base_of(sid);
                if self.hr_lost.iter().any(|h| h.0 == base) {
                    if !self.rg.on && self.rg.eligible && self.inline_depth == 0 {
                        self.rg_late_on();
                    }
                    self.rg.shared_clo = Some((base, self.clo_ret_param.iter().find(|x| x.0 == base).map(|x| x.1)));
                }
                let r = self.call_fn(cid, Some(recv), &args, None, expected, SP);
                self.rg.shared_clo = None;
                self.rg_took_call(c);
                // (those regions are in the closure's type, so they last while it is used: the closure holds what the
                // arguments passed to such parameters borrow (`g(&a); a = 2; g(&a);` is E0506). A local's closure called
                // by its name: one in a struct's field may have the field's bound as its signature)
                let callee = strip_paren(func);
                if r.is_ok() && self.bk_on() && self.bk.level >= 2 && matches!(callee, Expr::Path(None, p) if p.segs.len() == 1) {
                    if let Some(ps) = self.hr_lost.iter().find(|h| h.0 == base).map(|h| h.1.clone()) {
                        let mut srcs = vec![];
                        for &i in &ps {
                            if let Some(a) = args.get(i) {
                                self.bk_direct_pub(a, &mut srcs);
                            }
                        }
                        // (through a reference to the closure (`r(&a)` with `r = &g`): the closure it points to holds
                        // them too)
                        let through = matches!(callee, Expr::Path(None, p) if ident_of(p).and_then(|n| self.bk_local_of(n)).is_some_and(|l| matches!(self.bk.locals[l as usize].ty, Ty::Ref(..))));
                        if through && !srcs.is_empty() {
                            if let Expr::Path(None, p) = callee {
                                if let Some(l) = ident_of(p).and_then(|n| self.bk_local_of(n)) {
                                    self.bk_ev(borrowck::Ev::HoldThrough(l, srcs.clone()));
                                }
                            }
                        }
                        self.bk_flow_into(callee, &srcs);
                    }
                }
                // (a result that holds references holds what the arguments its body makes it of borrow: `some(&x)`
                // with `let some = |a| Some(a);`, or `let some = Some;`)
                if let Ok((_, rt)) = &r {
                    if self.bk_on() && self.bk_ty(*rt).1 {
                        if let Some(from) = self.clo_ret_from.iter().find(|x| x.0 == base).map(|x| x.1.clone()).filter(|f| !f.is_empty()) {
                            self.bk_frame_end_call_from(&args, &from);
                            return r;
                        }
                    }
                }
                r
            }
            Ty::Ref(_, Ty::Dyn(s)) | Ty::Box(Ty::Dyn(s)) => {
                // (a bound that writes `'static` (`dyn Fn(&'static u8)`): checked from here on)
                if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && s.contains("'static") {
                    self.rg_late_on();
                }
                let r = self.dyn_fn_call(v, s, &args, SP);
                self.rg_dyn_fn_args(s, &args);
                // (its result holds the loans of the arguments it borrows by the bound's lifetimes, as a generic
                // `F: Fn(&i32) -> &i32`'s: `let r = f(&x);` with `f: &dyn Fn(&i32) -> &i32`)
                if let Ok((_, rt)) = &r {
                    let from = if matches!(rt, Ty::Ref(..)) || self.bk_ty(*rt).1 { crate::ast_summary::fn_bound_ret_from(s) } else { vec![] };
                    self.bk_frame_end_call_from(&args, &from);
                    return r;
                }
                r
            }
            Ty::Fn(f) => {
                let r = self.indirect_call(Indirect::Ptr(v), &f.params, f.ret, &args, SP);
                self.rg_fnptr_call(func, t, &args);
                self.rg_took_call(c);
                // (a result that holds references may hold what the arguments of such types borrow, as an elided
                // signature's: `let f: fn(&String) -> &String = id; let g = f(&temp());`, `some(&temp())` of
                // `let some = Some;`. Not a result whose regions the region check knows are all 'static
                // (`f: fn(&i32) -> &'static i32`): the type keeps no region, the pointer's tree does)
                let holds = r.is_ok() && self.bk_on() && self.bk_ty(f.ret).1;
                let static_ret = holds && self.rg_on() && {
                    let prog = self.prog;
                    let n = f.params.iter().try_fold(0usize, |k, &p| Some(k + super::regionck::count(prog, p, 0)?));
                    match (self.rt_expr(func), n) {
                        (Some((_, tr)), Some(n)) => tr.get(n..).is_some_and(|ret| !ret.is_empty() && ret.iter().all(|&r| r == super::regionck::STATIC)),
                        _ => false,
                    }
                };
                if holds && !static_ret {
                    let from: Vec<usize> = f.params.iter().enumerate().filter(|(_, p)| self.bk_ty(**p).1).map(|(i, _)| i).collect();
                    self.bk_frame_end_call_from(&args, &from);
                    // (and the reborrows of those arguments' places of a `&mut`, without the region check)
                    for (i, ln) in std::mem::take(&mut self.bk.fnptr_reborrows) {
                        if from.contains(&i) {
                            self.bk.src(borrowck::Src::Loan(ln));
                        }
                    }
                    return r;
                }
                r
            }
            t => err(SP, format!("expected a function, found `{}`", self.prog.tn(t))),
        };
        self.bk_frame_end_call();
        r
    }

    /// Whether the method named `name` (with its overloads) exists and none of them is a trait impl's
    pub(crate) fn no_trait_method(&self, name: &str) -> bool {
        let inh = |i: u32| self.prog.sigs[i as usize].impl_trait.is_none();
        let Some(&i) = self.prog.index.get(name) else { return false };
        inh(i) && self.prog.overloads.get(name).is_none_or(|cs| cs.iter().all(|&c| inh(c)))
    }

    /// An assignment to (a field or element of) a static not declared `mut` (E0594)
    pub(crate) fn immutable_static(&self, left: &Expr) -> Option<String> {
        if !self.prog.any_immutable_static {
            return None;
        }
        let mut e = strip_paren(left);
        let mut whole = true;
        while let Expr::Field(b, ..) | Expr::Index(b, ..) = e {
            e = strip_paren(b);
            whole = false;
        }
        let Expr::Path(None, p) = e else { return None };
        // (the hot path: a name that is no static's of the program, as almost every assignment's)
        if !p.segs.last().is_some_and(|s| self.prog.const_index.contains_key(&s.name)) {
            return None;
        }
        // (`N` not shadowed by a local, or `m::N`: by the path's static)
        if ident_of(p).is_some_and(|id| self.lookup(id).is_some()) {
            return None;
        }
        let ci = match self.static_of(e) {
            Some(ci) => ci,
            None => match p.segs.as_slice() {
                [seg] if seg.args.is_empty() => *self.prog.const_index.get(&seg.name)?,
                _ => return None,
            },
        };
        let c = &self.prog.consts[ci as usize];
        if c.stat != Some(false) || crate::parse::extern_static(&c.expr).is_some() {
            return None;
        }
        let n = &p.segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("::");
        Some(if whole { format!("cannot assign to immutable static item `{n}` (E0594)") } else { format!("cannot assign to a part of `{n}`, as `{n}` is an immutable static item (E0594)") })
    }

    /// Why reading static ci needs `unsafe` (E0133): a `static mut`, or an extern block's static (foreign memory)
    pub(crate) fn unsafe_static_read(&self, ci: u32) -> Option<&'static str> {
        let c = &self.prog.consts[ci as usize];
        if c.stat == Some(true) {
            Some("use of mutable static is unsafe and requires unsafe block (E0133)")
        } else if c.stat.is_some() && crate::parse::extern_static(&c.expr).is_some() {
            Some("use of extern static is unsafe and requires unsafe block (E0133)")
        } else {
            None
        }
    }

    pub(crate) fn call(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        // (`<S as Tr>::m(..)`: Tr's impl among the overloads of `S::m`, as a call that named its trait; any other call
        // names none, also inside the arguments of one that did)
        let forced = match c {
            Expr::Call(func, _) => match &**func {
                Expr::Path(Some(_), p) if p.segs.len() >= 2 => Some((p.segs[p.segs.len() - 1].name.clone(), p.segs[p.segs.len() - 2].name.clone())),
                _ => None,
            },
            _ => None,
        };
        let prev = std::mem::replace(&mut self.explicit_trait, forced);
        let r = self.call_inner(c, expected);
        self.explicit_trait = prev;
        r
    }

    fn call_inner(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Call(func, cargs) = c else { return err(SP, "internal: expected a call") };
        // (`u32::max_value()` / `i8::min_value()`: the integer type's `MAX` / `MIN`, as `u32::MAX` (#265))
        if let Some((t, k)) = crate::parse::int_bound_call(func, cargs) {
            let v = crate::program::prim_assoc_const(t, k).unwrap_or(0);
            return Ok((Some(self.int_const(t, v)), t));
        }
        // (E0040: `Drop::drop(&mut x)`, `<S as Drop>::drop(..)`)
        if let (true, Expr::Path(_, p)) = (self.user_code, &**func) {
            if p.segs.len() >= 2 && p.segs[p.segs.len() - 1].name == "drop" && p.segs[p.segs.len() - 2].name == "Drop" {
                return Err(self.at(p.pos, "explicit use of destructor method (E0040)".to_string()));
            }
        }
        // (`std::clone::Clone::clone(&x)`, `std::fmt::Write::write_str(&mut s, ..)`: the call its last two segments
        // make, `Clone::clone(&x)`, as the type inference reads it (#153))
        if let Expr::Path(None, p) = &**func {
            if let Some(k) = self.prog.std_trait_path_skip(p) {
                let tail = Expr::Path(None, crate::parse::Path { global: false, segs: p.segs[k..].to_vec(), pos: p.pos });
                let ne: &Expr = self.keep_expr(Expr::Call(Box::new(tail), cargs.clone()));
                return self.expr(ne, expected);
            }
        }
        // (`f1(x)` after `let f1 = f;` (f generic): `f(x)`)
        if let (false, Expr::Path(None, p)) = (self.fn_aliases.is_empty(), strip_paren(func)) {
            if let Some(&(_, target)) = ident_of(p).and_then(|n| self.fn_aliases.iter().rev().find(|a| a.0 == n)) {
                let ne: &Expr = self.keep_expr(Expr::Call(Box::new(target.clone()), cargs.clone()));
                return self.expr(ne, expected);
            }
        }
        // (`core::mem::MaybeUninit::<S>::uninit()` / `std::vec::Vec::<u8>::new()`: std's modules before a type of the
        //  prelude with type arguments are left out, as without them (libc 0.2's offset_of!))
        if let Expr::Path(None, p) = strip_paren(func) {
            let n = p.segs.len();
            if n >= 4 && matches!(p.segs[0].name.as_str(), "std" | "core" | "alloc") {
                let k = (1..n - 1).find(|&k| !p.segs[k].args.is_empty());
                if let Some(k) = k.filter(|&k| {
                    k >= 2
                        && p.segs[1..k].iter().all(|s| s.args.is_empty() && s.name.starts_with(|c: char| c.is_ascii_lowercase()))
                        && self.prog.struct_index.contains_key(p.segs[k].name.as_str())
                }) {
                    let np = crate::parse::Path { global: false, segs: p.segs[k..].to_vec(), pos: p.pos };
                    let ne: &Expr = self.keep_expr(Expr::Call(Box::new(Expr::Path(None, np)), cargs.clone()));
                    return self.expr(ne, expected);
                }
            }
        }
        // `(*f)(x)` with f: `Box<dyn Fn ..>` / `&dyn Fn ..` / a closure behind a reference: `f(x)`
        if let Expr::Unary(op, inner, _) = strip_paren(func) {
            if op == "*" && matches!(self.peek(inner), Some(Ty::Box(_) | Ty::Ref(..))) {
                let ne = self.keep_expr(Expr::Call(inner.clone(), cargs.clone()));
                return self.expr(ne, expected);
            }
        }
        if let Some(r) = self.default_call(c, expected)? {
            return Ok(r);
        }
        // (`T::A::m(..)` / `<T as Tr>::A::m(..)` / `<u16 as From<u8>>::from(..)` that no fn of the program answers: the
        // call on the type the path names, `<u16>::m(..)`, which reaches the builtins too)
        if let Expr::Path(q, p) = &**func {
            if (p.segs.len() >= 3 || q.is_some()) && self.callee(func).is_none() {
                if let Some(nf) = self.assoc_path_func(q.as_deref(), p) {
                    let args = self.from_source_args(p, cargs)?;
                    let ne: &Expr = self.keep_expr(Expr::Call(Box::new(nf), args));
                    return self.expr(ne, expected);
                }
            }
        }
        // (`<J as PartialEq<str>>::eq(a, b)`: the program's impl for that right-hand side, `J::<PartialEq<str>>::eq`
        // (`ne` is its negation), which `a == b` would not find from b's type alone (`&&str` coerces to `&str`))
        if let (Expr::Path(Some(qs), p), [a, b]) = (&**func, cargs.as_slice()) {
            let n = p.segs.len();
            if n >= 2 && p.segs[n - 2].name == "PartialEq" && matches!(p.segs[n - 1].name.as_str(), "eq" | "ne") && !self.prog.peq.is_empty() {
                if let [GArg::Ty(rt)] = p.segs[n - 2].args.as_slice() {
                    if let (Ok(st), Ok(r)) = (self.parse_ty(qs), self.parse_ty(rt)) {
                        let (sr, rr) = (Ty::Ref(false, intern(st)), Ty::Ref(false, intern(r)));
                        let f = self.prog.peq.iter().copied().find(|&f| {
                            let sg = &self.prog.sigs[f as usize];
                            sg.generics.is_empty() && sg.params.len() == 2 && sg.params[0] == sr && sg.params[1] == rr
                        });
                        if let Some(f) = f {
                            // (the impl's own `ne` when it has one, `J::<PartialEq<str>>::ne`; else the default, `!eq`)
                            let own_ne = (p.segs[n - 1].name == "ne")
                                .then(|| self.prog.names[f as usize].split('#').next().unwrap_or("").strip_suffix("::eq").map(|b| format!("{b}::ne")))
                                .flatten()
                                .and_then(|ne| self.prog.index.get(&ne).copied());
                            if let Some(g) = own_ne {
                                let args: [&Expr; 2] = [a, b];
                                return self.call_fn(g, None, &args, None, Some(Ty::Bool), SP);
                            }
                            if p.segs[n - 1].name == "ne" {
                                let mut q = p.clone();
                                q.segs[n - 1].name = "eq".into();
                                q.segs[n - 1].text = "eq".into();
                                let call = Expr::Call(Box::new(Expr::Path(Some(qs.clone()), q)), cargs.clone());
                                let ne = self.keep_expr(Expr::Unary("!".into(), Box::new(Expr::Paren(Box::new(call))), Default::default()));
                                return self.expr(ne, expected);
                            }
                            let args: [&Expr; 2] = [a, b];
                            return self.call_fn(f, None, &args, None, Some(Ty::Bool), SP);
                        }
                    }
                }
            }
        }
        // (`<T as Clone>::clone(x)`: `(x).clone()` naming its trait (not an inherent `clone` of T), for x: &T, a derive's
        // clone too; x of another referent is mismatched types (`<u8 as Clone>::clone(&1u32)`))
        if let (Expr::Path(Some(qs), p), [x]) = (&**func, cargs.as_slice()) {
            let n = p.segs.len();
            if n >= 2 && p.segs[n - 2].name == "Clone" && p.segs[n - 1].name == "clone" && p.segs[n - 2].args.is_empty() && self.lookup("Clone").is_none() {
                // (x is `&Self` after deref coercions (`&&1u8` for `<u8 as Clone>`); Self a reference: its clone is a copy of
                // it, not its referent's clone (`<&Q as Clone>::clone(&&q)` is `&Q`))
                let mut arg = x.clone();
                if let (Ok(st), Some(xt)) = (self.parse_ty(qs), self.peek(x)) {
                    if !self.prog.has_param(st) && !self.prog.has_param(xt) && !has_hole(self.prog, st) {
                        let mut t = xt;
                        loop {
                            match t {
                                Ty::Ref(_, i) if *i == st => break,
                                Ty::Ref(_, i @ Ty::Ref(..)) => {
                                    arg = Expr::Unary("*".into(), Box::new(Expr::Paren(Box::new(arg))), Default::default());
                                    t = *i;
                                }
                                _ => return err(expr_start(x), format!("mismatched types: expected `&{}`, found `{}`", self.prog.tn(st), self.prog.tn(xt))),
                            }
                        }
                        if matches!(st, Ty::Ref(false, _)) {
                            let d = self.keep_expr(Expr::Unary("*".into(), Box::new(Expr::Paren(Box::new(arg))), Default::default()));
                            return self.expr(d, expected);
                        }
                    }
                }
                let e = self.keep_expr(mcall(Expr::Paren(Box::new(arg)), "clone", vec![]));
                self.explicit_traits.insert(e as *const Expr as usize, "Clone".to_string());
                return self.expr(e, expected);
            }
        }
        // `From::from(x)` to an expected type T: `T::from(x)`; `Into::into(x)`: `x.into()`
        if let (Expr::Path(None, p), [x]) = (&**func, cargs.as_slice()) {
            let n = p.segs.len();
            if n >= 2 && p.segs.iter().all(|s| s.args.is_empty()) && (n == 2 || matches!(p.segs[0].name.as_str(), "std" | "core")) {
                match (p.segs[n - 2].name.as_str(), p.segs[n - 1].name.as_str()) {
                    ("Into", "into") if self.lookup("Into").is_none() => {
                        let e = self.keep_expr(mcall(x.clone(), "into", vec![]));
                        self.explicit_traits.insert(e as *const Expr as usize, "Into".to_string());
                        return self.expr(e, expected);
                    }
                    ("From", "from") => {
                        let simple = |t: Ty| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char) || matches!(t, Ty::Struct(sid) if self.prog.structs[sid as usize].base.is_none());
                        // (`<T as From<_>>::from(x)`, which takes From's impl beside an inherent `from`; T's `from` with no
                        // From impl at all (the prelude's `String::from` and the like stand for From): `T::from(x)`)
                        if let Some(t) = expected.filter(|&t| simple(t) && !self.prog.has_param(t)) {
                            let n = format!("{}::from", self.ty_key(t));
                            let src = if self.trait_candidate(&n, "From").is_some() {
                                Some(format!("<{} as From>::from", self.prog.tn(t)))
                            } else if (t.is_int() || t.is_float()) && self.peek(x).is_some_and(|a| a.is_int() || a.is_float() || matches!(a, Ty::Bool | Ty::Char)) {
                                // (`let b: u32 = From::from(4u16)`: a number's `from`, the widening cast; a narrowing
                                // one, `u8` from `256u16`, is E0277 as in rustc)
                                let a = self.peek(x).unwrap();
                                if !crate::program::prim_from(a, t) {
                                    return err(SP, format!("the trait `From<{}>` is not implemented for `{}` (E0277)", self.prog.tn(a), self.prog.tn(t)));
                                }
                                Some(format!("{}::from", self.prog.tn(t)))
                            } else if self.no_trait_method(&n) {
                                Some(format!("{}::from", self.prog.tn(t)))
                            } else {
                                None
                            };
                            if let Some(Ok(Expr::Path(q, tp))) = src.map(|s| crate::parse::parse_expr(&s)) {
                                let ne = self.keep_expr(Expr::Call(Box::new(Expr::Path(q, crate::parse::Path { pos: p.pos, ..tp })), cargs.clone()));
                                return self.expr(ne, expected);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // (`u16::from(5u32)`: a number's `from` of another primitive is the widening one only, E0277 else as in rustc)
        if let (Expr::Path(None, p), [x]) = (&**func, cargs.as_slice()) {
            if let [ts, m] = p.segs.as_slice() {
                if m.name == "from" && ts.args.is_empty() && m.args.is_empty() {
                    if let Some(t) = Ty::prim(&ts.name).filter(|t| t.is_int() || t.is_float()) {
                        // (the program's own `from` of the number is one of another trait's; its `impl From<J> for f64`
                        // leaves the numbers' widening `from`)
                        if self.prog.fn_by_p(&PPath::of(p), self.self_sid).is_none() || self.trait_candidate(&format!("{}::from", ts.name), "From").is_some() {
                            if let Some(a) = self.peek(x).filter(|a| a.is_int() || a.is_float() || matches!(a, Ty::Bool | Ty::Char)) {
                                // (not `f32::from(3.14)`: an unsuffixed float literal falls back to f32 there, which rustc accepts;
                                // `f32::from(1f64)` is E0277)
                                if !crate::program::prim_from(a, t) && !(t == Ty::F32 && a == Ty::F64 && super::p2_call::is_float_lit(x)) {
                                    return err(SP, format!("the trait `From<{}>` is not implemented for `{}` (E0277)", self.prog.tn(a), self.prog.tn(t)));
                                }
                            }
                        }
                    }
                }
            }
        }
        // `i32::try_from(x)` (an integer type): `x.try_into()` to that type; not of a value of another type with the
        // program's `impl TryFrom<J> for u32`, which is called as written
        if let (Expr::Path(None, p), [x]) = (&**func, cargs.as_slice()) {
            if p.segs.len() == 2 && p.segs[1].name == "try_from" {
                let own = || {
                    self.trait_candidate(&format!("{}::try_from", p.segs[0].name), "TryFrom").is_some()
                        && !self.peek(x).is_some_and(|a| a.is_int() || a.is_float() || matches!(a, Ty::Bool | Ty::Char))
                };
                if let Some(t) = Ty::from_suffix(&p.segs[0].name).filter(|t| t.is_int() && !own()) {
                    if let Some(rt) = self.result_of(t, "TryFromIntError") {
                        let e = self.keep_expr(mcall(x.clone(), "try_into", vec![]));
                        self.try_into_hint = Some(t);
                        return self.expr(e, Some(rt));
                    }
                }
            }
        }
        // (#144: `Tr::m(&&s)` of an impl for references to a type parameter: Self is `&S` as the argument has it, no
        // autoderef, which the method call the rewrite below makes would do)
        if !self.prog.ref_dispatch.is_empty() {
            if let Some(r) = self.ref_impl_ufcs(c, expected)? {
                return Ok(r);
            }
        }
        if let Some(e) = self.prim_method_call(c) {
            let e = self.keep_expr(e);
            // (`crate::a::M::m(&x)`: the trait the path names, which a trait in scope must not replace)
            if let (Expr::Path(None, p), Expr::MethodCall(..)) = (&**func, e) {
                let pp = crate::program::PPath::of(p);
                let q = self.prog.strip_mods_p(&pp).unwrap_or(pp);
                if q.segs.len() == 2 && self.prog.traits.contains_key(&q.segs[0].name) {
                    self.explicit_traits.insert(e as *const Expr as usize, q.segs[0].name.clone());
                }
            }
            return self.expr(e, expected);
        }
        // (`MyTrait::use_self(val)` of a trait's default method, with val a trait object: the method call with the
        // trait named, so the impl for the object (`impl MyTrait for dyn Obj {}`, for `dyn Obj + 'static`) is the one
        // called, as in a module, where the path names the default itself)
        if let (Expr::Path(None, p), Some(first)) = (&**func, cargs.first()) {
            if p.segs.len() == 2 && !p.global && p.segs[0].args.is_empty() {
                if let Some(id) = self.prog.fn_by_p(&PPath::of(p), self.self_sid) {
                    let s = &self.prog.sigs[id as usize];
                    let tk = self.prog.names[id as usize].rsplit_once("::").map(|(t, _)| t.to_string()).filter(|t| self.prog.traits.contains_key(t));
                    // (of the receiver's form: a `&self` method takes `&dyn Tr`, not a Box by value (E0308 then))
                    let obj = matches!((s.params.first(), self.peek(first)), (Some(Ty::Ref(..)), Some(Ty::Ref(_, Ty::Dyn(_)))) | (Some(Ty::Box(_)), Some(Ty::Box(Ty::Dyn(_)))));
                    if let (Some(tk), true, true, true) = (tk, s.has_self, s.has_body, obj) {
                        let span = crate::parse::Span { start: p.pos, at: p.pos };
                        let recv = first.clone();
                        let m = Expr::MethodCall(Box::new(Expr::Paren(Box::new(recv))), p.segs[1].name.clone(), p.segs[1].args.clone(), cargs[1..].to_vec(), span);
                        let m = self.keep_expr(m);
                        self.explicit_traits.insert(m as *const Expr as usize, tk);
                        return self.expr(m, expected);
                    }
                }
            }
        }
        // `Trait::method(x, args)` (`Clone::clone(&s)`, `Add::add(a, b)`): the method call `x.method(args)`
        if let (Expr::Path(None, p), Some(first)) = (&**func, cargs.first()) {
            let known = matches!(
                p.segs[0].name.as_str(),
                "Clone" | "ToString" | "Add" | "Sub" | "Mul" | "Div" | "Rem" | "Neg" | "Not" | "BitAnd" | "BitOr" | "BitXor" | "Shl"
                    | "Shr" | "PartialEq" | "PartialOrd" | "Ord" | "Iterator" | "IntoIterator" | "Into" | "AsRef" | "AsMut" | "Hash"
                    | "Deref" | "DerefMut" | "Borrow" | "ToOwned" | "AddAssign" | "SubAssign" | "MulAssign" | "DivAssign" | "Index"
                    | "Drop" | "Fn" | "FnMut" | "FnOnce" | "Debug" | "Display"
            );
            // (a user trait only when the path names no function itself: itoa's `Unsigned::fmt(x, buf)` does)
            let direct = self.prog.fn_by_p(&PPath::of(p), self.self_sid);
            // (`Foo::<i32>::get`: with arguments, the first segment is the trait, not a module)
            let user = self.prog.traits.contains_key(&p.segs[0].name) && (direct.is_none() || !p.segs[0].args.is_empty());
            // (`Foo::<i32>::get(&x)`: the trait's arguments pick its impl; with one impl of the trait, it is that one)
            let targs_ok = p.segs[0].args.is_empty() || (user && self.prog.impl_targs.get(&p.segs[0].name).map_or(0, |v| v.len()) <= 1);
            if !p.global && p.segs.len() == 2 && targs_ok && (known || user)
                && !self.prog.struct_index.contains_key(&p.segs[0].name)
                && direct.is_none_or(|id| self.prog.sigs[id as usize].has_self)
            {
                let span = crate::parse::Span { start: p.pos, at: p.pos };
                // (an operator: the expression `a + b`, which also covers primitive types)
                let op = match (p.segs[0].name.as_str(), p.segs[1].name.as_str(), cargs.len()) {
                    ("Add", "add", 2) => Some("+"),
                    ("Sub", "sub", 2) => Some("-"),
                    ("Mul", "mul", 2) => Some("*"),
                    ("Div", "div", 2) => Some("/"),
                    ("Rem", "rem", 2) => Some("%"),
                    ("BitAnd", "bitand", 2) => Some("&"),
                    ("BitOr", "bitor", 2) => Some("|"),
                    ("BitXor", "bitxor", 2) => Some("^"),
                    ("Shl", "shl", 2) => Some("<<"),
                    ("Shr", "shr", 2) => Some(">>"),
                    _ => None,
                };
                if let Some(op) = op {
                    let b = Expr::Binary(op.to_string(), Box::new(Expr::Paren(Box::new(cargs[0].clone()))), Box::new(Expr::Paren(Box::new(cargs[1].clone()))), span);
                    let b = self.keep_expr(b);
                    return self.expr(b, expected);
                }
                if let (("Neg", "neg") | ("Not", "not"), 1) = ((p.segs[0].name.as_str(), p.segs[1].name.as_str()), cargs.len()) {
                    let o = if p.segs[0].name == "Neg" { "-" } else { "!" };
                    let u = Expr::Unary(o.to_string(), Box::new(Expr::Paren(Box::new(cargs[0].clone()))), span);
                    let u = self.keep_expr(u);
                    return self.expr(u, expected);
                }
                let recv = match first {
                    Expr::Ref(_, x) => (**x).clone(),
                    x => x.clone(),
                };
                let m = Expr::MethodCall(Box::new(Expr::Paren(Box::new(recv))), p.segs[1].name.clone(), p.segs[1].args.clone(), cargs[1..].to_vec(), span);
                let m = self.keep_expr(m);
                // (`Borrow::borrow(&s)` of std's Borrow (not a trait of the program's of that name): in scope as the path
                // names it, whatever is imported: std's impls (borrow_rewrite))
                if let Expr::MethodCall(r, ..) = m {
                    if p.segs[0].name == "Borrow" && p.segs[1].name == "borrow" && cargs.len() == 1 && !self.prog.user_traits.iter().any(|t| t.rsplit("::").next() == Some("Borrow")) {
                        if let Some((ne, _)) = self.borrow_rewrite(r, expected, true) {
                            let ne = self.keep_expr(ne);
                            return self.expr(ne, expected);
                        }
                    }
                }
                // (`Foo::tag(&s)`: the trait's method, not an inherent `tag` of the receiver's type)
                if user {
                    self.explicit_traits.insert(m as *const Expr as usize, p.segs[0].name.clone());
                }
                return self.expr(m, expected);
            }
        }
        if let Expr::Path(_, p) = &**func {
            self.rg_written_on(p);
            if let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(p), self.self_sid) {
                let v = &self.prog.structs[sid as usize].variants[vi];
                if v.kind != crate::types::VariantKind::Tuple || v.fields.len() != cargs.len() {
                    return err(SP, format!("variant `{}` takes {} fields", v.name, v.fields.len()));
                }
                self.lifetime_arity(sid, p, "enum")?;
                self.variant_targs_allowed(p, "tuple variant")?;
                let args: Vec<(usize, &Expr)> = cargs.iter().enumerate().collect();
                // (`Ok::<_, ()>(1)` / `Some::<u8>(3)`: the turbofish gives the type arguments it names)
                if let Some(ta) = p.segs.iter().rev().find_map(|s| if s.args.is_empty() { None } else { Some(&s.args) }) {
                    let ts: Vec<Option<Ty>> = ta
                        .iter()
                        .filter_map(|a| match a {
                            GArg::Ty(crate::parse::Ty::Infer) => Some(None),
                            GArg::Ty(t) => Some(self.parse_ty(t).ok().filter(|&t| !has_hole(self.prog, t))),
                            _ => None,
                        })
                        .collect();
                    if ts.len() == self.prog.structs[self.prog.base_of(sid) as usize].generics.len() {
                        self.adt_targs = Some(ts);
                    }
                }
                let r = self.variant_value(sid, vi, &args, expected);
                self.rg_took_call(c);
                self.rg_written_ctor(c, p);
                return r;
            }
            // Building a tuple struct `P(1, 2)`
            // (`U::<u8>(7)` / `Foo::<3>([1, 2, 3])`: the struct without the turbofish, which gives its type arguments)
            let (pp, targs) = self.turbofish_split(p);
            if let Some(sid) = self.prog.struct_by_p(&pp, self.self_sid) {
                if self.lookup(&p.segs[0].name).is_none() && self.prog.fn_by_p(&pp, self.self_sid).is_none() {
                    let n = self.prog.structs[sid as usize].fields.len();
                    if n != cargs.len() {
                        return err(SP, format!("this struct takes {n} fields"));
                    }
                    self.lifetime_arity(sid, p, "struct")?;
                    if let Some(args) = targs {
                        let ts = self.turbofish_targs(sid, args)?;
                        // (`let x: S<u16> = S::<u8>(1u16)`: the turbofish against the expected instance)
                        self.turbofish_vs_expected(sid, &ts, expected)?;
                        self.adt_targs = Some(ts);
                    }
                    let args: Vec<(usize, &Expr)> = cargs.iter().enumerate().collect();
                    let r = self.adt_value(sid, None, &args, expected, SP);
                    if let Ok((_, t)) = &r {
                        self.rg_ctor(c, *t, &args);
                        self.rg_written_ctor(c, p);
                    }
                    return r;
                }
            }
        }
        if let Some(r) = self.intrinsic_call(c, expected)? {
            return Ok(r);
        }
        let Some((mut id, _)) = self.callee(func) else {
            // A value call: a closure / `&dyn Fn`
            return self.call_value(c, expected);
        };
        let args: Vec<&Expr> = cargs.iter().collect();
        let tf: Option<&[GArg]> = match &**func {
            Expr::Path(_, p) => match p.segs.last() {
                Some(s) if !s.args.is_empty() => Some(&s.args[..]),
                _ => None,
            },
            _ => None,
        };
        // `Buf::<8>::new()` / `Vec::<i32>::new()`: type arguments of the type segment
        if let Expr::Path(qs, p) = &**func {
            let n = p.segs.len();
            // (`<i32 as Foo<i32>>::get`: the segment before the fn is the trait, not the self type)
            let trait_seg = qs.is_some() && n == 2;
            if let (true, Some(q)) = (trait_seg, qs) {
                // (Self is the qualified type, when it is known: `<&'static u32 as Bazoom<_>>::method`)
                self.seg_self = self.parse_ty(q).ok().filter(|&t| !self.prog.has_param(t) && !has_hole(self.prog, t));
            } else if n >= 2 && !p.segs[n - 2].args.is_empty() {
                let seg = p.segs[n - 2].clone();
                let ty = crate::parse::Ty::Path(None, Path { global: false, segs: vec![seg], pos: Default::default() });
                let st = self.parse_ty(&ty)?;
                self.seg_self = Some(st);
                // (`<Box<[u8]>>::from_abi(js)` / `Box::<[u8]>::from_abi`: among impls for instances of the type, the one
                // for this one)
                let group = self.prog.names[id as usize].split('#').next().unwrap_or("").to_string();
                if let Some(cs) = self.prog.overloads.get(&group).filter(|_| !self.prog.has_param(st)) {
                    let own = |c: u32| self.prog.sigs[c as usize].self_ty.is_some_and(|t| self.prog.match_ty_exact(t, st, &mut vec![None; self.prog.sigs[c as usize].generics.len()]));
                    if !own(id) {
                        if let Some(&c) = cs.iter().find(|&&c| own(c)) {
                            id = c;
                        }
                    }
                }
            } else if n == 2 && self.prog.sigs[id as usize].self_ty.is_some_and(|t| self.prog.has_param(t)) {
                // `T::dec(r)` (T a concrete type in the instance): the generic impl's type arguments from Self
                let seg = p.segs[0].clone();
                let ty = crate::parse::Ty::Path(None, Path { global: false, segs: vec![seg], pos: Default::default() });
                self.seg_self = self.parse_ty(&ty).ok().filter(|&t| !self.prog.has_param(t));
            }
        }
        // (a trait's default fn called through a type (`S::f()` / `<S as Tr>::f()`): Self is that type)
        if self.seg_self.is_none() && self.prog.sigs[id as usize].trait_of.is_some() && matches!(self.prog.sigs[id as usize].self_ty, None | Some(Ty::Param(0))) && !self.prog.sigs[id as usize].generics.is_empty() {
            if let Expr::Path(q, p) = &**func {
                self.seg_self = match q {
                    Some(q) => self.parse_ty(q).ok(),
                    None => self.path_self_ty(p),
                }
                .filter(|&t| !self.prog.has_param(t));
            }
        }
        // (with no expected type, from the type inference record: the Self of `x = Tr::dec(r)?`)
        let expected = expected.or_else(|| {
            let sig = &self.prog.sigs[id as usize];
            if sig.trait_of.is_some() && !sig.has_self {
                self.lets.get(&(c as *const Expr as usize)).copied().filter(|&t| !self.prog.has_param(t))
            } else {
                None
            }
        });
        if let Expr::Path(_, p) = &**func {
            self.check_unsafe_call(id, p.pos)?;
        }
        // (the region check reports at the call: call_fn is given no span for it. Also when it is off, but may begin
        // at the call (a callee that requires 'static: `s::<T>()`))
        if self.rg_on() || self.rg.eligible || self.user_code {
            self.rg.call_sp = borrowck::pos(c);
        }
        // (the lifetimes written on the type: `A::<'a>::new`, `<A<'a>>::new`, `<A<'a> as Tr>::f`)
        let mut written = None;
        if let Expr::Path(q, p) = &**func {
            let seg = match q.as_deref() {
                Some(crate::parse::Ty::Path(None, qp)) => qp.segs.last(),
                Some(_) => None,
                None => p.segs.len().checked_sub(2).map(|i| &p.segs[i]),
            };
            let ls: Vec<String> = seg.map(|s| s.args.iter().filter_map(|a| if let GArg::Lifetime(l) = a { Some(l.clone()) } else { None }).collect()).unwrap_or_default();
            written = (!ls.is_empty()).then(|| (self.rg.depth + 1, id, ls));
        }
        // (a call of this kind in the arguments keeps the outer one's for after it)
        let outer = std::mem::replace(&mut self.rg.call_self, written);
        self.bk_arg_flows_before(id, &args);
        // (`<&[u8] as Tr>::m(&v)`: the impl the qualified type names, whose self type it is; not another overload
        // the arguments would pick (`&Vec<u8>` coerces to both `&[T]` and `&mut [u8]`'s receiver))
        // (only when it is the one impl for that self type: `From<u8>` and `From<u16>` for `W<u32>` are still told apart
        // by the argument, `W::<u32>::from(1u16)`)
        let mut kept = false;
        let group = self.prog.overloads.get(self.prog.names[id as usize].split('#').next().unwrap_or(""));
        let one_for = |st: Ty| {
            let exact = |c: u32| self.prog.sigs[c as usize].self_ty.is_some_and(|t| self.prog.match_ty_exact(t, st, &mut vec![None; self.prog.sigs[c as usize].generics.len()]));
            exact(id) && group.is_some_and(|g| g.iter().filter(|&&c| exact(c)).count() == 1)
        };
        if let (Some(st), Expr::Path(None, _)) = (self.seg_self, &**func) {
            if one_for(st) {
                self.keep_overload.set(Some(id));
                kept = true;
            }
        }
        if let Expr::Path(Some(q), _) = &**func {
            if group.is_some() {
                if let Ok(qt) = self.parse_ty(q) {
                    if !self.prog.has_param(qt) && one_for(qt) {
                        self.keep_overload.set(Some(id));
                        kept = true;
                    }
                }
            }
        }
        let r = self.call_fn(id, None, &args, tf, expected, SP);
        if kept {
            self.keep_overload.set(None);
        }
        self.rg.call_self = outer;
        if r.is_ok() {
            self.bk_arg_flows(id, &args);
        }
        self.rg_took_call(c);
        self.seg_self = None;
        r
    }
}

/// t names `Self` (the parameter 0 of a trait's method) itself, not through an associated type (`Self::Item`)
/// E0038: why the trait `tr` is not dyn compatible, with its methods dyn calls (Program::dyn_methods), or None. A
/// supertrait of the program's trait that takes Self by default (`trait Expr: PartialEq`); a method without
/// `where Self: Sized` that has no `self`, or (the program's own trait's) that names `Self` in its parameters or result
/// or has generics of its own
/// `bounded_exempt`: a method with any bound on Self counts as `where Self: Sized` (a bound's trait may be Sized,
/// `where Self: Tr1` with `trait Tr1: Sized`, which the supertraits recorded leave out)
pub fn dyn_incompatible(prog: &crate::program::Program, tr: &str, decls: &[u32], bounded_exempt: bool) -> Option<String> {
    // (E0038 too: a supertrait of the program's trait that takes Self by default (`trait Expr: PartialEq`))
    {
        let mut st = vec![tr.to_string()];
        let mut seen: Vec<String> = vec![];
        while let Some(t) = st.pop() {
            if seen.contains(&t) {
                continue;
            }
            for sup in prog.trait_supers.get(&t).cloned().unwrap_or_default() {
                if sup == "#SelfRhs" {
                    return Some(format!("the trait `{tr}` is not dyn compatible: it uses `Self` as a type parameter (E0038)"));
                }
                st.push(sup.split('<').next().unwrap_or("").trim().to_string());
            }
            seen.push(t);
        }
    }
    // (E0038: a method without `where Self: Sized` makes the trait not dyn compatible when it has no `self`,
    // or, for a method the program's own trait declares (not a supertrait's, not the prelude's, which leave
    // out their `where Self: Sized`), when `Self` is in its parameters or result)
    for &d in decls {
        let ds = &prog.sigs[d as usize];
        // (a bound of Self's other than its own trait's: `where Self: Tr1<..>`)
        let named = |b: &str, t: &str| b == t || b.starts_with(&format!("{t}<"));
        let other_bound = |g: &(String, Vec<String>)| g.1.iter().any(|b| !named(b, tr) && !ds.trait_of.as_deref().is_some_and(|t| named(b, t)));
        let sized = ds.generics.first().is_some_and(|g| g.1.iter().any(|b| b == "Sized") || (bounded_exempt && other_bound(g)));
        if sized {
            continue;
        }
        let own = prog.item_of.get(d as usize).copied().flatten().is_some_and(|ii| ii < prog.prelude_from);
        let self_used = own && ds.trait_of.as_deref() == Some(tr) && (ds.params.iter().skip(1).any(|&p| mentions_self(prog, p)) || mentions_self(prog, ds.ret));
        // (generics of its own: more than Self and the trait's own parameters)
        let own_generics = own && ds.trait_of.as_deref().and_then(|t| prog.trait_ngen.get(t)).is_some_and(|&n| ds.generics.len() > n);
        if !ds.has_self || self_used || own_generics {
            let m = prog.names[d as usize].rsplit("::").next().unwrap_or("").to_string();
            return Some(format!("the trait `{tr}` is not dyn compatible: `{m}` (E0038)"));
        }
    }
    None
}

fn mentions_self(prog: &crate::program::Program, t: Ty) -> bool {
    match t {
        Ty::Param(0) => true,
        Ty::Ref(_, x) | Ty::Ptr(_, x) | Ty::Array(x, _) | Ty::Slice(x) | Ty::Box(x) => mentions_self(prog, *x),
        Ty::Struct(sid) => prog.structs[sid as usize].symbolic && prog.structs[sid as usize].args.iter().any(|&a| mentions_self(prog, a)),
        Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&a| mentions_self(prog, a)),
        _ => false,
    }
}

/// whether a place expression runs a block on the way (`*{ x = z; &mut *y }`)
fn place_has_block(e: &Expr) -> bool {
    match e {
        Expr::Block(..) => true,
        Expr::Paren(x) | Expr::Unary(_, x, _) | Expr::Field(x, _, _) => place_has_block(x),
        Expr::Index(b, i, _) => place_has_block(b) || place_has_block(i),
        _ => false,
    }
}

/// a closure expression's body
fn c_body(c: &Expr) -> Expr {
    match c {
        Expr::Closure(_, _, _, b, ..) => (**b).clone(),
        e => e.clone(),
    }
}

/// whether a closure body is (or ends with) a `panic!`-like macro
fn panic_tail(e: &Expr) -> bool {
    let mut e = e;
    loop {
        match e {
            Expr::Paren(x) => e = x,
            Expr::Block(_, _, b) => match b.stmts.last() {
                Some(Stmt::Expr(x, _)) => e = x,
                _ => return false,
            },
            Expr::Macro(p, _, _) => return p.segs.last().is_some_and(|s| matches!(s.name.as_str(), "panic" | "unreachable" | "todo" | "unimplemented")),
            _ => return false,
        }
    }
}

/// whether a closure body ends by diverging in a way that makes `()` its return type: a `loop` with no `break`, a
/// `return`, or a `panic!`-like macro when a `return;` elsewhere says the type is `()` (a bare `|| panic!()` stays
/// open: passed to `fn f<R: Tr>(_: impl Fn() -> R)` rustc's fallback is `!`, not `()`)
fn diverging_tail(e: &Expr) -> bool {
    fn has_break(e: &Expr) -> bool {
        let mut found = false;
        crate::ast_closure::each_child_pub(e, &mut |c| found |= matches!(c, Expr::Break(..)) || has_break(c));
        found
    }
    fn has_unit_return(e: &Expr) -> bool {
        let mut found = false;
        crate::ast_closure::each_child_pub(e, &mut |c| {
            found |= matches!(c, Expr::Return(None, _)) || (!matches!(c, Expr::Closure(..)) && has_unit_return(c))
        });
        found
    }
    fn tail(e: &Expr) -> &Expr {
        match e {
            Expr::Paren(x) => tail(x),
            Expr::Block(_, _, b) => match b.stmts.last() {
                Some(Stmt::Expr(x, _)) => tail(x),
                _ => e,
            },
            e => e,
        }
    }
    match tail(e) {
        Expr::Loop(..) => !has_break(tail(e)),
        Expr::Return(..) => true,
        Expr::Macro(p, _, _) if p.segs.last().is_some_and(|s| matches!(s.name.as_str(), "panic" | "unreachable" | "todo" | "unimplemented")) => has_unit_return(e),
        _ => false,
    }
}

impl Cx<'_> {
    /// Where an inline const `const { .. }` names a local of the code around it (None if it does not, or if it binds
    /// names of its own with a `let` or a closure, which this does not follow)
    pub(crate) fn const_reads_local(&self, c: &Expr) -> Option<Sp> {
        fn walk(e: &Expr, out: &mut Vec<(String, Sp)>, own: &mut bool) {
            match e {
                Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => out.push((p.segs[0].name.clone(), p.pos)),
                Expr::Closure(..) | Expr::Let(..) | Expr::Match(..) | Expr::For(..) | Expr::Macro(..) => *own = true,
                Expr::Block(_, _, b) | Expr::Const(b) | Expr::Loop(_, b) => {
                    for st in &b.stmts {
                        match st {
                            Stmt::Expr(x, _) => walk(x, out, own),
                            _ => *own = true,
                        }
                    }
                }
                e => crate::ast_closure::each_child_pub(e, &mut |x| walk(x, out, own)),
            }
        }
        let (mut names, mut own) = (vec![], false);
        walk(c, &mut names, &mut own);
        if own {
            return None;
        }
        names.into_iter().find(|(n, p)| p.line != 0 && self.lookup(n).is_some()).map(|(_, p)| p)
    }
}

/// t holds a reference (in a fn pointer's parameters, a struct's arguments ..)
fn has_ref(t: Ty, depth: u32) -> bool {
    if depth > 16 {
        return true;
    }
    match t {
        Ty::Ref(..) => true,
        Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => has_ref(*i, depth + 1),
        Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&x| has_ref(x, depth + 1)),
        _ => false,
    }
}

/// A trait object's principal trait (None for an auto-only object: `dyn Send`, `dyn Sync`, `dyn Send + Sync`) and
/// whether it names Send and Sync.
fn dyn_norm(s: &str) -> (Option<&str>, bool, bool) {
    let (c, send, sync) = crate::types::dyn_autos(s);
    match c {
        "Send" => (None, true, sync),
        "Sync" => (None, send, true),
        c => (Some(c), send, sync),
    }
}
