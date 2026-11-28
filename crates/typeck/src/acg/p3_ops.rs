use super::*;
use crate::parse::{Expr, GArg};
use crate::types::{Ty, intern};
use crate::acg::clif::MemFlagsData;

/// `(x)` (the same shape as the parentheses of syn's parse_quote!)
fn paren(e: &Expr) -> Expr {
    Expr::Paren(Box::new(e.clone()))
}

impl Cx<'_> {
    /// The place an operator's error is reported at: the binary expression's start
    fn op_pos(&self, e: &Expr) -> Sp {
        if let Expr::Binary(_, _, _, sp) | Expr::AssignOp(_, _, _, sp) = e { sp.start } else { SP }
    }

    /// E0277 for arithmetic of a float and an unsuffixed integer literal (`x + 100` with `x: f32`), where the literal
    /// `lit` (an operand of `e`) reported a mismatch when read as the float; else the error `m` as it is. rustc points
    /// at the operator
    fn int_float_op(&self, e: &Expr, lit: &Expr, m: String) -> String {
        let (Expr::Binary(op, l, r, sp) | Expr::AssignOp(op, l, r, sp)) = e else { return m };
        let at_lit = matches!(strip_paren(lit), Expr::Lit(t, _) if t.as_ptr() as usize == self.int_as_float);
        if !at_lit || !matches!(op.as_str(), "+" | "-" | "*" | "/" | "%") {
            return m;
        }
        let float = |x: &Expr| self.peek(x).filter(|t| t.is_float()).map(|t| self.prog.tn(t));
        let right = std::ptr::eq(lit, &**r);
        let Some((lt, rt)) = (if right { float(l).map(|t| (t, "{integer}".to_string())) } else { float(r).map(|t| ("{integer}".to_string(), t)) }) else {
            return m;
        };
        let (l, r) = (lt, rt);
        let msg = match (op.as_str(), matches!(e, Expr::AssignOp(..))) {
            ("+", false) => format!("cannot add `{r}` to `{l}`"),
            ("-", false) => format!("cannot subtract `{r}` from `{l}`"),
            ("*", false) => format!("cannot multiply `{l}` by `{r}`"),
            ("/", false) => format!("cannot divide `{l}` by `{r}`"),
            ("%", false) => format!("cannot calculate the remainder of `{l}` divided by `{r}`"),
            ("+", true) => format!("cannot add-assign `{r}` to `{l}`"),
            ("-", true) => format!("cannot subtract-assign `{r}` from `{l}`"),
            ("*", true) => format!("cannot multiply-assign `{l}` by `{r}`"),
            ("/", true) => format!("cannot divide-assign `{l}` by `{r}`"),
            _ => format!("cannot calculate and assign the remainder of `{l}` divided by `{r}`"),
        };
        self.at(sp.at, format!("{msg} (E0277)"))
    }

    /// rustc's E0369 message for `l op r` (a placeholder `T#` shown as `T`, an unsuffixed literal as `{integer}` /
    /// `{float}`, an unknown type as `_`)
    fn cannot_op(&self, op: &str, lt: Ty, right: &Expr) -> String {
        let show = |t: Ty| self.prog.tn(t).replace('#', "").replace("__u_", "");
        let l = show(lt);
        // (a float first: `1E3` passes is_int_lit too)
        let r = if super::p2_call::is_float_lit(right) {
            "{float}".to_string()
        } else if super::p2_call::is_int_lit(right) {
            "{integer}".to_string()
        } else {
            self.peek(right).map_or("_".to_string(), show)
        };
        match op {
            "+" => format!("cannot add `{r}` to `{l}` (E0369)"),
            "-" => format!("cannot subtract `{r}` from `{l}` (E0369)"),
            "*" => format!("cannot multiply `{l}` by `{r}` (E0369)"),
            "/" => format!("cannot divide `{l}` by `{r}` (E0369)"),
            _ => format!("cannot calculate the remainder of `{l}` divided by `{r}` (E0369)"),
        }
    }

    pub(crate) fn unary(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Unary(op, x, _) = e else { unreachable!() };
        let op = op.as_str();
        // `-v` / `!v` (impl Neg / Not): a method call
        if let ("-" | "!", Some(Ty::Struct(sid))) = (op, self.peek(x)) {
            let m = if op == "-" { "neg" } else { "not" };
            if self.prog.method(sid, m).is_some() {
                let ne = mcall(paren(x), m, vec![]);
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        // (`-&v` / `!&v` with `impl Neg for &J`: that impl's, `(&v).__ref_neg()`)
        if let ("-" | "!", Some(Ty::Ref(_, Ty::Struct(sid)))) = (op, self.peek(x)) {
            let m = if op == "-" { "__ref_neg" } else { "__ref_not" };
            if self.prog.method(*sid, m).is_some() {
                let ne = mcall(paren(x), m, vec![]);
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
        }
        // A negative literal (`-128i8` / `-2147483648`) becomes a value right here (not a checked negation of MIN)
        if op == "-" {
            if let Expr::Lit(t, _) = strip_paren(x) {
                // (not `-2f32`: digits with a float's suffix are a float, negated below)
                if let Some(Lit::Int(i)) = lit::lit(t).filter(|l| !matches!(l, Lit::Int(i) if matches!(i.suffix(), "f32" | "f64"))) {
                    let lit_e: &Expr = strip_paren(x);
                    // (`-129i8` / `let x: i8 = -129;`: past the type, overflowing_literals; `-128i8` is not)
                    if let (Some(t), true) = (Ty::from_suffix(i.suffix()).or(expected.filter(|t| t.is_int())), self.user_code && !self.prog.allow_overflowing_literals) {
                        if t.signed() && !super::p2_call::lit_fits(t, i.base10_digits(), true) {
                            return err(SP, format!("literal out of range for `{}`", self.prog.tn(t)));
                        }
                    }
                    let ty = Ty::from_suffix(i.suffix())
                        .or(expected.filter(|t| t.is_int()))
                        .or_else(|| self.lets.get(&(lit_e as *const Expr as usize)).copied().filter(|t| t.is_int()))
                        .or_else(|| self.lets.get(&(e as *const Expr as usize)).copied().filter(|t| t.is_int()))
                        .unwrap_or(Ty::I32);
                    if ty.signed() {
                        if let Ok(n) = i.base10_digits().parse::<u128>() {
                            let v = self.int_const(ty, (n as i128).wrapping_neg());
                            return Ok((Some(v), ty));
                        }
                    }
                }
            }
        }
        let (v, mut t) = self.expr(x, expected)?;
        let Some(mut v) = v else { return Ok((None, t)) };
        // `-&x` / `!&b` (impl Neg for &i32 and so on): reads the number behind the reference
        if let Ty::Ref(_, i) = t {
            if i.is_int() || i.is_float() || *i == Ty::Bool {
                v = self.b.ins().load(clif_ty(*i).unwrap(), mf(), v, 0);
                t = *i;
            }
        }
        match op {
            "-" if t.is_int() && t.signed() => {
                if self.overflow_checks {
                    let min = self.int_const(t, crate::program::prim_assoc_const(t, "MIN").unwrap());
                    let bad = self.b.ins().icmp(IntCC::Equal, v, min);
                    self.ovf_panic(bad, 6, expr_start(e));
                }
                Ok((Some(self.b.ins().ineg(v)), t))
            }
            "-" if t.is_float() => Ok((Some(self.b.ins().fneg(v)), t)),
            "!" if t == Ty::Bool => Ok((Some(self.b.ins().bxor_imm_u(v, 1)), t)),
            "!" if t.is_int() => Ok((Some(self.b.ins().bnot(v)), t)),
            _ => err(SP, format!("unsupported unary operator for `{}`", self.prog.tn(t))),
        }
    }

    /// A struct of the program in t that does not implement tr (PartialEq / PartialOrd), the type of a comparison's
    /// operands: itself, the elements of an array / slice / tuple, behind references. A prelude type or a generic
    /// struct's instance is left to the impls it has
    fn lacks_cmp(&self, t: Ty, rt: &dyn Fn() -> Option<Ty>, tr: &str, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        match t {
            // (`impl PartialEq for &X`: the reference's own impl, before its target's)
            Ty::Ref(_, i) => {
                let own = self.prog.ref_impls.contains(&(t, tr.to_string()))
                    || self.prog.ref_impl_pat_holds(t, tr);
                !own && self.lacks_cmp(*i, &|| match rt() { Some(Ty::Ref(_, r)) => Some(*r), _ => None }, tr, depth + 1)
            }
            // (`impl PartialEq<R> for [N; 1]`: an impl for arrays / slices of the program's answers the comparison as a
            // whole, before its elements)
            Ty::Array(i, _) | Ty::Slice(i) => {
                // (elements first: only when they lack the trait is an impl for the whole looked for)
                if !self.lacks_cmp(*i, &|| match rt() { Some(Ty::Array(r, _) | Ty::Slice(r)) => Some(*r), _ => None }, tr, depth + 1) {
                    return false;
                }
                // (the program's own impls: the prelude's are for elements that are PartialEq themselves)
                let own = |f: u32| self.prog.item_of.get(f as usize).copied().flatten().is_some_and(|ii| ii < self.prog.prelude_from);
                // (an impl whose Self is this operand and whose right-hand side is the other operand (when known):
                // `impl PartialEq<RR> for [NN; 1]` answers `[NN] == RR`, not `[NN] == [NN]`)
                let r = rt();
                let fits = |f: u32| {
                    let sig = &self.prog.sigs[f as usize];
                    let (Some(&Ty::Ref(_, ps)), Some(&Ty::Ref(_, pr))) = (sig.params.first(), sig.params.get(1)) else { return false };
                    let mut sol = vec![None; sig.generics.len()];
                    self.prog.match_ty(*ps, t, &mut sol) && r.is_none_or(|r| self.prog.match_ty(*pr, r, &mut sol))
                };
                let fs = if tr == "PartialEq" { &self.prog.peq } else { &self.prog.pord };
                let whole = fs.iter().any(|&f| own(f) && (self.prog.names[f as usize].starts_with("__array::") || self.prog.names[f as usize].starts_with("__slice::")) && fits(f))
                    || self.prog.prim_method(t, if tr == "PartialEq" { "eq" } else { "partial_cmp" }).is_some_and(|f| own(f) && fits(f));
                !whole
            }
            Ty::Struct(sid) => {
                let b = self.prog.base_of(sid);
                if self.prog.is_tuple(b) {
                    // (a tuple compares with its own type only, `impl<A: PartialEq, ..> PartialEq for (A, ..)`: each field
                    // against its own type)
                    return self.prog.structs[sid as usize].fields.iter().any(|f| self.lacks_cmp(f.1, &|| Some(f.1), tr, depth + 1));
                }
                let user = self.prog.struct_item.get(b as usize).is_some_and(|&i| i < self.prog.prelude_from);
                // (`impl PartialEq<i32> for S`: an impl for another right-hand side counts, looked for only when the
                // plain one is missing)
                // (an impl of the trait (derived or written), not an inherent `eq` / `partial_cmp`, which
                // Program::implements also counts)
                user && b == sid && !self.prog.impls.contains(&(b, tr.to_string())) && !self.prog.blanket_traits.contains(tr)
                    && !{
                        // (`impl PartialEq<i32> for S`: the functions `S::<PartialEq<i32>>::eq`, which `==` picks directly)
                        let fs = if tr == "PartialEq" { &self.prog.peq } else { &self.prog.pord };
                        let n = &self.prog.structs[b as usize].name;
                        fs.iter().any(|&f| self.prog.names[f as usize].strip_prefix(n.as_str()).is_some_and(|r| r.starts_with("::<")))
                    }
            }
            _ => false,
        }
    }

    /// `a op b` (Expr::Binary) and the compound assignment `a op= b` (Expr::AssignOp)
    pub(crate) fn binary(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let (op, left, right, assign) = match e {
            Expr::Binary(op, l, r, _) => (op.as_str(), &**l, &**r, false),
            Expr::AssignOp(op, l, r, _) => (op.as_str(), &**l, &**r, true),
            _ => unreachable!(),
        };
        // (a fn item has no `==`: only fn pointers compare, `f as fn() == g as fn()`)
        if !assign && self.user_code && is_cmp(op) && self.fn_item(strip_paren(left)).is_some() {
            let pos = if let Expr::Binary(_, _, _, sp) = e { sp.start } else { SP };
            return err(pos, format!("binary operation `{op}` cannot be applied to a fn item (cast it to a fn pointer)"));
        }
        // (`()` compares equal to itself and orders as equal: there are no bits to compare; `&()` too. The operands
        // are still evaluated, for what they do)
        if !assign && is_cmp(op) {
            let strip = |t: Option<Ty>| {
                let mut t = t?;
                while let Ty::Ref(_, i) = t {
                    t = *i;
                }
                Some(t)
            };
            // (the same type on both sides: `() == &()` is E0308, as any comparison of a value with a reference)
            if strip(self.peek(left)) == Some(Ty::Unit) && strip(self.peek(right)) == Some(Ty::Unit) && self.peek(left) == self.peek(right) {
                self.expr(left, None)?;
                self.expr(right, None)?;
                let v = matches!(op, "==" | "<=" | ">=") as i64;
                return Ok((Some(self.b.ins().iconst(types::I8, v)), Ty::Bool));
            }
        }
        // Short-circuit evaluation
        // (a chain of one operator (`a && b && c`) is lowered as one: each operand's short-circuit goes to the end, so
        // the borrow check sees that c runs only after b did, as rustc's control flow does)
        if !assign && (op == "&&" || op == "||") {
            let mut ops: Vec<&Expr> = vec![right];
            let mut l0 = left;
            while let Expr::Binary(o, a, b, _) = l0 {
                if o != op {
                    break;
                }
                ops.push(b);
                l0 = a;
            }
            ops.push(l0);
            ops.reverse();
            let last = ops.pop().unwrap();
            let merge = self.b.create_block();
            let res = self.b.append_block_param(merge, types::I8);
            for (i, x) in ops.into_iter().enumerate() {
                let (l, lt) = self.expr(x, Some(Ty::Bool))?;
                self.same(SP, lt, Ty::Bool)?;
                let Some(l) = l else {
                    // (the first operand diverges: so does the whole. A later one: the operands before it may
                    // short-circuit to the end, whose value is the chain's)
                    if i == 0 {
                        return Ok((None, Ty::Never));
                    }
                    self.b.ins().trap(TrapCode::unwrap_user(1));
                    self.b.switch_to_block(merge);
                    return Ok((Some(res), Ty::Bool));
                };
                let rhs_blk = self.b.create_block();
                if op == "&&" {
                    self.b.ins().brif(l, rhs_blk, &[], merge, &[l]);
                } else {
                    self.b.ins().brif(l, merge, &[l], rhs_blk, &[]);
                }
                self.b.switch_to_block(rhs_blk);
            }
            let right = last;
            let (r, rt) = self.expr(right, Some(Ty::Bool))?;
            self.same(SP, rt, Ty::Bool)?;
            match r {
                Some(r) => self.b.ins().jump(merge, &[r]),
                None => self.b.ins().trap(TrapCode::unwrap_user(1)),
            };
            self.b.switch_to_block(merge);
            return Ok((Some(res), Ty::Bool));
        }

        // (E0369 / E0368: an operator on a type parameter whose bounds have not the operator's trait (`a * b` with
        // `T: MyMul<f64>`: a method of that name is not the operator). Not one behind a reference, which a where
        // clause may give it (`for<'x> &'x T: Add<&'x T>`)
        // (behind a reference: no check, and none of the mismatches the numeric path would make: the placeholder's
        // instance is left unchecked)
        // (only where placeholders are: zustc check's last round)
        let placeholders = matches!(op, "+" | "-" | "*" | "/" | "%") && self.prog.any_placeholder();
        if let Some(lt @ Ty::Ref(_, Ty::Struct(sid))) = if placeholders { self.peek(left) } else { None } {
            if self.prog.structs[*sid as usize].placeholder.is_some() && self.peek(right).is_some_and(|r| !matches!(r, Ty::Ref(..) | Ty::Struct(_))) {
                return err(SP, "an operator on a reference to a type parameter and a value of another type");
            }
            // (`&a + a` with `a: N`: no bound of N's is for `&N`. Reported only when no where clause may give it, which
            // placeholder_mismatch tells)
            if let (Some(Ty::Struct(_)), "+" | "-" | "*" | "/" | "%", false) = (self.peek(right), op, assign) {
                if self.prog.structs[*sid as usize].placeholder.is_some() {
                    return err(self.op_pos(e), self.cannot_op(op, lt, right));
                }
            }
        }
        if let Some(lt @ Ty::Struct(sid)) = if placeholders { self.peek(left) } else { None } {
            let tr = match op {
                "+" => "Add",
                "-" => "Sub",
                "*" => "Mul",
                "/" => "Div",
                "%" => "Rem",
                _ => "",
            };
            if let (Some(bs), false) = (&self.prog.structs[sid as usize].placeholder, tr.is_empty()) {
                // (the trait of the operator's form, `AddAssign` for `+=`; a bound's supertraits give it too
                // (`trait Sum: Add<Output = Self>`); a bound zustc does not know may)
                let want = if assign { format!("{tr}Assign") } else { tr.to_string() };
                let prog = self.prog;
                fn gives(prog: &Program, n: &str, want: &str, depth: u32) -> bool {
                    let n = n.rsplit("::").next().unwrap_or(n).trim();
                    n == want
                        || depth < 8 && prog.trait_supers.get(n).is_some_and(|ss| ss.iter().any(|s| gives(prog, s.split('<').next().unwrap_or(s), want, depth + 1)))
                        || !prog.traits.contains_key(n) && !matches!(n, "Copy" | "Clone" | "Debug" | "Display" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" | "Default" | "Sized" | "Send" | "Sync")
                            && !["Add", "Sub", "Mul", "Div", "Rem"].iter().any(|o| n == *o || n == format!("{o}Assign"))
                }
                let named = |b: &String| b.split('<').next().is_some_and(|n| gives(prog, n, &want, 0));
                if !bs.iter().any(named) {
                    let m = if assign {
                        format!("binary assignment operation `{op}=` cannot be applied to type `{}` (E0368)", self.prog.tn(lt).replace('#', ""))
                    } else {
                        self.cannot_op(op, lt, right)
                    };
                    return err(self.op_pos(e), m);
                }
            }
        }
        // Operator overloading (`impl Add for V` / String + &str): `a.add(b)` / `a.add_assign(b)`
        if let Some(Ty::Struct(sid)) = self.peek(left) {
            let m = match (op, assign) {
                ("+", false) => "add",
                ("-", false) => "sub",
                ("*", false) => "mul",
                ("/", false) => "div",
                ("%", false) => "rem",
                ("&", false) => "bitand",
                ("|", false) => "bitor",
                ("^", false) => "bitxor",
                ("<<", false) => "shl",
                (">>", false) => "shr",
                ("+", true) => "add_assign",
                ("-", true) => "sub_assign",
                ("*", true) => "mul_assign",
                ("/", true) => "div_assign",
                ("%", true) => "rem_assign",
                ("&", true) => "bitand_assign",
                ("|", true) => "bitor_assign",
                ("^", true) => "bitxor_assign",
                ("<<", true) => "shl_assign",
                (">>", true) => "shr_assign",
                _ => "",
            };
            if !m.is_empty() && self.prog.method(sid, m).is_some() {
                let ne = mcall(paren(left), m, vec![right.clone()]);
                let ne = self.keep_expr(ne);
                // (an operator: `&mut self` is not two-phase; a compound assignment's is, reserved as a shared
                // borrow while the right side is evaluated: `a += a.0` reads a, `x += x` cannot move it)
                if assign {
                    self.bk.op_assign_call = ne as *const Expr as usize;
                } else {
                    self.bk.op_call = ne as *const Expr as usize;
                }
                return self.expr(ne, expected);
            }
            // (E0369 / E0368: an arithmetic operator on a struct or enum of the program's that has no impl of its trait:
            // `W(1) + 1`, `a + b` of `a: Wrapper<T>`)
            if matches!(op, "+" | "-" | "*" | "/" | "%") && self.user_code && self.prog.structs[sid as usize].placeholder.is_none() {
                let b = self.prog.base_of(sid);
                let own = self.prog.struct_item.get(b as usize).is_some_and(|&ii| ii < self.prog.prelude_from);
                let tr = match (op, assign) {
                    ("+", false) => "Add",
                    ("-", false) => "Sub",
                    ("*", false) => "Mul",
                    ("/", false) => "Div",
                    ("%", false) => "Rem",
                    ("+", true) => "AddAssign",
                    ("-", true) => "SubAssign",
                    ("*", true) => "MulAssign",
                    ("/", true) => "DivAssign",
                    _ => "RemAssign",
                };
                let lt = Ty::Struct(sid);
                if own && !self.prog.is_closure(b) && self.prog.default_method(sid, m).is_none() && !self.prog.implements(lt, tr) {
                    let msg = if assign {
                        format!("binary assignment operation `{op}=` cannot be applied to type `{}` (E0368)", self.prog.tn(lt))
                    } else {
                        self.cannot_op(op, lt, right)
                    };
                    return err(self.op_pos(e), msg);
                }
            }
        }
        // `&a + &b` (`impl Add for &M`): `(&a).__ref_add(&b)`
        if let Some(Ty::Ref(_, Ty::Struct(sid))) = self.peek(left) {
            let m = match (op, assign) {
                ("+", false) => "add",
                ("-", false) => "sub",
                ("*", false) => "mul",
                ("/", false) => "div",
                ("%", false) => "rem",
                ("&", false) => "bitand",
                ("|", false) => "bitor",
                ("^", false) => "bitxor",
                ("<<", false) => "shl",
                (">>", false) => "shr",
                _ => "",
            };
            let rm = format!("__ref_{m}");
            if !m.is_empty() && self.prog.method(*sid, &rm).is_some() {
                let ne = mcall(paren(left), &rm, vec![right.clone()]);
                let ne = self.keep_expr(ne);
                return self.expr(ne, expected);
            }
            // (`&a == &b` with `impl PartialEq for &X`: that impl's eq, `(&a).eq(&&b)`, not X's fields)
            // (only PartialEq's `eq` for `&X` (another trait's `eq` gets the same name), and both sides `&X`)
            // (among the `__ref_eq` overloads: `impl Other for &X` may come before `impl PartialEq for &X`)
            let rname = format!("{}::__ref_eq", self.prog.structs[self.prog.base_of(*sid) as usize].name);
            let peq_ref = self.prog.method(*sid, "__ref_eq").is_some() && self.trait_candidate(&rname, "PartialEq").is_some();
            if matches!(op, "==" | "!=") && !assign && peq_ref && self.peek(right) == self.peek(left) {
                let call = mcall(paren(left), "eq", vec![Expr::Ref(false, Box::new(paren(right)))]);
                let ne = if op == "!=" { Expr::Unary("!".into(), Box::new(Expr::Paren(Box::new(call))), Default::default()) } else { call };
                let ne = self.keep_expr(ne);
                // (the call is PartialEq's eq, whichever trait's `eq` the type has first)
                let call_at = match ne {
                    Expr::Unary(_, i, _) => match &**i {
                        Expr::Paren(c) => &**c as *const Expr as usize,
                        c => c as *const Expr as usize,
                    },
                    c => c as *const Expr as usize,
                };
                self.explicit_traits.insert(call_at, "PartialEq".to_string());
                return self.expr(ne, expected);
            }
        }
        // Compound assignment a op= b
        let compound = match op {
            "+" | "-" | "*" | "/" | "%" | "^" | "&" | "|" | "<<" | ">>" if assign => Some(op),
            _ => None,
        };
        if let Some(op) = compound {
            // (for primitive operands the right side is evaluated first, then the place on the left (as rustc does:
            // `{ log(L); x }.0 += { log(R); 0 }` logs R, L))
            // (in a generic fn, `x += y` with x of a type parameter is a call of `add_assign` (left first), even when
            // the instance's type is primitive)
            // (a left side whose type is not known ahead (`(*r.borrow_mut()).y`) is placed first: its type is the
            // hint for a literal on the right. One of only a variable and fields has no effects: placing it first is
            // not observable, and needs no type lookup ahead)
            let mut root = strip_paren(left);
            let mut plain = true;
            while let Expr::Field(x, _, _) | Expr::Index(x, _, _) = root {
                plain &= matches!(root, Expr::Field(..));
                root = strip_paren(x);
            }
            plain &= matches!(root, Expr::Path(..));
            let lt0 = if plain { None } else { self.peek(left) };
            let generic = lt0.is_some_and(|t| self.env.params.iter().any(|(_, pt)| *pt == t));
            let mut lhs = None;
            let simple = matches!(root, Expr::Path(..) | Expr::Block(..));
            if plain || generic || lt0.is_none() || !simple {
                let pm = std::mem::replace(&mut self.bk.place_mut, true);
                let p = self.place(left);
                self.bk.place_mut = pm;
                lhs = Some(p?);
            }
            let lt1 = lhs.map(|p| p.ty()).or(lt0);
            let rhint = if matches!(op, "<<" | ">>") { self.peek(right) } else { lt1 };
            // (a literal takes the place's type, whatever type inference recorded for it: `c[0] *= 10` with c a
            // chunk whose element type inference did not reach)
            let (r, rt) = self.expr(right, rhint.or(lt1)).map_err(|m| self.int_float_op(e, right, m))?;
            let p = match lhs {
                Some(p) => p,
                None => {
                    let pm = std::mem::replace(&mut self.bk.place_mut, true);
                    let p = self.place(left);
                    self.bk.place_mut = pm;
                    p?
                }
            };
            let t = p.ty();
            // (an unsuffixed literal takes the place's type, which type inference may not have reached: `c[0] *= 10`
            // with c a chunk of a `[u8]`)
            let (r, rt) = match strip_paren(right) {
                Expr::Lit(text, _) if rt != t && (t.is_int() || t.is_float()) && rt.is_int() && !matches!(op, "<<" | ">>") && text.bytes().all(|c| c.is_ascii_digit() || c == b'_') => self.lit(text, Some(t))?,
                _ => (r, rt),
            };
            let Some(mut r) = r else {
                return Ok((None, Ty::Never));
            };
            let (l, _) = self.read(p);
            // `acc += x` (x: &i32 / &f32): a reference to a number becomes its value (as core's `AddAssign<&i32>`)
            let mut rt = rt;
            while let Ty::Ref(_, i) = rt {
                if !(i.is_int() || i.is_float() || matches!(i, Ty::Ref(..))) || rt.is_fat() {
                    break;
                }
                r = self.b.ins().load(clif_ty(*i).unwrap_or(PTR), mf(), r, 0);
                rt = *i;
            }
            let Some(l) = l else {
                return err(SP, "unsupported operand");
            };
            let v = self.arith(e, op, l, t, r, rt)?;
            self.write(p, Some(v));
            return Ok((None, Ty::Unit));
        }

        let is_shift = matches!(op, "<<" | ">>");
        let hint = if is_cmp(op) {
            self.peek(left).or_else(|| self.peek(right))
        } else if is_shift {
            self.peek(left).or(expected)
        } else {
            self.peek(left).or_else(|| self.peek(right)).or(expected)
        };
        // A comparison compares by reference and does not move (temporaries are owned until the end of the statement)
        let cmp = is_cmp(op);
        // `x * 10` (x: &i64): the literal's type is the reference's pointee
        let deref_num = |t: Option<Ty>| match t {
            Some(Ty::Ref(_, i)) if i.is_int() || i.is_float() => Some(*i),
            t => t,
        };
        let hint = deref_num(hint);
        let (l, lt) = self.operand(left, hint, cmp).map_err(|m| self.int_float_op(e, left, m))?;
        let Some(mut l) = l else {
            // (a `()` has no value either, but it does not diverge: `() + 1` has no `Add` (E0369))
            if lt == Ty::Unit && self.user_code && !cmp {
                return err(self.op_pos(e), self.cannot_op(op, lt, right));
            }
            return Ok((None, Ty::Never));
        };
        // (E0369: `X == X` for a struct of the program that is not PartialEq (`<` and the others: PartialOrd), also as an
        // element of an array, a slice, a tuple or behind a reference)
        if cmp && !assign && self.user_code && !lt.is_int() && self.lacks_cmp(lt, &|| self.peek(right), if op == "==" || op == "!=" { "PartialEq" } else { "PartialOrd" }, 0) {
            let pos = if let Expr::Binary(_, _, _, sp) = e { sp.start } else { SP };
            return err(pos, format!("binary operation `{op}` cannot be applied to type `{}` (E0369)", self.prog.tn(lt)));
        }
        // (a fat raw pointer's pair, copied before the right side runs: `p == { p = q; p }` compares the old p)
        if lt.is_fat() && matches!(lt, Ty::Ptr(..)) {
            let tmp = self.temp(lt);
            self.copy(tmp, l, lt);
            l = tmp;
        }
        let rhint = if is_shift { self.peek(right) } else { deref_num(Some(lt)) };
        // (`v < (1, 5)` / `v == 2`: a literal right side expects R of the left type's `PartialOrd<R>` / `PartialEq<R>`)
        let rhint = match op {
            "<" | "<=" | ">" | ">=" => self.rhs_hint_from_impls(&self.prog.pord.clone(), lt, right).or(rhint),
            "==" | "!=" => self.rhs_hint_from_impls(&self.prog.peq.clone(), lt, right).or(rhint),
            _ => rhint,
        };
        // (string / byte string literals keep their own type (`v == b"crate"`: v's Vec<u8> is not the expected type))
        let lit_str = matches!(strip_paren(right), Expr::Lit(t, _) if matches!(lit::lit(t), Some(Lit::ByteStr(_) | Lit::Str(_))));
        let rhint = if lit_str && matches!(self.strip_refs(lt), Ty::Struct(_)) { None } else { rhint.or(Some(lt)) };
        let (r, rt) = self.operand(right, rhint, cmp).map_err(|m| self.int_float_op(e, right, m))?;
        let Some(r) = r else {
            // (`1 + x` with `x: ()`: not a value that diverges, one that no number adds (E0277), or compares with (E0308))
            if rt == Ty::Unit && lt != Ty::Unit && self.user_code {
                if cmp {
                    return err(self.op_pos(e), format!("mismatched types: expected `{}`, found `()`", self.prog.tn(lt)));
                }
                return err(self.op_pos(e), self.cannot_op(op, lt, right).replace("(E0369)", "(E0277)"));
            }
            return Ok((None, Ty::Never));
        };

        // `String == &str` / `&str == String`: compares with the String as a &str
        let (mut l, mut lt, mut r, mut rt) = (l, lt, r, rt);
        // `i32 + &i32` and so on: a reference to an integer becomes its value first (as core's `impl Add<&i32> for i32`)
        if let (Ty::Ref(_, a), Ty::Ref(_, c)) = (lt, rt) {
            if a == c && (a.is_int() || a.is_float() || matches!(a, Ty::Char | Ty::Bool)) && !is_shift {
                let ct = clif_ty(*a).unwrap();
                l = self.b.ins().load(ct, mf(), l, 0);
                r = self.b.ins().load(ct, mf(), r, 0);
                lt = *a;
                rt = *a;
            }
        }
        // (`a == b` with `a: &i32, b: i32`: no `PartialEq<i32>` for `&i32`, nor `PartialOrd` (E0277); `+` and the others
        // have their `&i32 + i32` impls)
        if cmp && self.user_code {
            let num = |t: Ty| t.is_int() || t.is_float() || matches!(t, Ty::Char | Ty::Bool);
            let one_ref = match (lt, rt) {
                (Ty::Ref(_, i), t) | (t, Ty::Ref(_, i)) => *i == t && num(t),
                _ => false,
            };
            if one_ref {
                return err(self.op_pos(e), format!("can't compare `{}` with `{}` (E0277)", self.prog.tn(lt), self.prog.tn(rt)));
            }
        }
        if !matches!(op, "==" | "!=") || !lt.is_agg() {
            if let (true, Ty::Ref(_, i)) = (lt.is_int() || lt.is_float(), rt) {
                if *i == lt {
                    r = self.b.ins().load(clif_ty(lt).unwrap(), mf(), r, 0);
                    rt = lt;
                }
            } else if let (Ty::Ref(_, i), true) = (lt, rt.is_int() || rt.is_float()) {
                if *i == rt && !is_shift {
                    l = self.b.ins().load(clif_ty(rt).unwrap(), mf(), l, 0);
                    lt = rt;
                }
            }
        }
        // (the two sides of a shift may differ in type: `&u8 >> 4` / `x << &n`)
        if is_shift {
            if let Ty::Ref(_, i) = lt {
                if i.is_int() {
                    l = self.b.ins().load(clif_ty(*i).unwrap(), mf(), l, 0);
                    lt = *i;
                }
            }
            if let Ty::Ref(_, i) = rt {
                if i.is_int() {
                    r = self.b.ins().load(clif_ty(*i).unwrap(), mf(), r, 0);
                    rt = *i;
                }
            }
        }
        let str_ref = Ty::Ref(false, intern(Ty::Str));
        if matches!(op, "==" | "!=") && lt != rt {
            // (`&String == &str` / `&&str == &str` too: after removing the reference)
            if let (Ty::Ref(_, Ty::Struct(_) | Ty::Ref(..)), true) = (lt, rt == str_ref) {
                if let Ty::Ref(_, i) = lt {
                    // (a `&String` value is the String's place itself. `&&T` reads one level. `&&str` is represented as `&str`)
                    if matches!(i, Ty::Ref(..)) && !i.is_fat() {
                        l = self.b.ins().load(PTR, mf(), l, 0);
                    }
                    lt = *i;
                }
            } else if let (true, Ty::Ref(_, Ty::Struct(_) | Ty::Ref(..))) = (lt == str_ref, rt) {
                if let Ty::Ref(_, i) = rt {
                    // (a `&String` value is the String's place itself. `&&T` reads one level. `&&str` is represented as `&str`)
                    if matches!(i, Ty::Ref(..)) && !i.is_fat() {
                        r = self.b.ins().load(PTR, mf(), r, 0);
                    }
                    rt = *i;
                }
            }
            if let (Ty::Struct(sid), true) = (lt, rt == str_ref) {
                if let Some((f, ft)) = self.deref_to_slice(l, sid, false) {
                    l = f;
                    lt = ft;
                }
            } else if let (true, Ty::Struct(sid)) = (lt == str_ref, rt) {
                if let Some((f, ft)) = self.deref_to_slice(r, sid, false) {
                    r = f;
                    rt = ft;
                }
            }
        }
        // (fat raw pointers `*const [T]` / `*const dyn Tr`: their values, the data word then the metadata word, not the
        // addresses of the pairs that hold them)
        if lt.is_fat() && matches!(lt, Ty::Ptr(..)) && lt == rt && is_cmp(op) {
            // (pointers cannot be compared in a `const fn`, as below for thin ones)
            if self.user_code && self.prog.names.get(self.root as usize).is_some_and(|n| self.prog.const_fns.contains_key(n)) {
                let pos = if let Expr::Binary(_, _, _, sp) = e { sp.start } else { SP };
                return err(pos, "pointers cannot be reliably compared during const eval");
            }
            let (pa, pb) = (self.b.ins().load(PTR, mf(), l, 0), self.b.ins().load(PTR, mf(), r, 0));
            let (ma, mb) = (self.b.ins().load(PTR, mf(), l, 8), self.b.ins().load(PTR, mf(), r, 8));
            let v = match op {
                "==" | "!=" => {
                    let cc = if op == "==" { IntCC::Equal } else { IntCC::NotEqual };
                    let d = self.b.ins().icmp(cc, pa, pb);
                    let m = self.b.ins().icmp(cc, ma, mb);
                    if op == "==" { self.b.ins().band(d, m) } else { self.b.ins().bor(d, m) }
                }
                _ => {
                    let (strict, eq_ok) = match op {
                        "<" => (IntCC::UnsignedLessThan, IntCC::UnsignedLessThan),
                        "<=" => (IntCC::UnsignedLessThan, IntCC::UnsignedLessThanOrEqual),
                        ">" => (IntCC::UnsignedGreaterThan, IntCC::UnsignedGreaterThan),
                        _ => (IntCC::UnsignedGreaterThan, IntCC::UnsignedGreaterThanOrEqual),
                    };
                    // (data words differ: their order; equal: the metadata's)
                    let d_ord = self.b.ins().icmp(strict, pa, pb);
                    let d_eq = self.b.ins().icmp(IntCC::Equal, pa, pb);
                    let m_ord = self.b.ins().icmp(eq_ok, ma, mb);
                    let tie = self.b.ins().band(d_eq, m_ord);
                    self.b.ins().bor(d_ord, tie)
                }
            };
            return Ok((Some(v), Ty::Bool));
        }
        // `<` / `<=` / `>` / `>=` of different types: partial_cmp of `impl PartialOrd<R> for S` (None makes all false)
        if matches!(op, "<" | "<=" | ">" | ">=") && lt != rt {
            let cands = self.prog.pord.clone();
            if let Some((v, t)) = self.trait_bin(&cands, l, lt, r, rt, SP)? {
                let (c, _) = self.call_named("__exr_pord_code", vec![Ty::Unit], &[(v.unwrap(), Ty::Ref(false, intern(t)))], SP)?;
                let c = c.unwrap();
                let e = self.ord_code_test(c, op);
                return Ok((Some(e), Ty::Bool));
            }
        }
        // Ordering of `&str` / `String`: the prelude's __exr_str_cmp
        if matches!(op, "<" | "<=" | ">" | ">=") && self.prog.index.contains_key("__exr_str_cmp") {
            let str_ref = Ty::Ref(false, intern(Ty::Str));
            let as_str = |this: &mut Self, v: Value, t: Ty| -> Option<Value> {
                match t {
                    Ty::Ref(_, Ty::Str) => Some(v),
                    Ty::Struct(sid) => this.deref_to_slice(v, sid, false).filter(|(_, ft)| *ft == str_ref).map(|f| f.0),
                    Ty::Ref(_, Ty::Struct(sid)) => this.deref_to_slice(v, *sid, false).filter(|(_, ft)| *ft == str_ref).map(|f| f.0),
                    _ => None,
                }
            };
            if let (Some(x), Some(y)) = (as_str(self, l, lt), as_str(self, r, rt)) {
                let (c, _) = self.call_named("__exr_str_cmp", vec![Ty::Unit], &[(x, str_ref), (y, str_ref)], SP)?;
                let cc = match op {
                    "<" => IntCC::SignedLessThan,
                    "<=" => IntCC::SignedLessThanOrEqual,
                    ">" => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                return Ok((Some(self.b.ins().icmp_imm_s(cc, c.unwrap(), 0)), Ty::Bool));
            }
        }
        // Ordering of tuples, structs, enums, arrays, slices, Vecs and so on: per-type glue (__exr_cmp)
        if matches!(op, "<" | "<=" | ">" | ">=") && !(lt.is_int() || lt.is_float() || matches!(lt, Ty::Bool | Ty::Char)) {
            self.same(SP, rt, lt)?;
            let (la, t) = self.ord_addr(l, lt);
            let (ra, _) = self.ord_addr(r, rt);
            // A hand-written partial_cmp (no cmp): None makes all of them false
            if let Ty::Struct(sid) = t {
                if !self.prog.is_tuple(self.prog.base_of(sid))
                    && self.prog.method(sid, "partial_cmp").is_some()
                    && self.prog.method(sid, "cmp").is_none()
                    && self.prog.index.contains_key("__exr_pord_code")
                {
                    let rr = Ty::Ref(false, intern(t));
                    self.scopes.push(vec![]);
                    self.bind("__exr_ca".into(), rr, Some(la));
                    self.bind("__exr_cb".into(), rr, Some(ra));
                    let ne = crate::parse::parse_expr("__exr_pord_code::<()>(&__exr_ca.partial_cmp(__exr_cb))")?;
                    let ne = self.keep_expr(ne);
                    let (c, _) = self.expr(ne, Some(Ty::I32))?;
                    self.scopes.pop();
                    let e = self.ord_code_test(c.unwrap(), op);
                    return Ok((Some(e), Ty::Bool));
                }
            }
            // (-1 / 0 / 1, or 2 for unordered (a NaN inside))
            let c = self.cmp_call(la, ra, t)?;
            let e = self.ord_code_test(c, op);
            return Ok((Some(e), Ty::Bool));
        }
        // (`v == b"crate"` / `&v == b"x"` (v: Vec<u8>): peeled to `Vec<T> == [T; N]`, same value (the place's address))
        if matches!(op, "==" | "!=") && lt != rt {
            let is_vec = |this: &Self, t: Ty| matches!(t, Ty::Struct(sid) if this.vec_fields(sid).is_some());
            let peel = |this: &Self, t: Ty| match t {
                Ty::Ref(_, i @ (Ty::Array(..) | Ty::Struct(_))) if i.is_agg() && (matches!(i, Ty::Array(..)) || is_vec(this, *i)) => *i,
                t => t,
            };
            let (pl, pr) = (peel(self, lt), peel(self, rt));
            if (is_vec(self, pl) && matches!(pr, Ty::Array(..))) || (is_vec(self, pr) && matches!(pl, Ty::Array(..))) {
                lt = pl;
                rt = pr;
            }
        }
        // Mixed-type `==` / `!=`: calls eq of a PartialEq impl for the right type (`impl PartialEq<[T; N]> for Vec<T>`)
        if matches!(op, "==" | "!=") && lt != rt {
            if let Some(e) = self.trait_eq(l, lt, r, rt, SP)? {
                let e = if op == "!=" { self.b.ins().bxor_imm_u(e, 1) } else { e };
                return Ok((Some(e), Ty::Bool));
            }
        }
        // `&a == &b` (a, b structs and so on): peel the references and compare the contents
        // `&&str == &&str` / `&&[T] == &&[T]`: a `&&str` value is where the `&str` is (= a `&str` value): peel the type
        while let (true, Ty::Ref(_, i)) = (lt == rt, lt) {
            if !i.is_fat() {
                break;
            }
            lt = *i;
            rt = *i;
        }
        if matches!(op, "==" | "!=") && lt == rt && matches!(lt, Ty::Ref(..)) {
            let (mut l, mut r, mut t) = (l, r, lt);
            while let Ty::Ref(_, i) = t {
                if i.is_fat() || !matches!(i, Ty::Ref(..)) && !i.is_agg() {
                    break;
                }
                if !i.is_agg() {
                    l = self.b.ins().load(PTR, mf(), l, 0);
                    r = self.b.ins().load(PTR, mf(), r, 0);
                }
                t = *i;
            }
            if t.is_agg() {
                let e = self.agg_eq(l, r, t, SP)?;
                let e = if op == "!=" { self.b.ins().bxor_imm_u(e, 1) } else { e };
                return Ok((Some(e), Ty::Bool));
            }
        }
        // `&&u32 == &&u32` / `&x < &y`: follow the references and compare the scalars
        let (mut l, mut r) = (l, r);
        if is_cmp(op) && lt == rt {
            let mut base = lt;
            while let Ty::Ref(_, i) = base {
                base = *i;
            }
            if base != lt && !base.is_agg() && !base.is_fat() && (base.is_int() || base.is_float() || matches!(base, Ty::Bool | Ty::Char | Ty::Fn(_) | Ty::Ptr(..))) {
                while let Ty::Ref(_, i) = lt {
                    let ct = if matches!(i, Ty::Ref(..)) { PTR } else { clif_ty(*i).unwrap() };
                    l = self.b.ins().load(ct, mf(), l, 0);
                    r = self.b.ins().load(ct, mf(), r, 0);
                    lt = *i;
                }
                rt = lt;
            }
        }
        if is_cmp(op) {
            // (a `*mut T` or a reference compared with a `*const T`: coerced to it, `c_p == m_p`, `c_p == &x`)
            if !rt.is_fat() && matches!(lt, Ty::Ptr(..)) && super::raw_ptr_coerces(rt, lt) {
                rt = lt;
            }
            self.same(SP, rt, lt)?;
            // Aggregate == / !=: a hand-written eq if there is one, else a structural comparison (as derive(PartialEq))
            if matches!(op, "==" | "!=") && lt.is_agg() {
                let e = self.agg_eq(l, r, lt, SP)?;
                let e = if op == "!=" { self.b.ins().bxor_imm_u(e, 1) } else { e };
                return Ok((Some(e), Ty::Bool));
            }
            // Float comparison (NaN is false for == / < and so on, true for !=)
            if lt.is_float() {
                let cc = match op {
                    "==" => FloatCC::Equal,
                    "!=" => FloatCC::NotEqual,
                    "<" => FloatCC::LessThan,
                    "<=" => FloatCC::LessThanOrEqual,
                    ">" => FloatCC::GreaterThan,
                    _ => FloatCC::GreaterThanOrEqual,
                };
                return Ok((Some(self.b.ins().fcmp(cc, l, r)), Ty::Bool));
            }
            // (`Box<T>`: the values it points to are compared)
            if let Ty::Box(i) = lt {
                if !lt.is_fat() {
                    if matches!(op, "==" | "!=") {
                        let e = self.eq_at(l, r, lt, SP)?;
                        let e = if op == "!=" { self.b.ins().bxor_imm_u(e, 1) } else { e };
                        return Ok((Some(e), Ty::Bool));
                    }
                    let c = self.cmp_call(l, r, *i)?;
                    let e = self.ord_code_test(c, op);
                    return Ok((Some(e), Ty::Bool));
                }
            }
            // (pointers cannot be compared in a `const fn`: their addresses are not known during const eval)
            if matches!(lt, Ty::Ptr(..) | Ty::Fn(_)) && self.user_code && self.prog.names.get(self.root as usize).is_some_and(|n| self.prog.const_fns.contains_key(n)) {
                let pos = if let Expr::Binary(_, _, _, sp) = e { sp.start } else { SP };
                return err(pos, "pointers cannot be reliably compared during const eval");
            }
            if !(lt.is_int() || matches!(lt, Ty::Bool | Ty::Char | Ty::Ptr(..) | Ty::Fn(_))) {
                return err(SP, format!("comparison of `{}` is not supported", self.prog.tn(lt)));
            }
            let s = lt.signed();
            let cc = match op {
                "==" => IntCC::Equal,
                "!=" => IntCC::NotEqual,
                "<" if s => IntCC::SignedLessThan,
                "<" => IntCC::UnsignedLessThan,
                "<=" if s => IntCC::SignedLessThanOrEqual,
                "<=" => IntCC::UnsignedLessThanOrEqual,
                ">" if s => IntCC::SignedGreaterThan,
                ">" => IntCC::UnsignedGreaterThan,
                ">=" if s => IntCC::SignedGreaterThanOrEqual,
                ">=" => IntCC::UnsignedGreaterThanOrEqual,
                _ => unreachable!(),
            };
            return Ok((Some(self.b.ins().icmp(cc, l, r)), Ty::Bool));
        }
        let v = self.arith(e, op, l, lt, r, rt)?;
        Ok((Some(v), lt))
    }

    /// `l == r` (types differ) by an impl of PartialEq<R>. Self type S and right side R match either the value's type
    /// or its pointee (`&[T]` with `impl .. for [T]`, `&[T; N]` with `PartialEq<[T; N]>`)
    pub(crate) fn trait_eq(&mut self, l: Value, lt: Ty, r: Value, rt: Ty, span: Sp) -> R<Option<Value>> {
        let cands = self.prog.peq.clone();
        Ok(self.trait_bin(&cands, l, lt, r, rt, span)?.and_then(|v| v.0))
    }

    /// Picks from cands (candidates `fn m(&self, other: &R)`) the one that fits the types of l and r, and calls it
    pub(crate) fn trait_bin(&mut self, cands: &[u32], l: Value, lt: Ty, r: Value, rt: Ty, span: Sp) -> R<Option<V>> {
        // (value type t, whether to add a reference): for a value, pass &t; for a reference, its pointee is the type
        let views = |t: Ty| -> Vec<(Ty, bool)> {
            match t {
                Ty::Ref(_, i) => vec![(t, true), (*i, false)],
                t => vec![(t, true)],
            }
        };
        for &id in cands {
            let sig = &self.prog.sigs[id as usize];
            let (Some(&Ty::Ref(_, ps)), Some(&Ty::Ref(_, pr))) = (sig.params.first(), sig.params.get(1)) else {
                continue;
            };
            let ng = sig.generics.len();
            for &(xl, addl) in &views(lt) {
                for &(xr, addr) in &views(rt) {
                    let mut sol = vec![None; ng];
                    if !(self.prog.match_ty(*ps, xl, &mut sol) && self.prog.match_ty(*pr, xr, &mut sol)) {
                        continue;
                    }
                    let Some(targs) = sol.into_iter().collect::<Option<Vec<Ty>>>() else { continue };
                    let la = if addl { self.addr_of_value(l, lt) } else { l };
                    let ra = if addr { self.addr_of_value(r, rt) } else { r };
                    let name = self.prog.names[id as usize].clone();
                    let v = self.call_named(&name, targs, &[(la, Ty::Ref(false, intern(xl))), (ra, Ty::Ref(false, intern(xr)))], span)?;
                    return Ok(Some(v));
                }
            }
        }
        Ok(None)
    }

    /// Decides `<` and so on from partial_cmp's result code (-1 / 0 / 1, None is 2)
    pub(crate) fn ord_code_test(&mut self, c: Value, op: &str) -> Value {
        let is = |this: &mut Self, k: i64| this.b.ins().icmp_imm_s(IntCC::Equal, c, k);
        match op {
            "<" => is(self, -1),
            ">" => is(self, 1),
            // (<= is -1 or 0, >= is 0 or 1. None (2) is false for all)
            "<=" => {
                let (x, y) = (is(self, -1), is(self, 0));
                self.b.ins().bor(x, y)
            }
            _ => {
                let (x, y) = (is(self, 1), is(self, 0));
                self.b.ins().bor(x, y)
            }
        }
    }

    /// The R of lt's comparison impls that fits the right side's shape (literal, tuple, array), if exactly one does
    pub(crate) fn rhs_hint_from_impls(&self, cands: &[u32], lt: Ty, rhs: &Expr) -> Option<Ty> {
        if cands.is_empty() {
            return None;
        }
        let fits = |t: Ty, e: &Expr| -> bool {
            match strip_paren(e) {
                Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Int(_))) => t.is_int(),
                Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Float(_))) => t.is_float(),
                Expr::Unary(op, _, _) if op == "-" => t.is_int() || t.is_float(),
                Expr::Tuple(tp) => matches!(t, Ty::Struct(sid) if self.prog.is_tuple(self.prog.base_of(sid))
                    && self.prog.structs[sid as usize].args.len() == tp.len()),
                Expr::Array(a) => matches!(t, Ty::Array(_, n) if n as usize == a.len()),
                _ => false,
            }
        };
        let lt0 = match lt {
            Ty::Ref(_, i) => *i,
            t => t,
        };
        let mut found = None;
        for &id in cands {
            let sig = &self.prog.sigs[id as usize];
            let (Some(&Ty::Ref(_, ps)), Some(&Ty::Ref(_, pr))) = (sig.params.first(), sig.params.get(1)) else { continue };
            if sig.is_generic() || self.prog.has_param(*pr) {
                continue;
            }
            if (*ps == lt || *ps == lt0) && fits(*pr, rhs) {
                if found.is_some_and(|f| f != *pr) {
                    return None;
                }
                found = Some(*pr);
            }
        }
        found
    }

    /// A reference to the value v of type t (an address for an aggregate)
    pub(crate) fn addr_of_value(&mut self, v: Value, t: Ty) -> Value {
        if t.is_agg() {
            return v;
        }
        let a = self.temp(t);
        self.b.ins().store(mf(), v, a, 0);
        a
    }

    /// Whether a and b (values of type t; addresses for an aggregate) are equal (an i8 0/1)
    pub(crate) fn agg_eq(&mut self, a: Value, b: Value, t: Ty, span: Sp) -> R<Value> {
        if let Ty::Struct(sid) = t {
            if let Some(id) = self.prog.method(sid, "eq") {
                let sig0 = self.prog.sigs[id as usize].clone();
                let (fid, sig) = if sig0.is_generic() {
                    // eq of a generic impl (`impl<T: PartialEq> Vec<T>`): instantiate it
                    let mut sol = vec![None; sig0.generics.len()];
                    if let Some(st) = sig0.self_ty {
                        self.prog.match_ty(st, t, &mut sol);
                    }
                    let r = Ty::Ref(false, intern(t));
                    self.resolve_call(id, &[r, r], sol, None, span)?
                } else {
                    (id, sig0)
                };
                let f = self.fref(fid, &sig);
                let call = self.b.ins().call(f, &[a, b]);
                return Ok(self.b.inst_results(call)[0]);
            }
        }
        self.eq_at(a, b, t, span)
    }

    pub(crate) fn eq_at(&mut self, a: Value, b: Value, t: Ty, span: Sp) -> R<Value> {
        let one = |this: &mut Self| this.b.ins().iconst(types::I8, 1);
        match t {
            Ty::Unit => Ok(one(self)),
            // (a fat raw pointer: its data word and its metadata word)
            t if t.is_fat() && matches!(t, Ty::Ptr(..)) => {
                let (pa, pb) = (self.b.ins().load(PTR, mf(), a, 0), self.b.ins().load(PTR, mf(), b, 0));
                let (ma, mb) = (self.b.ins().load(PTR, mf(), a, 8), self.b.ins().load(PTR, mf(), b, 8));
                let d = self.b.ins().icmp(IntCC::Equal, pa, pb);
                let m = self.b.ins().icmp(IntCC::Equal, ma, mb);
                Ok(self.b.ins().band(d, m))
            }
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char | Ty::Ptr(..)) => Ok(self.b.ins().icmp(IntCC::Equal, a, b)),
            t if t.is_float() => Ok(self.b.ins().fcmp(FloatCC::Equal, a, b)),
            // `&str` / `&[T]`: compare by length and contents
            // (`Box<[T]>` / `Box<str>` likewise: the same (pointer, length) pair)
            Ty::Ref(_, Ty::Str | Ty::Slice(_)) | Ty::Box(Ty::Str | Ty::Slice(_)) => {
                let e = match t {
                    Ty::Ref(_, Ty::Slice(e)) | Ty::Box(Ty::Slice(e)) => **e,
                    _ => Ty::U8,
                };
                let (es, _) = self.prog.size_align(e);
                let pa = self.b.ins().load(PTR, mf(), a, 0);
                let la = self.b.ins().load(types::I64, mf(), a, 8);
                let pb = self.b.ins().load(PTR, mf(), b, 0);
                let lb = self.b.ins().load(types::I64, mf(), b, 8);
                let done = self.b.create_block();
                let res = self.b.append_block_param(done, types::I8);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let i = self.b.declare_var(types::I64);
                let z = self.b.ins().iconst(types::I64, 0);
                self.b.def_var(i, z);
                let zero8 = self.b.ins().iconst(types::I8, 0);
                let same_len = self.b.ins().icmp(IntCC::Equal, la, lb);
                self.b.ins().brif(same_len, head, &[], done, &[zero8]);
                self.b.switch_to_block(head);
                let iv = self.b.use_var(i);
                let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, la);
                let one8 = self.b.ins().iconst(types::I8, 1);
                self.b.ins().brif(more, body, &[], done, &[one8]);
                self.b.switch_to_block(body);
                let off = self.b.ins().imul_imm_s(iv, es as i64);
                let xa = self.b.ins().iadd(pa, off);
                let xb = self.b.ins().iadd(pb, off);
                let (x, y) = match clif_ty(e) {
                    Some(ct) if !e.is_agg() => (self.b.ins().load(ct, mf(), xa, 0), self.b.ins().load(ct, mf(), xb, 0)),
                    _ => (xa, xb),
                };
                let eq = self.agg_or_scalar_eq(x, y, e, span)?;
                let next = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, next);
                let zero8 = self.b.ins().iconst(types::I8, 0);
                self.b.ins().brif(eq, head, &[], done, &[zero8]);
                self.b.switch_to_block(done);
                Ok(res)
            }
            // (a struct behind a Box / reference goes to the glue fn: a recursive `enum T { R(&'static T) }` is not inlined)
            Ty::Box(inner @ Ty::Struct(_)) | Ty::Ref(_, inner @ Ty::Struct(_)) if self.prog.index.contains_key("__exr_eq") => {
                let r = Ty::Ref(false, intern(*inner));
                let (v, _) = self.call_named("__exr_eq", vec![*inner], &[(a, r), (b, r)], span)?;
                Ok(v.unwrap())
            }
            // References and Boxes compare by the pointee's value
            Ty::Ref(_, inner) | Ty::Box(inner) if !t.is_fat() => {
                let (x, y) = match clif_ty(*inner) {
                    Some(ct) if !inner.is_agg() => (self.b.ins().load(ct, mf(), a, 0), self.b.ins().load(ct, mf(), b, 0)),
                    _ => (a, b),
                };
                self.agg_or_scalar_eq(x, y, *inner, span)
            }
            Ty::Array(e, n) => {
                let (es, _) = self.prog.size_align(*e);
                let mut acc = one(self);
                for i in 0..n {
                    let (x, y) = self.load_field(a, b, *e, i * es);
                    let c = self.agg_or_scalar_eq(x, y, *e, span)?;
                    acc = self.b.ins().band(acc, c);
                }
                Ok(acc)
            }
            Ty::Struct(sid) => {
                let sd = &self.prog.structs[sid as usize];
                if !sd.is_enum {
                    let fields = sd.fields.clone();
                    // (scalar fields: all compared; otherwise the comparison stops at the first unequal field, as
                    // derive(PartialEq) does: a later field's `eq` may not be called)
                    if fields.iter().all(|f| f.1.is_int() || matches!(f.1, Ty::Bool | Ty::Char | Ty::Unit)) {
                        let mut acc = one(self);
                        for (_, ft, off) in fields {
                            let (x, y) = self.load_field(a, b, ft, off);
                            let c = self.agg_or_scalar_eq(x, y, ft, span)?;
                            acc = self.b.ins().band(acc, c);
                        }
                        return Ok(acc);
                    }
                    let done = self.b.create_block();
                    let res = self.b.append_block_param(done, types::I8);
                    // (a `#[repr(packed)]` struct's fields may be misaligned: an aggregate one is compared from aligned
                    // copies, as rustc's derive copies them out)
                    let packed = self.prog.packed.contains_key(&self.prog.structs[self.prog.base_of(sid) as usize].name);
                    for (_, ft, off) in fields {
                        let (mut x, mut y) = self.load_field(a, b, ft, off);
                        if packed && ft.is_agg() {
                            let (tx, ty) = (self.temp(ft), self.temp(ft));
                            self.copy(tx, x, ft);
                            self.copy(ty, y, ft);
                            x = tx;
                            y = ty;
                        }
                        let c = self.agg_or_scalar_eq(x, y, ft, span)?;
                        let next = self.b.create_block();
                        let z = self.b.ins().iconst(types::I8, 0);
                        self.b.ins().brif(c, next, &[], done, &[z]);
                        self.b.switch_to_block(next);
                    }
                    let o = one(self);
                    self.b.ins().jump(done, &[o]);
                    self.b.switch_to_block(done);
                    return Ok(res);
                }
                // enum: the tags are equal, and so are that variant's fields
                let (tag, variants) = (sd.tag, sd.variants.iter().map(|v| (v.discr, v.fields.clone())).collect::<Vec<_>>());
                let ct = clif_ty(tag).unwrap();
                let ta = self.b.ins().load(ct, mf(), a, 0);
                let tb = self.b.ins().load(ct, mf(), b, 0);
                let tag_eq = self.b.ins().icmp(IntCC::Equal, ta, tb);
                // If the fields are all numbers, compare all variants without branching (safe to read)
                let plain = variants
                    .iter()
                    .all(|(_, fs)| fs.iter().all(|f| f.1.is_int() || f.1.is_float() || matches!(f.1, Ty::Bool | Ty::Char | Ty::Unit)));
                if plain {
                    let mut any = self.b.ins().iconst(types::I8, 0);
                    for (discr, fields) in variants {
                        let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, discr);
                        let mut acc = is_v;
                        for (_, ft, off) in fields {
                            let (x, y) = self.load_field(a, b, ft, off);
                            let c = self.agg_or_scalar_eq(x, y, ft, span)?;
                            acc = self.b.ins().band(acc, c);
                        }
                        any = self.b.ins().bor(any, acc);
                    }
                    return Ok(self.b.ins().band(tag_eq, any));
                }
                // Branch on the tag, then compare only that variant's fields
                // (reading another variant's place would follow a String's pointer when it is `None`, and so on)
                let done = self.b.create_block();
                let res = self.b.append_block_param(done, types::I8);
                let zero = self.b.ins().iconst(types::I8, 0);
                let start = self.b.create_block();
                self.b.ins().brif(tag_eq, start, &[], done, &[zero]);
                self.b.switch_to_block(start);
                for (discr, fields) in variants {
                    if fields.is_empty() {
                        let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, discr);
                        let one_v = self.b.ins().iconst(types::I8, 1);
                        let next = self.b.create_block();
                        self.b.ins().brif(is_v, done, &[one_v], next, &[]);
                        self.b.switch_to_block(next);
                        continue;
                    }
                    let is_v = self.b.ins().icmp_imm_s(IntCC::Equal, ta, discr);
                    let vb = self.b.create_block();
                    let next = self.b.create_block();
                    self.b.ins().brif(is_v, vb, &[], next, &[]);
                    self.b.switch_to_block(vb);
                    let mut acc = self.b.ins().iconst(types::I8, 1);
                    for (_, ft, off) in fields {
                        let (x, y) = self.load_field(a, b, ft, off);
                        let c = self.agg_or_scalar_eq(x, y, ft, span)?;
                        acc = self.b.ins().band(acc, c);
                    }
                    self.b.ins().jump(done, &[acc]);
                    self.b.switch_to_block(next);
                }
                let z2 = self.b.ins().iconst(types::I8, 0);
                self.b.ins().jump(done, &[z2]);
                self.b.switch_to_block(done);
                Ok(res)
            }
            t => err(span, format!("`==` is not supported for `{}`", self.prog.tn(t))),
        }
    }

    pub(crate) fn agg_or_scalar_eq(&mut self, x: Value, y: Value, t: Ty, span: Sp) -> R<Value> {
        if t.is_agg() { self.agg_eq(x, y, t, span) } else { self.eq_at(x, y, t, span) }
    }

    /// The values of type t at a + off / b + off (addresses for an aggregate)
    pub(crate) fn load_field(&mut self, a: Value, b: Value, t: Ty, off: u32) -> (Value, Value) {
        match clif_ty(t) {
            Some(ct) if !t.is_agg() => (self.b.ins().load(ct, mf(), a, off as i32), self.b.ins().load(ct, mf(), b, off as i32)),
            _ => (self.addr_plus(a, off as i32), self.addr_plus(b, off as i32)),
        }
    }

    /// b is the whole operation (Binary / AssignOp, taken only for its span), op the operator without `=`
    pub(crate) fn arith(&mut self, b_expr: &Expr, op: &str, l: Value, lt: Ty, r: Value, rt: Ty) -> R<Value> {
        if matches!(op, "<<" | ">>") {
            if !lt.is_int() || !rt.is_int() {
                return err(SP, "shift of non-integer");
            }
            if self.overflow_checks && !self.iconst_of(r).is_some_and(|k| (0..lt.bits() as i64).contains(&k)) {
                // A shift by the type's width or more (a negative amount is large as unsigned too)
                let bad = self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, r, lt.bits() as i64);
                self.ovf_panic(bad, if op == "<<" { 7 } else { 8 }, expr_start(b_expr));
            }
            let ins = self.b.ins();
            return Ok(match op {
                "<<" => ins.ishl(l, r),
                _ if lt.signed() => ins.sshr(l, r),
                _ => ins.ushr(l, r),
            });
        }
        // The prelude's char range (`'a'..='e'`) gets the next char with `v + 1`
        if lt == Ty::Char && rt.is_int() && matches!(op, "+" | "-") {
            let r32 = match rt.bits() {
                32 => r,
                b if b > 32 => self.b.ins().ireduce(types::I32, r),
                _ => self.b.ins().uextend(types::I32, r),
            };
            return Ok(if op == "+" { self.b.ins().iadd(l, r32) } else { self.b.ins().isub(l, r32) });
        }
        self.same(SP, rt, lt)?;
        if lt.is_float() {
            return Ok(match op {
                "+" => self.b.ins().fadd(l, r),
                "-" => self.b.ins().fsub(l, r),
                "*" => self.b.ins().fmul(l, r),
                "/" => self.b.ins().fdiv(l, r),
                // (Cranelift has no float remainder, so libm's fmod)
                "%" => {
                    let f = if lt == Ty::F32 { "__exr_fmodf" } else { "__exr_fmod" };
                    let (v, _) = self.call_named(f, vec![], &[(l, lt), (r, lt)], SP)?;
                    v.unwrap()
                }
                _ => return err(SP, format!("unsupported operator for `{}`", self.prog.tn(lt))),
            });
        }
        let bool_ok = matches!(op, "&" | "|" | "^");
        if !(lt.is_int() || (lt == Ty::Bool && bool_ok)) {
            return err(SP, format!("unsupported operator for `{}`", self.prog.tn(lt)));
        }
        let s = lt.signed();
        // Cranelift has no 128-bit division, so it is a prelude function
        if lt.bits() == 128 && matches!(op, "/" | "%") {
            return self.div128(l, r, lt, op == "%", SP);
        }
        // Division by 0 and MIN / -1 panic (even when optimized, as in rustc)
        if lt.is_int() && matches!(op, "/" | "%") && self.user_code {
            let rem = op == "%";
            let cr = self.iconst_of(r);
            if cr.is_none() || cr == Some(0) {
                let zero = self.b.ins().icmp_imm_u(IntCC::Equal, r, 0);
                self.ovf_panic(zero, if rem { 4 } else { 3 }, expr_start(b_expr));
            }
            if s && (cr.is_none() || cr == Some(-1)) {
                let min = self.int_const(lt, crate::program::prim_assoc_const(lt, "MIN").unwrap());
                let am = self.b.ins().icmp(IntCC::Equal, l, min);
                let bm = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
                let both = self.b.ins().band(am, bm);
                self.ovf_panic(both, if rem { 10 } else { 9 }, expr_start(b_expr));
            }
        }
        // Overflow of addition, subtraction and multiplication
        if lt.is_int() && self.overflow_checks && matches!(op, "+" | "-" | "*") {
            let kind = match op {
                "+" => 0,
                "-" => 1,
                _ => 2,
            };
            if lt.bits() < 128 || kind == 2 {
                let (v, of) = self.arith_ovf(kind as u8, s, l, r, lt);
                self.ovf_panic(of, kind, expr_start(b_expr));
                return Ok(v);
            }
            // (128 bit: addition and subtraction check the result's sign and order; multiplication is not checked)
            if kind < 2 {
                let v = if kind == 0 { self.b.ins().iadd(l, r) } else { self.b.ins().isub(l, r) };
                let of = if !s {
                    if kind == 0 { self.b.ins().icmp(IntCC::UnsignedLessThan, v, l) } else { self.b.ins().icmp(IntCC::UnsignedLessThan, l, r) }
                } else {
                    let x = if kind == 0 { self.b.ins().bxor(l, v) } else { self.b.ins().bxor(l, r) };
                    let y = if kind == 0 { self.b.ins().bxor(r, v) } else { self.b.ins().bxor(l, v) };
                    let m = self.b.ins().band(x, y);
                    let z = self.int_const(lt, 0);
                    self.b.ins().icmp(IntCC::SignedLessThan, m, z)
                };
                self.ovf_panic(of, kind, expr_start(b_expr));
                return Ok(v);
            }
        }
        let ins = self.b.ins();
        Ok(match op {
            "+" => ins.iadd(l, r),
            "-" => ins.isub(l, r),
            "*" => ins.imul(l, r),
            "/" if s => ins.sdiv(l, r),
            "/" => ins.udiv(l, r),
            "%" if s => ins.srem(l, r),
            "%" => ins.urem(l, r),
            "&" => ins.band(l, r),
            "|" => ins.bor(l, r),
            "^" => ins.bxor(l, r),
            _ => return err(SP, "unsupported binary operator"),
        })
    }

    /// Value of a type-only condition (`__exr_int_kind::<T>() == 0` / `needs_drop::<T>()` with `!` / `||` / `&&`).
    /// The prelude's per-type branches then skip compiling the unused side (and make no instances for it)
    pub(crate) fn type_cond(&self, e: &Expr) -> Option<bool> {
        let targ = |c: &Expr| -> Option<(String, Ty)> {
            let Expr::Call(func, _) = c else { return None };
            let Expr::Path(_, p) = &**func else { return None };
            let last = p.segs.last()?;
            let Some(GArg::Ty(t)) = last.args.first() else { return None };
            Some((last.name.clone(), self.parse_ty(t).ok()?))
        };
        let int_of = |e: &Expr| -> Option<i64> {
            match strip_paren(e) {
                Expr::Lit(l, _) => match lit::lit(l) {
                    Some(Lit::Int(i)) => i.base10_digits().parse().ok(),
                    _ => None,
                },
                _ => None,
            }
        };
        match strip_paren(e) {
            Expr::Unary(op, x, _) if op == "!" => self.type_cond(x).map(|b| !b),
            Expr::Binary(op, l, r, _) => match op.as_str() {
                "||" => match self.type_cond(l) {
                    Some(true) => Some(true),
                    Some(false) => self.type_cond(r),
                    None => None,
                },
                "&&" => match self.type_cond(l) {
                    Some(false) => Some(false),
                    Some(true) => self.type_cond(r),
                    None => None,
                },
                "==" | "!=" => {
                    let c @ Expr::Call(..) = strip_paren(l) else { return None };
                    let (name, t) = targ(c)?;
                    if name != "__exr_int_kind" || self.prog.has_param(t) {
                        return None;
                    }
                    let k = if t.is_int() && t.bits() <= 64 { 1 + t.signed() as i64 } else { 0 };
                    let n = int_of(r)?;
                    Some((k == n) == (op == "=="))
                }
                _ => None,
            },
            c @ Expr::Call(..) => {
                let (name, t) = targ(c)?;
                (name == "needs_drop" && !self.prog.has_param(t)).then(|| self.needs_drop(t))
            }
            _ => None,
        }
    }

    pub(crate) fn if_(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::If(cond, then_branch, else_branch) = e else { unreachable!() };
        // let chains (`if let Some(x) = o && x > 0 { A } else { B }`): nested ifs, B in each failing branch
        // (`if let Some(x) = o { if x > 0 { A } else { B } } else { B }`)
        if let Expr::Binary(op, ..) = &**cond {
            if op == "&&" {
                fn chain<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
                    match e {
                        Expr::Binary(op, l, r, _) if op == "&&" => {
                            chain(l, out);
                            chain(r, out);
                        }
                        e => out.push(e),
                    }
                }
                let mut cs = vec![];
                chain(cond, &mut cs);
                if cs.iter().any(|c| matches!(c, Expr::Let(..))) {
                    let mut body = then_branch.clone();
                    for (i, c) in cs.iter().enumerate().rev() {
                        let nested = Expr::If(Box::new((*c).clone()), body, else_branch.clone());
                        if i == 0 {
                            let at = cs
                                .iter()
                                .map(|c| match c {
                                    Expr::Let(_, x) => expr_start(x),
                                    c => expr_start(c),
                                })
                                .find(|p| p.line != 0)
                                .unwrap_or(SP);
                            let ne: &Expr = self.keep_expr(nested);
                            return self.expr(ne, expected).map_err(|m| self.at(at, m));
                        }
                        body = crate::parse::Block { stmts: vec![Stmt::Expr(nested, false)], spos: Default::default() };
                    }
                }
            }
        }
        if let Expr::Let(..) = &**cond {
            return self.if_let(e, cond, expected);
        }
        // (a condition the types decide: only the side taken)
        match self.type_cond(cond) {
            Some(true) => return self.block(then_branch, expected),
            Some(false) => {
                return match else_branch {
                    Some(e) => self.expr(e, expected),
                    None => Ok((None, Ty::Unit)),
                };
            }
            None => {}
        }
        // (the condition is a scope of its own: its temporaries are dropped before either branch runs)
        let t0 = self.temps.len();
        let bk0 = self.bk.stmt_temps.len();
        let (c, ct) = self.expr(cond, Some(Ty::Bool))?;
        self.same(SP, ct, Ty::Bool)?;
        let Some(c) = c else {
            return Ok((None, Ty::Never));
        };
        self.drop_temps(t0);
        self.bk_temps_end(bk0);
        let then_blk = self.b.create_block();
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        self.b.ins().brif(c, then_blk, &[], else_blk, &[]);

        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;

        self.b.switch_to_block(then_blk);
        let (tv, tt) = self.block(then_branch, expected)?;
        // (`if c { false }` with no `else`, where no value is wanted: the block must be `()`, at its tail as rustc
        // points (E0308); where one is wanted, E0317 below)
        if else_branch.is_none() && expected.is_none_or(|t| t == Ty::Unit) && !matches!(tt, Ty::Unit | Ty::Never) && self.user_code {
            let at = block_end_pos(then_branch);
            if at.line != 0 {
                let t = self.prog.tn(tt).replace('#', "");
                return Err(self.at(at, format!("mismatched types: expected `()`, found `{t}`")));
            }
        }
        self.finish_arm(tv, tt, merge, &mut result_ty, &mut param, SP)?;

        self.b.switch_to_block(else_blk);
        let (ev, et) = match else_branch {
            Some(e) => self.expr(e, result_ty.or(expected))?,
            None => (None, Ty::Unit),
        };
        // (a mismatch is the else branch's, at its tail as rustc points; with no else, at the `if` (E0317))
        let at = else_branch.as_deref().map_or(SP, branch_pos);
        self.finish_arm(ev, et, merge, &mut result_ty, &mut param, at)?;

        self.b.switch_to_block(merge);
        match result_ty {
            None => Ok((None, Ty::Never)),
            Some(t) => Ok((param, t)),
        }
    }

    pub(crate) fn finish_arm(
        &mut self,
        v: Option<Value>,
        t: Ty,
        merge: Block,
        result_ty: &mut Option<Ty>,
        param: &mut Option<Value>,
        span: Sp,
    ) -> R<()> {
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        // (`&&T` where the arms are `&T`: the deref coercion, `Imp::Borrowed(x) => x` with x: &&[u8])
        let (v, t) = match *result_ty {
            Some(rt) if rt != t => self.deref_coerce(v, t, rt),
            _ => (v, t),
        };
        match *result_ty {
            Some(rt) => self.same(span, t, rt)?,
            None => {
                *result_ty = Some(t);
                if let Some(ct) = clif_ty(t) {
                    *param = Some(self.b.append_block_param(merge, ct));
                }
            }
        }
        match v {
            Some(v) => self.b.ins().jump(merge, &[v]),
            None => self.b.ins().jump(merge, &[]),
        };
        Ok(())
    }
}

impl Cx<'_> {
    // --- enum values ------------------------------------------------------

    /// Builds an enum value by writing the tag and each field to a temporary. args is (field index, expression)
    pub(crate) fn variant_value(&mut self, sid: u32, vi: usize, args: &[(usize, &Expr)], expected: Option<Ty>) -> R<V> {
        let sid = self.cow_of_str(sid, args, expected).unwrap_or(sid);
        let r = self.adt_value(sid, Some(vi), args, expected, SP);
        if let Ok((_, t)) = &r {
            self.rg_variant(*t, vi, args);
        }
        r
    }

    /// `Cow::Borrowed(s)` / `Cow::Owned(s)` of a `Cow<str>` (the expected type, or an argument `&str` / `String`):
    /// prelude's `__CowStr`, whose variants are in the same order. Prelude's `Cow<T>` stands for `Cow<[T]>` only
    pub(crate) fn cow_of_str(&self, sid: u32, args: &[(usize, &Expr)], expected: Option<Ty>) -> Option<u32> {
        let cow = *self.prog.struct_index.get("Cow")?;
        if self.prog.base_of(sid) != cow {
            return None;
        }
        let cs = *self.prog.struct_index.get("__CowStr")?;
        match expected {
            Some(Ty::Struct(e)) if e == cs => return Some(cs),
            // (an instance inference settled on; `Cow<T0>` or the definition itself, still open, says nothing)
            Some(t @ Ty::Struct(e)) if self.prog.base_of(e) == cow && e != cow && !self.prog.has_param(t) => return None,
            _ => {}
        }
        let string = self.prog.struct_index.get("String").copied();
        // (the argument's own type first: inference may have unified it with the slice `Cow`'s field)
        let a = args.first()?.1;
        match self.peek_syn(a).or_else(|| self.peek(a))? {
            Ty::Ref(_, Ty::Str) => Some(cs),
            Ty::Struct(s) if Some(s) == string => Some(cs),
            _ => None,
        }
    }

    /// Builds a struct (vi = None) / enum variant (vi = Some) value in a temporary. args is (field index,
    /// expression). A generic definition gets its instance from the expected type, then from the argument types
    pub(crate) fn adt_value(&mut self, base: u32, vi: Option<usize>, args: &[(usize, &Expr)], expected: Option<Ty>, span: Sp) -> R<V> {
        if !self.rg.on && !self.prog.adt_regions.is_empty() {
            self.rg_adt_on(self.prog.base_of(base));
        }
        // (where the return value is written: not passed to calls inside the field expressions)
        let hint = self.dst_hint.take();
        let fields_of = |this: &Self, sid: u32| -> Vec<(String, Ty, u32)> {
            let sd = &this.prog.structs[sid as usize];
            match vi {
                Some(vi) => sd.variants[vi].fields.clone(),
                None => sd.fields.clone(),
            }
        };
        let bdef = &self.prog.structs[base as usize];
        let n_gen = bdef.generics.len();
        let mut inst = if n_gen == 0 {
            Some(base)
        } else {
            match expected {
                // (an unsized instance (`Wrap<[u8]>`) is no value's type: the fields decide, and the value is coerced)
                // (nor the generic definition itself, which inference gives when it found no arguments
                // (`S { f: Box::new(F) }` with `f: Box<dyn X<T>>`), when the fields' types name the parameters; a unit
                // variant's (`Register::<0>::Field1`) has none)
                Some(Ty::Struct(e))
                    if self.prog.base_of(e) == self.prog.base_of(base)
                        && (self.prog.structs[e as usize].base.is_some() || fields_of(self, e).iter().all(|f| !self.prog.has_param(f.1)))
                        && !self.prog.has_param(Ty::Struct(e))
                        && !Ty::Struct(e).is_dst_struct() =>
                {
                    Some(e)
                }
                _ => None,
            }
        };
        let gen_fields = fields_of(self, self.prog.base_of(base));
        let mut sol: Vec<Option<Ty>> = vec![None; n_gen];
        if let Some(ta) = self.adt_targs.take().filter(|t| t.len() == n_gen) {
            sol = ta;
        }
        let mut vals = vec![];
        for &(fi, e) in args {
            let exp = match inst {
                Some(i) => Some(fields_of(self, i)[fi].1),
                None => Some(self.prog.dyn_unmarked(self.prog.partial_subst(gen_fields[fi].1, &sol))).filter(|&t| !self.prog.has_param(t)),
            };
            let (v, vt) = self.expr(e, exp)?;
            if vt == Ty::Never {
                return Ok((None, Ty::Never));
            }
            // (a field's value is a coercion site: a place of a `&mut` where a reference is expected is reborrowed, and
            // the value holds the loan (`S { f: x }` with `f: &i32`, then `*x = 1` while it lives, E0506))
            if let Some(w @ Ty::Ref(..)) = exp {
                self.bk_reborrow_let(e, w);
            }
            if inst.is_none() {
                self.prog.match_ty(gen_fields[fi].1, vt, &mut sol);
            }
            // (a field's value of the wrong type: at the value (the field's name when shorthand), as rustc points)
            vals.push((fi, v, vt, expr_start(e)));
        }
        let inst = match inst.take() {
            Some(i) => i,
            None => match sol.iter().copied().collect::<Option<Vec<Ty>>>() {
                Some(targs) => self.prog.instantiate(self.prog.base_of(base), targs).or_else(|e| err(span, e))?,
                None => {
                    let n = &self.prog.structs[base as usize].name;
                    return err(span, format!("type annotations needed for `{n}<..>`"));
                }
            },
        };
        let t = Ty::Struct(inst);
        let a = match hint {
            Some(d) if t == self.ret => d,
            _ => self.temp_fresh(t),
        };
        let fields = fields_of(self, inst);
        if let Some(vi) = vi {
            let sd = &self.prog.structs[inst as usize];
            let (tag, discr) = (sd.tag, sd.variants[vi].discr);
            let tv = self.b.ins().iconst(clif_ty(tag).unwrap(), discr);
            self.b.ins().store(mf(), tv, a, 0);
        }
        for (fi, v, vt, sp) in vals {
            // (a type whose fields are not known (of a placeholder's associated type): an error, not a panic)
            let Some(&(_, fty, off)) = fields.get(fi) else {
                return err(sp, format!("internal: `{}` has no field {fi}", self.prog.tn(t)));
            };
            // (`V(x)` with x: &&T into a &T field: deref coercion)
            let (v, vt) = self.deref_coerce(v, vt, fty);
            self.same(sp, vt, fty)?;
            self.write(Place::Mem(a, off as i32, fty), v);
        }
        Ok((Some(a), t))
    }

    // --- tuples, arrays, slices ------------------------------------------

    /// If `S<[T; N]>` can coerce to `S<[T]>` / `S<T>` to `S<dyn Tr>` (a struct whose last field is unsized),
    /// the pair's second half (the tail's element count N / T's vtable) and the unsized type
    /// (want may be generic (`&Fat<[T]>` for a parameter): it is matched into sol)
    pub(crate) fn unsize_tail_sol(&self, want: Ty, got: Ty, sol: &mut [Option<Ty>]) -> Option<(TailMeta, Ty)> {
        if !want.is_dst_struct() {
            return None;
        }
        let (m, u) = self.unsized_of(Some(want), got)?;
        let ok = if self.prog.has_param(want) { self.prog.match_ty(want, u, sol) } else { want == u };
        ok.then_some((m, u))
    }

    pub(crate) fn unsize_tail(&self, want: Ty, got: Ty) -> Option<TailMeta> {
        let mut sol = vec![None; 64];
        self.unsize_tail_sol(want, got, &mut sol).map(|x| x.0)
    }

    /// The pair for a value at v unsized by m: (v, N) or (v, the vtable)
    pub(crate) fn make_unsized(&mut self, v: Value, m: TailMeta, span: Sp) -> R<Value> {
        match m {
            TailMeta::Len(n) => Ok(self.make_fat(v, n)),
            TailMeta::Dyn(t, tr) => self.to_dyn(v, t, tr.as_str(), span),
        }
    }

    /// The last field's type of a struct instance (a symbolic one (`Fat<[T]>`) has no laid-out fields: from its
    /// definition's last field)
    pub(crate) fn struct_tail_of(&self, t: Ty) -> Option<Ty> {
        let Ty::Struct(s) = t else { return None };
        let d = &self.prog.structs[s as usize];
        if let Some(f) = d.fields.last() {
            return Some(f.1);
        }
        let b = &self.prog.structs[d.base? as usize];
        match b.fields.last()?.1 {
            Ty::Param(j) => d.args.get(j as usize).copied(),
            t if !self.prog.has_param(t) => Some(t),
            _ => None,
        }
    }

    /// got (`S<[T; N]>`, `S<T>`) unsized as want asks (`[T]` for an array, `dyn Tr` for a sized type, through
    /// nested structs' tails): (what the pair's second word is, the unsized type)
    fn unsized_of(&self, want: Option<Ty>, got: Ty) -> Option<(TailMeta, Ty)> {
        match (want, got) {
            (None | Some(Ty::Slice(_)), Ty::Array(e, n)) => return Some((TailMeta::Len(n as i64), Ty::Slice(e))),
            (Some(Ty::Dyn(tr)), t) if !t.is_unsized() => {
                return Some((TailMeta::Dyn(t, tr), Ty::Dyn(tr)));
            }
            _ => {}
        }
        let Ty::Struct(y) = got else { return None };
        if got.is_dst_struct() || self.prog.structs[y as usize].is_enum {
            return None;
        }
        let d = &self.prog.structs[y as usize];
        let base = d.base?;
        if want.is_some_and(|w| !matches!(w, Ty::Struct(x) if self.prog.base_of(x) == base)) {
            return None;
        }
        let (_, tail, _) = *d.fields.last()?;
        let (m, ut) = self.unsized_of(want.and_then(|w| self.struct_tail_of(w)), tail)?;
        // (the arguments that make the tail ut: from the definition's last field (`Foo<T>` in `Outer<T>`))
        let bl = self.prog.structs[base as usize].fields.last()?.1;
        let mut sol = vec![None; d.args.len()];
        if !self.prog.match_ty(bl, ut, &mut sol) {
            return None;
        }
        let args: Vec<Ty> = d.args.iter().zip(&sol).map(|(&a, s)| s.unwrap_or(a)).collect();
        if args == d.args {
            return None;
        }
        let id = self.prog.instantiate(base, args).ok()?;
        let u = Ty::Struct(id);
        // (only the tail may change: a parameter the tail mentions is not to be used by another field
        // (`struct Foo<T: ?Sized>(Box<T>, T)`, E0308 in rustc))
        let uf = &self.prog.structs[id as usize].fields;
        let k = d.fields.len() - 1;
        let same = uf.len() == d.fields.len() && uf[..k].iter().zip(&d.fields[..k]).all(|(a, b)| a.1 == b.1 && a.2 == b.2);
        (u.is_dst_struct() && same && uf[k].1 == ut).then_some((m, u))
    }

    pub(crate) fn make_fat(&mut self, ptr: Value, len: i64) -> Value {
        let l = self.b.ins().iconst(types::I64, len);
        self.make_fat_v(ptr, l)
    }

    /// Builds a (pointer, length) pair in a temporary and returns its address
    pub(crate) fn make_fat_v(&mut self, ptr: Value, len: Value) -> Value {
        let ss = self.b.create_stack_slot(16, 1 << 3);
        let a = self.b.ins().stack_addr(PTR, ss, 0);
        self.b.ins().store(mf(), ptr, a, 0);
        self.b.ins().store(mf(), len, a, 8);
        a
    }

    pub(crate) fn elem_addr(&mut self, base: Value, i: Value, elem: Ty) -> Value {
        let (size, _) = self.prog.size_align(elem);
        let off = self.b.ins().imul_imm_s(i, size as i64);
        self.b.ins().iadd(base, off)
    }

    /// (result, overflowed) of add (kind 0) / sub (1) / mul (2). 128 bit is built by hand (no Cranelift *_overflow)
    pub(crate) fn arith_ovf(&mut self, kind: u8, s: bool, l: Value, r: Value, t: Ty) -> (Value, Value) {
        if t.bits() < 128 {
            let ins = self.b.ins();
            return match (kind, s) {
                (0, true) => ins.sadd_overflow(l, r),
                (0, false) => ins.uadd_overflow(l, r),
                (1, true) => ins.ssub_overflow(l, r),
                (1, false) => ins.usub_overflow(l, r),
                (_, true) => ins.smul_overflow(l, r),
                (_, false) => ins.umul_overflow(l, r),
            };
        }
        if kind < 2 {
            let v = if kind == 0 { self.b.ins().iadd(l, r) } else { self.b.ins().isub(l, r) };
            let of = if !s {
                if kind == 0 { self.b.ins().icmp(IntCC::UnsignedLessThan, v, l) } else { self.b.ins().icmp(IntCC::UnsignedLessThan, l, r) }
            } else {
                let x = if kind == 0 { self.b.ins().bxor(l, v) } else { self.b.ins().bxor(l, r) };
                let y = if kind == 0 { self.b.ins().bxor(r, v) } else { self.b.ins().bxor(l, v) };
                let m = self.b.ins().band(x, y);
                let z = self.int_const(t, 0);
                self.b.ins().icmp(IntCC::SignedLessThan, m, z)
            };
            return (v, of);
        }
        // Multiplication: checks the product of the absolute values by 64-bit halves
        let zero = self.int_const(t, 0);
        let (ua, ub, neg) = if s {
            let an = self.b.ins().icmp(IntCC::SignedLessThan, l, zero);
            let bn = self.b.ins().icmp(IntCC::SignedLessThan, r, zero);
            let la = self.b.ins().ineg(l);
            let ra = self.b.ins().ineg(r);
            let ua = self.b.ins().select(an, la, l);
            let ub = self.b.ins().select(bn, ra, r);
            (ua, ub, Some(self.b.ins().bxor(an, bn)))
        } else {
            (l, r, None)
        };
        let (al, ah) = self.b.ins().isplit(ua);
        let (bl, bh) = self.b.ins().isplit(ub);
        let ah_nz = self.b.ins().icmp_imm_u(IntCC::NotEqual, ah, 0);
        let bh_nz = self.b.ins().icmp_imm_u(IntCC::NotEqual, bh, 0);
        let both = self.b.ins().band(ah_nz, bh_nz);
        let (c1, o1) = self.b.ins().umul_overflow(ah, bl);
        let (c2, o2) = self.b.ins().umul_overflow(al, bh);
        let (cross, o3) = self.b.ins().uadd_overflow(c1, c2);
        let hi = self.b.ins().umulhi(al, bl);
        let (_, o4) = self.b.ins().uadd_overflow(cross, hi);
        let mut of = self.b.ins().bor(both, o1);
        of = self.b.ins().bor(of, o2);
        of = self.b.ins().bor(of, o3);
        of = self.b.ins().bor(of, o4);
        let mag = self.b.ins().imul(ua, ub);
        let v = self.b.ins().imul(l, r);
        if let Some(neg) = neg {
            // |product| goes up to 2^127 if negative, 2^127 - 1 if positive
            let max = self.int_const(t, i128::MAX);
            let over_pos = self.b.ins().icmp(IntCC::UnsignedGreaterThan, mag, max);
            let one = self.int_const(t, 1);
            let max1 = self.b.ins().iadd(max, one);
            let over_neg = self.b.ins().icmp(IntCC::UnsignedGreaterThan, mag, max1);
            let over = self.b.ins().select(neg, over_neg, over_pos);
            of = self.b.ins().bor(of, over);
        }
        (v, of)
    }

    /// No inference record (a codegen-rebuilt node): the syntax types an expression of variables, fields, deref, parens
    pub(crate) fn peek_or_static(&self, e: &Expr) -> Option<Ty> {
        self.peek(e).or_else(|| self.static_ty(e))
    }

    pub(crate) fn static_ty(&self, e: &Expr) -> Option<Ty> {
        match e {
            Expr::Paren(p) => self.static_ty(p),
            Expr::Path(_, p) => self.lookup(ident_of(p)?).map(|(_, t)| t),
            Expr::Unary(op, x, _) if op == "*" => match self.static_ty(x)? {
                Ty::Ref(_, i) | Ty::Box(i) | Ty::Ptr(_, i) => Some(*i),
                _ => None,
            },
            Expr::Ref(m, x) => self.static_ty(x).map(|t| Ty::Ref(*m, intern(t))),
            Expr::Field(base, name, _) => {
                let Ty::Struct(sid) = self.strip_refs(self.static_ty(base)?) else { return None };
                if let Some((t, _)) = self.prog.field(sid, name) {
                    return Some(t);
                }
                // (a field behind Deref)
                let d = self.prog.method(sid, "deref")?;
                let Ty::Ref(_, Ty::Struct(ts)) = self.prog.sigs[d as usize].ret else { return None };
                self.prog.field(*ts, name).map(|(t, _)| t)
            }
            _ => None,
        }
    }

    /// If e is a value of a struct implementing Deref (not Vec, Box or String), `*e.deref_mut()` (else `*e.deref()`)
    pub(crate) fn user_deref(&self, e: &Expr) -> Option<Expr> {
        let Ty::Struct(sid) = self.peek(e)? else { return None };
        self.user_deref_sid(e, sid)
    }

    pub(crate) fn user_deref_sid(&self, e: &Expr, sid: u32) -> Option<Expr> {
        if self.vec_fields(sid).is_some() {
            return None;
        }
        // (the prelude's `Pin<&T>`: its `deref_mut` would borrow the pin itself mutably, where std's `Pin` has no
        // `DerefMut` for a shared pointer (`s.0.borrow_mut()` of `s: Pin<&S>` needs no `mut s`))
        let base = self.prog.base_of(sid);
        let shared_pin = self.prog.structs[base as usize].name == "Pin"
            && self.prog.struct_item.get(base as usize).is_none_or(|&i| i >= self.prog.prelude_from)
            && matches!(self.prog.structs[sid as usize].args.first(), Some(Ty::Ref(false, _)));
        // (a place read or borrowed shared is reached by `deref`, one written or borrowed `&mut` by `deref_mut`, as rustc
        // does: a Deref with effects counts them (`let _ = p.x;` calls `deref` once))
        let shared = shared_pin || !self.bk.place_mut && self.prog.method(sid, "deref").is_some();
        let m = if !shared && self.prog.method(sid, "deref_mut").is_some() { "deref_mut" } else { "deref" };
        let did = self.prog.method(sid, m)?;
        if !matches!(self.prog.sigs[did as usize].ret, Ty::Ref(_, i) if !i.is_fat() && !matches!(i, Ty::Str | Ty::Slice(_) | Ty::Dyn(_))) {
            return None;
        }
        Some(Expr::Unary("*".into(), Box::new(mcall(paren(e), m, vec![])), crate::parse::Span::default()))
    }

    /// (start address, length, element type) of an indexed target (array / slice / a reference to one)
    pub(crate) fn indexable(&mut self, e: &Expr) -> R<(Value, Value, Ty)> {
        let own = std::mem::take(&mut self.index_owns);
        // (`g[i]` (g: MutexGuard<Vec<T>> and so on): `(*g)[i]`)
        if let Some(d) = self.user_deref(e) {
            let d = self.keep_expr(d);
            // (a read through `deref_mut` (a RefMut's) borrows the guard shared, as `*g` does: `let g = ..; g[0]`)
            self.deref_shared(d);
            return self.indexable(d);
        }
        // (-O: what was read before the loop)
        if !self.hoisted.is_empty() {
            // (the key is the expression's Debug string, in place of the syn version's token string)
            let key = format!("{e:?}");
            if let Some(&(_, b, l, t, chk)) = self.hoisted.iter().rev().find(|h| h.0 == key) {
                if let Some((ok, len, i)) = chk {
                    let sp = match e {
                        Expr::Index(_, _, isp) => isp.at,
                        _ => expr_start(e),
                    };
                    self.index_check(ok, len, i, sp);
                }
                return Ok((b, l, t));
            }
        }
        // (`c[0]` of a struct whose `Deref` is to a slice / str (`type Target = [T]`): `c.deref()[0]`, `deref_mut`
        // for a `&mut c[0]`)
        let mut pt = self.peek(e);
        while let Some(Ty::Ref(_, i)) = pt {
            pt = Some(*i);
        }
        if let Some(Ty::Struct(sid)) = pt {
            if self.vec_fields(sid).is_none() {
                let m = if self.bk.place_mut && self.prog.method(sid, "deref_mut").is_some() { "deref_mut" } else { "deref" };
                if let Some(did) = self.prog.method(sid, m) {
                    if matches!(self.prog.sigs[did as usize].ret, Ty::Ref(_, Ty::Slice(_))) {
                        let d = self.keep_expr(mcall(paren(e), m, vec![]));
                        return self.indexable(d);
                    }
                }
            }
        }
        let is_const = matches!(e, Expr::Path(..)) && self.const_of(e).is_some();
        let mut p = if is_place_expr(e) && !is_const {
            self.place(e)?
        } else {
            // (a new value an index expression indexes (`[A(1)][0]`, `f()[0]`, `Box::new([..])[0]`, `vec![..][1]`) is a
            // temporary of the statement: dropped with them. Not for the other callers (a `for` loop owns its array, or
            // moves its elements out))
            let fresh = own
                && matches!(strip_paren(e), Expr::Array(_) | Expr::Repeat(..) | Expr::Call(..) | Expr::MethodCall(..) | Expr::Macro(..) | Expr::Struct(..) | Expr::Tuple(_));
            match self.expr(e, None)? {
                (Some(a), t) if t.is_agg() => {
                    if fresh {
                        self.own_temp(a, t);
                    }
                    Place::Mem(a, 0, t)
                }
                (Some(v), t @ Ty::Box(i)) => {
                    if fresh {
                        self.own_temp(v, t);
                    }
                    Place::Mem(v, 0, *i)
                }
                (Some(v), Ty::Ref(_, i)) => Place::Mem(v, 0, *i),
                (_, t) => {
                    return err(SP, format!("cannot index into a value of type `{}`", self.prog.tn(t)));
                }
            }
        };
        loop {
            p = match p {
                Place::Mem(a, o, Ty::Array(t, n)) => {
                    let base = self.addr_plus(a, o);
                    let len = self.b.ins().iconst(types::I64, n as i64);
                    return Ok((base, len, *t));
                }
                // (`&[T]` / `Box<[T]>` in memory, or a slice place: the (pointer, length) pair)
                Place::Mem(a, o, Ty::Ref(_, Ty::Slice(t)) | Ty::Box(Ty::Slice(t))) => {
                    let base = self.b.ins().load(PTR, mf(), a, o);
                    let len = self.b.ins().load(types::I64, mf(), a, o + 8);
                    return Ok((base, len, **t));
                }
                Place::Mem(a, o, Ty::Slice(t)) => {
                    let base = self.b.ins().load(PTR, mf(), a, o);
                    let len = self.b.ins().load(types::I64, mf(), a, o + 8);
                    return Ok((base, len, *t));
                }
                // Vec<T>: the (ptr, len) fields
                Place::Mem(a, o, Ty::Struct(sid)) if self.vec_fields(sid).is_some() => {
                    let (po, lo, t) = self.vec_fields(sid).unwrap();
                    let base = self.b.ins().load(PTR, mf(), a, o + po as i32);
                    let len = self.b.ins().load(types::I64, mf(), a, o + lo as i32);
                    return Ok((base, len, t));
                }
                Place::Mem(a, o, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.ins().load(PTR, mf(), a, o);
                    Place::Mem(ptr, 0, *i)
                }
                Place::Var(v, Ty::Ref(_, i) | Ty::Box(i)) => {
                    let ptr = self.b.use_var(v);
                    Place::Mem(ptr, 0, *i)
                }
                p => {
                    return err(SP, format!("cannot index into a value of type `{}`", self.prog.tn(p.ty())));
                }
            }
        }
    }

    /// `&a[lo..hi]` / `&a[..]` / `&a[lo..=hi]` (rg is an Expr::Range)
    pub(crate) fn slice_range(&mut self, base: &Expr, rg: &Expr, m: bool) -> R<V> {
        let Expr::Range(start, end, inclusive) = rg else { unreachable!() };
        // `&s[a..b]` (String / &str): cuts at byte positions, panics if not on a char boundary
        let bt = self.peek(base).map(|t| self.strip_refs(t));
        let is_string = matches!(bt, Some(Ty::Struct(sid))
            if self.prog.structs[sid as usize].base.is_none() && self.prog.structs[sid as usize].name == "String");
        let str_fat = if is_string {
            let ne = mcall(paren(base), "as_str", vec![]);
            let ne = self.keep_expr(ne);
            Some(self.expr(ne, None)?)
        } else if matches!(self.peek(base), Some(Ty::Ref(_, Ty::Str))) {
            Some(self.expr(base, None)?)
        } else {
            None
        };
        if let Some((fv, _)) = str_fat {
            let Some(fat) = fv else { return Ok((None, Ty::Never)) };
            let ptr = self.b.ins().load(PTR, mf(), fat, 0);
            let len = self.b.ins().load(types::I64, mf(), fat, 8);
            let (lo, hi) = self.range_bounds(rg, len)?;
            let c1 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, lo, hi);
            self.panic_unless(c1);
            let c2 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, hi, len);
            self.panic_unless(c2);
            for idx in [lo, hi] {
                let ok = self.char_boundary(ptr, len, idx);
                self.panic_unless(ok);
            }
            let start = self.b.ins().iadd(ptr, lo);
            let n = self.b.ins().isub(hi, lo);
            let fat = self.make_fat_v(start, n);
            return Ok((Some(fat), Ty::Ref(false, intern(Ty::Str))));
        }
        let (ptr, len, elem) = self.indexable(base)?;
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
        let c1 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, lo, hi);
        self.panic_unless(c1);
        let c2 = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, hi, len);
        self.panic_unless(c2);
        let start = self.elem_addr(ptr, lo, elem);
        let n = self.b.ins().isub(hi, lo);
        let fat = self.make_fat_v(start, n);
        Ok((Some(fat), Ty::Ref(m, intern(Ty::Slice(intern(elem))))))
    }

    pub(crate) fn tuple_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Tuple(elems) = e else { unreachable!() };
        let base = self.prog.tuple_base(elems.len());
        let args: Vec<(usize, &Expr)> = elems.iter().enumerate().collect();
        self.adt_value(base, None, &args, expected, SP)
    }

    pub(crate) fn array_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Array(elems) = e else { unreachable!() };
        let mut elem = match expected {
            Some(Ty::Array(t, _)) | Some(Ty::Slice(t)) => Some(*t),
            // (`v == []` / `v == [1, 2]`: the element type of the Vec it is compared with)
            Some(Ty::Struct(sid)) if self.prog.structs[self.prog.base_of(sid) as usize].name == "Vec" => {
                self.prog.structs[sid as usize].args.first().copied()
            }
            _ => None,
        };
        // (elements that are distinct closures with no captures and one signature: their fn pointer, as rustc
        // coerces them; also over the first element's closure type inference may have given)
        let is_clo = |t: Option<Ty>| matches!(t, Some(Ty::Struct(s)) if self.prog.closure_of_struct.contains_key(&self.prog.base_of(s)));
        // (also over the first element's `&String`, which inference may have given)
        let string_ref = |t: Option<Ty>| matches!(t, Some(Ty::Ref(_, Ty::Struct(s))) if self.prog.struct_index.get("String") == Some(&self.prog.base_of(*s)));
        if elem.is_none() || string_ref(elem) {
            if let Some(t) = self.str_refs_lub(&elems.iter().collect::<Vec<_>>()) {
                elem = Some(t);
            }
        }
        if elem.is_none() || is_clo(elem) {
            if let Some(f) = self.closures_lub(&elems.iter().collect::<Vec<_>>()) {
                self.lub_fnptr += 1;
                let r = self.array_lit(e, Some(Ty::Array(crate::types::intern(f), elems.len() as u32)));
                self.lub_fnptr -= 1;
                return r;
            }
        }
        // (a Vec's element type is only a hint: if the elements differ, theirs wins (`v == ["a"]` with v: Vec<String>))
        let mut soft = matches!(expected, Some(Ty::Struct(_)));
        // `[&a as &dyn Tr, &b]`: if any element is `&dyn`, all of them become that
        if !matches!(elem, Some(Ty::Ref(_, Ty::Dyn(_)))) {
            if let Some(d) = elems.iter().find_map(|e| match e {
                Expr::Cast(_, ty) => match self.parse_ty(ty) {
                    Ok(t @ Ty::Ref(_, Ty::Dyn(_))) => Some(t),
                    _ => None,
                },
                _ => None,
            }) {
                elem = Some(d);
            }
        }
        let mut vals = vec![];
        // (an element type the context gives: each element is coerced to it by deref, as at a coercion site
        // (`[&x]` with x: `&[u8]` where `[&[u8]; 1]` is expected))
        let given = !soft && matches!(elem, Some(Ty::Ref(..)));
        for e in elems {
            let (v, t) = self.expr(e, elem)?;
            if t == Ty::Never {
                return Ok((None, Ty::Never));
            }
            // (a coercion site, as a struct literal's field: a place of a `&mut` is reborrowed, and the array holds the
            // loan (`[rr]` of rr: `&mut &mut i32` as a `[&mut i32; 1]`, then `**rr = 1` while it lives, E0506))
            if let (true, Some(w @ Ty::Ref(..))) = (given, elem) {
                self.bk_reborrow_let(e, w);
            }
            let (v, t) = match elem {
                Some(et) if given && t != et && matches!(t, Ty::Ref(_, Ty::Ref(..))) => self.deref_coerce(v, t, et),
                _ => (v, t),
            };
            match elem {
                Some(et) if soft && t != et => elem = Some(t),
                Some(et) => self.same(SP, t, et)?,
                None => elem = Some(t),
            }
            soft = false;
            vals.push(v);
        }
        let Some(elem) = elem else {
            return err(SP, "type annotations needed for an empty array");
        };
        let t = Ty::Array(intern(elem), vals.len() as u32);
        let base = self.temp(t);
        let (size, _) = self.prog.size_align(elem);
        for (i, v) in vals.into_iter().enumerate() {
            self.write(Place::Mem(base, (i as u32 * size) as i32, elem), v);
        }
        Ok((Some(base), t))
    }

    /// An array's length (a literal or a const)
    pub(crate) fn array_len(&self, e: &Expr) -> R<u32> {
        match e {
            Expr::Lit(l, _) if matches!(lit::lit(l), Some(Lit::Int(_))) => {
                let Some(Lit::Int(i)) = lit::lit(l) else { unreachable!() };
                i.base10_digits().parse::<u32>().or_else(|er| err(SP, er))
            }
            Expr::Path(..) if self.const_generic(e).is_some() => Ok(self.const_generic(e).unwrap()),
            // (E0435: a local where a constant goes, `let n = 3; [0; n]`, at its name)
            Expr::Path(None, p) if self.user_code && self.const_of(e).is_none() && ident_of(p).is_some_and(|n| self.lookup(n).is_some()) => {
                Err(self.at(p.pos, "attempt to use a non-constant value in a constant (E0435)".to_string()))
            }
            Expr::Path(_, p) if ident_of(p).is_some() || self.const_of(e).is_some() => {
                // (an associated constant `S::N` is looked up by its const name `S::N`)
                let name = match self.const_of(e) {
                    Some(ci) => self.prog.consts[ci as usize].name.clone(),
                    None => ident_of(p).unwrap().to_string(),
                };
                let ts = crate::types::TySyn::Array(Box::new(crate::types::TySyn::Prim(Ty::U8)), crate::types::ArrLen::Const(name));
                match self.prog.resolve(&ts) {
                    Ok(Ty::Array(_, n)) => Ok(n),
                    Ok(_) => unreachable!(),
                    Err(er) => err(SP, er),
                }
            }
            e => match crate::ast_summary::const_int(e) {
                Ok(v) if (0..=u32::MAX as i64).contains(&v) => Ok(v as u32),
                Err(_) => {
                    // (an expression over other consts: Program evaluates its text)
                    let ts = crate::ast_types::tysyn(&crate::parse::Ty::Array(Box::new(crate::parse::Ty::Path(None, path_of("u8"))), Box::new(e.clone())), None)
                        .or_else(|er| err(SP, self.item_line_err(er)))?;
                    match self.prog.resolve(&ts) {
                        Ok(Ty::Array(_, n)) => Ok(n),
                        Ok(_) => err(SP, "array length must be a literal or a const"),
                        Err(er) => err(SP, er),
                    }
                }
                _ => err(SP, "array length must be a literal or a const"),
            },
        }
    }

    pub(crate) fn repeat_lit(&mut self, e: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Repeat(x, len) = e else { unreachable!() };
        // (`[0; _]`: the length the expected array type has)
        let infer = matches!(strip_paren(len), Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "_");
        let n = match (infer, expected) {
            (true, Some(Ty::Array(_, n))) if n < crate::types::CONST_LEN_BASE => n,
            _ => self.array_len(len)?,
        };
        // (the element's type: of an expected array, or of a slice, as `&[0; N]` where `&[u8]` is expected, behind a
        // reference that is not promoted to static data (a length that is not a literal))
        let exp = match expected {
            Some(Ty::Array(t, _) | Ty::Slice(t)) => Some(*t),
            _ => None,
        };
        let (v, elem) = self.expr(x, exp)?;
        if elem == Ty::Never {
            return Ok((None, Ty::Never));
        }
        // (coerced to the expected element type by deref, as array_lit's elements, and reborrowed as they are)
        if let Some(w @ Ty::Ref(..)) = exp {
            self.bk_reborrow_let(x, w);
        }
        let (v, elem) = match exp {
            Some(et @ Ty::Ref(..)) if elem != et && matches!(elem, Ty::Ref(_, Ty::Ref(..))) => self.deref_coerce(v, elem, et),
            _ => (v, elem),
        };
        // (E0277: two or more copies of a value need it to be Copy; a constant (`[C; 4]`, `[const { .. }; 4]`) is
        // evaluated for each)
        let constant = match strip_paren(x) {
            // (the resolver's: `Self::C`, `m::C`, a local of the name shadows it)
            e @ Expr::Path(None, p) => p.segs.len() >= 1 && (p.segs.len() >= 2 || self.lookup(&p.segs[0].name).is_none()) && self.const_of(e).is_some(),
            Expr::Const(_) => true,
            _ => false,
        };
        // (Copy as a trait: `&mut T` is not, though the borrow check's is_copy counts it, as it is reborrowed)
        let is_copy = |p: &Program, t: Ty| super::borrowck::is_copy(p, t, 0) && !copy_mut_ref(p, t, 0);
        if n >= 2 && self.user_code && !constant && !is_copy(self.prog, elem) {
            // (the type that is not Copy: an argument of a Copy type, as rustc names it (`Option<Bar>`: `Bar`,
            // `Option<&mut i32>`: `&mut i32`))
            let mut culprit = elem;
            while let Ty::Struct(sid) = culprit {
                let p = self.prog;
                let derived_copy = p.impls.contains(&(p.base_of(sid), "Copy".to_string()));
                match p.structs[sid as usize].args.iter().find(|&&a| !is_copy(p, a)) {
                    Some(&a) if derived_copy => culprit = a,
                    _ => break,
                }
            }
            return err(expr_start(x), format!("the trait bound `{}: Copy` is not satisfied (E0277)", self.prog.tn(culprit)));
        }
        let t = Ty::Array(intern(elem), n);
        let base = self.temp(t);
        let (size, _) = self.prog.size_align(elem);
        // (an array of a constant integer (`[0u8; 24]`): written 8 bytes at a time, the values side by side)
        // (small arrays per element: accessed at the element width, so the slot's elements can become variables (SROA))
        // (large ones (over 256 bytes) loop: `[0u8; 4_000_000_000]` does not become a sequence of writes)
        if let (Some(c), true) = (v.and_then(|v| self.iconst_of(v)), elem.is_int() && size <= 4 && size * n >= 8 && n > 16 && (size as u64) * (n as u64) <= 256) {
            let mask = if size == 8 { u64::MAX } else { (1u64 << (size * 8)) - 1 };
            let mut pat = 0u64;
            for k in 0..(8 / size) {
                pat |= ((c as u64) & mask) << (k * size * 8);
            }
            let total = size * n;
            let mut fl = MemFlagsData::new();
            fl.set_notrap();
            let mut off = 0;
            for (w, ty) in [(8u32, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
                while off + w <= total {
                    let pv = self.b.ins().iconst(ty, (pat & if w == 8 { u64::MAX } else { (1u64 << (w * 8)) - 1 }) as i64);
                    self.b.ins().store(fl, pv, base, off as i32);
                    off += w;
                }
            }
            return Ok((Some(base), t));
        }
        if n <= 16 {
            for i in 0..n {
                self.write(Place::Mem(base, (i * size) as i32, elem), v);
            }
        } else {
            // A large array is filled by a loop
            let i = self.b.declare_var(types::I64);
            let z = self.b.ins().iconst(types::I64, 0);
            self.b.def_var(i, z);
            let head = self.b.create_block();
            let body = self.b.create_block();
            let exit = self.b.create_block();
            self.b.ins().jump(head, &[]);
            self.b.switch_to_block(head);
            let iv = self.b.use_var(i);
            let c = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, iv, n as i64);
            self.b.ins().brif(c, body, &[], exit, &[]);
            self.b.switch_to_block(body);
            let addr = self.elem_addr(base, iv, elem);
            self.write(Place::Mem(addr, 0, elem), v);
            let nx = self.b.ins().iadd_imm_s(iv, 1);
            self.b.def_var(i, nx);
            self.b.ins().jump(head, &[]);
            self.b.switch_to_block(exit);
        }
        Ok((Some(base), t))
    }
}

/// (an instance whose Copy needs its arguments' (`Option<&mut T>`, a derive of the program's), not
/// `PhantomData<&mut T>`)
fn copy_needs_args(p: &Program, sid: u32) -> bool {
    let b = p.base_of(sid);
    let own = p.struct_item.get(b as usize).is_some_and(|&ii| ii < p.prelude_from);
    p.impls.contains(&(b, "Copy".to_string())) && if own { p.derived.get(&b).is_some_and(|d| d.iter().any(|x| x == "Copy")) } else { p.clone_needs_args(b, false) }
}
/// t holds a `&mut` that makes it not Copy as a trait (borrowck's is_copy counts `&mut T`, which is reborrowed):
/// `&mut T`, an array or tuple of one, an instance whose Copy needs its arguments' (`Option<&mut T>`)
pub(crate) fn copy_mut_ref(p: &Program, t: Ty, depth: u32) -> bool {
    match t {
        Ty::Ref(true, _) => true,
        Ty::Array(e, _) => depth < 16 && copy_mut_ref(p, *e, depth + 1),
        Ty::Struct(sid) if p.is_tuple(sid) => depth < 16 && p.structs[sid as usize].fields.iter().any(|f| copy_mut_ref(p, f.1, depth + 1)),
        Ty::Struct(sid) => depth < 16 && copy_needs_args(p, sid) && p.structs[sid as usize].args.iter().any(|&a| copy_mut_ref(p, a, depth + 1)),
        _ => false,
    }
}
