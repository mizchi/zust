use super::*;
use crate::acg::clif::MemFlagsData;
use crate::parse::GArg;

impl Cx<'_> {
    /// Built-in methods of arrays / slices (len / is_empty / get / get_mut / first / last / swap / contains)
    /// The instance of `Result<t, E>` (E is the name of a type in the prelude)
    pub(crate) fn result_of(&self, t: Ty, e: &str) -> Option<Ty> {
        let r = *self.prog.struct_index.get("Result")?;
        let ei = *self.prog.struct_index.get(e)?;
        self.prog.instantiate(r, vec![t, Ty::Struct(ei)]).ok().map(Ty::Struct)
    }

    /// Whether `recv.splitn(n, p)` is a slice's (`__exr_slice_splitn`), not a str's: the receiver is known to be a slice
    /// or an array, or p is a closure and the receiver (or the base of its range, `s[..]`) is not known to be text
    fn slice_splitn(&self, recv: &Expr, args: &[Expr]) -> bool {
        let text = |t: Option<Ty>| match t.map(|t| self.strip_refs(t)) {
            Some(Ty::Str) => true,
            Some(Ty::Struct(sid)) => self.prog.structs[self.prog.base_of(sid) as usize].name == "String",
            _ => false,
        };
        let t = self.peek(recv);
        if matches!(t.map(|t| self.strip_refs(t)), Some(Ty::Slice(_) | Ty::Array(..))) {
            return true;
        }
        let base_text = matches!(strip_paren(recv), Expr::Index(b, ix, _) if matches!(&**ix, Expr::Range(..)) && text(self.peek(b)));
        matches!(args.get(1).map(strip_paren), Some(Expr::Closure(..))) && !text(t) && !base_text
    }

    pub(crate) fn builtin_method(&mut self, mc: &Expr) -> R<Option<V>> {
        let Expr::MethodCall(mc_recv, mc_name, mc_tf, mc_args, _) = mc else { unreachable!() };
        let mc_recv: &Expr = mc_recv;
        let _ = (mc_name, mc_tf, mc_args);
        let name = mc_name.to_string();
        // sort / binary_search of arrays / slices: prelude fns (`__exr_slice_sort(&mut a)`)
        let slice_fn = match name.as_str() {
            "sort" | "sort_unstable" => Some(("__exr_slice_sort", true)),
            "sort_by" | "sort_unstable_by" => Some(("__exr_slice_sort_by", true)),
            "sort_by_key" | "sort_unstable_by_key" => Some(("__exr_slice_sort_by_key", true)),
            "sort_by_cached_key" => Some(("__exr_slice_sort_by_cached_key", true)),
            "binary_search" => Some(("__exr_slice_bsearch", false)),
            "binary_search_by" => Some(("__exr_slice_bsearch_by", false)),
            "binary_search_by_key" => Some(("__exr_slice_bsearch_by_key", false)),
            "windows" => Some(("__exr_slice_windows", false)),
            "is_sorted" => Some(("__exr_slice_is_sorted", false)),
            "is_sorted_by" => Some(("__exr_slice_is_sorted_by", false)),
            "is_sorted_by_key" => Some(("__exr_slice_is_sorted_by_key", false)),
            "chunks" => Some(("__exr_slice_chunks", false)),
            "chunks_exact" => Some(("__exr_slice_chunks_exact", false)),
            "chunks_mut" => Some(("__exr_slice_chunks_mut", true)),
            "chunks_exact_mut" => Some(("__exr_slice_chunks_exact_mut", true)),
            "split_first_chunk" => Some(("__exr_slice_split_first_chunk", false)),
            "first_chunk" => Some(("__exr_slice_first_chunk", false)),
            "to_vec" => Some(("__exr_slice_to_vec", false)),
            "copy_from_slice" => Some(("__exr_slice_copy_from", true)),
            "clone_from_slice" => Some(("__exr_slice_clone_from", true)),
            "split_at" => Some(("__exr_slice_split_at", false)),
            "split_at_mut" => Some(("__exr_slice_split_at_mut", true)),
            "split_first" => Some(("__exr_slice_split_first", false)),
            "split_last" => Some(("__exr_slice_split_last", false)),
            // (a slice's splitn takes a predicate; a str's (`"a,b"[..].splitn(2, ',')`) is the str helper's, also where
            // the type of the range's receiver is not known here)
            "splitn" if self.slice_splitn(mc_recv, mc_args) => Some(("__exr_slice_splitn", false)),
            // (a slice's to_owned is its to_vec: only a receiver known to be one. An array's is a clone of it, a
            // `[T; N]`, and a str's a String, also of a `s[..n]` whose type is not known here)
            "to_owned" if matches!(self.peek(mc_recv).map(|t| self.strip_refs(t)), Some(Ty::Slice(_))) => Some(("__exr_slice_to_vec", false)),
            "reverse" => Some(("__exr_slice_reverse", true)),
            "fill" => Some(("__exr_slice_fill", true)),
            "rotate_left" => Some(("__exr_slice_rotate_left", true)),
            "rotate_right" => Some(("__exr_slice_rotate_right", true)),
            _ => None,
        }
        .filter(|(f, _)| self.prog.index.contains_key(*f));
        // `names.join(", ")` / `parts.concat()` (a Vec / array / slice whose elements are &str / String)
        if matches!(name.as_str(), "join" | "concat")
            && mc_args.len() <= 1
            && self.prog.index.contains_key("__exr_join_str")
        {
            let own = match self.peek(&*mc_recv).map(|t| self.strip_refs(t)) {
                // (the join of a struct that is not a Vec (`path_buf.join("x")`) is not built in)
                Some(Ty::Struct(sid)) => self.vec_fields(sid).is_none(),
                _ => false,
            };
            if !own {
                // (a temporary receiver `vec![..].join(..)` is evaluated with a `&` added and dropped at the end of the statement)
                let r = &*mc_recv;
                let re: Expr = if is_place_expr(r) { r.clone() } else { Expr::Ref(false, Box::new(Expr::Paren(Box::new(r.clone())))) };
                let (base, len, elem) = self.indexable(&re)?;
                // concat of a Vec of Vecs: clone the elements into one Vec
                if let (Ty::Struct(es), "concat") = (elem, name.as_str()) {
                    if let Some((_, _, inner)) = self.vec_fields(es) {
                        let fat = self.make_fat_v(base, len);
                        let st = Ty::Ref(false, intern(Ty::Slice(intern(elem))));
                        let (v, t) = self.call_named("__exr_concat_vec", vec![inner], &[(fat, st)], SP)?;
                        return Ok(Some((v, t)));
                    }
                }
                let f = match elem {
                    Ty::Ref(_, Ty::Str) => "__exr_join_str",
                    Ty::Struct(es) if self.prog.structs[es as usize].name == "String" => "__exr_join_string",
                    t => return err(SP, format!("`{name}` on elements of type `{}` is not supported", self.prog.tn(t))),
                };
                let fat = self.make_fat_v(base, len);
                let st = Ty::Ref(false, intern(Ty::Slice(intern(elem))));
                let sref = Ty::Ref(false, intern(Ty::Str));
                let sep = match mc_args.first() {
                    Some(a) if self.peek(a) == Some(Ty::Char) => {
                        let (v, _) = self.expr(a, Some(Ty::Char))?;
                        self.char_to_str(v.unwrap(), SP)?
                    }
                    Some(a) => {
                        let (v, t) = self.expr(a, Some(sref))?;
                        self.same(SP, t, sref)?;
                        v.unwrap()
                    }
                    None => {
                        let e: Expr = Expr::Lit("\"\"".to_string(), crate::parse::Pos::default());
                        self.expr(&e, Some(sref))?.0.unwrap()
                    }
                };
                return self.call_named(f, vec![Ty::Unit], &[(fat, st), (sep, sref)], SP).map(Some);
            }
        }
        // `v.drain(a..b)` / `v.drain(..)`: Vec::__drain(lo, hi)
        if name == "drain" && mc_args.len() == 1 {
            if let (Expr::Range(rg_start, rg_end, rg_closed), Some(Ty::Struct(sid))) = (strip_paren(&mc_args[0]), self.peek(&*mc_recv).map(|t| self.strip_refs(t))) {
                if self.vec_fields(sid).is_some() && self.prog.method(sid, "__drain").is_some() {
                    let r = &*mc_recv;
                    let lo: Expr = match rg_start {
                        Some(e) => (**e).clone(),
                        None => Expr::Lit("0usize".to_string(), crate::parse::Pos::default()),
                    };
                    let hi: Expr = match (rg_end, *rg_closed) {
                        (Some(e), true) => Expr::Binary("+".to_string(), Box::new(Expr::Paren(e.clone())), Box::new(Expr::Lit("1".to_string(), crate::parse::Pos::default())), crate::parse::Span::default()),
                        (Some(e), _) => (**e).clone(),
                        (None, _) => mcall(Expr::Paren(Box::new(r.clone())), "len", vec![]),
                    };
                    let e: Expr = mcall(Expr::Paren(Box::new(r.clone())), "__drain", vec![lo, hi]);
                    return self.expr(&e, None).map(Some);
                }
            }
        }
        // `v[..n].sort()`: a range index is borrowed automatically
        if let (Some((f, m)), Expr::Index(_, ix_index, _)) = (slice_fn, strip_paren(&*mc_recv)) {
            if matches!(&**ix_index, Expr::Range(..)) {
                let r = &*mc_recv;
                if m {
                    self.bk_mut_use(r);
                }
                let e: Expr = if m { Expr::Ref(true, Box::new(r.clone())) } else { Expr::Ref(false, Box::new(r.clone())) };
                let (v, t) = self.expr(&e, None)?;
                let Some(v) = v else { return Ok(Some((None, Ty::Never))) };
                let id = self.prog.index[f];
                let args: Vec<&Expr> = mc_args.iter().collect();
                return self.call_fn(id, Some((v, t)), &args, None, None, SP).map(Some);
            }
        }
        let Some(mut rt) = self.peek(&*mc_recv) else {
            return Ok(None);
        };
        // (a value is never of an unsized type: even when recorded as `[T]` / `str`, it is actually a reference (`tok(k).to_vec()`))
        if matches!(rt, Ty::Slice(_) | Ty::Str) {
            rt = Ty::Ref(false, intern(rt));
        }
        while let Ty::Ref(_, i) = rt {
            if rt.is_fat() {
                break;
            }
            rt = *i;
        }
        // Methods of String that String itself does not have (split etc.) are &str's methods
        if let Ty::Struct(sid) = rt {
            let sd = &self.prog.structs[sid as usize];
            if sd.base.is_none()
                && sd.name == "String"
                && self.prog.self_method(sid, &name).is_none()
                && (matches!(
                    name.as_str(),
                    "split" | "split_whitespace" | "lines" | "trim" | "starts_with" | "ends_with" | "contains"
                        | "find" | "chars"
                ) || self.prog.index.contains_key(&format!("__exr_str_{name}"))
                    // (a trait's method for str, the program's or the prelude's (`__PathExt`'s `to_string_lossy` of an
                    // OsString, which is a String here): through the deref, as rustc's probe reaches it (#256))
                    || self.prog.prim_method(Ty::Str, &name).is_some_and(|id| self.prog.sigs[id as usize].has_self))
            {
                let r = &*mc_recv;
                let e: Expr = Expr::MethodCall(
                    Box::new(mcall(Expr::Paren(Box::new(r.clone())), "as_str", vec![])),
                    mc_name.clone(),
                    mc_tf.clone(),
                    mc_args.clone(), crate::parse::Span::default()
                );
                // (copy the type that type inference recorded for the original call onto the rebuilt node too: the T of `t.parse()`)
                let mut keys: Vec<usize> = vec![];
                if let Some(&t) = self.lets.get(&(mc as *const Expr as usize)) {
                    self.lets.insert(&e as *const Expr as usize, t);
                    keys.push(&e as *const Expr as usize);
                    // (in our own syntax tree, the method call node is the expression itself)
                }
                // (the receiver `(r).as_str()` is &str and `(r)` is the original receiver's type: the rebuilt node has no type inference record)
                if let Expr::MethodCall(nm_recv, _, _, _, _) = &e {
                    let a = &**nm_recv as *const Expr as usize;
                    self.lets.insert(a, Ty::Ref(false, intern(Ty::Str)));
                    keys.push(a);
                    if let Expr::MethodCall(inner_recv, _, _, _, _) = &**nm_recv {
                        let b = &**inner_recv as *const Expr as usize;
                        self.lets.insert(b, rt);
                        keys.push(b);
                        if let Expr::Paren(pp) = &**inner_recv {
                            let c = &**pp as *const Expr as usize;
                            self.lets.insert(c, rt);
                            keys.push(c);
                        }
                    }
                }
                let r = self.expr(&e, None).map(Some);
                // (e is thrown away here, so also remove the records for its address)
                for k in keys {
                    self.lets.remove(&k);
                }
                return r;
            }
        }
        // (a Vec too, as a slice when Vec itself has no method of that name: `v.copy_from_slice(..)`)
        let vec_slice = matches!(rt, Ty::Struct(sid) if self.vec_fields(sid).is_some() && self.prog.self_method(sid, &name).is_none());
        if let (Some((f, m)), true) = (slice_fn, vec_slice || matches!(rt, Ty::Array(..) | Ty::Ref(_, Ty::Slice(_)))) {
            if let Some(&id) = self.prog.index.get(f) {
                // Make the receiver `&[T]` / `&mut [T]`, and pass the arguments (closures too) as the original expressions
                // (not through a reference (`s.sort()` of `s: &mut [i32]` needs no `mut s`)
                if m && is_place_expr(&*mc_recv) && !matches!(rt, Ty::Ref(..)) {
                    self.bk_mut_use(&*mc_recv);
                }
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let (fat, st) = (self.make_fat_v(base, len), Ty::Ref(m, intern(Ty::Slice(intern(elem)))));
                let args: Vec<&Expr> = mc_args.iter().collect();
                // (`split_first_chunk::<16>()`: the turbofish goes from the first type parameter of the prelude fn)
                let tf: Option<&[crate::parse::GArg]> = (!mc_tf.is_empty()).then_some(&mc_tf[..]);
                return self.call_fn(id, Some((fat, st)), &args, tf, None, SP).map(Some);
            }
        }
        let elem = match rt {
            Ty::Array(t, _) => *t,
            Ty::Ref(_, Ty::Slice(t)) => **t,
            // &str has the same representation as [u8]
            Ty::Ref(_, Ty::Str) => return self.str_method(mc),
            Ty::Ptr(_, t) => return self.ptr_method(mc, rt, *t),
            t if t.is_float() => return self.float_method(mc, t),
            t if t.is_int() || t == Ty::Bool || t == Ty::Char => {
                let r = self.int_method(mc, t)?;
                // `is_alphabetic` / `to_digit` etc. of char: the prelude's `__exr_char_<m>`
                if r.is_none() && t == Ty::Char && self.prog.index.contains_key(&format!("__exr_char_{name}")) {
                    let (v, vt) = self.expr(&*mc_recv, None)?;
                    let Some(mut v) = v else { return Ok(Some((None, Ty::Never))) };
                    let mut vt = vt;
                    while let Ty::Ref(_, i) = vt {
                        v = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), v, 0);
                        vt = *i;
                    }
                    return self.helper_method("__exr_char_", &name, mc, (v, Ty::Char));
                }
                return Ok(r);
            }
            _ => return Ok(None),
        };
        let args: Vec<&Expr> = mc_args.iter().collect();
        let opt = |this: &Self, t: Ty| -> R<Ty> {
            let ob = *this
                .prog
                .struct_index
                .get("Option")
                .ok_or("Option is not available")?;
            Ok(Ty::Struct(this.prog.instantiate(ob, vec![t])?))
        };
        match name.as_str() {
            // `xs.iter()` / `xs.iter_mut()` as a value: the prelude's Iter / IterMut
            "iter" | "iter_mut" if self.prog.index.contains_key("__exr_slice_iter") || name == "iter_mut" => {
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let m = name == "iter_mut";
                let fat = self.make_fat_v(base, len);
                let f = if m { "__exr_slice_iter_mut" } else { "__exr_slice_iter" };
                self.call_named(f, vec![elem], &[(fat, Ty::Ref(m, intern(Ty::Slice(intern(elem)))))], SP)
                    .map(Some)
            }
            "len" | "is_empty" => {
                let (_, len, _) = self.indexable(&*mc_recv)?;
                if name == "len" {
                    Ok(Some((Some(len), Ty::Usize)))
                } else {
                    let c = self.b.ins().icmp_imm_u(IntCC::Equal, len, 0);
                    Ok(Some((Some(c), Ty::Bool)))
                }
            }
            // `s.try_into()`: Ok([T; N]) if the length is N (copying the elements)
            "try_into" if args.is_empty() => {
                let hint = self.try_into_hint.take();
                let rt = self.lets.get(&(mc as *const Expr as usize)).copied().or_else(|| self.result_of(hint?, "TryFromSliceError"));
                let Some(Ty::Struct(rs)) = rt else {
                    return err(SP, "cannot infer the target type of `try_into`");
                };
                let Some(&dt) = self.prog.structs[rs as usize].args.first() else {
                    return err(SP, "cannot infer the target type of `try_into`");
                };
                // (`&s[..]` → `&[T; N]` / `&mut [T; N]` stays a reference (no copy))
                let (n, by_ref) = match dt {
                    Ty::Array(_, n) => (n, false),
                    Ty::Ref(_, Ty::Array(_, n)) => (*n, true),
                    _ => return err(SP, "`try_into` on a slice is only supported into arrays"),
                };
                let (base, len, _) = self.indexable(&*mc_recv)?;
                let out = self.temp(Ty::Struct(rs));
                let vs = &self.prog.structs[rs as usize].variants;
                let okv = vs.iter().position(|v| v.name == "Ok").unwrap();
                let errv = vs.iter().position(|v| v.name == "Err").unwrap();
                let off = vs[okv].fields[0].2;
                let c = self.b.ins().icmp_imm_u(IntCC::Equal, len, n as i64);
                let yes = self.b.create_block();
                let no = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(c, yes, &[], no, &[]);
                self.b.switch_to_block(yes);
                self.store_tag(out, rs, okv);
                if by_ref {
                    self.b.ins().store(mf(), base, out, off as i32);
                } else {
                    let dst = self.addr_plus(out, off as i32);
                    self.copy(dst, base, dt);
                }
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(no);
                self.store_tag(out, rs, errv);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(out), Ty::Struct(rs))))
            }
            // `s.get(a..b)` / `get_mut`: Some(&s[a..b]) if the range is inside
            "get" | "get_mut" if matches!(args.first(), Some(Expr::Range(..))) => {
                let Some(rg) = args.first().copied() else { unreachable!() };
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let (lo, hi) = self.range_bounds(rg, len)?;
                let m = name == "get_mut";
                let st = Ty::Ref(m, intern(Ty::Slice(intern(elem))));
                let ot = opt(self, st)?;
                let Ty::Struct(oi) = ot else { unreachable!() };
                let out = self.temp(ot);
                let vpos = |this: &Self, n: &str| this.prog.structs[oi as usize].variants.iter().position(|v| v.name == n).unwrap();
                let (some_vi, none_vi) = (vpos(self, "Some"), vpos(self, "None"));
                let c1 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, lo, hi);
                let c2 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, hi, len);
                let c = self.b.ins().band(c1, c2);
                let inb = self.b.create_block();
                let outb = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(c, inb, &[], outb, &[]);
                self.b.switch_to_block(inb);
                self.store_tag(out, oi, some_vi);
                let p = self.elem_addr(base, lo, elem);
                let n = self.b.ins().isub(hi, lo);
                let fat = self.make_fat_v(p, n);
                let off = self.prog.structs[oi as usize].variants[some_vi].fields[0].2;
                let dst = self.addr_plus(out, off as i32);
                self.copy(dst, fat, st);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(outb);
                self.store_tag(out, oi, none_vi);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(out), ot)))
            }
            "get" | "get_mut" | "first" | "last" => {
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let i = match name.as_str() {
                    "first" => self.b.ins().iconst(types::I64, 0),
                    "last" => self.b.ins().iadd_imm_s(len, -1),
                    _ => {
                        let Some(a) = args.first() else {
                            return err(SP, "get needs an index");
                        };
                        let (v, t) = self.expr(a, Some(Ty::Usize))?;
                        self.same(SP, t, Ty::Usize)?;
                        v.unwrap()
                    }
                };
                let rty = Ty::Ref(name == "get_mut", intern(elem));
                let ot = opt(self, rty)?;
                let Ty::Struct(oi) = ot else { unreachable!() };
                let out = self.temp(ot);
                let vpos = |this: &Self, n: &str| {
                    this.prog.structs[oi as usize]
                        .variants
                        .iter()
                        .position(|v| v.name == n)
                        .unwrap()
                };
                let (some_vi, none_vi) = (vpos(self, "Some"), vpos(self, "None"));
                let inb = self.b.create_block();
                let outb = self.b.create_block();
                let done = self.b.create_block();
                // even if len - 1 is negative (empty), the unsigned comparison makes it out of range
                let c = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                self.b.ins().brif(c, inb, &[], outb, &[]);
                self.b.switch_to_block(inb);
                self.store_tag(out, oi, some_vi);
                let addr = self.elem_addr(base, i, elem);
                let off = self.prog.structs[oi as usize].variants[some_vi].fields[0].2;
                self.b.ins().store(mf(), addr, out, off as i32);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(outb);
                self.store_tag(out, oi, none_vi);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(out), ot)))
            }
            "swap" => {
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let [a1, a2] = args[..] else {
                    return err(SP, "swap needs two indices");
                };
                let (i, _) = self.expr(a1, Some(Ty::Usize))?;
                let (j, _) = self.expr(a2, Some(Ty::Usize))?;
                let (i, j) = (i.unwrap(), j.unwrap());
                let c1 = self.b.ins().icmp(IntCC::UnsignedLessThan, i, len);
                self.panic_unless(c1);
                let c2 = self.b.ins().icmp(IntCC::UnsignedLessThan, j, len);
                self.panic_unless(c2);
                let pi = self.elem_addr(base, i, elem);
                let pj = self.elem_addr(base, j, elem);
                let tmp = self.temp(elem);
                let (vi, _) = self.read(Place::Mem(pi, 0, elem));
                let (vj, _) = self.read(Place::Mem(pj, 0, elem));
                self.write(Place::Mem(tmp, 0, elem), vi);
                let (vt, _) = self.read(Place::Mem(tmp, 0, elem));
                self.write(Place::Mem(pi, 0, elem), vj);
                self.write(Place::Mem(pj, 0, elem), vt);
                Ok(Some((None, Ty::Unit)))
            }
            "contains" => {
                if !(elem.is_int() || matches!(elem, Ty::Bool | Ty::Char)) {
                    if let Some(&id) = self.prog.index.get("__exr_slice_contains") {
                        let (base, len, elem) = self.indexable(&*mc_recv)?;
                        let fat = self.make_fat_v(base, len);
                        let st = Ty::Ref(false, intern(Ty::Slice(intern(elem))));
                        return self.call_fn(id, Some((fat, st)), &args, None, None, SP).map(Some);
                    }
                    return err(
                        SP,
                        "contains is only supported for integer / bool / char elements",
                    );
                }
                let (base, len, elem) = self.indexable(&*mc_recv)?;
                let Some(a) = args.first() else {
                    return err(SP, "contains needs an argument");
                };
                let (x, xt) = self.expr(a, Some(Ty::Ref(false, intern(elem))))?;
                self.same(SP, xt, Ty::Ref(false, intern(elem)))?;
                let xv = self
                    .b
                    .ins()
                    .load(clif_ty(elem).unwrap(), mf(), x.unwrap(), 0);
                let i = self.b.declare_var(types::I64);
                let z = self.b.ins().iconst(types::I64, 0);
                self.b.def_var(i, z);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let done = self.b.create_block();
                let res = self.b.append_block_param(done, types::I8);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let iv = self.b.use_var(i);
                let c = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, len);
                let f0 = self.b.ins().iconst(types::I8, 0);
                self.b.ins().brif(c, body, &[], done, &[f0]);
                self.b.switch_to_block(body);
                let addr = self.elem_addr(base, iv, elem);
                let ev = self.b.ins().load(clif_ty(elem).unwrap(), mf(), addr, 0);
                let eq = self.b.ins().icmp(IntCC::Equal, ev, xv);
                let nx = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, nx);
                let t1 = self.b.ins().iconst(types::I8, 1);
                self.b.ins().brif(eq, done, &[t1], head, &[]);
                self.b.switch_to_block(done);
                Ok(Some((Some(res), Ty::Bool)))
            }
            "as_ptr" | "as_mut_ptr" => {
                let (base, _, elem) = self.indexable(&*mc_recv)?;
                Ok(Some((
                    Some(base),
                    Ty::Ptr(name == "as_mut_ptr", intern(elem)),
                )))
            }
            // (`r.into_iter()` with r: `&[T; N]` / `&[T]` (a generic fn's `I: IntoIterator`): `r.iter()`; `r.iter_mut()`
            // of a `&mut` one)
            "into_iter"
                if matches!(self.peek(mc_recv), Some(Ty::Ref(false, Ty::Array(..) | Ty::Slice(_))))
                    && self.prog.index.contains_key("__exr_slice_iter") =>
            {
                let e: &Expr = self.keep_expr(mcall(Expr::Paren(Box::new(mc_recv.clone())), "iter", vec![]));
                self.expr(e, None).map(Some)
            }
            "into_iter"
                if matches!(self.peek(mc_recv), Some(Ty::Ref(true, Ty::Array(..) | Ty::Slice(_))))
                    && self.prog.index.contains_key("__exr_slice_iter_mut") =>
            {
                let e: &Expr = self.keep_expr(mcall(Expr::Paren(Box::new(mc_recv.clone())), "iter_mut", vec![]));
                self.expr(e, None).map(Some)
            }
            "iter" | "iter_mut" | "into_iter" | "enumerate" | "rev" | "copied" | "cloned" => err(
                SP,
                format!("`.{name}()` is only supported directly in a `for` loop"),
            ),
            _ => Ok(None),
        }
    }

    /// &str: len / is_empty / as_bytes / as_ptr (the representation is the same as &[u8])
    pub(crate) fn str_method(&mut self, mc: &Expr) -> R<Option<V>> {
        let Expr::MethodCall(mc_recv, mc_name, mc_tf, mc_args, _) = mc else { unreachable!() };
        let mc_recv: &Expr = mc_recv;
        let _ = (mc_name, mc_tf, mc_args);
        let (v, mut t) = self.expr(&*mc_recv, None)?;
        let Some(mut fat) = v else {
            return Ok(Some((None, Ty::Never)));
        };
        // `&&&str` etc.: follow down to the fat reference (the address of where it is stored)
        while let Ty::Ref(_, i) = t {
            if t.is_fat() {
                break;
            }
            if !i.is_fat() {
                fat = self.b.ins().load(PTR, mf(), fat, 0);
            }
            t = *i;
        }
        let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
        Ok(Some(match mc_name.to_string().as_str() {
            "len" => (Some(self.b.ins().load(types::I64, mf(), fat, 8)), Ty::Usize),
            "is_empty" => {
                let l = self.b.ins().load(types::I64, mf(), fat, 8);
                (Some(self.b.ins().icmp_imm_u(IntCC::Equal, l, 0)), Ty::Bool)
            }
            "as_bytes" => (Some(fat), u8s),
            "as_ptr" => (
                Some(self.b.ins().load(PTR, mf(), fat, 0)),
                Ty::Ptr(false, intern(Ty::U8)),
            ),
            // split / lines / trim / starts_with / ends_with / contains / find: prelude fns
            "split" | "split_whitespace" | "lines" | "trim" | "starts_with" | "ends_with" | "contains" | "find"
                if !(mc_name.to_string() == "split" && mc_args.first().is_some_and(|a| self.peek(a) != Some(Ty::Char)))
                    && !matches!(mc_args.first(), Some(Expr::Closure(..)))
                    && self.prog.index.contains_key(match mc_name.to_string().as_str() {
                    "split" => "__exr_str_split",
                    "split_whitespace" => "__exr_str_split_ws",
                    "lines" => "__exr_str_lines",
                    "trim" => "__exr_str_trim",
                    "starts_with" => "__exr_str_starts_with",
                    "ends_with" => "__exr_str_ends_with",
                    _ => "__exr_str_find",
                }) =>
            {
                let sref = Ty::Ref(false, intern(Ty::Str));
                let name = mc_name.to_string();
                let f = match name.as_str() {
                    "split" => "__exr_str_split",
                    "split_whitespace" => "__exr_str_split_ws",
                    "lines" => "__exr_str_lines",
                    "trim" => "__exr_str_trim",
                    "starts_with" => "__exr_str_starts_with",
                    "ends_with" => "__exr_str_ends_with",
                    _ => "__exr_str_find",
                };
                let mut args = vec![(fat, sref)];
                for a in mc_args {
                    // a char pattern (`s.contains('x')`) is made UTF-8 and passed as &str
                    let char_pat = name != "split" && self.peek(a) == Some(Ty::Char);
                    let exp = if name == "split" || char_pat { Ty::Char } else { sref };
                    let (v, t) = self.expr(a, Some(exp))?;
                    if name == "split" && t != Ty::Char {
                        return err(SP, "only `char` separators are supported in `split`");
                    }
                    // (a `&&str` / `&String` pattern becomes &str)
                    let (v, t) = match t {
                        Ty::Ref(_, Ty::Ref(_, Ty::Str)) => (v, sref),
                        Ty::Ref(_, Ty::Struct(sid)) if exp == sref => match v.and_then(|x| self.deref_to_slice(x, *sid, false)) {
                            Some((fat, ft)) => (Some(fat), ft),
                            None => (v, t),
                        },
                        _ => (v, t),
                    };
                    self.same(SP, t, exp)?;
                    if char_pat {
                        let ss = self.b.create_stack_slot(4, 1 << 0);
                        let buf = self.b.ins().stack_addr(PTR, ss, 0);
                        let bt = Ty::Ptr(true, intern(Ty::U8));
                        let (n, _) = self.call_named(
                            "__exr_char_utf8",
                            vec![Ty::Unit],
                            &[(v.unwrap(), Ty::Char), (buf, bt)],
                            SP,
                        )?;
                        let fat = self.make_fat_v(buf, n.unwrap());
                        args.push((fat, sref));
                        continue;
                    }
                    args.push((v.unwrap(), t));
                }
                let r = self.call_named(f, vec![Ty::Unit], &args, SP)?;
                // contains is whether the result of find is Some
                if name == "contains" {
                    let (v, t) = r;
                    let v = v.unwrap();
                    let tag = self.b.ins().load(types::I8, mf(), v, 0);
                    let _ = t;
                    let some = self.b.ins().icmp_imm_u(IntCC::NotEqual, tag, 0);
                    return Ok(Some((Some(some), Ty::Bool)));
                }
                return Ok(Some(r));
            }
            // `s.chars()` / `s.bytes()`
            "chars" => {
                return self
                    .call_named("__exr_str_chars", vec![Ty::Unit], &[(fat, Ty::Ref(false, intern(Ty::Str)))], SP)
                    .map(Some);
            }
            // `"..".to_string()` / `to_owned()` are the prelude's String::from
            "to_string" | "to_owned" => {
                return self
                    .call_named("String::from", vec![], &[(fat, Ty::Ref(false, intern(Ty::Str)))], SP)
                    .map(Some);
            }
            m => {
                // the prelude's `__exr_str_<m>` (`s.split("ab")` is __exr_str_split_str)
                // (if the pattern is a closure, `__exr_str_<m>_fn`)
                let fname = format!("{m}_fn");
                let h = if m == "split" && !matches!(mc_args.first(), Some(Expr::Closure(..))) {
                    "split_str"
                } else if matches!(mc_args.first(), Some(Expr::Closure(..)))
                    && self.prog.index.contains_key(&format!("__exr_str_{fname}"))
                {
                    fname.as_str()
                } else {
                    m
                };
                if let Some(r) = self.helper_method("__exr_str_", h, mc, (fat, Ty::Ref(false, intern(Ty::Str))))? {
                    return Ok(Some(r));
                }
                return err(SP, format!("no method `{m}` on `&str`"));
            }
        }))
    }

    /// Raw pointers: add / sub / offset / is_null / read / write / read_volatile / write_volatile / cast
    pub(crate) fn ptr_method(&mut self, mc: &Expr, pt: Ty, elem: Ty) -> R<Option<V>> {
        let Expr::MethodCall(mc_recv, mc_name, mc_tf, mc_args, _) = mc else { unreachable!() };
        let mc_recv: &Expr = mc_recv;
        let _ = (mc_name, mc_tf, mc_args);
        // (E0133: the pointer methods that are `unsafe fn` in std; the wrapping_* ones, is_null and cast are safe)
        if matches!(mc_name.as_str(), "add" | "sub" | "offset" | "offset_from" | "read" | "read_volatile" | "read_unaligned" | "write" | "write_volatile" | "write_unaligned") {
            if let Expr::MethodCall(_, _, _, _, msp) = mc {
                self.check_unsafe_op(mc_name, msp.at)?;
            }
        }
        let (p, _) = self.expr(&*mc_recv, Some(pt))?;
        let p = p.unwrap();
        let args: Vec<&Expr> = mc_args.iter().collect();
        let name = mc_name.to_string();
        Ok(Some(match name.as_str() {
            "add" | "sub" | "offset" | "wrapping_add" | "wrapping_sub" | "wrapping_offset" => {
                let it = if name.contains("offset") {
                    Ty::Isize
                } else {
                    Ty::Usize
                };
                let (n, _) = self.expr(args[0], Some(it))?;
                let mut n = n.unwrap();
                if name.ends_with("sub") {
                    n = self.b.ins().ineg(n);
                }
                (Some(self.elem_addr(p, n, elem)), pt)
            }
            // (`p.offset_from(q)`: the distance in elements, `(p - q) / size_of::<T>()`)
            "offset_from" => {
                let (q, qt) = self.expr(args[0], Some(pt))?;
                // (`*const T` for both receivers: a `*mut T` of the same pointee too)
                if !matches!(qt, Ty::Ptr(_, i) if *i == elem) {
                    self.same(SP, qt, pt)?;
                }
                let d = self.b.ins().isub(p, q.unwrap());
                let (size, _) = self.prog.size_align(elem);
                let d = if size > 1 {
                    let sz = self.b.ins().iconst(PTR, size as i64);
                    self.b.ins().sdiv(d, sz)
                } else {
                    d
                };
                (Some(d), Ty::Isize)
            }
            "is_null" => (Some(self.b.ins().icmp_imm_u(IntCC::Equal, p, 0)), Ty::Bool),
            "read" | "read_volatile" | "read_unaligned" => {
                if elem.is_agg() {
                    // copy so that the value does not change even if the pointee is rewritten later
                    let a = self.temp(elem);
                    self.copy(a, p, elem);
                    (Some(a), elem)
                } else {
                    self.read(Place::Mem(p, 0, elem))
                }
            }
            "write" | "write_volatile" | "write_unaligned" => {
                let (v, vt) = self.expr(args[0], Some(elem))?;
                self.same(SP, vt, elem)?;
                self.write(Place::Mem(p, 0, elem), v);
                (None, Ty::Unit)
            }
            "cast" => {
                let to = match mc_tf.is_empty() {
                    false => match mc_tf.first() {
                        Some(GArg::Ty(t)) => self.parse_ty(t)?,
                        _ => return err(SP, "cast needs a type argument"),
                    },
                    // (no turbofish: the type inference gave the call, `v128_load(data.cast())`)
                    true => match self.peek(mc) {
                        Some(Ty::Ptr(_, t)) if !self.prog.has_param(*t) => *t,
                        _ => return err(SP, "`cast` needs a turbofish: `p.cast::<T>()`"),
                    },
                };
                let Ty::Ptr(m, _) = pt else { unreachable!() };
                (Some(p), Ty::Ptr(m, intern(to)))
            }
            // (`impl Tr for *const T`: the method of `__ptr`; method_call passes the pointer)
            m if self.prog.prim_method(pt, m).is_some() => return Ok(None),
            m => {
                return err(
                    SP,
                    format!("unsupported raw pointer method `{m}`"),
                );
            }
        }))
    }

    /// Built-in integer methods (wrapping_* / checked_* / saturating_* / min / max / abs / pow /
    /// count_ones / leading_zeros / trailing_zeros / is_power_of_two / rotate_* / swap_bytes / signum)
    pub(crate) fn int_method(&mut self, mc: &Expr, t: Ty) -> R<Option<V>> {
        let Expr::MethodCall(mc_recv, mc_name, mc_tf, mc_args, _) = mc else { unreachable!() };
        let mc_recv: &Expr = mc_recv;
        let _ = (mc_name, mc_tf, mc_args);
        let name = mc_name.to_string();
        let args: Vec<&Expr> = mc_args.iter().collect();
        if !INT_BUILTINS.contains(&name.as_str()) {
            return Ok(None);
        }
        // (the names that exist only for signed or only for unsigned types: on the others they are no built-in
        // method, checked before the receiver is lowered, so the lookup that follows sees it unlowered)
        // (and `div_ceil` / `wrapping_add_signed` of a signed type (the one unstable in std, the other unsigned-only), of
        // a char or bool; `div_ceil` of a 128-bit one, which Cranelift cannot divide)
        let unsigned_only = matches!(name.as_str(), "div_ceil" | "wrapping_add_signed");
        if matches!((name.as_str(), t.signed()), ("saturating_abs" | "saturating_neg", false) | ("next_multiple_of", true))
            || (unsigned_only && (t.signed() || !t.is_int() || (name == "div_ceil" && t.bits() == 128)))
        {
            return Ok(None);
        }
        // if the receiver is `&u64` / `&mut u64`, the pointee's value (auto-deref)
        let (a, a_ty) = if matches!(self.peek(&*mc_recv), Some(Ty::Ref(..))) {
            self.expr(&*mc_recv, None)?
        } else {
            self.expr(&*mc_recv, Some(t))?
        };
        let Some(mut a) = a else {
            return Ok(Some((None, Ty::Never)));
        };
        let mut at = a_ty;
        while let Ty::Ref(_, i) = at {
            a = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), a, 0);
            at = *i;
        }
        let ct = clif_ty(t).unwrap();
        let s = t.signed();
        let arg = |this: &mut Self, i: usize, want: Ty| -> R<Value> {
            let Some(e) = args.get(i) else {
                return err(SP, format!("`{name}` needs an argument"));
            };
            let (v, vt) = this.expr(e, Some(want))?;
            this.same(SP, vt, want)?;
            Ok(v.unwrap())
        };
        let ins_bool = |_this: &mut Self, c: Value| -> V { (Some(c), Ty::Bool) };
        let ascii = |this: &mut Self, lo: u8, hi: u8| -> Value {
            let x = this.b.ins().iadd_imm_s(a, -(lo as i64));
            this.b
                .ins()
                .icmp_imm_u(IntCC::UnsignedLessThanOrEqual, x, (hi - lo) as i64)
        };
        let r: V = match name.as_str() {
            "wrapping_add" => {
                let b = arg(self, 0, t)?;
                (Some(self.b.ins().iadd(a, b)), t)
            }
            // (unsigned: `sum.wrapping_add_signed(isize::from(d))`, unicode-width's; the bits of the signed one added)
            "wrapping_add_signed" => {
                let st = match t {
                    Ty::U8 => Ty::I8,
                    Ty::U16 => Ty::I16,
                    Ty::U32 => Ty::I32,
                    Ty::U64 => Ty::I64,
                    Ty::U128 => Ty::I128,
                    _ => Ty::Isize,
                };
                let b = arg(self, 0, st)?;
                (Some(self.b.ins().iadd(a, b)), t)
            }
            "wrapping_sub" => {
                let b = arg(self, 0, t)?;
                (Some(self.b.ins().isub(a, b)), t)
            }
            "wrapping_mul" => {
                let b = arg(self, 0, t)?;
                (Some(self.b.ins().imul(a, b)), t)
            }
            "wrapping_neg" => (Some(self.b.ins().ineg(a)), t),
            "wrapping_abs" => (Some(if s { self.b.ins().iabs(a) } else { a }), t),
            // (MIN saturates to MAX)
            "saturating_abs" | "saturating_neg" if s => {
                let v = if name == "saturating_abs" { self.b.ins().iabs(a) } else { self.b.ins().ineg(a) };
                let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                let max = self.int_const(t, crate::program::prim_assoc_const(t, "MAX").unwrap());
                let bad = self.b.ins().icmp(IntCC::Equal, a, min);
                (Some(self.b.ins().select(bad, max, v)), t)
            }
            // (signed: only MIN overflows; unsigned neg: everything but 0)
            "overflowing_neg" | "overflowing_abs" => {
                let v = if name == "overflowing_abs" { if s { self.b.ins().iabs(a) } else { a } } else { self.b.ins().ineg(a) };
                let of = if s {
                    let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                    self.b.ins().icmp(IntCC::Equal, a, min)
                } else if name == "overflowing_neg" {
                    self.b.ins().icmp_imm_u(IntCC::NotEqual, a, 0)
                } else {
                    self.b.ins().iconst(types::I8, 0)
                };
                self.pair_with_bool(v, t, of)?
            }
            "min" | "max" => {
                let b = arg(self, 0, t)?;
                let ins = self.b.ins();
                let v = match (name.as_str(), s) {
                    ("min", true) => ins.smin(a, b),
                    ("min", false) => ins.umin(a, b),
                    ("max", true) => ins.smax(a, b),
                    _ => ins.umax(a, b),
                };
                (Some(v), t)
            }
            "abs" => {
                // (as in debug, abs of MIN panics)
                if self.overflow_checks && s {
                    let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                    let bad = self.b.ins().icmp(IntCC::Equal, a, min);
                    self.ovf_panic(bad, 6, expr_start(mc));
                }
                (Some(self.b.ins().iabs(a)), t)
            }
            // (the absolute value of MIN is representable too: view it as the unsigned type of the same width)
            "unsigned_abs" => {
                let v = if s { self.b.ins().iabs(a) } else { a };
                let ut = match t {
                    Ty::I8 => Ty::U8,
                    Ty::I16 => Ty::U16,
                    Ty::I32 => Ty::U32,
                    Ty::I64 => Ty::U64,
                    Ty::I128 => Ty::U128,
                    Ty::Isize => Ty::Usize,
                    t => t,
                };
                (Some(v), ut)
            }
            "checked_abs" | "checked_neg" => {
                let v = if name == "checked_abs" { self.b.ins().iabs(a) } else { self.b.ins().ineg(a) };
                let bad = if s {
                    let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                    self.b.ins().icmp(IntCC::Equal, a, min)
                } else if name == "checked_neg" {
                    // (for unsigned, only 0)
                    self.b.ins().icmp_imm_u(IntCC::NotEqual, a, 0)
                } else {
                    self.b.ins().iconst(types::I8, 0)
                };
                self.option_if(v, t, bad)?
            }
            "abs_diff" => {
                let b = arg(self, 0, t)?;
                let ins = self.b.ins();
                let (hi, lo) = if s {
                    (ins.smax(a, b), self.b.ins().smin(a, b))
                } else {
                    (ins.umax(a, b), self.b.ins().umin(a, b))
                };
                let ut = match t {
                    Ty::I8 => Ty::U8,
                    Ty::I16 => Ty::U16,
                    Ty::I32 => Ty::U32,
                    Ty::I64 => Ty::U64,
                    Ty::Isize => Ty::Usize,
                    t => t,
                };
                (Some(self.b.ins().isub(hi, lo)), ut)
            }
            "signum" => {
                let z = self.iconst_t(ct, 0);
                let gt = self.b.ins().icmp(IntCC::SignedGreaterThan, a, z);
                let lt = self.b.ins().icmp(IntCC::SignedLessThan, a, z);
                let g = self.b.ins().uextend(ct, gt);
                let l = self.b.ins().uextend(ct, lt);
                (Some(self.b.ins().isub(g, l)), t)
            }
            "is_positive" => {
                let c = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, a, 0);
                ins_bool(self, c)
            }
            "is_negative" => {
                let c = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, a, 0);
                ins_bool(self, c)
            }
            "count_ones" => {
                let v = self.b.ins().popcnt(a);
                (Some(self.convert(v, t, Ty::U32)), Ty::U32)
            }
            "count_zeros" => {
                let n = self.b.ins().bnot(a);
                let v = self.b.ins().popcnt(n);
                (Some(self.convert(v, t, Ty::U32)), Ty::U32)
            }
            "leading_zeros" => {
                let v = self.b.ins().clz(a);
                (Some(self.convert(v, t, Ty::U32)), Ty::U32)
            }
            "trailing_zeros" => {
                let v = self.b.ins().ctz(a);
                (Some(self.convert(v, t, Ty::U32)), Ty::U32)
            }
            "is_power_of_two" => {
                let c = self.b.ins().popcnt(a);
                let c = self.b.ins().icmp_imm_u(IntCC::Equal, c, 1);
                ins_bool(self, c)
            }
            "rotate_left" | "rotate_right" => {
                let b = arg(self, 0, Ty::U32)?;
                let v = if name == "rotate_left" {
                    self.b.ins().rotl(a, b)
                } else {
                    self.b.ins().rotr(a, b)
                };
                (Some(v), t)
            }
            // (x86-64 is little-endian: `to_le` is the value, `to_be` swaps its bytes)
            "to_le" => (Some(a), t),
            "swap_bytes" | "to_be" => (
                Some(if t.bits() == 8 {
                    a
                } else {
                    self.b.ins().bswap(a)
                }),
                t,
            ),
            "pow" | "wrapping_pow" | "checked_pow" | "saturating_pow" | "overflowing_pow" => {
                // exponentiation by squaring (also keeps the overflow flag: of the multiplied result and of the square still to be used)
                let checked = ((name != "pow" && name != "wrapping_pow") || (name == "pow" && self.overflow_checks)) && t.bits() < 128;
                let e0 = arg(self, 0, Ty::U32)?;
                let ofv = self.b.declare_var(types::I8);
                let f0 = self.b.ins().iconst(types::I8, 0);
                self.b.def_var(ofv, f0);
                let r = self.b.declare_var(ct);
                let bv = self.b.declare_var(ct);
                let ev = self.b.declare_var(types::I32);
                let one = self.iconst_t(ct, 1);
                self.b.def_var(r, one);
                self.b.def_var(bv, a);
                self.b.def_var(ev, e0);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let e = self.b.use_var(ev);
                self.b.ins().brif(e, body, &[], done, &[]);
                self.b.switch_to_block(body);
                let e = self.b.use_var(ev);
                let bit = self.b.ins().band_imm_u(e, 1);
                let rv = self.b.use_var(r);
                let bb = self.b.use_var(bv);
                let (rm, b2, e2) = if checked {
                    let (rm, o1) = if s { self.b.ins().smul_overflow(rv, bb) } else { self.b.ins().umul_overflow(rv, bb) };
                    let (b2, o2) = if s { self.b.ins().smul_overflow(bb, bb) } else { self.b.ins().umul_overflow(bb, bb) };
                    let e2 = self.b.ins().ushr_imm_u(e, 1);
                    // (the last square is not used, so its overflow is not counted)
                    let more = self.b.ins().icmp_imm_u(IntCC::NotEqual, e2, 0);
                    let o2 = self.b.ins().band(o2, more);
                    let bitb = self.b.ins().icmp_imm_u(IntCC::NotEqual, bit, 0);
                    let o1 = self.b.ins().band(o1, bitb);
                    let o = self.b.ins().bor(o1, o2);
                    let old = self.b.use_var(ofv);
                    let o = self.b.ins().bor(old, o);
                    self.b.def_var(ofv, o);
                    (rm, b2, e2)
                } else {
                    let rm = self.b.ins().imul(rv, bb);
                    let b2 = self.b.ins().imul(bb, bb);
                    (rm, b2, self.b.ins().ushr_imm_u(e, 1))
                };
                let rn = self.b.ins().select(bit, rm, rv);
                self.b.def_var(r, rn);
                self.b.def_var(bv, b2);
                self.b.def_var(ev, e2);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(done);
                let v = self.b.use_var(r);
                let of = self.b.use_var(ofv);
                match name.as_str() {
                    "checked_pow" => self.option_if(v, t, of)?,
                    "overflowing_pow" => self.pair_with_bool(v, t, of)?,
                    "pow" if checked => {
                        self.ovf_panic(of, 2, expr_start(mc));
                        (Some(v), t)
                    }
                    "saturating_pow" => {
                        // MIN for an odd power of a negative base, MAX otherwise
                        let max = self.int_const(t, crate::program::prim_assoc_const(t, "MAX").unwrap());
                        let sat = if s {
                            let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                            let z = self.iconst_t(ct, 0);
                            let nb = self.b.ins().icmp(IntCC::SignedLessThan, a, z);
                            let odd = self.b.ins().band_imm_u(e0, 1);
                            let odd = self.b.ins().icmp_imm_u(IntCC::NotEqual, odd, 0);
                            let negr = self.b.ins().band(nb, odd);
                            self.b.ins().select(negr, min, max)
                        } else {
                            max
                        };
                        (Some(self.b.ins().select(of, sat, v)), t)
                    }
                    _ => (Some(v), t),
                }
            }
            "saturating_add" | "saturating_sub" => {
                let b = arg(self, 0, t)?;
                // (Cranelift's *_sat are for vectors, so look at the overflow and replace with the edge value)
                let add = name == "saturating_add";
                let (v, of) = match (add, s) {
                    (true, false) => self.b.ins().uadd_overflow(a, b),
                    (true, true) => self.b.ins().sadd_overflow(a, b),
                    (false, false) => self.b.ins().usub_overflow(a, b),
                    (false, true) => self.b.ins().ssub_overflow(a, b),
                };
                let max = self.int_const(t, crate::program::prim_assoc_const(t, "MAX").unwrap());
                let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                let sat = if s {
                    // overflows downward if the number added is negative, upward if the number subtracted is negative
                    let z = self.iconst_t(ct, 0);
                    let bneg = self.b.ins().icmp(IntCC::SignedLessThan, b, z);
                    if add { self.b.ins().select(bneg, min, max) } else { self.b.ins().select(bneg, max, min) }
                } else if add {
                    max
                } else {
                    min
                };
                (Some(self.b.ins().select(of, sat, v)), t)
            }
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" | "checked_rem" => {
                let b = arg(self, 0, t)?;
                let (v, of) = match name.as_str() {
                    "checked_add" => self.arith_ovf(0, s, a, b, t),
                    "checked_sub" => self.arith_ovf(1, s, a, b, t),
                    "checked_mul" => self.arith_ovf(2, s, a, b, t),
                    _ => {
                        // division by 0 (and signed MIN / -1) is None. Replace with a safe divisor before dividing
                        let zero = self.b.ins().icmp_imm_u(IntCC::Equal, b, 0);
                        let bad = if s {
                            let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                            let am = self.b.ins().icmp(IntCC::Equal, a, min);
                            let bm = self.b.ins().icmp_imm_s(IntCC::Equal, b, -1);
                            let both = self.b.ins().band(am, bm);
                            self.b.ins().bor(zero, both)
                        } else {
                            zero
                        };
                        let one = self.iconst_t(ct, 1);
                        let safe = self.b.ins().select(bad, one, b);
                        if t.bits() == 128 {
                            let span = SP;
                            let q = self.div128(a, safe, t, name == "checked_rem", span)?;
                            (q, bad)
                        } else {
                        let ins = self.b.ins();
                        let q = match (name.as_str(), s) {
                            ("checked_div", true) => ins.sdiv(a, safe),
                            ("checked_div", false) => ins.udiv(a, safe),
                            ("checked_rem", true) => ins.srem(a, safe),
                            _ => ins.urem(a, safe),
                        };
                        (q, bad)
                        }
                    }
                };
                self.option_if(v, t, of)?
            }
            // a tuple of (value, whether it overflowed)
            // `a.wrapping_shl(n)`: the shift amount modulo the width; checked / overflowing tell whether n >= width
            "wrapping_shl" | "wrapping_shr" | "checked_shl" | "checked_shr" | "overflowing_shl" | "overflowing_shr" => {
                let n = arg(self, 0, Ty::U32)?;
                let bits = t.bits() as i64;
                let big = self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, n, bits);
                let n = self.convert(n, Ty::U32, t);
                let n = self.b.ins().band_imm_u(n, bits - 1);
                let v = if name.ends_with("shl") {
                    self.b.ins().ishl(a, n)
                } else if s {
                    self.b.ins().sshr(a, n)
                } else {
                    self.b.ins().ushr(a, n)
                };
                match &name[..3] {
                    "wra" => (Some(v), t),
                    "che" => self.option_if(v, t, big)?,
                    _ => self.pair_with_bool(v, t, big)?,
                }
            }
            // bit order reversed: swap neighbouring bits, pairs, nibbles, then the bytes
            // (128 bits: each 64-bit half reversed, and the halves swapped)
            "reverse_bits" => {
                let rev = |this: &mut Self, mut x: Value, ct: Type, bits: u32| -> Value {
                    for (sh, m) in [(1, 0x5555_5555_5555_5555u64), (2, 0x3333_3333_3333_3333), (4, 0x0f0f_0f0f_0f0f_0f0f)] {
                        let mk = this.iconst_t(ct, m as i64);
                        let shc = this.iconst_t(ct, sh);
                        let hi = this.b.ins().ushr(x, shc);
                        let hi = this.b.ins().band(hi, mk);
                        let lo = this.b.ins().band(x, mk);
                        let lo = this.b.ins().ishl(lo, shc);
                        x = this.b.ins().bor(hi, lo);
                    }
                    if bits == 8 { x } else { this.b.ins().bswap(x) }
                };
                let x = if t.bits() == 128 {
                    let (lo, hi) = self.b.ins().isplit(a);
                    let (lo, hi) = (rev(self, lo, types::I64, 64), rev(self, hi, types::I64, 64));
                    self.b.ins().iconcat(hi, lo)
                } else {
                    rev(self, a, ct, t.bits())
                };
                (Some(x), t)
            }
            // (`MIN / -1` overflows: (MIN, true) / (0, true); a zero divisor panics as `/` does. 128 bits divide in
            // the prelude, as `/` does)
            "overflowing_div" | "overflowing_rem" | "wrapping_div" | "wrapping_rem" => {
                let b = arg(self, 0, t)?;
                let zero = self.b.ins().icmp_imm_u(IntCC::Equal, b, 0);
                let rem = name.ends_with("rem");
                self.ovf_panic(zero, if rem { 4 } else { 3 }, SP);
                let of = if s {
                    let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                    let a_min = self.b.ins().icmp(IntCC::Equal, a, min);
                    let m1 = self.iconst_t(ct, -1);
                    let b_m1 = self.b.ins().icmp(IntCC::Equal, b, m1);
                    self.b.ins().band(a_min, b_m1)
                } else {
                    self.b.ins().iconst(types::I8, 0)
                };
                let one = self.iconst_t(ct, 1);
                let sb = self.b.ins().select(of, one, b);
                let v = match (rem, s) {
                    _ if t.bits() == 128 => self.div128(a, sb, t, rem, SP)?,
                    (false, true) => self.b.ins().sdiv(a, sb),
                    (false, false) => self.b.ins().udiv(a, sb),
                    (true, true) => self.b.ins().srem(a, sb),
                    (true, false) => self.b.ins().urem(a, sb),
                };
                if name.starts_with("wrapping") {
                    (Some(v), t)
                } else {
                    self.pair_with_bool(v, t, of)?
                }
            }
            "overflowing_add" | "overflowing_sub" | "overflowing_mul" => {
                let b = arg(self, 0, t)?;
                let (v, of) = match name.as_str() {
                    "overflowing_add" => self.arith_ovf(0, s, a, b, t),
                    "overflowing_sub" => self.arith_ovf(1, s, a, b, t),
                    _ => self.arith_ovf(2, s, a, b, t),
                };
                self.pair_with_bool(v, t, of)?
            }
            // `a.next_multiple_of(b)` (unsigned): a if a % b == 0, otherwise a + (b - a % b)
            "next_multiple_of" if !s => {
                let b = arg(self, 0, t)?;
                let r = self.b.ins().urem(a, b);
                let d = self.b.ins().isub(b, r);
                let up = self.b.ins().iadd(a, d);
                let z = self.b.ins().icmp_imm_u(IntCC::Equal, r, 0);
                (Some(self.b.ins().select(z, a, up)), t)
            }
            // `a.div_ceil(b)` (unsigned): a / b, one more when a % b is not 0. A b of 0 panics as `a / b` does
            "div_ceil" => {
                let b = arg(self, 0, t)?;
                if self.user_code && self.iconst_of(b).is_none_or(|c| c == 0) {
                    let zero = self.b.ins().icmp_imm_u(IntCC::Equal, b, 0);
                    self.ovf_panic(zero, 3, expr_start(mc));
                }
                let q = self.b.ins().udiv(a, b);
                let r = self.b.ins().urem(a, b);
                let nz = self.b.ins().icmp_imm_u(IntCC::NotEqual, r, 0);
                let one = self.b.ins().uextend(ct, nz);
                (Some(self.b.ins().iadd(q, one)), t)
            }
            "rem_euclid" | "div_euclid" => {
                let b = arg(self, 0, t)?;
                if !s {
                    let v = if name == "rem_euclid" { self.b.ins().urem(a, b) } else { self.b.ins().udiv(a, b) };
                    (Some(v), t)
                } else {
                    // r = a % b; r + |b| if r < 0, and the quotient is q - sign(b)
                    let rr = self.b.ins().srem(a, b);
                    let q = self.b.ins().sdiv(a, b);
                    let z = self.iconst_t(ct, 0);
                    let neg = self.b.ins().icmp(IntCC::SignedLessThan, rr, z);
                    if name == "rem_euclid" {
                        let ab = self.b.ins().iabs(b);
                        let fixed = self.b.ins().iadd(rr, ab);
                        (Some(self.b.ins().select(neg, fixed, rr)), t)
                    } else {
                        let bpos = self.b.ins().icmp(IntCC::SignedGreaterThan, b, z);
                        let one = self.iconst_t(ct, 1);
                        let qm = self.b.ins().isub(q, one);
                        let qp = self.b.ins().iadd(q, one);
                        let adj = self.b.ins().select(bpos, qm, qp);
                        (Some(self.b.ins().select(neg, adj, q)), t)
                    }
                }
            }
            "next_power_of_two" => {
                // 1 if x <= 1, otherwise 1 << (bits - clz(x - 1))
                let one = self.iconst_t(ct, 1);
                let xm = self.b.ins().isub(a, one);
                let lz = self.b.ins().clz(xm);
                let bits = self.iconst_t(ct, t.bits() as i64);
                let sh = self.b.ins().isub(bits, lz);
                let p = self.b.ins().ishl(one, sh);
                let small = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, a, one);
                (Some(self.b.ins().select(small, one, p)), t)
            }
            // (0 and negative numbers panic: a negative signed value is treated as 0 and left to ilog's panic)
            "ilog2" => {
                let lz = self.b.ins().clz(a);
                let lz = self.convert(lz, t, Ty::U32);
                let bits = self.b.ins().iconst(types::I32, t.bits() as i64 - 1);
                (Some(self.b.ins().isub(bits, lz)), Ty::U32)
            }
            "ilog10" => {
                let w = if t.bits() == 128 { a } else { self.convert(a, t, if t.signed() { Ty::I128 } else { Ty::U128 }) };
                let (v, _) = self.call_named("__exr_ilog10", vec![Ty::Unit], &[(w, Ty::U128)], SP)?;
                (v, Ty::U32)
            }
            // (a negative value panics, as std's does; 128 bits go through the prelude's u128 version)
            "isqrt" => {
                if s {
                    let z = self.int_const(t, 0);
                    let neg = self.b.ins().icmp(IntCC::SignedLessThan, a, z);
                    // (at the method's name, as std's #[track_caller] gives it)
                    let at = match mc {
                        Expr::MethodCall(_, _, _, _, sp) if sp.at.line != 0 => sp.at,
                        _ => expr_start(mc),
                    };
                    self.msg_panic(neg, "argument of integer square root cannot be negative", at);
                }
                let v = if t.bits() == 128 {
                    let (v, _) = self.call_named("__exr_isqrt128", vec![Ty::Unit], &[(a, Ty::U128)], SP)?;
                    v.unwrap()
                } else {
                    let w = if ct == types::I64 { a } else { self.b.ins().uextend(types::I64, a) };
                    let (v, _) = self.call_named("__exr_isqrt", vec![Ty::Unit], &[(w, Ty::U64)], SP)?;
                    let v = v.unwrap();
                    if ct == types::I64 { v } else { self.b.ins().ireduce(ct, v) }
                };
                (Some(v), t)
            }
            "try_into" => {
                // the type comes from type inference's record (Result<T, TryFromIntError>)
                let hint = self.try_into_hint.take();
                let rt = self.lets.get(&(mc as *const Expr as usize)).copied().or_else(|| self.result_of(hint?, "TryFromIntError")).or_else(|| {
                    // (our own syntax tree has no pos_key records, so use peek)
                    if self.in_fmt_args > 0 {
                        self.peek(mc)
                    } else {
                        None
                    }
                });
                let Some(Ty::Struct(rs)) = rt else {
                    return err(SP, "cannot infer the target type of `try_into`");
                };
                let Some(&dt) = self.prog.structs[rs as usize].args.first() else {
                    return err(SP, "cannot infer the target type of `try_into`");
                };
                if !dt.is_int() {
                    return err(SP, "`try_into` is only supported between integer types");
                }
                // widen to i128 and check the range (the upper half of u128 is compared unsigned)
                let w = if ct == types::I128 {
                    a
                } else if s {
                    self.b.ins().sextend(types::I128, a)
                } else {
                    self.b.ins().uextend(types::I128, a)
                };
                let z = self.int_const(Ty::I128, 0);
                let lo_ok = if s && !dt.signed() {
                    self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, w, z)
                } else if s {
                    let lo = self.int_const(Ty::I128, crate::program::prim_assoc_const(dt, "MIN").unwrap());
                    self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, w, lo)
                } else {
                    self.b.ins().iconst(types::I8, 1)
                };
                let hi_ok = if dt == Ty::U128 {
                    self.b.ins().iconst(types::I8, 1)
                } else {
                    let hi = self.int_const(Ty::I128, crate::program::prim_assoc_const(dt, "MAX").unwrap());
                    if t == Ty::U128 {
                        self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, w, hi)
                    } else {
                        self.b.ins().icmp(IntCC::SignedLessThanOrEqual, w, hi)
                    }
                };
                let ok = self.b.ins().band(lo_ok, hi_ok);
                let dct = clif_ty(dt).unwrap();
                let v = if dct == types::I128 { w } else { self.b.ins().ireduce(dct, w) };
                let out = self.temp(Ty::Struct(rs));
                let vs = &self.prog.structs[rs as usize].variants;
                let okv = vs.iter().position(|v| v.name == "Ok").unwrap();
                let errv = vs.iter().position(|v| v.name == "Err").unwrap();
                let off = vs[okv].fields[0].2;
                let yes = self.b.create_block();
                let no = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(ok, yes, &[], no, &[]);
                self.b.switch_to_block(yes);
                self.store_tag(out, rs, okv);
                self.b.ins().store(mf(), v, out, off as i32);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(no);
                self.store_tag(out, rs, errv);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                (Some(out), Ty::Struct(rs))
            }
            "leading_ones" | "trailing_ones" => {
                let n = self.b.ins().bnot(a);
                let c = if name == "leading_ones" { self.b.ins().clz(n) } else { self.b.ins().ctz(n) };
                (Some(self.to_u32(c, ct)), Ty::U32)
            }
            "is_ascii_digit" => {
                let c = ascii(self, b'0', b'9');
                ins_bool(self, c)
            }
            "is_ascii_uppercase" => {
                let c = ascii(self, b'A', b'Z');
                ins_bool(self, c)
            }
            "is_ascii_lowercase" => {
                let c = ascii(self, b'a', b'z');
                ins_bool(self, c)
            }
            "is_ascii_alphabetic" => {
                let u = ascii(self, b'A', b'Z');
                let l = ascii(self, b'a', b'z');
                let c = self.b.ins().bor(u, l);
                ins_bool(self, c)
            }
            "is_ascii_alphanumeric" => {
                let u = ascii(self, b'A', b'Z');
                let l = ascii(self, b'a', b'z');
                let d = ascii(self, b'0', b'9');
                let c = self.b.ins().bor(u, l);
                let c = self.b.ins().bor(c, d);
                ins_bool(self, c)
            }
            "is_ascii_hexdigit" => {
                let d = ascii(self, b'0', b'9');
                let u = ascii(self, b'A', b'F');
                let l = ascii(self, b'a', b'f');
                let c = self.b.ins().bor(u, l);
                let c = self.b.ins().bor(c, d);
                ins_bool(self, c)
            }
            // (from ! to /, from : to @, from [ to `, from { to ~)
            "is_ascii_punctuation" => {
                let mut acc = self.b.ins().iconst(types::I8, 0);
                for (lo, hi) in [(b'!', b'/'), (b':', b'@'), (b'[', b'`'), (b'{', b'~')] {
                    let c = ascii(self, lo, hi);
                    acc = self.b.ins().bor(acc, c);
                }
                ins_bool(self, acc)
            }
            "is_ascii_graphic" => {
                let c = ascii(self, b'!', b'~');
                ins_bool(self, c)
            }
            "is_ascii_control" => {
                let lo = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, a, 0x20);
                let del = self.b.ins().icmp_imm_u(IntCC::Equal, a, 0x7f);
                let c = self.b.ins().bor(lo, del);
                ins_bool(self, c)
            }
            "is_ascii" => {
                let c = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, a, 0x80);
                ins_bool(self, c)
            }
            // (x86-64 is little-endian: writing as is gives le / ne; be reverses the bytes first)
            "to_le_bytes" | "to_be_bytes" | "to_ne_bytes" => {
                let n = t.bits() / 8;
                let at = Ty::Array(intern(Ty::U8), n);
                let out = self.temp(at);
                let v = if name == "to_be_bytes" && n > 1 { self.b.ins().bswap(a) } else { a };
                let mut fl = MemFlagsData::new();
                fl.set_notrap();
                self.b.ins().store(fl, v, out, 0);
                (Some(out), at)
            }
            "is_ascii_whitespace" => {
                let mut acc = self.b.ins().iconst(types::I8, 0);
                for ch in [b' ', b'\t', b'\n', 0x0c, b'\r'] {
                    let c = self.b.ins().icmp_imm_u(IntCC::Equal, a, ch as i64);
                    acc = self.b.ins().bor(acc, c);
                }
                ins_bool(self, acc)
            }
            "to_ascii_uppercase" | "to_ascii_lowercase" => {
                let up = name == "to_ascii_uppercase";
                let c = if up {
                    ascii(self, b'a', b'z')
                } else {
                    ascii(self, b'A', b'Z')
                };
                let flipped = self.b.ins().bxor_imm_u(a, 0x20);
                (Some(self.b.ins().select(c, flipped, a)), t)
            }
            _ => unreachable!(),
        };
        Ok(Some(r))
    }
}

/// The integer methods int_method builds in (std's inherent ones: before a trait's method of the name, by path too)
pub(crate) const INT_BUILTINS: &[&str] = &[
    "wrapping_add", "wrapping_sub", "wrapping_mul", "wrapping_neg", "checked_add", "checked_sub", "checked_mul",
    "checked_div", "checked_rem", "checked_pow", "wrapping_pow", "saturating_pow", "overflowing_pow",
    "overflowing_add", "overflowing_sub", "overflowing_mul", "overflowing_div", "overflowing_rem", "wrapping_div",
    "wrapping_rem", "rem_euclid", "div_euclid", "next_multiple_of", "div_ceil", "wrapping_add_signed",
    "next_power_of_two", "isqrt", "ilog2", "ilog10", "unsigned_abs", "try_into", "checked_abs", "checked_neg",
    "wrapping_abs", "saturating_abs", "saturating_neg", "overflowing_neg", "overflowing_abs", "leading_ones",
    "trailing_ones", "saturating_add", "saturating_sub", "min", "max", "abs", "pow", "count_ones", "count_zeros",
    "leading_zeros", "trailing_zeros", "is_power_of_two", "rotate_left", "rotate_right", "swap_bytes", "to_be",
    "to_le", "signum", "is_positive", "is_negative", "abs_diff", "is_ascii_digit", "is_ascii_alphabetic",
    "is_ascii_uppercase", "is_ascii_lowercase", "to_ascii_uppercase", "to_ascii_lowercase", "is_ascii_whitespace",
    "is_ascii_alphanumeric", "is_ascii_hexdigit", "is_ascii_punctuation", "is_ascii_graphic", "is_ascii_control",
    "is_ascii", "to_le_bytes", "to_be_bytes", "to_ne_bytes", "wrapping_shl", "wrapping_shr", "checked_shl",
    "checked_shr", "overflowing_shl", "overflowing_shr", "reverse_bits",
];
