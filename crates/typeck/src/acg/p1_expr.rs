use super::*;
use crate::parse::{Block, Expr, GArg, Pat, Path, Stmt};
use crate::program::{FnSig, PPath, TyEnv};
use crate::types::{Ty, intern};
use crate::acg::clif::{TrapCode, types};
use crate::ir::{IntCC, Value};

/// ラベルの名前 (syn の `l.ident` と同じく先頭の `'` を除く)
pub(crate) fn label_name(l: &str) -> String {
    l.trim_start_matches('\'').to_string()
}

/// クロージャ本体を普通の fn にする: `fn call(__env: (), a: _, __p1: _) { let (x, y) = __p1; 本体 }`。
/// (closure::synth と同じ。本体の型推論 (infer_ctx) に渡して、決まらない引数・戻り値の型を求めるのに使う)
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
        block: Block { stmts },
    }
}

impl Cx<'_> {
    // --- place (左辺値) ---------------------------------------------------

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
                let v = self.b.ins().iconst(types::I64, n as i64);
                let var = self.b.declare_var(types::I64);
                self.b.def_var(var, v);
                Ok(Place::Var(var, Ty::Usize))
            }
            // const (`LIMIT` / `S::N`) を場所として使う (`println!("{}", LIMIT)` / `&LIMIT`): 一時領域に置く
            Expr::Path(_, p) if self.const_of(e).is_some() && ident_of(p).is_none_or(|id| self.lookup(id).is_none()) => {
                let ci = self.const_of(e).unwrap();
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
                    if self.prog.consts[ci as usize].stat == Some(true) && self.unsafe_ok == 0 && p.pos.line != 0 && self.raw_ref == 0 {
                        return err(p.pos, "use of mutable static is unsafe and requires unsafe block (E0133)");
                    }
                    let t = self.prog.consts[ci as usize].ty;
                    let a = self.static_sym(ci);
                    return Ok(Place::Mem(a, 0, t));
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
                        // static はデータのアドレス
                        Some(&ci) if self.prog.consts[ci as usize].stat.is_some() => {
                            if self.prog.consts[ci as usize].stat == Some(true) && self.unsafe_ok == 0 && p.pos.line != 0 && self.raw_ref == 0 {
                                return err(p.pos, "use of mutable static is unsafe and requires unsafe block (E0133)");
                            }
                            let t = self.prog.consts[ci as usize].ty;
                            let a = self.static_sym(ci);
                            Ok(Place::Mem(a, 0, t))
                        }
                        // unit struct の値 (`struct U;` の `U`)
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
                            // `use std::f64::consts::PI;` してからの `PI`
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
            Expr::Field(base, fname, _) => {
                // (フィールドがなく Deref を実装した型 (`self.prog` で self: &Inf): `(*self.deref()).prog`)
                if let Some(Ty::Struct(sid)) = self.peek(base).map(|t| self.strip_refs(t)) {
                    if self.prog.field(sid, fname).is_none() {
                        // (a Deref chain that never reaches a field of that name: `impl Deref for Foo { type Target = Foo }`)
                        let mut t = sid;
                        let mut cycle = false;
                        for _ in 0..16 {
                            let Some(d) = self.prog.method(t, "deref") else { break };
                            let Ty::Ref(_, Ty::Struct(n)) = self.prog.sigs[d as usize].ret else { break };
                            t = *n;
                            if self.prog.field(t, fname).is_some() {
                                break;
                            }
                            if t == sid {
                                cycle = true;
                                break;
                            }
                        }
                        if cycle {
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
                            return self.place(f2);
                        }
                    }
                }
                let (a, o, sid) = self.struct_base(base)?;
                match self.prog.field(sid, fname) {
                    Some((ft, foff)) => Ok(Place::Mem(a, o + foff as i32, ft)),
                    None => err(SP, format!("no field `{fname}` on type `{}`", self.prog.structs[sid as usize].name)),
                }
            }
            Expr::Unary(op, x, sp) if op == "*" => {
                // `*g` (g: MutexGuard など Deref を実装した型): `*g.deref_mut()` / `*g.deref()`
                if let Some(d) = self.user_deref(x) {
                    let d = self.keep_expr(d);
                    self.deref_shared(d);
                    return self.place(d);
                }
                // `*b` は b をムーブしない
                let (v, t) = self.read_no_move(x)?;
                match (v, t) {
                    // (`match *x { [a, b] => .. }` with x: &[T]: the slice itself, as a place that holds its fat pointer;
                    // patterns on it bind by value)
                    // (a fat pointer's value is the address of its two words, which is what a slice place holds)
                    (Some(fat), Ty::Ref(_, Ty::Slice(e)) | Ty::Box(Ty::Slice(e))) => Ok(Place::Mem(fat, 0, Ty::Slice(e))),
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
                    _ => err(SP, format!("type `{}` cannot be dereferenced", self.prog.tn(t))),
                }
            }
            // `map[&k]`: get(&k).unwrap() の指す先
            Expr::Index(b, k, _)
                if matches!(self.peek(b).map(|t| self.strip_refs(t)),
                    Some(Ty::Struct(sid)) if self.prog.structs[self.prog.base_of(sid) as usize].name == "HashMap") =>
            {
                let e = self.keep_expr(mcall(mcall(Expr::Paren(b.clone()), "get", vec![(**k).clone()]), "unwrap", vec![]));
                let (v, t) = self.expr(e, None)?;
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
                let (v, t) = self.expr(e, None)?;
                match (v, t) {
                    (Some(p), Ty::Ref(_, inner)) => Ok(Place::Mem(p, 0, *inner)),
                    _ => err(SP, "`index` must return a reference"),
                }
            }
            Expr::Index(b, index, isp) if !matches!(&**index, Expr::Range(..)) => {
                let isp = *isp;
                let (base, len, elem) = self.indexable(b)?;
                let (i, it) = self.expr(index, Some(Ty::Usize))?;
                let Some(i) = i else {
                    return err(SP, "index diverges");
                };
                self.same(SP, it, Ty::Usize)?;
                // (`for k in 0..3` の k で長さ 3 以上の配列を引くなら、範囲内と分かっている)
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
                // 範囲外なら panic (rustc と同じく境界検査する)
                let ok = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                // (rustc の場所: Vec など Index の impl を通るものは `[`、配列・スライスは式の先頭)
                let via_impl = matches!(self.peek(b).map(|t| self.strip_refs(t)), Some(Ty::Struct(_)));
                let sp = if via_impl { isp.at } else { isp.start };
                self.index_check(ok, len, i, sp);
                let addr = self.elem_addr(base, i, elem);
                Ok(Place::Mem(addr, 0, elem))
            }
            _ => err(SP, "expression is not assignable"),
        }
    }

    /// フィールドアクセスの土台: 自動 deref しながら struct のアドレスを求める
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
            let r = self.expr(base, None)?;
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

    // --- 式 -------------------------------------------------------------

    /// 式を評価し、期待型への暗黙の変換 (`&[T; N]` → `&[T]`) をする
    pub(crate) fn expr(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // (the places inside a sub-expression are accesses of their own)
        let pd = std::mem::take(&mut self.bk.place_depth);
        self.bk.exp_none = std::mem::take(&mut self.bk.move_mut_next) && expected.is_none();
        let r = self.expr_inner(e, expected);
        self.bk.place_depth = pd;
        let (v, t) = r.map_err(|m| {
            // (エラーに、位置を持つ一番内側の式の先頭の行:桁を付ける。ACG_DEBUG_AT=1 なら式も)
            let m = if std::env::var_os("ACG_DEBUG_AT").is_some() && !m.contains(" (at `") {
                let d = format!("{e:?}");
                format!("{m} (at `{}`)", d.chars().take(400).collect::<String>())
            } else {
                m
            };
            self.at(err_pos(e), m)
        })?;
        // `&Box<T>` → `&T` (deref の強制変換: `e = x;` で x: &Box<Expr>)
        if let (Some(want @ Ty::Ref(m2, we)), Some(val), Ty::Ref(m1, Ty::Box(bi))) = (expected, v, t) {
            if *we == **bi && !matches!(bi, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) && (m1 || !m2) {
                let p = self.b.ins().load(PTR, mf(), val, 0);
                return Ok((Some(p), want));
            }
        }
        // `&T` / `&mut T` → `*const T` / `*mut T`
        if let (Some(want @ Ty::Ptr(m2, pe)), Ty::Ref(m1, re)) = (expected, t) {
            if pe == re && (m1 || !m2) {
                return Ok((v, want));
            }
        }
        if let (Some(want @ Ty::Ref(m2, Ty::Slice(we))), Some(v), Ty::Ref(m1, Ty::Array(ge, n))) = (expected, v, t) {
            if we == ge && (m1 || !m2) {
                let fat = self.make_fat(v, *n as i64);
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
        // `&mut T` → `&T` (`Box<T>` も `&mut T` なのでここで `&T` になる)
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
        // `Box<[T; N]>` → `Box<[T]>` (the pointer and N)
        if let (Some(want @ Ty::Box(Ty::Slice(we))), Some(v), Ty::Box(Ty::Array(ge, n))) = (expected, v, t) {
            if we == ge {
                let fat = self.make_fat(v, *n as i64);
                return Ok((Some(fat), want));
            }
        }
        // `Box<T>` → `Box<dyn Trait>`
        if let (Some(want @ Ty::Box(Ty::Dyn(tr))), Some(v), Ty::Box(inner)) = (expected, v, t) {
            if !matches!(inner, Ty::Dyn(_)) {
                let fat = self.to_dyn(v, *inner, tr.as_str(), SP)?;
                return Ok((Some(fat), want));
            }
        }
        // `&T` → `&dyn Trait` (データのポインタ + vtable)
        if let (Some(want @ Ty::Ref(m2, Ty::Dyn(tr))), Some(v), Ty::Ref(m1, inner)) = (expected, v, t) {
            if !matches!(inner, Ty::Dyn(_)) && (m1 || !m2) {
                let fat = self.to_dyn(v, *inner, tr.as_str(), SP)?;
                return Ok((Some(fat), want));
            }
        }
        Ok((v, t))
    }

    pub(crate) fn expr_inner(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // 期待型がなければ、型推論が後ろの使用から決めた型を使う
        // (syn 版の in_fmt_args のときの pos_key の引き直しは、自前の構文木にはないので省く)
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
                // `None::<u32>`: 型引数を書いたバリアント
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
                self.variant_value(sid, vi, &[], expected)
            }
            Expr::Match(..) => self.match_(e, expected),
            // const ジェネリクスの N: インスタンスの型引数の長さ
            Expr::Path(..) if self.const_generic(e).is_some() => {
                let n = self.const_generic(e).unwrap();
                // (`const N: u8`: the parameter's type is not kept; an expected integer type is taken as it)
                let t = match expected {
                    Some(t) if t.is_int() => t,
                    _ => Ty::Usize,
                };
                Ok((Some(self.int_const(t, n as i128)), t))
            }
            // const は式そのものを埋め込む
            Expr::Path(..) if self.const_of(e).is_some() => {
                let ci = self.const_of(e).unwrap();
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
                let r = self.read(p);
                if self.bk_on() && (!self.bk_ty(r.1).0 || exp_none && matches!(r.1, Ty::Ref(true, _))) {
                    self.bk_acc(e, borrowck::Acc::Move);
                }
                // drop が要る値を値として読んだらムーブ
                if self.needs_drop(r.1) {
                    let mut root = e;
                    while let Expr::Field(b, _, _) = strip_paren(root) {
                        root = b;
                    }
                    if let Expr::Path(_, rp) = strip_paren(root) {
                        if let Some(id) = ident_of(rp) {
                            if let Some(c) = self.caps.iter().find(|c| c.0 == id) {
                                // 値で持ったキャプチャ (FnOnce) から出す: 退避して環境の側を「空」(0) にする
                                // (環境はそのまま drop されるが、0 の Vec / String / Box は何も解放しない)
                                if let (false, Expr::Path(..), Place::Mem(a, o, t), Some(v)) = (c.2, strip_paren(e), p, r.0) {
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
                    // 一時値のフィールドのムーブ (`f().0`): 一時値ごとの drop をやめ、残りのフィールドだけをここで drop する
                    // (Drop を実装した型からはフィールドをムーブできないので、フィールドごとに drop してよい)
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
                            // フィールドだけのムーブ: 値を退避して元を「空」(0) にする。
                            // 持ち主はそのまま drop される (0 の Box は解放しない、0 の Option は None)
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
                let p = self.place(e)?;
                // (`*slice` as a value: its size is not known; only a place (a pattern, `&*s`) can be one)
                if matches!(p.ty(), Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) {
                    return err(expr_start(e), format!("the size for values of type `{}` cannot be known at compilation time", self.prog.tn(p.ty())));
                }
                if self.bk_on() && !self.bk_ty(p.ty()).0 {
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
                // `*b` (b: Box<T>、T は drop が要る): 中身のムーブ。b は drop しない
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
            Expr::Binary(..) => self.binary(e, expected),
            Expr::AssignOp(_, left, _, _) => {
                // (an overloaded `+=` is a call of `add_assign(&mut self, ..)`: no access of its own)
                let builtin = !matches!(self.peek(left), Some(Ty::Struct(_) | Ty::Ref(_, Ty::Struct(_))));
                let r = self.binary(e, expected)?;
                if builtin {
                    self.bk_acc(left, borrowck::Acc::Modify);
                }
                Ok(r)
            }
            Expr::Assign(left, right) => {
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
                // -O: 条件と本体で変わらない添字の受け手は、ループの前で読んでおく
                let nh = self.hoisted.len();
                if self.prog.opt {
                    self.hoist_index_bases(wbody, None, Some(cond))?;
                }
                self.b.ins().jump(header, &[]);
                self.b.switch_to_block(header);
                let t0 = self.temps.len();
                let (c, ct) = self.expr(cond, Some(Ty::Bool))?;
                self.same(SP, ct, Ty::Bool)?;
                // (the condition's temporaries are dropped before the body: `while borrow().ok() { .. }`)
                if c.is_some() {
                    self.drop_temps(t0);
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
                    label: self.pending_label.take(),
                    val: None,
                    is_block: false,
                    depth: self.scopes.len(),
                    temps: t0,
                });
                let (_, bt) = self.block(wbody, Some(Ty::Unit))?;
                self.loops.pop();
                if bt != Ty::Never {
                    self.same(SP, bt, Ty::Unit)?;
                    self.drop_for_exit(usize::MAX, t0);
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
                });
                let (_, bt) = self.block(lbody, Some(Ty::Unit))?;
                let lp = self.loops.pop().unwrap();
                if bt != Ty::Never {
                    self.same(SP, bt, Ty::Unit)?;
                    self.drop_for_exit(usize::MAX, lp.temps);
                    self.b.ins().jump(body, &[]);
                } else {
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                }
                self.b.switch_to_block(exit);
                if lp.breaks == 0 {
                    // break のない loop は値を返さない (型は !)
                    return Ok((None, Ty::Never));
                }
                // `break v` の値は exit の引数
                if let Some(t) = lp.val {
                    let v = self.b.block_params(exit).first().copied();
                    return Ok((v, t));
                }
                Ok((None, Ty::Unit))
            }
            Expr::Break(label, value) => {
                let label = label.as_deref().map(label_name);
                let Some(li) = self.loop_index(label.as_deref(), true) else {
                    return err(SP, "`break` outside of a loop (or unknown label)");
                };
                // `break v`: 値を評価してから抜ける (型は最初の break で決まる)
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
                let args: Vec<Value> = args;
                self.b.ins().jump(t, &args);
                Ok(self.dead())
            }
            Expr::Continue(label) => {
                let label = label.as_deref().map(label_name);
                let Some(li) = self.loop_index(label.as_deref(), false) else {
                    return err(SP, "`continue` outside of a loop (or unknown label)");
                };
                let lp = &self.loops[li];
                let (t, d, tt) = (lp.cont, lp.depth, lp.temps);
                self.drop_for_exit(d, tt);
                self.b.ins().jump(t, &[]);
                Ok(self.dead())
            }
            Expr::Return(value) => {
                let ret = self.ret;
                let v = match value {
                    Some(e) => {
                        self.mark_ret_call(e);
                        self.bk_frame();
                        let r = self.expr(e, Some(ret));
                        let srcs = self.bk_frame_end();
                        let (v, t) = r?;
                        if t == Ty::Never {
                            return Ok((None, Ty::Never));
                        }
                        if borrowck::has_refs(self.prog, ret, 0) {
                            { let _ = &srcs; self.bk_ret(e); }
                        }
                        self.same(expr_start(e), t, ret)?;
                        v
                    }
                    None => {
                        self.same(SP, Ty::Unit, ret)?;
                        None
                    }
                };
                self.drop_for_exit(0, 0);
                self.emit_return(v);
                Ok(self.dead())
            }
            Expr::Call(..) => self.call(e, expected),
            Expr::Closure(..) => self.closure_value(e, expected),
            Expr::Range(..) => self.range_value(e, expected),
            Expr::Tuple(t) if t.is_empty() => Ok((None, Ty::Unit)),
            Expr::Tuple(_) => self.tuple_lit(e, expected),
            Expr::Array(_) => self.array_lit(e, expected),
            Expr::Repeat(..) => self.repeat_lit(e, expected),
            Expr::Index(_, ix, _) if matches!(&**ix, Expr::Range(..)) => err(SP, "range indexing needs a reference: `&a[lo..hi]`"),
            Expr::Index(..) => {
                let p = self.place(e)?;
                if self.bk_on() && !self.bk_ty(p.ty()).0 {
                    self.bk_acc(e, borrowck::Acc::Move);
                }
                Ok(self.read(p))
            }
            Expr::For(..) => self.for_(e),
            Expr::MethodCall(..) => self.method_call(e, expected),
            Expr::Cast(x, ty) => {
                let to = self.parse_ty(ty)?;
                // `&x as &dyn Trait` は期待型つきの評価 (強制変換) と同じ
                // (`&mut a as &mut [u8]` (a: 配列) も強制変換)
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
                // (`Box::new(s) as Box<dyn Tr>` / `as Rc<dyn Tr>`: the unsizing coercion too)
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
                    return Ok((v, t));
                }
                let int_lit = matches!(strip_paren(x), Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Int(i)) if i.suffix().is_empty()));
                let hint = if int_lit && matches!(to, Ty::Ptr(..)) {
                    // (`0 as *mut u8`: an address)
                    Some(Ty::Usize)
                } else {
                    self.peek(x).or(match &**x {
                        Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Float(_))) => Some(Ty::F64),
                        _ => Some(Ty::I32),
                    })
                };
                let (v, from) = self.expr(x, hint)?;
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
                // 参照 → 生ポインタ、生ポインタ ⇔ 生ポインタ / usize
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
                    // (`&[T]` as `*const [T]`: the same fat pointer)
                    (Ty::Ref(_, Ty::Slice(e)), Ty::Ptr(_, Ty::Slice(e2))) if e == e2 => return Ok((v, to)),
                    (Ty::Ref(_, a), Ty::Ptr(_, b)) if a == b || matches!(*a, Ty::Array(e, _) if e == b) => {
                        return Ok((v, to));
                    }
                    // (a raw slice pointer is the address of its (data, length) pair here: as a thin pointer, the data
                    // word)
                    (Ty::Ptr(_, Ty::Slice(_) | Ty::Str), Ty::Ptr(_, b)) if !matches!(b, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) => {
                        return Ok((v.map(|a| self.b.ins().load(PTR, mf(), a, 0)), to));
                    }
                    (Ty::Ptr(..), Ty::Ptr(..)) => return Ok((v, to)),
                    // (a fn pointer as a raw pointer: the same address)
                    (Ty::Fn(_), Ty::Ptr(..)) => return Ok((v, to)),
                    (Ty::Ptr(..), t) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    // 関数ポインタのアドレス (`f as usize`)
                    (Ty::Fn(_), t) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    (t, Ty::Ptr(..)) if matches!(t, Ty::Usize | Ty::U64 | Ty::Isize | Ty::I64) => {
                        return Ok((v, to));
                    }
                    // (a narrower or wider integer and a raw pointer: through usize, as `as` does)
                    (t, Ty::Ptr(_, i)) if t.is_int() && t.bits() <= 64 && !matches!(i, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) => {
                        return Ok((v.map(|x| self.convert(x, t, Ty::Usize)), to));
                    }
                    (Ty::Ptr(_, i), t) if t.is_int() && t.bits() <= 64 && !matches!(i, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) => {
                        return Ok((v.map(|x| self.convert(x, Ty::Usize, t)), to));
                    }
                    _ => {}
                }
                // フィールドのない enum → 整数: タグ (= 判別値) を読む
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
                // 浮動小数点が関わるキャスト (Rust の `as` と同じく、浮動小数点 → 整数は飽和し NaN は 0)
                if from.is_float() || to.is_float() {
                    let v = v.unwrap();
                    let span = SP;
                    // 128 bit 整数 ⇔ 浮動小数点は prelude の関数 (Cranelift にない)
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
                            // 64 bit に飽和させてから、狭い型は範囲に切り詰める
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
                // 整数 ⇔ char / bool → 整数 (char には u8 からだけ)
                let ok = (to.is_int() && (from.is_int() || matches!(from, Ty::Bool | Ty::Char))) || (to == Ty::Char && matches!(from, Ty::U8 | Ty::Char));
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
            Expr::Const(b) => self.block(b, expected),
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
        let t = inner_exp.filter(|t| !matches!(t, Ty::Dyn(_) | Ty::Slice(_) | Ty::Str)).or_else(|| self.peek(rexpr));
        let t = t.filter(|&t| !self.needs_drop(t) && !self.prog.has_param(t) && !self.interior_mut(t, 0))?;
        let dv = crate::ast_consteval::eval(self.prog, rexpr, t).ok()?;
        if !dv.relocs.is_empty() || dv.bytes.is_empty() {
            return None;
        }
        let a = self.data_addr(dv.bytes);
        Some((Some(a), Ty::Ref(false, intern(t))))
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
                    return Ok((v, Ty::Ref(m, intern(pointee))));
                }
            }
        }
        // `&a[lo..hi]` → スライス (fat pointer)
        if let Expr::Index(b, ix, _) = &**rexpr {
            if let Expr::Range(..) = &**ix {
                return self.slice_range(b, ix, m);
            }
        }
        let fn_item = matches!(&**rexpr, Expr::Path(..) if self.fn_item(rexpr).is_some());
        // (`&None` などのバリアント・定数の名前は場所でなく一時値)
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
            let pm = std::mem::replace(&mut self.bk.place_mut, m);
            let pl = self.place(rexpr);
            self.bk.place_mut = pm;
            let pl = pl?;
            // (`&mut x as *mut _` / coerced to a raw pointer: not a loan)
            if !matches!(expected, Some(Ty::Ptr(..))) {
                self.bk_borrow(rexpr, m, err_pos(rexpr));
            }
            return match pl {
                // `&mut v[i]` (v: Vec<Box<T>>) を `&mut T` の位置に: Box の中身を指す (deref の強制変換)
                Place::Mem(a, o, Ty::Box(x)) if matches!(expected, Some(Ty::Ref(_, e)) if *e == *x) && !matches!(x, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) => {
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
        // 一時値への参照: 一時領域に置いてそのアドレスを取る
        let inner_exp = match expected {
            // (`f(&3)` with `f(x: *const isize)`: the pointee too, for the coercion to the raw pointer)
            // (not a pointer to a pointer: `&mut &mut x` is not `*mut *mut T`)
            Some(Ty::Ref(_, i)) => Some(*i),
            Some(Ty::Ptr(_, i)) if !matches!(i, Ty::Ptr(..) | Ty::Ref(..)) => Some(*i),
            _ => None,
        };
        // 定数式への共有参照は静的データに昇格する (`fn f() -> &'static S { &S { x: 1 } }`)
        if !m && is_promotable(self, rexpr) {
            if let Some(r) = self.promote(rexpr, inner_exp) {
                return Ok(r);
            }
        }
        let (v, t) = self.expr(rexpr, inner_exp)?;
        if t == Ty::Never {
            return Ok((None, Ty::Never));
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
        // `&一時値`: 一時値は文 (let ならブロック) の終わりまで所有する
        self.own_temp_at(a, t);
        Ok((Some(a), Ty::Ref(m, intern(t))))
    }

    pub(crate) fn struct_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Struct(path, sfields, rest) = e else { return err(SP, "internal: expected a struct literal") };
        if let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(path), self.self_sid) {
            let v = &self.prog.structs[sid as usize].variants[vi];
            if v.kind != crate::types::VariantKind::Struct {
                return err(SP, "this variant is not a struct-like variant");
            }
            let mut args = vec![];
            for (fid, fe) in sfields {
                // (タプルの番号 `0: x` は名前でない)
                if fid.parse::<u32>().is_ok() {
                    return err(SP, "expected a field name");
                }
                let Some(fi) = v.fields.iter().position(|f| f.0 == *fid) else {
                    return err(SP, format!("variant `{}` has no field named `{fid}`", v.name));
                };
                if args.iter().any(|&(i, _)| i == fi) {
                    return err(SP, format!("field `{fid}` specified more than once"));
                }
                args.push((fi, fe));
            }
            if args.len() != v.fields.len() || rest.is_some() {
                return err(SP, format!("missing fields in initializer of `{}`", v.name));
            }
            return self.variant_value(sid, vi, &args, expected);
        }
        let Some(sid) = self.prog.struct_by_p(&PPath::of(path), self.self_sid) else {
            return err(SP, "cannot find struct");
        };
        // `S { a: 1, ..base }`: base を一時変数に置き、書かれていないフィールドをそこからムーブする
        // (書いたフィールドの元の値は、一時変数のスコープの終わりで drop される)
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
                let ne: &Expr = self.keep_expr(Expr::Block(None, false, crate::parse::Block { stmts }));
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
        let mut args = vec![];
        for (name, fe) in sfields {
            let Some(fi) = fields.iter().position(|f| f.0 == *name) else {
                return err(SP, format!("struct `{}` has no field named `{name}`", self.prog.structs[sid as usize].name));
            };
            if args.iter().any(|&(i, _)| i == fi) {
                return err(SP, format!("field `{name}` specified more than once"));
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
        self.adt_value(sid, None, &args, expected, SP)
    }

    /// `|..| ..`: キャプチャを struct に詰めた値。型引数 (キャプチャ・引数・戻り値の型) をここで決める
    pub(crate) fn closure_value(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Closure(_, inputs, _, _, _) = c else { return err(SP, "internal: expected a closure") };
        let prog = self.prog;
        let hint = self.closure_hint.take();
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
        // 期待型がこのクロージャの型そのもの (戻り値位置の `impl Fn`) なら型引数ごと使う
        let exp_args = match expected {
            Some(Ty::Struct(sid)) if prog.base_of(sid) == cd.sid => Some(prog.structs[sid as usize].args.clone()),
            _ => None,
        };
        // 期待型が `dyn Fn(A) -> B` なら引数と戻り値の手がかりにする (`fn(A) -> B` も)
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
        let mut caps = vec![];
        for (i, n) in cd.captures.iter().enumerate() {
            let Some((loc, t)) = self.lookup(n) else {
                return err(SP, format!("cannot capture `{n}` here"));
            };
            let by_move = cd.moved.get(i).copied().unwrap_or(false);
            let ct = if by_move { t } else { Ty::Ptr(true, intern(t)) };
            sol[cd.n_parent + i] = Some(ct);
            caps.push((loc, t));
            self.bk_capture(c, n, loc, t, by_move);
        }
        let mut g = cd.n_parent + cd.captures.len();
        let mut pslots = vec![];
        for (i, p) in inputs.iter().enumerate() {
            if p.1.is_some() {
                continue;
            }
            pslots.push((i, g));
            // 呼び先の境界 → 親の型推論 (`let f = |x| x; f(1u8)` の使われ方) の順
            sol[g] = hint
                .as_ref()
                .and_then(|h| h.0.get(i).copied())
                .filter(|&t| !prog.has_param(t))
                .or_else(|| self.lets.get(&(&p.0 as *const Pat as usize)).copied());
            g += 1;
        }
        // 戻り値: 注釈 → 手がかり → 本体の型推論
        let ri = ng - 1;
        sol[ri] = cd
            .ret_ann
            .map(|t| prog.partial_subst(t, &sol))
            .or(hint.as_ref().and_then(|h| h.1).filter(|&t| !prog.has_param(t)))
            .or_else(|| self.lets.get(&(c as *const Expr as usize)).copied().filter(|&t| !prog.has_param(t)))
            .or(hint.as_ref().and_then(|h| h.1));
        if std::env::var_os("ACG_DEBUG_CLO").is_some() {
            eprintln!("closure k={k} ret_ann={:?} hint={:?} rec={:?} -> {:?}", cd.ret_ann, hint, self.lets.get(&(c as *const Expr as usize)), sol[ri]);
        }
        if let Some(a) = exp_args {
            for (s, a) in sol.iter_mut().zip(a) {
                *s = Some(a);
            }
        }
        // 一部だけ分かっている戻り値型 (`Result<?, u8>`) は型推論の手がかりにする
        let partial_ret = sol[ri].filter(|&t| prog.has_param(t));
        if partial_ret.is_some() {
            sol[ri] = None;
        }
        // 決まらない引数・戻り値は本体の型推論で (`let f = |x| x + k;`)
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
        let Some(args) = sol.into_iter().collect::<Option<Vec<Ty>>>() else {
            return err(SP, "cannot infer the return type of this closure; annotate it (`|..| -> T { .. }`)");
        };
        // (a closure with no captures where a fn pointer is expected: the address of its fn-pointer instance)
        if matches!(expected, Some(Ty::Fn(_))) && cd.captures.is_empty() {
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
        // 呼ばれなくても本体を検査・コンパイルする (未使用なら --gc-sections で消える)
        {
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(prog, cid, args).or_else(|e| err(SP, e))?;
            drop(g);
            self.requested.push(iid);
        }
        let st = Ty::Struct(sid);
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

    /// `&T` (アドレス v) を `&dyn tr` の fat pointer にする
    pub(crate) fn to_dyn(&mut self, v: Value, t: Ty, tr: &str, span: Sp) -> R<Value> {
        let mut fns = vec![];
        if let (Some((_, bps, _)), Ty::Fn(f)) = (crate::ast_summary::parse_fn_bound(tr), t) {
            // 関数ポインタ: データ (関数ポインタを置いた場所) から呼ぶ prelude の中継関数
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
            let Some(cid) = cid else {
                return err(span, format!("`{}` is not a closure", self.prog.tn(t)));
            };
            let Ty::Struct(sid) = t else { unreachable!() };
            let args = self.prog.structs[sid as usize].args.clone();
            let mut g = self.insts.lock().unwrap();
            let iid = g.request(self.prog, cid, args).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(iid);
            fns.push(iid);
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
            if !auto && !self.prog.implements(t, tr) {
                return err(span, format!("the trait `{tr}` is not implemented for `{}`", self.prog.tn(t)));
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
            // (E0038 too: a supertrait of the program's trait that takes Self by default (`trait Expr: PartialEq`))
            {
                let mut st = vec![tr.to_string()];
                let mut seen: Vec<String> = vec![];
                while let Some(t) = st.pop() {
                    if seen.contains(&t) {
                        continue;
                    }
                    for sup in self.prog.trait_supers.get(&t).cloned().unwrap_or_default() {
                        if sup == "#SelfRhs" {
                            return err(span, format!("the trait `{tr}` is not dyn compatible: it uses `Self` as a type parameter (E0038)"));
                        }
                        st.push(sup.split('<').next().unwrap_or("").trim().to_string());
                    }
                    seen.push(t);
                }
            }
            // (E0038: a method without `where Self: Sized` makes the trait not dyn compatible when it has no `self`,
            // or, for a method the program's own trait declares (not a supertrait's, not the prelude's, which leave
            // out their `where Self: Sized`), when `Self` is in its parameters or result)
            for &d in &decls {
                let ds = &self.prog.sigs[d as usize];
                let sized = ds.generics.first().is_some_and(|g| g.1.iter().any(|b| b == "Sized"));
                if sized {
                    continue;
                }
                let own = self.prog.item_of.get(d as usize).copied().flatten().is_some_and(|ii| ii < self.prog.prelude_from);
                let self_used = own && ds.trait_of.as_deref() == Some(tr) && (ds.params.iter().skip(1).any(|&p| mentions_self(self.prog, p)) || mentions_self(self.prog, ds.ret));
                // (generics of its own: more than Self and the trait's own parameters)
                let own_generics = own && ds.trait_of.as_deref().and_then(|t| self.prog.trait_ngen.get(t)).is_some_and(|&n| ds.generics.len() > n);
                if !ds.has_self || self_used || own_generics {
                    let m = self.prog.names[d as usize].rsplit("::").next().unwrap_or("").to_string();
                    return err(span, format!("the trait `{tr}` is not dyn compatible: `{m}` (E0038)"));
                }
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
                // ジェネリックな impl (`impl<T> Tr for W<T>`) のメソッドはさらにインスタンス化する
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
        // 最後の欄は drop glue (`Box<dyn ..>` の drop で使う)
        let de = self.vtable_drop_entry(t);
        fns.push(de);
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

    /// vtable の idx 番目を呼ぶ。fat は `&dyn` の (データ, vtable) のアドレス
    pub(crate) fn dyn_call(&mut self, fat: Value, idx: usize, params: &[Ty], ret: Ty, args: &[&Expr], span: Sp) -> R<V> {
        self.indirect_call(Indirect::Vtable(fat, idx), params, ret, args, span)
    }

    /// 関数ポインタ / vtable 経由の呼び出し
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
        for (a, &pt) in args.iter().zip(params) {
            let (mut v, mut t) = self.expr(a, Some(pt))?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            // (`&Box<T>` / `&&T` を `&T` の引数に: deref の強制変換)
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

    /// `&dyn Fn(A) -> B` の呼び出し (vtable の 0 番目がクロージャの本体)
    pub(crate) fn dyn_fn_call(&mut self, fat: Value, s: &str, args: &[&Expr], span: Sp) -> R<V> {
        let (ps, r) = self.dyn_fn_sig(s).or_else(|e| err(span, e))?;
        self.dyn_call(fat, 0, &ps, r, args, span)
    }

    /// `x.m(..)` (x: `&dyn Trait`)
    pub(crate) fn dyn_method_call(&mut self, fat: Value, tr: &str, mc: &Expr) -> R<V> {
        let Expr::MethodCall(_, name, _, margs, _) = mc else { return err(SP, "internal: expected a method call") };
        let (trn, targs) = self.dyn_trait_args(tr);
        let binds = if tr.contains('=') { self.dyn_bindings(tr) } else { vec![] };
        let tr = trn.as_str();
        let decls = self.prog.dyn_methods(tr);
        let Some(idx) = decls.iter().position(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(name.as_str())) else {
            return err(SP, format!("no method named `{name}` found for `dyn {tr}`"));
        };
        let ds = &self.prog.sigs[decls[idx] as usize];
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
        // (the trait's parameters are the arguments of `dyn Tr<A>`; Self は引数・戻り値に出てこない (object safe) 前提)
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
        self.dyn_call(fat, idx, &ps, ret, &args, SP)
    }

    /// 呼び先の引数の型 pt が `F` / `&F` / `&mut F` で、F に `Fn(A) -> B` の境界があれば (A, B)
    /// (呼び先のシグネチャの型パラメータを Param で含む)
    pub(crate) fn fn_bound_of(&self, sig: &FnSig, pt: Ty) -> Option<(Vec<Ty>, Ty)> {
        let pt = match pt {
            Ty::Ref(_, i) => *i,
            t => t,
        };
        let Ty::Param(g) = pt else { return None };
        let (_, bounds) = sig.generics.get(g as usize)?;
        let (_, ps, r) = bounds.iter().find_map(|b| crate::ast_summary::parse_fn_bound(b))?;
        let names: Vec<&str> = sig.generics.iter().map(|g| g.0.as_str()).collect();
        let ps = ps.iter().map(|t| self.prog.resolve_g(t, &names).ok()).collect::<Option<Vec<Ty>>>()?;
        Some((ps, self.prog.resolve_g(&r, &names).ok()?))
    }

    /// クロージャの型 (struct のインスタンス) なら、その引数と戻り値の型
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

    /// 関連型の制約から型パラメータを解く (`I: Iterator<Item = T>` / `where Self: Iterator<Item = &T>`)。
    /// `collect::<Vec<_>>()` の `_` は `Self::Item` で埋める
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

    /// 値としての `a..b` / `a..=b`: prelude の Range / RangeInclusive
    pub(crate) fn range_value(&mut self, rg: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Range(start, end, incl) = rg else { return err(SP, "internal: expected a range") };
        let incl = *incl;
        // `(1..)`: 終わりのない範囲
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

    /// `Fn(A, B) -> R` (dyn の中身) の引数と戻り値の型
    pub(crate) fn dyn_fn_sig(&self, s: &str) -> R<(Vec<Ty>, Ty)> {
        let Some((_, ps, r)) = crate::ast_summary::parse_fn_bound(s) else {
            return Err(format!("`dyn {s}` is not a function type"));
        };
        let ps = ps.iter().map(|t| self.prog.resolve_env(t, &self.env)).collect::<R<Vec<Ty>>>()?;
        Ok((ps, self.prog.resolve_env(&r, &self.env)?))
    }

    /// 関数でない値の呼び出し `f(x)`: クロージャ (の参照) か `&dyn Fn`
    pub(crate) fn call_value(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Call(func, cargs) = c else { return err(SP, "internal: expected a call") };
        // 呼び出しはムーブしない (Fn / FnMut は参照で呼ぶ)。一時値なら文の終わりまで所有する
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
        self.bk_frame();
        let r = match t {
            Ty::Struct(sid) => {
                let Some(&cid) = self.prog.closure_of_struct.get(&self.prog.base_of(sid)) else {
                    return err(SP, format!("expected a function, found `{}`", self.prog.tn(t)));
                };
                let recv = (v, Ty::Ptr(true, intern(t)));
                self.call_fn(cid, Some(recv), &args, None, expected, SP)
            }
            Ty::Ref(_, Ty::Dyn(s)) | Ty::Box(Ty::Dyn(s)) => self.dyn_fn_call(v, s, &args, SP),
            Ty::Fn(f) => self.indirect_call(Indirect::Ptr(v), &f.params, f.ret, &args, SP),
            t => err(SP, format!("expected a function, found `{}`", self.prog.tn(t))),
        };
        self.bk_frame_end_call();
        r
    }

    pub(crate) fn call(&mut self, c: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Call(func, cargs) = c else { return err(SP, "internal: expected a call") };
        // (`f1(x)` after `let f1 = f;` (f generic): `f(x)`)
        if let (false, Expr::Path(None, p)) = (self.fn_aliases.is_empty(), strip_paren(func)) {
            if let Some(&(_, target)) = ident_of(p).and_then(|n| self.fn_aliases.iter().rev().find(|a| a.0 == n)) {
                let ne: &Expr = self.keep_expr(Expr::Call(Box::new(target.clone()), cargs.clone()));
                return self.expr(ne, expected);
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
        // `From::from(x)` to an expected type T: `T::from(x)`; `Into::into(x)`: `x.into()`
        if let (Expr::Path(None, p), [x]) = (&**func, cargs.as_slice()) {
            let n = p.segs.len();
            if n >= 2 && p.segs.iter().all(|s| s.args.is_empty()) && (n == 2 || matches!(p.segs[0].name.as_str(), "std" | "core")) {
                match (p.segs[n - 2].name.as_str(), p.segs[n - 1].name.as_str()) {
                    ("Into", "into") if self.lookup("Into").is_none() => {
                        let e = self.keep_expr(mcall(x.clone(), "into", vec![]));
                        return self.expr(e, expected);
                    }
                    ("From", "from") => {
                        let simple = |t: Ty| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char) || matches!(t, Ty::Struct(sid) if self.prog.structs[sid as usize].base.is_none());
                        if let Some(t) = expected.filter(|&t| simple(t) && !self.prog.has_param(t)) {
                            if let Ok(Expr::Path(None, tp)) = crate::parse::parse_expr(&format!("{}::from", self.prog.tn(t))) {
                                let ne = self.keep_expr(Expr::Call(Box::new(Expr::Path(None, crate::parse::Path { pos: p.pos, ..tp })), cargs.clone()));
                                return self.expr(ne, expected);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // `i32::try_from(x)` (an integer type): `x.try_into()` to that type
        if let (Expr::Path(None, p), [x]) = (&**func, cargs.as_slice()) {
            if p.segs.len() == 2 && p.segs[1].name == "try_from" {
                if let Some(t) = Ty::from_suffix(&p.segs[0].name).filter(|t| t.is_int()) {
                    if let Some(rt) = self.result_of(t, "TryFromIntError") {
                        let e = self.keep_expr(mcall(x.clone(), "try_into", vec![]));
                        self.try_into_hint = Some(t);
                        return self.expr(e, Some(rt));
                    }
                }
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
            let user = self.prog.traits.contains_key(&p.segs[0].name) && direct.is_none();
            if !p.global && p.segs.len() == 2 && p.segs[0].args.is_empty() && (known || user)
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
                return self.expr(m, expected);
            }
        }
        if let Expr::Path(_, p) = &**func {
            if let Some((sid, vi)) = self.prog.variant_by_p(&PPath::of(p), self.self_sid) {
                let v = &self.prog.structs[sid as usize].variants[vi];
                if v.kind != crate::types::VariantKind::Tuple || v.fields.len() != cargs.len() {
                    return err(SP, format!("variant `{}` takes {} fields", v.name, v.fields.len()));
                }
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
                return self.variant_value(sid, vi, &args, expected);
            }
            // タプル構造体の構築 `P(1, 2)`
            if let Some(sid) = self.prog.struct_by_p(&PPath::of(p), self.self_sid) {
                if self.lookup(&p.segs[0].name).is_none() && self.prog.fn_by_p(&PPath::of(p), self.self_sid).is_none() {
                    let n = self.prog.structs[sid as usize].fields.len();
                    if n != cargs.len() {
                        return err(SP, format!("this struct takes {n} fields"));
                    }
                    let args: Vec<(usize, &Expr)> = cargs.iter().enumerate().collect();
                    return self.adt_value(sid, None, &args, expected, SP);
                }
            }
        }
        if let Some(r) = self.intrinsic_call(c, expected)? {
            return Ok(r);
        }
        let Some((id, _)) = self.callee(func) else {
            // 値の呼び出し: クロージャ / `&dyn Fn`
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
        // `Buf::<8>::new()` / `Vec::<i32>::new()`: 型のセグメントの型引数
        if let Expr::Path(_, p) = &**func {
            let n = p.segs.len();
            if n >= 2 && !p.segs[n - 2].args.is_empty() {
                let seg = p.segs[n - 2].clone();
                let ty = crate::parse::Ty::Path(None, Path { global: false, segs: vec![seg], pos: Default::default() });
                self.seg_self = Some(self.parse_ty(&ty)?);
            } else if n == 2 && self.prog.sigs[id as usize].self_ty.is_some_and(|t| self.prog.has_param(t)) {
                // `T::dec(r)` (T はインスタンスの中で具体的な型): ジェネリックな impl の型引数を Self から
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
        // (期待型が無ければ型推論の記録から: `x = Tr::dec(r)?` の Self)
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
        let r = self.call_fn(id, None, &args, tf, expected, SP);
        self.seg_self = None;
        r
    }
}

/// t names `Self` (the parameter 0 of a trait's method) itself, not through an associated type (`Self::Item`)
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
            found |= matches!(c, Expr::Return(None)) || (!matches!(c, Expr::Closure(..)) && has_unit_return(c))
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
        Expr::Return(_) => true,
        Expr::Macro(p, _, _) if p.segs.last().is_some_and(|s| matches!(s.name.as_str(), "panic" | "unreachable" | "todo" | "unimplemented")) => has_unit_return(e),
        _ => false,
    }
}
