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
    Expr::Block(None, false, PBlock { stmts: lets })
}

impl Cx<'_> {
    /// `recv.m(args)`: レシーバを自動 deref して struct を見つけ、
    /// メソッドの self 型に合わせて自動参照 (`&self` / `&mut self`) かコピー (`self`) で渡す
    pub(crate) fn method_call(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        // (a call that named its trait: set while this call is resolved, and the outer call's back after)
        let forced = match e {
            Expr::MethodCall(_, m, ..) => self.explicit_traits.get(&(e as *const Expr as usize)).map(|t| (m.clone(), t.clone())),
            _ => None,
        };
        let prev = std::mem::replace(&mut self.explicit_trait, forced);
        let n = self.caller_locs.len();
        let r = self.method_call_inner(e, expected);
        self.caller_locs.truncate(n);
        self.explicit_trait = prev;
        r
    }

    fn method_call_inner(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::MethodCall(recv, method, gargs, margs, msp) = e else { unreachable!() };
        let msp = *msp;
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
        // (`x.m()` with x of a type parameter `T: Tr` in a generic fn, instantiated as a primitive: Tr's `m`, which a
        // builtin method of the same name (`to_string` ..) would otherwise answer: `<T as Tr>::m(&x, ..)`)
        // (`impl Deref<Target = [T; N]>` (or a slice, str) with no method m of its own: `t.m()` is `t.deref().m()`,
        // decided before anything is evaluated, so the user's deref runs once; only for the structs Program found such a
        // deref of)
        if let Some(Ty::Struct(sid)) = self.peek(recv).map(|t| self.strip_refs(t)).filter(|t| matches!(t, Ty::Struct(s) if !self.prog.deref_to_seq.is_empty() && self.prog.deref_to_seq.contains(&self.prog.base_of(*s)))) {
            if self.prog.method(sid, method).is_none() && self.prog.default_method(sid, method).is_none() {
                if let Some(did) = self.prog.method(sid, "deref") {
                    if matches!(self.prog.sigs[did as usize].ret, Ty::Ref(_, Ty::Array(..) | Ty::Slice(_) | Ty::Str)) {
                        let d = Expr::MethodCall(Box::new(Expr::Paren(recv.clone())), "deref".to_string(), vec![], vec![], msp);
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
            if self.prog.method(sid, method).is_none() && self.prog.default_method(sid, method).is_none() {
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
        if let Expr::MethodCall(ir, iname, _, iargs, _) = strip_paren(recv) {
            if iargs.is_empty() && matches!(iname.as_str(), "as_slice" | "as_mut_slice") {
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
        if margs.is_empty() && matches!(method, "as_slice" | "as_mut_slice") {
            if let Some(Ty::Array(..)) = self.peek(recv).map(|t| self.strip_refs(t)) {
                let ix = Expr::Index(Box::new(paren(recv.clone())), Box::new(full_range()), crate::parse::Span::default());
                let e = Expr::Ref(method != "as_slice", Box::new(ix));
                return self.expr(&e, expected);
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
                        let e = Expr::Block(None, false, PBlock { stmts });
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
        // `o.flatten()` (Option<Option<T>>): `o.unwrap_or(None)`
        if method == "flatten" && margs.is_empty() {
            if let Some(Ty::Struct(sid)) = self.peek(recv) {
                let opt = |this: &Self, s: u32| this.prog.structs[this.prog.base_of(s) as usize].name == "Option";
                if opt(self, sid) && matches!(self.prog.structs[sid as usize].args.first(), Some(Ty::Struct(i)) if opt(self, *i)) {
                    let e = mcall(paren(recv.clone()), "unwrap_or", vec![path_expr("None")]);
                    return self.expr(&e, expected);
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
        if let Some(&bid) = self.prog.blanket.get(method) {
            let pt = self.peek(recv);
            let prim = |t: Ty| t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char);
            let own = match pt.map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.method(sid, method).is_some(),
                _ => false,
            };
            if !own && pt.is_some_and(|t| prim(t) || matches!(t, Ty::Ref(_, Ty::Str))) {
                let (v, t) = self.expr(recv, None)?;
                let Some(v) = v else { return Ok((None, Ty::Never)) };
                // (&self に渡すのは値のアドレス。&str は fat pointer の置き場所のアドレスがそのまま &&str)
                let addr = if t.is_fat() {
                    v
                } else {
                    let ss = self.slot(t);
                    self.b.ins().stack_store(v, ss, 0);
                    self.b.ins().stack_addr(PTR, ss, 0)
                };
                let args: Vec<&Expr> = margs.iter().collect();
                return self.call_fn(bid, Some((addr, Ty::Ref(false, intern(t)))), &args, tf, expected, SP);
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
                _ => false,
            };
            let target = expected.or_else(|| self.lets.get(&(e as *const Expr as usize)).copied());
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
                            let args: [&Expr; 1] = [recv];
                            return self.call_fn(id, None, &args, None, Some(Ty::Struct(sid)), SP);
                        }
                    }
                    Some(t) if t.is_int() || t.is_float() => {
                        let e = Expr::Cast(Box::new(paren(recv.clone())), ty_named(&self.prog.tn(t)));
                        return self.expr(&e, Some(t));
                    }
                    _ => {}
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
                if self.prog.method(sid, method).is_none() && self.prog.default_method(sid, method).is_none() {
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
            if self.prog.method(sid, method).is_none() && self.prog.default_method(sid, method).is_none() {
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
                        return self.dyn_method_call_on(tmp, tr, e, Some(vt));
                    }
                }
            }
        }
        let mut recv_temp = None;
        // (whether the receiver was reached through a reference: a `self` by value there moves out of it)
        let mut through_ref = false;
        // (a Box value that is not a place: `Box::new(s).m()`, for `self: Box<Self>`)
        let mut recv_box: Option<(Value, Ty)> = None;
        let mut p = if is_place_expr(recv) {
            self.place(recv)?
        } else {
            let r = self.expr(recv, None)?;
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
                    Place::Mem(v, 0, *i)
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
        let mut derefs = 0;
        let (a, o, sid, id) = loop {
            let (a, o, sid) = loop {
                p = match p {
                    Place::Mem(a, o, Ty::Struct(sid)) => break (a, o, sid),
                    Place::Mem(a, o, rt @ (Ty::Ref(_, Ty::Dyn(tr)) | Ty::Box(Ty::Dyn(tr)))) => {
                        let fat = self.addr_plus(a, o);
                        return self.dyn_method_call_on(fat, tr, e, Some(rt));
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
            if let Some(id) = self.prog.method(sid, &name).or_else(|| self.prog.default_method(sid, &name)) {
                break (a, o, sid, id);
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
            return err(SP, format!("no method named `{name}` found for `{sname}`"));
        };
        let sname = self.prog.structs[sid as usize].name.clone();
        if !self.prog.sigs[id as usize].has_self {
            return err(SP, format!("`{sname}::{name}` is an associated function, not a method"));
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
                        None => self.mark_moved(recv),
                    }
                    return self.call_fn(id, Some((bv, p0t)), &margs.iter().collect::<Vec<_>>(), tf, expected, SP);
                }
            }
        }
        if let (Ty::Struct(ps), true) = (self.prog.sigs[id as usize].params[0], via_deref) {
            if self.prog.structs[self.prog.base_of(ps) as usize].name == "Pin" && p0t == Ty::Struct(ps) {
                if let Place::Mem(a0, o0, _) = p0 {
                    let v0 = self.addr_plus(a0, o0);
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
                    if margs.iter().any(|a| borrowck::mentions(a, n)) {
                        let l = &self.bk.loans[*ln as usize];
                        let res = borrowck::Loan { place: l.place.clone(), mutable: false, sp: l.sp, at: l.at, quiet: l.quiet, capture: l.capture };
                        let rs = self.bk.loans.len() as u32;
                        self.bk.loans.push(res);
                        self.bk.reserved.push(rs);
                        self.bk_push(Some(borrowck::Ev::Acc(place.clone(), borrowck::Acc::Borrow(rs), *sp)));
                        self.bk_call_temp(&[borrowck::Src::Loan(rs)], true);
                        bk_op_temp = true;
                    }
                }
                bk_late = ev;
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
        if !matches!(recv_ty, Ty::Ref(..)) && self.needs_drop(st) {
            match recv_temp {
                Some(f) => self.clear_flag(f),
                None => self.mark_moved(recv),
            }
        }
        let args: Vec<&Expr> = margs.iter().collect();
        // (a `&mut self` method of a value that holds references stores what its arguments borrow:
        // `v.push(&mut x)`)
        let into_recv = self.bk_on() && !via_deref && matches!(recv_ty, Ty::Ref(true, _)) && self.bk_ty(st).1;
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
        // (the region check reports at the method call: call_fn is given no span for it)
        if self.rg_on() {
            self.rg.call_sp = borrowck::pos(e);
        }
        let r = self.call_fn(id, Some((recv_v, recv_ty)), &args, tf, expected, SP);
        self.rg_took_call(e);
        if into_recv {
            let srcs = self.bk_frame_end();
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
    pub(crate) fn takes_mut_self(&self, t: Ty, m: &str) -> bool {
        let t = self.strip_refs(t);
        if let Ty::Struct(ts) = t {
            if let Some(id) = self.prog.method(ts, m).or_else(|| self.prog.default_method(ts, m)) {
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

    fn check_spawn_closure(&self, c: &Expr) -> R<()> {
        let Expr::Closure(mv, _, _, _, cpos) = strip_paren(c) else { return Ok(()) };
        for n in crate::ast_closure::free_vars(strip_paren(c)) {
            let Some((_, t)) = self.lookup(&n) else { continue };
            if not_send(self.prog, t) {
                return Err(self.at(*cpos, format!("`{}` cannot be sent between threads safely", self.prog.tn(t))));
            }
            if !*mv && !crate::ast_closure::moves_capture(strip_paren(c), &[n.clone()]) {
                return Err(self.at(*cpos, format!("closure may outlive the current function, but it borrows `{n}`, which is owned by the current function")));
            }
        }
        Ok(())
    }

    /// 組み込みの AsRef (`s.as_ref()` → `s.as_bytes()` / `&s[..]`)。受け手が as_ref を持つ構造体 (Option など) なら None
    pub(crate) fn as_ref_rewrite(&self, recv: &Expr, expected: Option<Ty>) -> Option<Expr> {
        let (target, is_string, seq) = self.as_ref_target(recv, expected)?;
        let r = paren(recv.clone());
        Some(if target == "str" {
            if is_string { mcall(r, "as_str", vec![]) } else { Expr::Ref(false, Box::new(Expr::Index(Box::new(r), Box::new(Expr::Range(None, None, false)), Default::default()))) }
        } else if !seq {
            mcall(r, "as_bytes", vec![])
        } else {
            Expr::Ref(false, Box::new(Expr::Index(Box::new(r), Box::new(Expr::Range(None, None, false)), Default::default())))
        })
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
        let Expr::MethodCall(recv, method, gargs, margs, _) = e else { unreachable!() };
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
                let c = Expr::Call(Box::new(path_expr(&f)), args);
                return self.expr(&c, expected).map(Some);
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
                    let c = Expr::Call(Box::new(path_expr(&f)), args);
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
            let (v, vt) = self.expr(recv, None)?;
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
                let a = v;
                v = a;
                vt = Ty::Ref(false, intern(vt));
            }
            let args: Vec<&Expr> = margs.iter().collect();
            return self.call_fn(id, Some((v, vt)), &args, tf, expected, SP).map(Some);
        }
        if !(t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Box(_))) {
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
        let Some(id) = self.prog.prim_self_method(t, &name).or_else(|| match t {
            Ty::Box(_) if boxed_num(t) => self.prog.prim_self_method(leaf(t), &name),
            _ => None,
        }) else {
            return Ok(None);
        };
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
        // 受け手: 値ならそのまま、&self ならメモリに置いてそのアドレス
        let (v, vt) = self.expr(recv, lit_int.then_some(t))?;
        let mut v = v.unwrap();
        let mut vt = vt;
        while let Ty::Ref(_, i) = vt {
            v = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
            vt = *i;
        }
        let (recv_v, recv_ty) = match self.prog.sigs[id as usize].params.first() {
            Some(Ty::Ref(m, _)) => {
                let a = self.temp(t);
                self.b.ins().store(mf(), v, a, 0);
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
            let imp = match args[0] {
                Ty::Struct(sid) => self.prog.method(sid, &m),
                // (only an impl of the trait the call named: `A::m(&0.0f32)` is not B's `f32::m`)
                t if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char) => self.prog.index.get(&format!("{}::{m}", self.prog.tn(t))).copied().filter(|&i| {
                    let it = self.prog.sigs[i as usize].impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
                    it.is_none() || it == sig.trait_of.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim())
                }),
                _ => None,
            };
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
                let own = args.len() - 1;
                let k0 = s2.len().saturating_sub(own);
                for (j, &a) in args[1..].iter().enumerate() {
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
    pub(crate) fn pick_overload(&self, id: u32, recv: Option<Ty>, args: &[&Expr]) -> u32 {
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
        let cands: &Vec<u32> = if own.has_self || cands.iter().all(|&c| self.prog.sigs[c as usize].self_ty == own.self_ty) {
            cands
        } else {
            same_self = cands.iter().copied().filter(|&c| self.prog.sigs[c as usize].self_ty == own.self_ty).collect();
            if same_self.len() <= 1 {
                return id;
            }
            &same_self
        };
        let Some(at) = recv.or_else(|| args.first().and_then(|a| self.peek(a))) else {
            return id;
        };
        let fits = |c: u32, at: Ty| {
            let sig = &self.prog.sigs[c as usize];
            let mut sol = vec![None; sig.generics.len()];
            sig.params.first().is_some_and(|&pt| self.prog.match_ty(pt, at, &mut sol))
        };
        // (impls of two traits of one name, `a::M` and `b::M` for u32: the one whose trait is in scope here)
        let base = |c: u32| self.prog.sigs[c as usize].impl_trait.as_deref().map(|t| t.split('<').next().unwrap_or(t).trim());
        // (`&String` は `&str` の候補に: deref の強制変換)
        let deref = match at {
            Ty::Ref(m, Ty::Struct(sid)) if self.prog.structs[*sid as usize].name == "String" => Some(Ty::Ref(m, intern(Ty::Str))),
            Ty::Ref(_, Ty::Ref(m, i)) => Some(Ty::Ref(*m, i)),
            _ => None,
        };
        // (`a::M::m(&x)`: the impl of the trait the call named, also through the deref coercion; never an inherent
        // method of the same name)
        if let Some((m, tr)) = &self.explicit_trait {
            let name = &self.prog.names[id as usize];
            // (an impl for a reference, `impl PartialEq for &X`: its methods are `X::__ref_eq`)
            let last = name.rsplit("::").next().map(|l| l.split('#').next().unwrap_or(l));
            if last == Some(m.as_str()) || last.and_then(|l| l.strip_prefix("__ref_")) == Some(m.as_str()) {
                let of_tr = |c: u32| base(c) == Some(tr.as_str());
                if let Some(&c) = cands.iter().find(|&&c| of_tr(c) && fits(c, at)) {
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
        // (an inherent method that fits comes first, as in rustc's method lookup: then nothing changes here)
        let inherent = cands.iter().any(|&c| self.prog.sigs[c as usize].impl_trait.is_none() && fits(c, at));
        if let Some(vis) = self.traits_in_scope().filter(|_| !inherent) {
            if let Some(&c) = cands.iter().find(|&&c| base(c).is_some_and(|t| vis.iter().any(|v| v == t)) && fits(c, at)) {
                return c;
            }
        }
        if let Some(&c) = cands.iter().find(|&&c| fits(c, at)) {
            return c;
        }
        deref.and_then(|d| cands.iter().copied().find(|&c| fits(c, d))).unwrap_or(id)
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
            return err(pos, format!("call to unsafe function `{}` is unsafe and requires unsafe block (E0133)", n.strip_prefix("__core_").unwrap_or(n)));
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
        let r = self.call_fn_inner(id, recv, args, turbofish, expected, span);
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
        self.rg.depth -= 1;
        self.bk_call_temps_end(t0);
        r
    }

    fn call_fn_inner(&mut self, id: u32, recv: Option<(Value, Ty)>, args: &[&Expr], turbofish: Option<&[GArg]>, expected: Option<Ty>, span: Sp) -> R<V> {
        // (the receiver's region tree is this call's: the calls in the arguments must not take it)
        let rg_recv = if recv.is_some() { self.rg.recv.take() } else { None };
        let rg_sp = std::mem::take(&mut self.rg.call_sp);
        if self.prog.names.get(id as usize).is_some_and(|n| n == "__core_spawn") {
            if let Some(c) = args.first() {
                self.check_spawn_closure(c)?;
            }
        }
        // (戻り値を書く先: 引数の中の呼び出しには渡さない)
        let hint = self.dst_hint.take();
        let id0 = id;
        let id = self.pick_overload(id, recv.map(|r| r.1), args);
        // (E0133 was checked on the fn found before the overload was chosen: an `unsafe fn` chosen in its place, by
        // the traits in scope, needs the same check)
        if id != id0 && self.prog.sigs[id as usize].is_unsafe && !self.prog.sigs[id0 as usize].is_unsafe && self.unsafe_ok == 0 && self.user_code {
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
        let mut vals: Vec<(Value, Ty, Sp)> = vec![];
        if let Some((r, rt)) = recv {
            self.prog.match_ty(sig0.params[0], rt, &mut sol);
            vals.push((r, rt, span));
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
                Some(match (t, strip_paren(a)) {
                    (Ty::Ref(..), Expr::Ref(_, inner)) => mcall(paren((**inner).clone()), "iter", vec![]),
                    (Ty::Ref(..), _) => mcall(paren((*a).clone()), "iter", vec![]),
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
        for (i, (a, &pt)) in args.iter().zip(&sig0.params[skip..]).enumerate() {
            let mut exp = Some(self.prog.partial_subst(pt, &sol)).filter(|&t| !self.prog.has_param(t));
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
            // one that moves them out only `FnOnce` (E0525, at the closure))
            if let (Some(x), Expr::Path(None, _)) = (&fbt, strip_paren(a)) {
                let want = x.2.trim_start();
                let kind = |k: &str| want.strip_prefix(k).is_some_and(|r| r.trim_start().starts_with('('));
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
            let flows = !(top_ref && matches!(pt, Ty::Ref(..)) && !self.bk_ret_from(id, i + skip));
            if top_ref {
                self.bk_frame();
            }
            let r = self.expr(a, exp);
            if matches!(pt, Ty::Ref(true, _)) {
                self.bk_reborrow_arg(a);
            }
            // (a closure written where a bound wants `Fn` / `FnMut`: it is that, so its body moving out what it captures
            // by value (`|| { let s = val; }`) is E0507, at the move. Passed by name, it is E0525 (above))
            if let (Some(x), Expr::Closure(..), true) = (&fbt, strip_paren(a), self.bk_on()) {
                let want = x.2.trim_start();
                let kind = |k: &str| want.strip_prefix(k).is_some_and(|r| r.trim_start().starts_with('('));
                let c = strip_paren(a);
                if (kind("Fn") || kind("FnMut")) && self.bk.fnonce_sure.contains(&(c as *const Expr as usize)) {
                    let caps: Vec<String> = crate::ast_closure::free_vars(c).into_iter().filter(|n| self.lookup(n).is_some_and(|(_, t)| !self.bk_ty(t).0) && !crate::ast_closure::rebinds(c, std::slice::from_ref(n))).collect();
                    if let Some((n, at)) = crate::ast_closure::move_site(c, &caps) {
                        let w = if kind("Fn") { "Fn" } else { "FnMut" };
                        return Err(self.at(at, format!("cannot move out of `{n}`, a captured variable in an `{w}` closure (E0507)")));
                    }
                }
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
                self.bk_call_temp(&srcs, matches!(strip_paren(a), Expr::Ref(..)));
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
            vals.push((v, t, expr_start(a)));
        }
        let arg_tys: Vec<Ty> = vals.iter().map(|v| v.1).collect();
        // 引数の型から改めて解く。引数に現れない型パラメータ (クロージャの戻り値など) は sol から
        let mut sol0 = self.turbofish_sol(&sig0, turbofish)?;
        for (a, b) in sol0.iter_mut().zip(&sol) {
            if a.is_none() {
                *a = *b;
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
        // (a callee that requires 'static (`fn f<T: 'static>`, `&'static T`): from here on, checked)
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && self.prog.sigs[id as usize].regions.as_deref().is_some_and(Self::callee_needs_static) {
            self.rg_late_on();
        }
        let sol_rg = if self.rg_on() { Some(sol0.clone()) } else { None };
        let (target, sig) = self.resolve_call(id, &arg_tys, sol0, expected, span)?;
        if let Some(sol) = sol_rg {
            self.rg_call(id, &sol, turbofish, args, &sig.params, sig.ret, rg_recv, skip == 1, if span.line == 0 { rg_sp } else { span });
        }
        if self.prog.names[id as usize] == "__core_transmute" && sig.params.len() == 1 {
            self.check_transmute(sig.params[0], sig.ret, span)?;
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
        for ((v, t, sp), &pt) in vals.into_iter().zip(&sig.params) {
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
    pub(crate) fn lit(&mut self, text: &str, expected: Option<Ty>) -> R<V> {
        let Some(l) = lit::lit(text) else { return err(SP, "unsupported literal") };
        match &l {
            // 浮動小数点: `1.5` / `2e3` / `1.0f32` / `1f64` (と、浮動小数点が期待される整数リテラル)
            Lit::Float(_) | Lit::Int(_)
                if matches!(l, Lit::Float(_))
                    || matches!(&l, Lit::Int(i) if matches!(i.suffix(), "f32" | "f64"))
                    || (matches!(&l, Lit::Int(i) if i.suffix().is_empty()) && expected.is_some_and(|t| t.is_float())) =>
            {
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
                let ty = Ty::from_suffix(i.suffix()).or(expected.filter(|t| t.is_int())).unwrap_or(Ty::I32);
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
        if !(t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char)) {
            return None;
        }
        let gens = &self.prog.sigs.get(self.root as usize)?.generics;
        for (g, bounds) in gens {
            if *g != declared || self.env.params.iter().find(|p| &p.0 == g).map(|p| p.1) != Some(t) {
                continue;
            }
            for b in bounds {
                let tr = b.split('<').next().unwrap_or(b).trim();
                let Some(id) = self.trait_candidate(&format!("{}::{method}", self.ty_key(t)), tr) else { continue };
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
                    if selfm && !matches!(tr.as_str(), "Ord" | "PartialOrd" | "PartialEq" | "Eq") {
                        let mut args = cargs.iter();
                        let a0 = args.next()?;
                        let rest: Vec<Expr> = args.cloned().collect();
                        let recv = match a0 {
                            Expr::Ref(_, e) => paren((**e).clone()),
                            _ => paren(a0.clone()),
                        };
                        // (a primitive receiver with an impl of this trait: `<u32 as Tr>::m(&x)`, which a builtin
                        // method of the same name (`to_string`) cannot take over as `x.m()` would let it)
                        if let Some(t) = self.peek(&recv).map(|t| self.strip_refs(t)) {
                            if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char) {
                                let key = format!("{}::{ms}", self.ty_key(t));
                                if self.trait_candidate(&key, &tr).is_some() {
                                    if let Ok(Expr::Path(q, qp)) = crate::parse::parse_expr(&format!("<{} as {tr}>::{ms}", self.prog.tn(t))) {
                                        return Some(Expr::Call(Box::new(Expr::Path(q, qp)), cargs.clone()));
                                    }
                                }
                                // (`A::m(&0.0f32)` taking A's default, beside another trait's `f32::m`: not `x.m()`, which
                                // would reach that one; the path call keeps the trait)
                                if self.prog.index.get(&key).is_some_and(|&i| self.prog.sigs[i as usize].impl_trait.as_deref().is_some_and(|it| it.split('<').next().unwrap_or(it).trim() != tr)) {
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
        let t = Ty::prim(ty)?;
        if t == Ty::Char || t == Ty::Bool {
            return None;
        }
        let ms = p.segs[1].name.as_str();
        let pp = PPath::of(p);
        // `i64::from(7u32)` / `u32::from('a')`: 数値への変換はキャスト (広げる変換だけが書ける)
        if ms == "from" && cargs.len() == 1 && self.prog.fn_by_p(&pp, self.self_sid).is_none() {
            let a0 = &cargs[0];
            return Some(paren(Expr::Cast(Box::new(paren(a0.clone())), ty_named(ty))));
        }
        if ms.starts_with("from") || ms.starts_with("try_from") || ms == "default" || ms == "new" {
            return None;
        }
        if self.prog.fn_by_p(&pp, self.self_sid).is_some() {
            return None;
        }
        let mut args = cargs.iter();
        let a0 = args.next()?;
        let rest: Vec<Expr> = args.cloned().collect();
        // (リテラルは型を付ける: `i32::pow(2, 10)` の 2 は i32)
        let recv = if matches!(a0, Expr::Lit(_, _)) { paren(Expr::Cast(Box::new(a0.clone()), ty_named(ty))) } else { paren(a0.clone()) };
        Some(Expr::MethodCall(Box::new(recv), ms.to_string(), vec![], rest, crate::parse::Span::default()))
    }
}

/// An integer literal without a suffix (`3`, `-3`)
fn is_int_lit(e: &Expr) -> bool {
    match strip_paren(e) {
        Expr::Lit(t, _) => t.starts_with(|c: char| c.is_ascii_digit()) && !t.contains('.') && !t.contains(['i', 'u']) && !t.contains('e'),
        Expr::Unary(op, x, _) => op == "-" && is_int_lit(x),
        _ => false,
    }
}

/// An unsuffixed float literal (`5.0`, `1e3`, `-2.5`)
fn is_float_lit(e: &Expr) -> bool {
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
/// Vec holds a raw pointer, and is Send when its elements are). A type parameter is taken as Send. (A `dyn Trait` is
/// taken as Send: zustc's types do not keep `+ Send`, tests/security/send_dyn_thread.rs.)
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
                    ("Rc", _) | ("MutexGuard" | "RwLockReadGuard" | "RwLockWriteGuard", false) => true,
                    // (a RefCell's guards share the cell's non-atomic borrow flag: neither Send nor Sync, as in std)
                    ("Ref" | "RefMut", _) => true,
                    ("Cell" | "RefCell" | "UnsafeCell" | "__Rcvr__", true) => true,
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
            sd.args.iter().any(|&a| auto_trait_fails(prog, a, sync, seen, d + 1))
                || sd.fields.iter().any(|f| auto_trait_fails(prog, f.1, sync, seen, d + 1))
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
fn hidden_param(n: &str) -> bool {
    n.starts_with("__impl") && n.len() > 6 && n.as_bytes()[6..].iter().all(|c| c.is_ascii_digit())
}
