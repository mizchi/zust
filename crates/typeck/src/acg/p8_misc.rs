use super::*;
use crate::parse::{Block as PBlock, Expr, GArg, Pat};
use crate::program::{FnSig, PPath};
use crate::types::{Ty, intern};
use crate::acg::clif::{MemFlagsData, types};
use crate::ir::{FloatCC, IntCC, Type, Value};

/// turbofish (None if empty)
fn tf_of(tf: &[GArg]) -> Option<&[GArg]> {
    (!tf.is_empty()).then_some(tf)
}

impl Cx<'_> {
    /// Whether type t is an iterator (a struct with next)
    pub(crate) fn is_iterator_ty(&self, t: Ty) -> bool {
        match t {
            Ty::Struct(sid) => self.prog.method(sid, "next").is_some() || self.prog.default_method(sid, "next").is_some(),
            _ => false,
        }
    }

    /// `v.extend(arg)` (Vec / BinaryHeap): instead of IntoIterator, turns an arg that is not an iterator into
    /// `arg.into_iter()` (value) / `arg.iter()` (reference / array). If the elements are `&T`, __extend_ref (T: Copy)
    pub(crate) fn extend_call(&mut self, mc: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::MethodCall(receiver, method, _, margs, _) = mc else { return Ok(None) };
        if method != "extend" || margs.len() != 1 {
            return Ok(None);
        }
        // (the `recv.extend(__exr_ext)` rebuilt below is an ordinary method call)
        if matches!(&margs[0], Expr::Path(_, p) if is_ident(p, "__exr_ext")) {
            return Ok(None);
        }
        let Some(rt) = self.peek(receiver).map(|t| self.strip_refs(t)) else {
            return Ok(None);
        };
        let Ty::Struct(sid) = rt else { return Ok(None) };
        let bname = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
        let is_vec = matches!(bname.as_str(), "Vec" | "BinaryHeap") && self.prog.method(sid, "__extend_ref").is_some();
        // (other collections and String: make the argument an iterator, then go to the prelude's extend)
        let other = matches!(bname.as_str(), "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "String" | "VecDeque")
            && self.prog.method(sid, "extend").is_some();
        if !is_vec && !other {
            return Ok(None);
        }
        let elem = if is_vec {
            *self.prog.structs[sid as usize].args.first().ok_or("internal: bad Vec")?
        } else {
            Ty::Unit
        };
        let arg = &margs[0];
        let at = self.peek(arg);
        let paren = |e: &Expr| Expr::Paren(Box::new(e.clone()));
        let iter_e: Option<Expr> = match at {
            None => None,
            Some(t) if self.is_iterator_ty(t) => None,
            Some(Ty::Ref(_, Ty::Array(..))) if matches!(strip_paren(arg), Expr::Ref(..)) => {
                Some(mcall(path_expr("__exr_arr"), "iter", vec![]))
            }
            Some(Ty::Ref(_, _)) => match strip_paren(arg) {
                Expr::Ref(_, inner) => Some(mcall(paren(inner), "iter", vec![])),
                _ => Some(mcall(paren(arg), "iter", vec![])),
            },
            Some(Ty::Array(e, _)) => {
                if self.needs_drop(*e) {
                    return err(SP, "`extend` with an array of values that need drop is not supported");
                }
                Some(mcall(path_expr("__exr_arr"), "iter", vec![]))
            }
            Some(_) => Some(mcall(paren(arg), "into_iter", vec![])),
        };
        let iter_e: Option<&'static Expr> = iter_e.map(|e| self.keep_expr(e));
        self.scopes.push(vec![]);
        // An array is first put in a local and then iter() is called (since a literal `[1, 2].iter()` is not a place)
        match (at, strip_paren(arg)) {
            (Some(Ty::Array(..)), _) => {
                let (v, t) = self.expr(arg, None)?;
                self.bind("__exr_arr".into(), t, v);
            }
            (Some(Ty::Ref(_, Ty::Array(e, _))), Expr::Ref(_, inner)) => {
                if self.needs_drop(**e) {
                    return err(SP, "`extend` with an array of values that need drop is not supported");
                }
                let (v, t) = self.expr(inner, None)?;
                self.bind("__exr_arr".into(), t, v);
            }
            _ => {}
        }
        // (what the iterator holds (`v.iter()` of `v.extend(&v)`): the `__exr_ext` local holds it until the call)
        self.bk_frame();
        let r = match iter_e {
            Some(e) => self.expr(e, None),
            None => self.expr(arg, None),
        };
        let srcs = self.bk_frame_end();
        let (v, t) = r?;
        if t == Ty::Never {
            self.scopes.pop();
            return Ok(Some((None, Ty::Never)));
        }
        let item = self.prog.assoc_of(t, "Item");
        let by_ref = is_vec && matches!(item, Some(Ty::Ref(_, i)) if *i == elem);
        self.bind("__exr_ext".into(), t, v);
        self.bk_def_named("__exr_ext", &srcs, iter_e.unwrap_or(arg));
        let r = Expr::Paren(receiver.clone());
        let e: Expr = if by_ref {
            mcall(r, "__extend_ref", vec![path_expr("__exr_ext")])
        } else {
            mcall(r, "extend", vec![path_expr("__exr_ext")])
        };
        let e = self.keep_expr(e);
        // (the argument was evaluated before: the receiver's `&mut` is taken while it is held (as an operator's, not
        // two-phase after it), so an iterator of the receiver (`v.extend(v.iter())`) conflicts with it (E0502))
        self.bk.op_call = e as *const Expr as usize;
        let res = self.expr(e, expected)?;
        self.pop_scope();
        Ok(Some(res))
    }
}

impl Cx<'_> {
    /// The loop that break / continue goes to (without a label, the innermost loop; labeled blocks are skipped)
    pub(crate) fn loop_index(&self, label: Option<&str>, is_break: bool) -> Option<usize> {
        self.loops.iter().rposition(|l| match label {
            Some(n) => l.label.as_deref() == Some(n) && (is_break || !l.is_block),
            None => !l.is_block,
        })
    }

    /// `'a: { .. break 'a v; .. }`
    pub(crate) fn labeled_block(&mut self, label: String, blk: &PBlock, expected: Option<Ty>) -> R<V> {
        let exit = self.b.create_block();
        self.loops.push(Loop {
            cont: exit,
            brk: exit,
            breaks: 0,
            label: Some(label),
            val: expected.filter(|_| false),
            is_block: true,
            depth: self.scopes.len(),
            temps: self.temps.len(),
            no_value: None,
        });
        let (v, t) = self.block(blk, expected)?;
        let lp = self.loops.pop().unwrap();
        if lp.breaks == 0 {
            return Ok((v, t));
        }
        // The tail value is also passed as exit's argument, the same as break
        if t != Ty::Never {
            if let Some(w) = lp.val {
                self.same(SP, t, w)?;
            } else if t != Ty::Unit {
                return err(SP, "a labeled block that breaks without a value must have type `()`");
            }
            let args: Vec<Value> = v.into_iter().collect();
            self.b.ins().jump(exit, &args);
        }
        self.b.switch_to_block(exit);
        match lp.val {
            Some(t) => Ok((self.b.block_params(exit).first().copied(), t)),
            None => Ok((None, Ty::Unit)),
        }
    }
}

/// The (leading elements, `..` (and `x @ ..`), trailing elements) of a slice pattern
pub(crate) fn slice_parts(sp: &Pat) -> (Vec<&Pat>, Option<&Pat>, Vec<&Pat>) {
    let mut pre = vec![];
    let mut post = vec![];
    let mut rest = None;
    let Pat::Slice(elems) = sp else { return (pre, rest, post) };
    for e in elems {
        let is_rest = match e {
            Pat::Rest => true,
            Pat::Ident(_, _, _, Some(sub)) => matches!(&**sub, Pat::Rest),
            _ => false,
        };
        if is_rest && rest.is_none() {
            rest = Some(e);
        } else if rest.is_some() {
            post.push(e);
        } else {
            pre.push(e);
        }
    }
    (pre, rest, post)
}

impl Cx<'_> {
    /// The value of a local variable's location (in the form passed as a block argument: an address for aggregate types)
    pub(crate) fn loc_arg(&mut self, loc: Loc, t: Ty) -> Option<Value> {
        match loc {
            Loc::Var(v) => Some(self.b.use_var(v)),
            Loc::Slot(ss) if t.is_agg() => Some(self.b.ins().stack_addr(PTR, ss, 0)),
            Loc::Slot(ss) => Some(self.b.ins().stack_load(clif_ty(t).unwrap_or(PTR), ss, 0)),
            Loc::Ptr(v) => Some(v),
            Loc::Zst(_) => None,
        }
    }

    /// The type when made a block argument (None if size 0)
    pub(crate) fn param_ty(&self, t: Ty) -> Option<Type> {
        if t.is_agg() || t.is_fat() { Some(PTR) } else { clif_ty(t) }
    }

    /// The scrutinee of a slice pattern: (start, length, element type, the binding mode if through a reference)
    pub(crate) fn slice_scrutinee(&mut self, p: Place, span: Sp) -> R<(Value, Value, Ty, Option<bool>)> {
        let mut p = p;
        let mut rmode = None;
        loop {
            match p.ty() {
                Ty::Array(e, n) => {
                    let Place::Mem(a, o, _) = p else {
                        return err(span, "internal: array pattern on a register");
                    };
                    let base = self.addr_plus(a, o);
                    let len = self.b.ins().iconst(types::I64, n as i64);
                    return Ok((base, len, *e, rmode));
                }
                Ty::Slice(e) => {
                    let Place::Mem(a, o, _) = p else {
                        return err(span, "internal: bad slice place");
                    };
                    let fat = self.addr_plus(a, o);
                    let base = self.b.ins().load(PTR, mf(), fat, 0);
                    let len = self.b.ins().load(types::I64, mf(), fat, 8);
                    return Ok((base, len, *e, rmode));
                }
                Ty::Ref(m, Ty::Slice(e)) => {
                    let fat = match p {
                        Place::Mem(a, o, _) => self.addr_plus(a, o),
                        Place::Var(v, _) => self.b.use_var(v),
                        _ => return err(span, "internal: bad slice place"),
                    };
                    let base = self.b.ins().load(PTR, mf(), fat, 0);
                    let len = self.b.ins().load(types::I64, mf(), fat, 8);
                    return Ok((base, len, **e, Some(rmode.unwrap_or(m))));
                }
                Ty::Ref(m, _) => {
                    let (q, _) = self.pat_deref_once(p);
                    p = q;
                    rmode = Some(rmode.unwrap_or(m));
                }
                t => return err(span, format!("slice pattern on `{}`", self.prog.tn(t))),
            }
        }
    }

    /// The places of the leading and trailing elements
    pub(crate) fn slice_elems<'p>(
        &mut self,
        base: Value,
        len: Value,
        elem: Ty,
        pre: &[&'p Pat],
        post: &[&'p Pat],
    ) -> Vec<(Place, &'p Pat)> {
        let (es, _) = self.prog.size_align(elem);
        let mut out = vec![];
        for (i, sub) in pre.iter().enumerate() {
            let a = self.b.ins().iadd_imm_s(base, (i * es as usize) as i64);
            out.push((Place::Mem(a, 0, elem), *sub));
        }
        for (j, sub) in post.iter().enumerate() {
            let idx = self.b.ins().iadd_imm_s(len, -((post.len() - j) as i64));
            let off = self.b.ins().imul_imm_s(idx, es as i64);
            let a = self.b.ins().iadd(base, off);
            out.push((Place::Mem(a, 0, elem), *sub));
        }
        out
    }
}

impl Cx<'_> {
    /// Encodes a char as UTF-8 into a temporary slot and makes it a `&str` fat pointer
    pub(crate) fn char_to_str(&mut self, c: Value, span: Sp) -> R<Value> {
        let ss = self.b.create_stack_slot(4, 1);
        let buf = self.b.ins().stack_addr(PTR, ss, 0);
        let bt = Ty::Ptr(true, intern(Ty::U8));
        let (n, _) = self.call_named("__exr_char_utf8", vec![Ty::Unit], &[(c, Ty::Char), (buf, bt)], span)?;
        Ok(self.make_fat_v(buf, n.unwrap()))
    }

    /// Turns `recv.name(args)` into the prelude's `{prefix}{name}(recv, args)` (None if there is none).
    /// A `&str` parameter also accepts a char (encoded as UTF-8) and a `&String`
    pub(crate) fn helper_method(&mut self, prefix: &str, name: &str, mc: &Expr, recv: (Value, Ty)) -> R<Option<V>> {
        let Expr::MethodCall(_, _, turbofish, margs, _) = mc else { return Ok(None) };
        let f = format!("{prefix}{name}");
        let Some(&id) = self.prog.index.get(&f) else { return Ok(None) };
        let sig = self.prog.sigs[id as usize].clone();
        if sig.params.len() != margs.len() + 1 {
            return err(SP, format!("`{name}` takes {} arguments", sig.params.len() - 1));
        }
        let args: Vec<&Expr> = margs.iter().collect();
        // Those with real type arguments (`parse::<T>`) are ordinary calls
        if !sig.generics.iter().all(|g| g.0 == "G") {
            let exp = self.lets.get(&(mc as *const Expr as usize)).copied();
            return self.call_fn(id, Some(recv), &args, tf_of(turbofish), exp, SP).map(Some);
        }
        let sref = Ty::Ref(false, intern(Ty::Str));
        let mut vals = vec![recv];
        for (a, &pt) in args.iter().zip(&sig.params[1..]) {
            if pt == sref && self.peek(a) == Some(Ty::Char) {
                let (v, _) = self.expr(a, Some(Ty::Char))?;
                let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                let fat = self.char_to_str(v, SP)?;
                vals.push((fat, sref));
                continue;
            }
            let (v, mut t) = self.expr(a, Some(pt))?;
            if t == Ty::Never {
                return Ok(Some((None, Ty::Never)));
            }
            let mut v = v;
            // `&String` → `&str`
            if let (true, Ty::Ref(_, Ty::Struct(sid)), Some(val)) = (pt == sref, t, v) {
                if let Some((fat, ft)) = self.deref_to_slice(val, *sid, false) {
                    v = Some(fat);
                    t = ft;
                }
            }
            // `&&str` → `&str` (the address pointing at the fat pointer pair is itself the &str value)
            if let (Ty::Ref(_, Ty::Ref(_, Ty::Str)), true) = (t, pt == sref) {
                t = sref;
            }
            self.same(SP, t, pt)?;
            if let Some(v) = v {
                vals.push((v, t));
            }
        }
        self.call_named(&f, vec![Ty::Unit], &vals, SP).map(Some)
    }
}

impl Cx<'_> {
    /// The (lo, hi) of a range `a..b` / `a..=b` / `..b` / `a..`
    pub(crate) fn range_bounds(&mut self, rg: &Expr, len: Value) -> R<(Value, Value)> {
        let Expr::Range(start, end, inclusive) = rg else {
            return err(SP, "internal: expected a range");
        };
        let lo = match start {
            Some(e) => {
                let (v, t) = self.expr(e, Some(Ty::Usize))?;
                self.same(SP, t, Ty::Usize)?;
                v.unwrap()
            }
            None => self.b.ins().iconst(types::I64, 0),
        };
        let hi = match end {
            Some(e) => {
                let (v, t) = self.expr(e, Some(Ty::Usize))?;
                self.same(SP, t, Ty::Usize)?;
                let v = v.unwrap();
                if *inclusive { self.b.ins().iadd_imm_s(v, 1) } else { v }
            }
            None => len,
        };
        Ok((lo, hi))
    }

    /// Whether byte position idx (<= len) is a UTF-8 char boundary (0/1 as an i8)
    pub(crate) fn char_boundary(&mut self, ptr: Value, len: Value, idx: Value) -> Value {
        let done = self.b.create_block();
        let res = self.b.append_block_param(done, types::I8);
        let load = self.b.create_block();
        let at_end = self.b.ins().icmp(IntCC::Equal, idx, len);
        let one = self.b.ins().iconst(types::I8, 1);
        self.b.ins().brif(at_end, done, &[one], load, &[]);
        self.b.switch_to_block(load);
        let a = self.b.ins().iadd(ptr, idx);
        let byte = self.b.ins().load(types::I8, mf(), a, 0);
        let top = self.b.ins().band_imm_u(byte, 0xC0);
        let ok = self.b.ins().icmp_imm_u(IntCC::NotEqual, top, 0x80);
        self.b.ins().jump(done, &[ok]);
        self.b.switch_to_block(done);
        res
    }
}

// --- clone (derive(Clone) etc.) ---------------------------------------------------

impl Cx<'_> {
    /// `x.clone()` (a type without its own clone method): integers etc. are read as is, others use __exr_clone::<T>
    pub(crate) fn clone_call(&mut self, mc: &Expr) -> R<Option<V>> {
        let Expr::MethodCall(receiver, method, _, margs, _) = mc else { return Ok(None) };
        if method != "clone" || !margs.is_empty() {
            return Ok(None);
        }
        if let Some(pt) = self.peek(receiver) {
            // (`r.clone()` with `r: &S` and S not Clone: the reference's own clone, a copy of r (`&S`). Not Clone: a type
            // of the program with no clone and no derive, or an instance with such an argument (`Option<NoClone>`), but
            // for the shared pointers)
            // (neither when a trait of the program gives S a `clone` (a default fn of `impl Duplicate for S {}`): that is
            // the method called)
            // (a method: an associated `fn clone() -> S` is not called by `r.clone()`)
            let user_clone = |sid: u32| self.prog.trait_clone_fns && self.prog.default_method(sid, "clone").is_some_and(|d| self.prog.sigs[d as usize].has_self);
            // (a closure's Clone is the compiler's, and asks it of each capture: E0277 for one that is not, also through
            // a reference, which rustc does not copy then)
            let closure_err = |sid: u32| -> Option<String> {
                if !self.prog.is_closure(self.prog.base_of(sid)) {
                    return None;
                }
                let f = self.prog.structs[sid as usize].fields.iter().find(|f| self.not_clone(f.1, 1))?;
                Some(format!("the trait bound `{}: Clone` is not satisfied (E0277)", self.prog.tn(f.1)))
            };
            if let Ty::Ref(false, i @ Ty::Struct(isid)) = pt {
                if self.not_clone(*i, 0) && !user_clone(*isid) {
                    if let (Some(m), true) = (closure_err(*isid), self.user_code) {
                        return err(SP, m);
                    }
                    return self.operand(receiver, None, false).map(Some);
                }
            }
            // (E0599: a value of a type surely not Clone and with no Deref to take its clone through: a derive whose
            // bounds an argument does not meet (`Bar<NotClone>`), or a type of the program's with no clone)
            if let (Ty::Struct(sid), true) = (pt, self.user_code) {
                let b = self.prog.base_of(sid);
                if self.not_clone(pt, 0) && self.prog.method(sid, "deref").is_none() && self.prog.method(b, "deref").is_none() && !user_clone(sid) {
                    if let Some(m) = closure_err(sid) {
                        return err(SP, m);
                    }
                    let derived = self.prog.derived.get(&b).is_some_and(|d| d.iter().any(|x| x == "Clone"));
                    let what = if self.prog.structs[b as usize].is_enum { "enum" } else { "struct" };
                    let tn = self.prog.tn(pt);
                    return err(SP, if derived {
                        format!("the method `clone` exists for {what} `{tn}`, but its trait bounds were not satisfied (E0599)")
                    } else {
                        format!("no method named `clone` found for {what} `{tn}` in the current scope (E0599)")
                    });
                }
            }
            let mut t = pt;
            while let Ty::Ref(_, i) = t {
                if t.is_fat() {
                    break;
                }
                t = *i;
            }
            if let Ty::Struct(sid) = t {
                if self.prog.method(sid, "clone").is_some() || user_clone(sid) {
                    return Ok(None);
                }
            }
        }
        // (the clone of `&str` / `&[T]` is a copy of the reference)
        let (v, vt) = self.operand(receiver, None, true)?;
        let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
        let (a, t) = self.ord_addr(v, vt);
        // (when, after evaluating without knowing the type, it turns out to be a type with its own clone)
        if let Ty::Struct(sid) = t {
            if let Some(id) = self.prog.method(sid, "clone") {
                return self.call_fn(id, Some((a, Ty::Ref(false, intern(t)))), &[], None, None, SP).map(Some);
            }
        }
        if !self.needs_drop(t) {
            let p = Place::Mem(a, 0, t);
            return Ok(Some(self.read(p)));
        }
        if !self.prog.index.contains_key("__exr_clone") {
            return err(SP, "internal: `__exr_clone` is missing from the prelude");
        }
        let r = Ty::Ref(false, intern(t));
        self.call_named("__exr_clone", vec![t], &[(a, r)], SP).map(Some)
    }

    /// Whether t is surely not Clone (clone_call): Program::surely_not_clone
    fn not_clone(&self, t: Ty, depth: u32) -> bool {
        self.prog.surely_not_clone(t, depth)
    }

    /// The body of `__exr_clone::<T>(x: &T) -> T`
    pub(crate) fn clone_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let mut ps = self.b.func_params();
        let Some(Ty::Ref(_, t)) = sig.params.first() else {
            return Err("internal: bad clone glue signature".into());
        };
        let t = **t;
        if is_sret(sig) {
            let dst = ps.remove(0);
            let src = ps[0];
            self.clone_into(src, dst, t)?;
            self.b.ins().return_(&[]);
        } else {
            let src = ps[0];
            match clif_ty(t) {
                Some(ct) => {
                    // (types that need drop even though they are scalar, such as Box, are cloned before being returned)
                    let tmp = self.temp(t);
                    self.clone_into(src, tmp, t)?;
                    let v = self.b.ins().load(ct, mf(), tmp, 0);
                    self.b.ins().return_(&[v]);
                }
                None => {
                    self.b.ins().return_(&[]);
                }
            }
        }
        Ok(())
    }

    /// Writes the clone of the value of t at src to dst
    pub(crate) fn clone_into(&mut self, src: Value, dst: Value, t: Ty) -> R<()> {
        if !self.needs_drop(t) {
            self.copy(dst, src, t);
            return Ok(());
        }
        match t {
            Ty::Struct(sid) => {
                // Its own clone (String / Vec / a hand-written impl Clone): call it
                if let Some(id) = self.prog.method(sid, "clone") {
                    let sig0 = self.prog.sigs[id as usize].clone();
                    let (fid, sig) = if sig0.is_generic() {
                        let mut sol = vec![None; sig0.generics.len()];
                        if let Some(st) = sig0.self_ty {
                            self.prog.match_ty(st, t, &mut sol);
                        }
                        let r = Ty::Ref(false, intern(t));
                        self.resolve_call(id, &[r], sol, None, SP)?
                    } else {
                        (id, sig0)
                    };
                    let f = self.fref(fid, &sig);
                    if is_sret(&sig) {
                        self.b.ins().call(f, &[dst, src]);
                    } else {
                        let call = self.b.ins().call(f, &[src]);
                        if let Some(&v) = self.b.inst_results(call).first() {
                            self.b.ins().store(mf(), v, dst, 0);
                        }
                    }
                    return Ok(());
                }
                let sd = &self.prog.structs[sid as usize];
                if !sd.is_enum {
                    let fields = sd.fields.clone();
                    for (_, ft, off) in fields {
                        let (s2, d2) = (self.addr_plus(src, off as i32), self.addr_plus(dst, off as i32));
                        self.clone_elem(s2, d2, ft)?;
                    }
                    return Ok(());
                }
                // enum: copy the discriminant and clone that variant's fields
                let tag_ty = sd.tag;
                let variants: Vec<(i64, Vec<(String, Ty, u32)>)> = sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect();
                let (size, _) = self.prog.size_align(t);
                let _ = size;
                let ct = clif_ty(tag_ty).unwrap();
                let tag = self.b.ins().load(ct, mf(), src, 0);
                // (first copy bitwise, then overwrite the fields that need it with clones)
                self.copy(dst, src, t);
                let done = self.b.create_block();
                for (d, fs) in variants {
                    if !fs.iter().any(|f| self.needs_drop(f.1)) {
                        continue;
                    }
                    let yes = self.b.create_block();
                    let next = self.b.create_block();
                    let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                    self.b.ins().brif(c, yes, &[], next, &[]);
                    self.b.switch_to_block(yes);
                    for (_, ft, off) in fs {
                        let (s2, d2) = (self.addr_plus(src, off as i32), self.addr_plus(dst, off as i32));
                        self.clone_elem(s2, d2, ft)?;
                    }
                    self.b.ins().jump(done, &[]);
                    self.b.switch_to_block(next);
                }
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Ok(())
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                for i in 0..n {
                    let (s2, d2) = (self.addr_plus(src, (i * es) as i32), self.addr_plus(dst, (i * es) as i32));
                    self.clone_elem(s2, d2, *e)?;
                }
                Ok(())
            }
            Ty::Box(inner) if !matches!(inner, Ty::Dyn(_)) => {
                let Some(&alloc) = self.prog.index.get(crate::prelude::BOX_ALLOC) else {
                    return Err("internal: the allocator is missing from the prelude".into());
                };
                let (size, _) = self.prog.size_align(*inner);
                let asig = self.prog.sigs[alloc as usize].clone();
                let fr = self.fref(alloc, &asig);
                let sz = self.b.ins().iconst(types::I64, size.max(1) as i64);
                let call = self.b.ins().call(fr, &[sz]);
                let np = self.b.inst_results(call)[0];
                let op = self.b.ins().load(PTR, mf(), src, 0);
                self.clone_elem(op, np, *inner)?;
                self.b.ins().store(mf(), np, dst, 0);
                Ok(())
            }
            t => Err(format!("`{}` cannot be cloned", self.prog.tn(t))),
        }
    }

    /// The clone of one element: types that need no drop are copied, others call __exr_clone::<t>
    pub(crate) fn clone_elem(&mut self, src: Value, dst: Value, t: Ty) -> R<()> {
        if !self.needs_drop(t) {
            self.copy(dst, src, t);
            return Ok(());
        }
        let r = Ty::Ref(false, intern(t));
        let (v, _) = self.call_named("__exr_clone", vec![t], &[(src, r)], SP)?;
        match v {
            Some(v) if t.is_agg() => self.copy(dst, v, t),
            Some(v) => {
                self.b.ins().store(mf(), v, dst, 0);
            }
            None => {}
        }
        Ok(())
    }
}

// --- Default ---------------------------------------------------------------------

impl Cx<'_> {
    /// `Default::default()` / `T::default()` / `i32::default()` / `Vec::default()` (types without a hand-written default)
    pub(crate) fn default_call(&mut self, c: &Expr, expected: Option<Ty>) -> R<Option<V>> {
        let Expr::Call(func, cargs) = c else { return Ok(None) };
        let Expr::Path(qself, path) = &**func else { return Ok(None) };
        if !cargs.is_empty() || path.segs.last().map(|s| s.name != "default").unwrap_or(true) {
            return Ok(None);
        }
        if !self.prog.index.contains_key("__exr_default") {
            return Ok(None);
        }
        // For `X::default()`, if X has its own default (a hand-written impl Default for X / its own fn), call it
        if qself.is_none() && path.segs.len() == 2 && path.segs[0].name != "Default" {
            if let Some(id) = self.prog.fn_by_p(&PPath::of(path), self.self_sid) {
                if self.prog.sigs[id as usize].has_body {
                    return Ok(None);
                }
            }
            if self.callee(func).is_some_and(|(id, _)| self.prog.sigs[id as usize].has_body) {
                return Ok(None);
            }
        }
        let exp = expected.or_else(|| self.lets.get(&(c as *const Expr as usize)).copied());
        // (`m::S::default()`: skip the module part)
        let stripped = if qself.is_none() { self.prog.strip_mods_p(&PPath::of(path)) } else { None };
        let mut path = path.clone();
        if let Some(q) = stripped {
            let k = path.segs.len() - q.segs.len();
            path = crate::parse::Path { global: false, segs: path.segs[k..].to_vec(), pos: Default::default() };
        }
        let path = &path;
        let t = if let Some(q) = qself {
            self.parse_ty(q).ok()
        } else {
            let segs: Vec<String> = path.segs.iter().map(|s| s.name.clone()).collect();
            match segs.as_slice() {
                [d, _] if d == "Default" => exp,
                [x, _] => {
                    let ty = crate::parse::Ty::Path(None, path_of(&path.segs[0].name));
                    // Just the name of a generic type (`Vec::default()`): from the expected type
                    // (also generic aliases `FxMap::default()`)
                    let from_exp = || {
                        exp.filter(|&e| {
                            self.prog.aliases.contains_key(x.as_str())
                                || matches!(e, Ty::Struct(sid) if self.prog.structs[self.prog.base_of(sid) as usize].name == *x)
                        })
                    };
                    // (a struct generic with defaults too, before them: `let b: Buf<5> = Buf::default();`)
                    let by_defaults = path.segs[0].args.is_empty() && self.prog.bare_by_defaults(x);
                    match self.parse_ty(&ty) {
                        Ok(t) if by_defaults => from_exp().or(Some(t)),
                        Ok(t) => Some(t),
                        Err(_) => from_exp(),
                    }
                }
                [_, _, ..] if segs.len() >= 3 && segs[segs.len() - 2] == "Default" => exp,
                // (`Self::Seed::default()` / `T::Output::default()`: the path before `default` is a type, an associated
                //  one of Self or a type parameter (rand_core's `SeedableRng::seed_from_u64`))
                [_, _, ..] => {
                    let mut tp = path.clone();
                    tp.segs.pop();
                    self.parse_ty(&crate::parse::Ty::Path(None, tp)).ok().or(exp)
                }
                _ => None,
            }
        };
        let Some(t) = t else {
            return err(SP, "cannot infer the type of `default()`; annotate it");
        };
        self.call_named("__exr_default", vec![t], &[], SP).map(Some)
    }

    /// The body of `__exr_default::<T>() -> T`
    pub(crate) fn default_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let ps = self.b.func_params();
        let t = sig.ret;
        if is_sret(sig) {
            self.default_into(ps[0], t)?;
            self.b.ins().return_(&[]);
        } else {
            match clif_ty(t) {
                Some(ct) => {
                    let tmp = self.temp(t);
                    self.default_into(tmp, t)?;
                    let v = self.b.ins().load(ct, mf(), tmp, 0);
                    self.b.ins().return_(&[v]);
                }
                None => {
                    self.b.ins().return_(&[]);
                }
            }
        }
        Ok(())
    }

    /// Writes the default value of t to dst
    pub(crate) fn default_into(&mut self, dst: Value, t: Ty) -> R<()> {
        // A hand-written `impl Default for S`
        if let Ty::Struct(sid) = t {
            let bname = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
            if let Some(&id) = self.prog.index.get(&format!("{bname}::default")) {
                let sig0 = self.prog.sigs[id as usize].clone();
                let (fid, sig) = if sig0.is_generic() {
                    let mut sol = vec![None; sig0.generics.len()];
                    self.prog.match_ty(sig0.ret, t, &mut sol);
                    self.resolve_call(id, &[], sol, Some(t), SP)?
                } else {
                    (id, sig0)
                };
                let f = self.fref(fid, &sig);
                if is_sret(&sig) {
                    self.b.ins().call(f, &[dst]);
                } else {
                    let call = self.b.ins().call(f, &[]);
                    if let Some(&v) = self.b.inst_results(call).first() {
                        self.b.ins().store(mf(), v, dst, 0);
                    }
                }
                return Ok(());
            }
        }
        // Otherwise fill with 0 and walk the fields (and the contents of Boxes)
        let (size, _) = self.prog.size_align(t);
        if size > 0 {
            let z = self.b.ins().iconst(types::I8, 0);
            let n = self.b.ins().iconst(types::I64, size as i64);
            let _ = (z, n);
            let zero = self.b.ins().iconst(types::I64, 0);
            let mut off = 0;
            while off + 8 <= size {
                self.b.ins().store(mf(), zero, dst, off as i32);
                off += 8;
            }
            let z8 = self.b.ins().iconst(types::I8, 0);
            while off < size {
                self.b.ins().store(mf(), z8, dst, off as i32);
                off += 1;
            }
        }
        match t {
            Ty::Struct(sid) if !self.prog.structs[sid as usize].is_enum => {
                let fields = self.prog.structs[sid as usize].fields.clone();
                for (_, ft, off) in fields {
                    if matches!(ft, Ty::Struct(_) | Ty::Box(_) | Ty::Array(..)) {
                        let d = self.addr_plus(dst, off as i32);
                        self.default_into(d, ft)?;
                    }
                }
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                if matches!(e, Ty::Struct(_) | Ty::Box(_)) {
                    for i in 0..n {
                        let d = self.addr_plus(dst, (i * es) as i32);
                        self.default_into(d, *e)?;
                    }
                }
            }
            Ty::Box(inner) => {
                let Some(&alloc) = self.prog.index.get(crate::prelude::BOX_ALLOC) else {
                    return Err("internal: the allocator is missing from the prelude".into());
                };
                let (size, _) = self.prog.size_align(*inner);
                let asig = self.prog.sigs[alloc as usize].clone();
                let fr = self.fref(alloc, &asig);
                let sz = self.b.ins().iconst(types::I64, size.max(1) as i64);
                let call = self.b.ins().call(fr, &[sz]);
                let np = self.b.inst_results(call)[0];
                self.default_into(np, *inner)?;
                self.b.ins().store(mf(), np, dst, 0);
            }
            _ => {}
        }
        Ok(())
    }
}

impl Cx<'_> {
    /// A const embeds the expression itself
    pub(crate) fn const_value(&mut self, p: &Expr, ci: u32) -> R<V> {
        // (E0391: a constant whose value needs its own (`const A: i32 = B; const B: i32 = A;`))
        thread_local! {
            static EVALUATING: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        if EVALUATING.with(|v| v.borrow().contains(&ci)) {
            return err(SP, format!("cycle detected when const-evaluating `{}` (E0391)", self.prog.consts[ci as usize].name));
        }
        EVALUATING.with(|v| v.borrow_mut().push(ci));
        let r = self.const_value_in(p, ci);
        EVALUATING.with(|v| {
            v.borrow_mut().pop();
        });
        r
    }

    fn const_value_in(&mut self, p: &Expr, ci: u32) -> R<V> {
        if !self.prog.static_self_consts.is_empty() {
            self.rg_static_self_const(p, ci);
        }
        if !self.prog.const_regions.is_empty() {
            self.rg_const_regions(p, ci);
        }
        let prog = self.prog;
        let c = &prog.consts[ci as usize];
        let (t, src) = (c.ty, c.expr.clone());
        // Integer consts are folded (`u8::MAX.ilog10() as usize + 1` is not computed at every use). The evaluation is typed, so `!` is also
        // at the type's width (`!0u8` is 255). Out-of-range results (u128 bit patterns) and expressions that cannot be evaluated are computed at run time
        if t.is_int() {
            match prog.const_int(&c.name, 0) {
                Ok(v) => {
                    let (lo, hi) = (crate::program::prim_assoc_const(t, "MIN").unwrap_or(0), crate::program::prim_assoc_const(t, "MAX").unwrap_or(0));
                    let in_range = if t == Ty::U128 { v >= 0 } else { lo <= v && v <= hi };
                    if in_range {
                        return Ok((Some(self.int_const(t, v)), t));
                    }
                }
                // (an overflow is the constant's error at compile time, as rustc's E0080: not a wrap or a panic at run
                // time)
                Err(e) if crate::program::const_eval_err(&e) => return Err(format!("{}:1: in constant `{}`: {e}", SP.line, c.name)),
                Err(_) => {}
            }
        }
        let ce: Expr = crate::parse::parse_expr(&src).or_else(|er| err(SP, er))?;
        let ce = self.keep_expr(ce);
        // (no line numbers for now: Sp carries no location)
        // (a type that names the impl's parameters is a hole: the expression takes the expected type at the use)
        let hint = self.const_hint.take();
        // (an impl's const: `Self` in it is the impl's type, `const ALIGN: usize = Self::BYTES - 1;` of `S::ALIGN` /
        // `v128 as Vector::ALIGN`)
        let saved = (self.self_sid, self.env.self_ty);
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let mentions_self = src.match_indices("Self").any(|(i, _)| !src[..i].ends_with(word) && !src[i + 4..].starts_with(word));
        if mentions_self {
            let base = c.name.rsplit_once("::").map(|(a, _)| a.split(" as ").next().unwrap_or(a));
            let st = base.and_then(|b| match prog.struct_index.get(b) {
                Some(&sid) => Some(Ty::Struct(sid)),
                None => Ty::prim(b).or_else(|| crate::simd_sig::vector_name(b)),
            });
            if let Some(st) = st {
                if let Ty::Struct(sid) = st {
                    self.self_sid = Some(sid);
                }
                self.env.self_ty = Some(st);
            }
        }
        let r = if has_hole(prog, t) {
            self.expr(ce, hint.filter(|&h| !has_hole(prog, h))).map_err(|e| format!("{}:1: in constant `{}`: {e}", SP.line, c.name))
        } else {
            self.expr(ce, Some(t)).map_err(|e| format!("{}:1: in constant `{}`: {e}", SP.line, c.name))
        };
        self.self_sid = saved.0;
        self.env.self_ty = saved.1;
        if has_hole(prog, t) {
            return r;
        }
        let (v, vt) = r?;
        self.same(SP, vt, t)?;
        Ok((v, t))
    }
}

// --- Floating-point methods ---------------------------------------------------------

impl Cx<'_> {
    /// The built-in methods of f32 / f64 (instructions for those that map to one, libm for the others)
    pub(crate) fn float_method(&mut self, mc: &Expr, t: Ty) -> R<Option<V>> {
        let Expr::MethodCall(receiver, method, _, margs, _) = mc else { return Ok(None) };
        let name = method.to_string();
        let _ = clif_ty(t);
        let f32 = t == Ty::F32;
        let (lib1, lib2) = (FLOAT_LIB1, FLOAT_LIB2);
        if !(FLOAT_BUILTINS.contains(&name.as_str()) || lib1.contains(&name.as_str()) || lib2.contains(&name.as_str())) {
            return Ok(None);
        }
        // The receiver (its pointee if a reference)
        let (a, at) = if matches!(self.peek(receiver), Some(Ty::Ref(..))) {
            self.expr(receiver, None)?
        } else {
            self.expr(receiver, Some(t))?
        };
        let Some(mut a) = a else { return Ok(Some((None, Ty::Never))) };
        let mut at = at;
        while let Ty::Ref(_, i) = at {
            a = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), a, 0);
            at = *i;
        }
        let args: Vec<&Expr> = margs.iter().collect();
        let arg = |this: &mut Self, i: usize, want: Ty| -> R<Value> {
            let Some(e) = args.get(i) else {
                return err(SP, format!("`{name}` needs an argument"));
            };
            let (v, vt) = this.expr(e, Some(want))?;
            let mut v = v.unwrap();
            let mut vt = vt;
            while let Ty::Ref(_, i) = vt {
                v = this.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
                vt = *i;
            }
            this.same(SP, vt, want)?;
            Ok(v)
        };
        let span = SP;
        let libm = |this: &mut Self, f: &str, vals: Vec<Value>| -> R<Value> {
            let n = format!("__exr_{f}{}", if f32 { "f" } else { "" });
            let a: Vec<(Value, Ty)> = vals.into_iter().map(|v| (v, t)).collect();
            let (r, _) = this.call_named(&n, vec![], &a, span)?;
            Ok(r.unwrap())
        };
        let fc = |this: &mut Self, x: f64| -> Value { if f32 { this.b.ins().f32const(x as f32) } else { this.b.ins().f64const(x) } };
        let r: V = match name.as_str() {
            "sqrt" => (Some(self.b.ins().sqrt(a)), t),
            "abs" => (Some(self.b.ins().fabs(a)), t),
            "floor" => (Some(self.b.ins().floor(a)), t),
            "ceil" => (Some(self.b.ins().ceil(a)), t),
            "trunc" => (Some(self.b.ins().trunc(a)), t),
            "fract" => {
                let tr = self.b.ins().trunc(a);
                (Some(self.b.ins().fsub(a, tr)), t)
            }
            "mul_add" => {
                let b = arg(self, 0, t)?;
                let c = arg(self, 1, t)?;
                (Some(self.b.ins().fma(a, b, c)), t)
            }
            "copysign" => {
                let b = arg(self, 0, t)?;
                (Some(self.b.ins().fcopysign(a, b)), t)
            }
            "recip" => {
                let one = fc(self, 1.0);
                (Some(self.b.ins().fdiv(one, a)), t)
            }
            "to_degrees" => {
                let k = fc(self, if f32 { (180.0f32 / std::f32::consts::PI) as f64 } else { 180.0 / std::f64::consts::PI });
                (Some(self.b.ins().fmul(a, k)), t)
            }
            "to_radians" => {
                let v = if f32 {
                    let k = fc(self, (std::f32::consts::PI / 180.0) as f64);
                    self.b.ins().fmul(a, k)
                } else {
                    let k = fc(self, std::f64::consts::PI / 180.0);
                    self.b.ins().fmul(a, k)
                };
                (Some(v), t)
            }
            // Rust's max / min: if one is NaN, the other
            "max" | "min" => {
                let b = arg(self, 0, t)?;
                let cc = if name == "max" { FloatCC::GreaterThan } else { FloatCC::LessThan };
                let pick_a = self.b.ins().fcmp(cc, a, b);
                let b_nan = self.b.ins().fcmp(FloatCC::Unordered, b, b);
                let use_a = self.b.ins().bor(pick_a, b_nan);
                (Some(self.b.ins().select(use_a, a, b)), t)
            }
            "clamp" => {
                let lo = arg(self, 0, t)?;
                let hi = arg(self, 1, t)?;
                let l = self.b.ins().fcmp(FloatCC::LessThan, a, lo);
                let x = self.b.ins().select(l, lo, a);
                let g = self.b.ins().fcmp(FloatCC::GreaterThan, x, hi);
                (Some(self.b.ins().select(g, hi, x)), t)
            }
            "is_nan" => (Some(self.b.ins().fcmp(FloatCC::Unordered, a, a)), Ty::Bool),
            "is_infinite" => {
                let ab = self.b.ins().fabs(a);
                let inf = fc(self, f64::INFINITY);
                (Some(self.b.ins().fcmp(FloatCC::Equal, ab, inf)), Ty::Bool)
            }
            "is_finite" => {
                let ab = self.b.ins().fabs(a);
                let inf = fc(self, f64::INFINITY);
                (Some(self.b.ins().fcmp(FloatCC::LessThan, ab, inf)), Ty::Bool)
            }
            "is_normal" => {
                let ab = self.b.ins().fabs(a);
                let inf = fc(self, f64::INFINITY);
                let mp = fc(self, if f32 { f32::MIN_POSITIVE as f64 } else { f64::MIN_POSITIVE });
                let c1 = self.b.ins().fcmp(FloatCC::LessThan, ab, inf);
                let c2 = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, ab, mp);
                (Some(self.b.ins().band(c1, c2)), Ty::Bool)
            }
            // (below the smallest normal and not zero: NaN compares as neither)
            "is_subnormal" => {
                let ab = self.b.ins().fabs(a);
                let mp = fc(self, if f32 { f32::MIN_POSITIVE as f64 } else { f64::MIN_POSITIVE });
                let z = fc(self, 0.0);
                let c1 = self.b.ins().fcmp(FloatCC::LessThan, ab, mp);
                let c2 = self.b.ins().fcmp(FloatCC::GreaterThan, ab, z);
                (Some(self.b.ins().band(c1, c2)), Ty::Bool)
            }
            // (the prelude's __exr_fp_classify, of the bits: an f32's widened to f64 would make its subnormals normal)
            "classify" => {
                let it = if f32 { types::I32 } else { types::I64 };
                let bits = self.b.ins().bitcast(it, MemFlagsData::new(), a);
                let bits = if f32 { self.b.ins().uextend(types::I64, bits) } else { bits };
                let fl = self.b.ins().iconst(types::I8, f32 as i64);
                self.call_named("__exr_fp_classify", vec![Ty::Unit], &[(bits, Ty::U64), (fl, Ty::Bool)], span)?
            }
            "is_sign_negative" | "is_sign_positive" => {
                let it = if f32 { types::I32 } else { types::I64 };
                let bits = self.b.ins().bitcast(it, MemFlagsData::new(), a);
                let c = if name == "is_sign_negative" { IntCC::SignedLessThan } else { IntCC::SignedGreaterThanOrEqual };
                (Some(self.b.ins().icmp_imm_s(c, bits, 0)), Ty::Bool)
            }
            "to_bits" => {
                let it = if f32 { types::I32 } else { types::I64 };
                (Some(self.b.ins().bitcast(it, MemFlagsData::new(), a)), if f32 { Ty::U32 } else { Ty::U64 })
            }
            // NaN for NaN, otherwise a signed 1
            "signum" => {
                let one = fc(self, 1.0);
                let s1 = self.b.ins().fcopysign(one, a);
                let nan = self.b.ins().fcmp(FloatCC::Unordered, a, a);
                (Some(self.b.ins().select(nan, a, s1)), t)
            }
            "powi" => {
                let n = arg(self, 0, Ty::I32)?;
                let f = if f32 { "__exr_powif" } else { "__exr_powi" };
                let (r, _) = self.call_named(f, vec![Ty::Unit], &[(a, t), (n, Ty::I32)], span)?;
                (r, t)
            }
            "abs_sub" => {
                let b = arg(self, 0, t)?;
                let d = self.b.ins().fsub(a, b);
                let z = fc(self, 0.0);
                let g = self.b.ins().fcmp(FloatCC::GreaterThan, d, z);
                (Some(self.b.ins().select(g, d, z)), t)
            }
            // log(base) = ln(x) / ln(base)
            "log" => {
                let b = arg(self, 0, t)?;
                let la = libm(self, "log", vec![a])?;
                let lb = libm(self, "log", vec![b])?;
                (Some(self.b.ins().fdiv(la, lb)), t)
            }
            "total_cmp" => {
                // Turn the bit patterns into totally ordered integers and compare
                let b = arg(self, 0, t)?;
                let it = if f32 { types::I32 } else { types::I64 };
                let sh = if f32 { 31 } else { 63 };
                let key = |this: &mut Self, v: Value| -> Value {
                    let bits = this.b.ins().bitcast(it, MemFlagsData::new(), v);
                    let s = this.b.ins().sshr_imm_s(bits, sh);
                    let m = this.b.ins().ushr_imm_u(s, 1);
                    this.b.ins().bxor(bits, m)
                };
                let ka = key(self, a);
                let kb = key(self, b);
                let c = self.cmp_scalar(ka, kb, true);
                let (o, ot) = self.call_named("__exr_ordering", vec![Ty::Unit], &[(c, Ty::I32)], span)?;
                (o, ot)
            }
            n if lib2.contains(&n) => {
                let b = arg(self, 0, t)?;
                let f = if n == "powf" { "pow" } else { n };
                (Some(libm(self, f, vec![a, b])?), t)
            }
            n => {
                let f = match n {
                    "ln" => "log",
                    "exp_m1" => "expm1",
                    "ln_1p" => "log1p",
                    m => m,
                };
                (Some(libm(self, f, vec![a])?), t)
            }
        };
        Ok(Some(r))
    }
}

impl Cx<'_> {
    /// The name of a constant brought in by `use std::f64::consts::PI;` (when it is neither a local variable nor a const)
    pub(crate) fn float_const_name(&self, p: &Expr) -> Option<f64> {
        let Expr::Path(_, path) = p else { return None };
        let id = ident_of(path)?;
        // (the fast check first: the name is uppercase only)
        let n = id.to_string();
        if !n.bytes().all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit()) || n.is_empty() {
            return None;
        }
        if self.lookup(&n).is_some() || self.prog.const_index.contains_key(&n) {
            return None;
        }
        Some(match n.as_str() {
            "PI" => 3.141592653589793_f64,
            "E" => 2.718281828459045_f64,
            "TAU" => 6.283185307179586_f64,
            "SQRT_2" => 1.4142135623730951_f64,
            "LN_2" => 0.6931471805599453_f64,
            "LN_10" => 2.302585092994046_f64,
            "FRAC_PI_2" => 1.5707963267948966_f64,
            "FRAC_PI_3" => 1.0471975511965979_f64,
            "FRAC_PI_4" => 0.7853981633974483_f64,
            "FRAC_PI_6" => 0.5235987755982989_f64,
            "FRAC_1_SQRT_2" => 0.7071067811865476_f64,
            "LOG2_E" => 1.4426950408889634_f64,
            "LOG10_E" => 0.4342944819032518_f64,
            _ => return None,
        })
    }
}

/// The float methods float_method builds in: libm's of one argument and of two, and its own (std's inherent ones:
/// before a trait's method of the name, by path too)
pub(crate) const FLOAT_LIB1: &[&str] = &[
    "exp", "exp2", "ln", "log10", "log2", "sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "asinh", "acosh", "atanh",
    "cbrt", "round", "exp_m1", "ln_1p",
];
pub(crate) const FLOAT_LIB2: &[&str] = &["powf", "atan2", "hypot"];
pub(crate) const FLOAT_BUILTINS: &[&str] = &[
    "sqrt", "abs", "floor", "ceil", "trunc", "fract", "mul_add", "copysign", "min", "max", "is_nan", "is_finite", "is_infinite",
    "is_sign_negative", "is_sign_positive", "signum", "to_bits", "recip", "to_degrees", "to_radians", "clamp", "powi", "log",
    "total_cmp", "abs_sub", "is_normal", "is_subnormal", "classify",
];

/// Whether `t.m(..)` of a number type t is a method codegen builds in (std's inherent method)
pub(crate) fn prim_builtin(t: Ty, m: &str) -> bool {
    if t.is_float() {
        FLOAT_BUILTINS.contains(&m) || FLOAT_LIB1.contains(&m) || FLOAT_LIB2.contains(&m)
    } else {
        t.is_int() && crate::acg::p4_methods::INT_BUILTINS.contains(&m)
    }
}
