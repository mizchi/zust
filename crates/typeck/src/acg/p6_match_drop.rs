use super::*;
use crate::parse::{Block as PBlock, Expr, Pat, Path};
use crate::types::{Ty, intern};
use crate::acg::clif::{AbiParam, Signature, TrapCode, types};
use crate::ir::{Block, IntCC, StackSlot, Value};
use lit::Lit;

/// 式がこの種類のリテラルか
fn lit_kind<F: Fn(&Lit) -> bool>(e: &Expr, f: F) -> bool {
    match e {
        Expr::Lit(t, _) => match lit::lit(t) {
            Some(l) => f(&l),
            None => false,
        },
        _ => false,
    }
}

/// ループのラベル (syn の `l.name.ident` と同じく `'` を除く)
fn label_name(l: &Option<String>) -> Option<String> {
    l.as_ref().map(|s| s.trim_start_matches('\'').to_string())
}

impl Cx<'_> {
    // --- match / if let / while let --------------------------------------

    /// match 対象を place にする。値なら一時変数に置く。None は発散
    pub(crate) fn scrutinee(&mut self, e: &Expr) -> R<Option<Place>> {
        self.scrutinee_as(e, None)
    }

    /// `let (a, b): (Vec<i32>, Vec<i32>) = ..`: 型注釈を期待型として渡す
    pub(crate) fn scrutinee_as(&mut self, e: &Expr, expected: Option<Ty>) -> R<Option<Place>> {
        self.last_scrut = None;
        // `v[..]` / `a[1..]` (サイズ不定の [T] の場所): fat pointer の置き場所を `[T]` 型の place として渡す
        // (スライスパターンだけが受け付け、要素は値で束縛する)
        if let Expr::Index(_, ix, _) = strip_paren(e) {
            if matches!(&**ix, Expr::Range(..)) {
                let r = self.keep_expr(Expr::Ref(false, Box::new(e.clone())));
                let (v, t) = self.expr(r, None)?;
                let (Some(v), Ty::Ref(_, Ty::Slice(el))) = (v, t) else {
                    return err(SP, "unsupported range scrutinee");
                };
                return Ok(Some(Place::Mem(v, 0, Ty::Slice(el))));
            }
        }
        let bare_none = expected.is_some() && matches!(strip_paren(e), Expr::Path(None, pa) if pa.segs.len() == 1 && pa.segs[0].name == "None");
        if is_place_expr(e) && !bare_none {
            let p = self.place(e)?;
            // (the annotation is the place's type (`let ref s: String = n;` with n an i32 is E0308); not a pointer's,
            // to which a coercion may apply)
            let coercible = |t: Ty| matches!(t, Ty::Ref(..) | Ty::Ptr(..) | Ty::Box(_) | Ty::Fn(_) | Ty::Dyn(_) | Ty::Never);
            if let Some(t) = expected.filter(|&t| t != p.ty() && !coercible(t) && !coercible(p.ty()) && !super::has_hole(self.prog, t)) {
                self.same(err_pos(e), p.ty(), t)?;
            }
            return Ok(Some(p));
        }
        let (v, t) = self.expr(e, expected)?;
        // 一時値の match 対象は文の終わりまで所有する (パターンがムーブしたらフラグを下ろす)
        if let Some(v) = v {
            if t != Ty::Never {
                self.last_scrut = self.own_temp(v, t);
            }
        }
        Ok(match (v, t) {
            (_, Ty::Never) => None,
            // (a fat reference's value is the address of its two words, which can be another variable's (`&*x`): a
            // pattern that binds it by reference (`let ref mut y = &*x;`) must not reach that variable)
            (Some(a), t @ Ty::Ref(..)) if t.is_fat() => {
                let tmp = self.temp(t);
                self.copy(tmp, a, t);
                Some(Place::Mem(tmp, 0, t))
            }
            (Some(a), t) if t.is_agg() => Some(Place::Mem(a, 0, t)),
            (Some(v), t) => {
                let var = self.b.declare_var(clif_ty(t).unwrap());
                self.b.def_var(var, v);
                Some(Place::Var(var, t))
            }
            (None, t) => Some(Place::Zst(t)),
        })
    }

    /// e is a capture the closure holds by value, of a type that needs drop, and a pattern binds part of it by value
    pub(crate) fn takes_capture(&self, e: &Expr, pats: &[&Pat]) -> bool {
        let Expr::Path(None, p) = strip_paren(e) else { return false };
        let Some(id) = ident_of(p) else { return false };
        self.caps.iter().any(|c| c.0 == id && !c.2)
            && self.lookup(id).is_some_and(|(l, t)| self.is_value_cap(id, l) && self.needs_drop(t))
            && pats.iter().any(|p| crate::ast_closure::binds_by_value(p))
    }

    /// `match None { Some(3u8) => .. }` / `if let Some(x) = None`: the Option type, its payload from a `Some(..)`
    /// pattern; with none, `()` (rustc's `!` fallback for `Some(v) => v` in a diverging context)
    pub(crate) fn bare_none_ty(&mut self, scrut: &Expr, pats: &[&Pat]) -> R<Option<Ty>> {
        if !matches!(strip_paren(scrut), Expr::Path(None, pa) if pa.segs.len() == 1 && pa.segs[0].name == "None" && pa.segs[0].args.is_empty()) {
            return Ok(None);
        }
        // (a local or const named `None` is not the variant)
        if self.lookup("None").is_some() {
            return Ok(None);
        }
        // (the type inference saw the uses: `if let Some(x) = None { match x { E::V => .. } }`)
        if let Some(&t) = self.lets.get(&(strip_paren(scrut) as *const Expr as usize)).or_else(|| self.lets.get(&(scrut as *const Expr as usize))) {
            if matches!(t, Ty::Struct(_)) && !self.prog.has_param(t) {
                return Ok(Some(t));
            }
        }
        let inner = pats.iter().find_map(|p| match p {
            Pat::TupleStruct(sp, a) if a.len() == 1 && sp.segs.len() == 1 && sp.segs[0].name == "Some" => self.payload_pat_ty(&a[0]),
            _ => None,
        });
        let ob = *self.prog.struct_index.get("Option").ok_or("Option is not available")?;
        Ok(Some(Ty::Struct(self.prog.instantiate(ob, vec![inner.unwrap_or(Ty::Unit)])?)))
    }

    /// the type a pattern pins down by itself (a literal, a const, a variant of a non-generic enum)
    fn payload_pat_ty(&mut self, p: &Pat) -> Option<Ty> {
        match p {
            Pat::Lit(e) => self.peek(e),
            Pat::Path(pa) | Pat::TupleStruct(pa, _) | Pat::Struct(pa, _, _) if pa.segs.len() == 2 => {
                let &sid = self.prog.struct_index.get(pa.segs[0].name.as_str())?;
                self.prog.structs[sid as usize].generics.is_empty().then_some(Ty::Struct(sid))
            }
            p => {
                let n = match p {
                    Pat::Path(pa) if pa.segs.len() == 1 => pa.segs[0].name.as_str(),
                    p => const_ident_pat(self.prog, p)?,
                };
                self.prog.const_index.get(n).map(|&ci| self.prog.consts[ci as usize].ty)
            }
        }
    }

    /// the type a pattern of number literals / constants pins down (a tuple of them: each element's)
    fn pat_ty(&mut self, p: &Pat) -> Option<Ty> {
        match p {
            Pat::Lit(e) if lit_suffix(e) == Some(true) => self.peek(e),
            Pat::Path(pa) if pa.segs.len() == 2 => match Ty::from_suffix(&pa.segs[0].name) {
                Some(t) if crate::program::prim_assoc_const(t, &pa.segs[1].name).is_some() => Some(t),
                _ => self.const_of(&Expr::Path(None, pa.clone())).map(|ci| self.prog.consts[ci as usize].ty),
            },
            Pat::Path(pa) if pa.segs.len() == 1 => {
                let n = pa.segs[0].name.as_str();
                self.prog.const_index.get(n).map(|&ci| self.prog.consts[ci as usize].ty)
            }
            Pat::Range(Some(a), _, _) => self.pat_ty(&range_end_pat(a)?),
            Pat::Tuple(ps) if !ps.is_empty() => {
                let ts = ps.iter().map(|p| self.pat_ty(p)).collect::<Option<Vec<Ty>>>()?;
                let base = self.prog.tuple_base(ts.len());
                Some(Ty::Struct(self.prog.instantiate(base, ts).ok()?))
            }
            _ => None,
        }
    }

    pub(crate) fn match_(&mut self, m: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::Match(scrut, arms) = m else {
            return err(SP, "internal: match_ on a non-match expression");
        };
        // (an arm with a guard and an or-pattern: the guard runs for each alternative that matches, left to right.
        // The arm becomes one arm per alternative)
        if arms.iter().any(|a| a.guard.is_some() && has_or(&a.pat)) {
            let mut out = vec![];
            let mut changed = false;
            for a in arms {
                let alts = if a.guard.is_some() { or_alts(&a.pat) } else { vec![] };
                if alts.len() < 2 || alts.len() > 32 {
                    out.push(a.clone());
                    continue;
                }
                changed = true;
                for p in alts {
                    out.push(crate::parse::Arm { pat: p, pos: a.pos, guard: a.guard.clone(), body: a.body.clone() });
                }
            }
            if changed {
                let m2 = self.keep_expr(Expr::Match(scrut.clone(), out));
                // (the region check follows the arms that were lowered)
                if self.rg_on() {
                    self.rg.alias.insert(m as *const Expr as usize, m2);
                }
                return self.match_(m2, expected);
            }
        }
        let pats: Vec<&Pat> = arms.iter().map(|a| &a.pat).collect();
        // (`match cap { V(s) => .. }` in a closure that holds `cap` by value: move it to a local first
        // (`let t = cap;` empties the closure's slot), then match the local; in place, both would drop `s`)
        if self.takes_capture(scrut, &pats) {
            let tk = format!("__exr_tk{}", self.temps.len());
            let blk = crate::parse::Block {
                stmts: vec![
                    crate::parse::Stmt::Let(Pat::Ident(false, false, tk.clone(), None), None, Some((**scrut).clone()), None, Default::default()),
                    crate::parse::Stmt::Expr(Expr::Match(Box::new(path_expr(&tk)), arms.clone()), false),
                ],
            };
            let ne: &Expr = self.keep_expr(Expr::Block(None, false, blk));
            // (the move into the local is the rewrite's, not the program's: `match cap { None => .., Some(x) => cap =
            // Some(x) }` leaves cap whole on every path. The borrow checker does not look at this match)
            let on = std::mem::replace(&mut self.bk.on, false);
            let r = self.expr(ne, expected);
            self.bk.on = on;
            return r;
        }
        // (`match [] { FOO => .. }`: an empty array scrutinee takes its type from a const pattern)
        let scrut_exp = match strip_paren(scrut) {
            Expr::Array(xs) if xs.is_empty() => pats.iter().find_map(|p| {
                let n = match p {
                    Pat::Path(pa) if pa.segs.len() == 1 => pa.segs[0].name.as_str(),
                    p => const_ident_pat(self.prog, p)?,
                };
                self.prog.const_index.get(n).map(|&ci| self.prog.consts[ci as usize].ty).filter(|t| matches!(t, Ty::Array(..)))
            }),
            // (`match None { Some(3u8) => .. }`: the payload type comes from a `Some(..)` pattern; with none, `()`
            // (rustc's `!` fallback for `Some(v) => v` in a diverging context))
            // (`match (0, 0) { (usize::MIN, usize::MAX) => .. }`: unsuffixed literals take their type from a pattern)
            e if lit_scrutinee(e) => pats.iter().find_map(|p| self.pat_ty(p).filter(|t| !self.prog.has_param(*t))),
            // (`match get() { x if x.method() => .., &S => .. }` with `fn get<T>() -> T`: the type inference read the
            // arms' patterns and guards, which the call's own arguments do not give)
            e @ Expr::Call(..) if self.lets.contains_key(&(e as *const Expr as usize)) => {
                let t = self.lets[&(e as *const Expr as usize)];
                (!self.prog.has_param(t) && !has_hole(self.prog, t)).then_some(t)
            }
            _ => self.bare_none_ty(scrut, &pats)?,
        };
        let Some(place) = self.bk_scrutinee(scrut, scrut_exp, &pats)? else {
            return Ok((None, Ty::Never));
        };
        // (E0004: the arms must cover the scrutinee's type; user code only)
        if self.user_code {
            if let Some(w) = self.non_exhaustive(arms, place.ty()) {
                return Err(self.at(expr_start(scrut), format!("non-exhaustive patterns: `{w}` not covered (E0004)")));
            }
            if self.prog.deny_unreachable {
                if let Some(i) = self.unreachable_arm(arms, place.ty()) {
                    return Err(self.at(arms[i].pos, "unreachable pattern".into()));
                }
            }
        }
        let bk_ss = self.bk.scrut.clone();
        let bk_sp = self.bk.scrut_place.clone();
        let rg_ss = self.rg.scrut.clone();
        let sflag = self.last_scrut;
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        // (the places a guard must not change, made at the first guard: see below)
        let mut guard_places: Option<Vec<(borrowck::BPlace, bool)>> = None;
        // 各 arm を上から順に試す (判定 → 束縛 → ガード → 本体)
        let mut after_guard = false;
        for arm in arms {
            let next = self.b.create_block();
            // (after a guard that failed, the next arm tests the scrutinee again: what the guard moved out of it is
            // used (E0382, at the match, as rustc reports it))
            if after_guard && self.bk_on() {
                if let Some((sp, at)) = &bk_sp {
                    let mut tested = vec![];
                    self.tested_places(&arm.pat, sp, place.ty(), &mut tested, 0);
                    for p in tested {
                        self.bk_acc_place(p, borrowck::Acc::Read, *at);
                    }
                    // (a binding of all of it by value (`x => x`) reads all of it, at the arm (a pattern has no
                    // position of its own))
                    if matches!(arm.pat, Pat::Ident(false, _, _, None)) {
                        self.bk_acc_place(sp.clone(), borrowck::Acc::Read, arm.pos);
                    }
                }
            }
            after_guard = arm.guard.is_some();
            // (a pattern has no position of its own: an error in it is reported at the arm's body)
            self.pat_test(&arm.pat, place, next).map_err(|m| self.at(arm.pos, m))?;
            self.scopes.push(vec![]);
            let ob = self.owned_binds;
            self.bk.scrut = bk_ss.clone();
            self.bk.scrut_place = bk_sp.clone();
            self.rg.scrut = rg_ss.clone();
            self.bk_bind_begin();
            self.bk.guard_arm = arm.guard.is_some();
            self.bk.arm_at = Some(arm.pos).filter(|p| p.line != 0);
            let r = self.pat_bind(&arm.pat, place, BindMode::Move);
            self.bk.arm_at = None;
            self.bk.guard_arm = false;
            self.bk_bind_end();
            r?;
            if self.owned_binds > ob {
                self.release_outer(scrut, sflag)?;
            }
            // (a guard must not change what the match tests: rustc's fake borrows (E0510). The places the patterns
            // test (`.0` of `(None, _)`; `match x { y if .. }` and `S { .. }` test nothing), and the deref prefixes of
            // the place bindings are bound from (`ptr` of `match *ptr { v if .. }`))
            if guard_places.is_none() && arm.guard.is_some() && self.bk_on() {
                if let Some((sp, _)) = &bk_sp {
                    let mut gs: Vec<(borrowck::BPlace, bool)> = vec![];
                    let mut tested = vec![];
                    for a in arms {
                        self.tested_places(&a.pat, sp, place.ty(), &mut tested, 0);
                    }
                    gs.extend(tested.into_iter().map(|p| (p, true)));
                    let mut names = vec![];
                    for a in arms {
                        crate::ast_closure::pat_idents(&a.pat, &mut names);
                    }
                    if names.iter().any(|n| n.starts_with(|c: char| c.is_lowercase() || c == '_')) {
                        for (i, x) in sp.proj.iter().enumerate() {
                            if matches!(x, borrowck::Proj::Deref(_) | borrowck::Proj::Shr) {
                                gs.push((borrowck::BPlace { local: sp.local, proj: sp.proj[..i].to_vec() }, false));
                            }
                        }
                    }
                    guard_places = Some(gs);
                }
            }
            let guard_place = match (&arm.guard, &guard_places) {
                (Some(_), Some(gs)) if self.bk_on() && !gs.is_empty() => Some(gs.clone()),
                _ => None,
            };
            if let Some(gp) = &guard_place {
                self.bk_ev(borrowck::Ev::Guard(Some(gp.clone())));
            }
            // (a guard sees the arm's bindings by shared reference: a closure in it that moves one out
            // (`|| { let bar = foo; }` with `ref mut foo`) is E0507, at the move)
            if let (Some(g), true) = (&arm.guard, self.bk_on()) {
                let mut names = vec![];
                crate::ast_closure::pat_idents(&arm.pat, &mut names);
                names.retain(|n| self.lookup(n).is_some_and(|(_, t)| !self.bk_ty(t).0 || matches!(t, Ty::Ref(true, _))));
                if !names.is_empty() {
                    let prog = self.prog;
                    let types: Vec<(String, Ty)> = names.iter().filter_map(|n| self.lookup(n).map(|(_, t)| (n.clone(), t))).collect();
                    // (a `ref mut` binding's `&mut` is moved by a `let` that binds it by value: not Copy here)
                    let by = |p: &Pat, n: &str| match types.iter().find(|x| x.0 == n) {
                        // (`let &mut n = foo;` matches the referent: it does not move the `&mut`)
                        Some((_, t @ Ty::Ref(true, _))) if matches!(p, Pat::Ref(..)) => borrowck::pat_moves(prog, p, *t, 0),
                        Some((_, Ty::Ref(true, _))) | None => crate::ast_closure::binds_by_value(p),
                        Some(x) => borrowck::pat_moves(prog, p, x.1, 0),
                    };
                    if let Some((n, at)) = guard_closure_move(g, &names, &by) {
                        return Err(self.at(at, format!("cannot move out of `{n}` in pattern guard (E0507)")));
                    }
                }
            }
            if let Some(g) = &arm.guard {
                // (`P if let Q = e && c => ..`: each `let` of the guard tests and binds in the arm's scope, each other
                // condition must hold; a failure goes to the next arm)
                fn chain<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
                    match e {
                        Expr::Binary(op, l, r, _) if op == "&&" => {
                            chain(l, out);
                            chain(r, out);
                        }
                        Expr::Paren(x) if matches!(&**x, Expr::Let(..)) => out.push(x),
                        e => out.push(e),
                    }
                }
                let mut cs = vec![];
                chain(g, &mut cs);
                if !cs.iter().any(|c| matches!(c, Expr::Let(..))) {
                    cs = vec![g];
                }
                for c in cs {
                    if let Expr::Let(lpat, lexpr) = c {
                        let Some(lplace) = ({ let ex = self.bare_none_ty(lexpr, &[&**lpat])?; self.bk_scrutinee(lexpr, ex, &[&**lpat])? }) else {
                            break;
                        };
                        self.pat_test(lpat, lplace, next)?;
                        self.bk_bind_begin();
                        let r = self.pat_bind(lpat, lplace, BindMode::Move);
                        self.bk_bind_end();
                        r?;
                        continue;
                    }
                    let (c, ct) = self.expr(c, Some(Ty::Bool))?;
                    self.same(SP, ct, Ty::Bool)?;
                    if let Some(c) = c {
                        self.require(c, next);
                    }
                }
            }
            if guard_place.is_some() {
                self.bk_ev(borrowck::Ev::Guard(None));
            }
            let t0 = self.temps.len();
            let (v, t) = self.expr(&arm.body, result_ty.or(expected))?;
            self.rg_keep(&arm.body);
            self.drop_tail_temps(t0, (v, t));
            self.pop_scope();
            self.finish_arm(v, t, merge, &mut result_ty, &mut param, SP)?;
            self.b.switch_to_block(next);
        }
        // どの arm にも合わない (rustc が網羅性を保証するので到達しない)
        self.b.ins().trap(TrapCode::unwrap_user(2));
        self.b.switch_to_block(merge);
        Ok(match result_ty {
            None => (None, Ty::Never),
            Some(t) => (param, t),
        })
    }

    pub(crate) fn if_let(&mut self, i: &Expr, l: &Expr, expected: Option<Ty>) -> R<V> {
        let Expr::If(_, then_branch, else_branch) = i else {
            return err(SP, "internal: if_let on a non-if expression");
        };
        let Expr::Let(lpat, lexpr) = l else {
            return err(SP, "internal: if_let without `let`");
        };
        let t0 = self.temps.len();
        let Some(place) = ({ let ex = self.bare_none_ty(lexpr, &[&**lpat])?; self.bk_scrutinee(lexpr, ex, &[&**lpat])? }) else {
            return Ok((None, Ty::Never));
        };
        let sflag = self.last_scrut;
        let t1 = self.temps.len();
        let else_blk = self.b.create_block();
        let merge = self.b.create_block();
        let mut result_ty: Option<Ty> = None;
        let mut param: Option<Value> = None;
        self.pat_test(lpat, place, else_blk)?;
        self.scopes.push(vec![]);
        let ob = self.owned_binds;
        self.bk_bind_begin();
        let r = self.pat_bind(lpat, place, BindMode::Move);
        self.bk_bind_end();
        r?;
        if self.owned_binds > ob {
            self.release_outer(lexpr, sflag)?;
        }
        let (tv, tt) = self.block(then_branch, expected)?;
        // (the pattern's bindings first: one may borrow from a temporary, a guard of a cell the temporary owns)
        self.pop_scope();
        // (Rust 2024: the scrutinee's temporaries are dropped at the end of the `if let`, on this path after the then
        // block: `(if let Some(_) = c.borrow().first() { 1 } else { 0 }, c.borrow_mut().len())` does not panic. Not
        // when the result may borrow from them, which rustc rejects (E0716) and zustc does not see yet; never the
        // result itself)
        if tt != Ty::Never && !self.bk_ty(tt).1 {
            let n = self.temps.len();
            let ts: Vec<Owned> = self.temps[t0.min(n)..t1.min(n)].iter().filter(|o| tv.is_none_or(|v| o.loc != Loc::Ptr(v))).copied().collect();
            for o in ts.into_iter().rev() {
                self.emit_owned_drop(o);
            }
        }
        self.finish_arm(tv, tt, merge, &mut result_ty, &mut param, SP)?;
        self.b.switch_to_block(else_blk);
        // (the scrutinee's temporaries are dropped before the else block (Rust 2024): `if let Some(x) =
        // c.borrow().first() { .. } else { c.borrow_mut().push(1) }` does not panic. Their drop flags are cleared,
        // so the end of the statement drops them only on the other path)
        if else_branch.is_some() {
            self.drop_for_exit(usize::MAX, t0);
        }
        let (ev, et) = match else_branch {
            Some(e) => self.expr(e, result_ty.or(expected))?,
            None => (None, Ty::Unit),
        };
        self.finish_arm(ev, et, merge, &mut result_ty, &mut param, SP)?;
        self.b.switch_to_block(merge);
        Ok(match result_ty {
            None => (None, Ty::Never),
            Some(t) => (param, t),
        })
    }

    pub(crate) fn while_let(&mut self, w: &Expr) -> R<V> {
        let Expr::While(label, cond, body) = w else {
            unreachable!()
        };
        self.pending_label = label_name(label);
        let Expr::Let(pat, e) = &**cond else {
            unreachable!()
        };
        self.while_let_parts(pat, e, body, "while")
    }

    /// `while let pat = e { body }` (for ループのイテレータ版からも使う)
    /// (kind: the loop the source wrote, `while` or `for`, for E0571)
    pub(crate) fn while_let_parts(&mut self, pat: &Pat, e: &Expr, body: &PBlock, kind: &'static str) -> R<V> {
        let header = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(header, &[]);
        self.b.switch_to_block(header);
        // 毎回 match 対象を評価し直す (一時値は 1 回ごとに drop する)
        let t0 = self.temps.len();
        let Some(place) = ({ let ex = self.bare_none_ty(e, &[pat])?; self.bk_scrutinee(e, ex, &[pat])? }) else {
            return Ok((None, Ty::Never));
        };
        let sflag = self.last_scrut;
        self.pat_test(pat, place, exit)?;
        self.scopes.push(vec![]);
        let ob = self.owned_binds;
        self.bk_bind_begin();
        let r = self.pat_bind(pat, place, BindMode::Move);
        self.bk_bind_end();
        r?;
        if self.owned_binds > ob {
            self.release_outer(e, sflag)?;
        }
        self.loops.push(Loop {
            cont: header,
            brk: exit,
            breaks: 0,
            label: self.pending_label.take(),
            val: None,
            is_block: false,
            depth: self.scopes.len() - 1,
            temps: t0,
            no_value: Some(kind),
        });
        let (_, bt) = self.block(body, Some(Ty::Unit))?;
        self.loops.pop();
        self.pop_scope();
        if bt != Ty::Never {
            self.same(SP, bt, Ty::Unit)?;
            self.drop_for_exit(usize::MAX, t0);
            self.b.ins().jump(header, &[]);
        } else {
            self.b.ins().trap(TrapCode::unwrap_user(1));
        }
        self.b.switch_to_block(exit);
        self.drop_temps(t0);
        Ok((None, Ty::Unit))
    }

    /// A path pattern that names a constant: a const of the program's (`Num::ZERO`) or a primitive type's
    /// (`u8::MAX`)
    pub(crate) fn path_const_pat(&self, p: &Path) -> bool {
        if p.segs.len() != 2 {
            return false;
        }
        let e = Expr::Path(None, p.clone());
        self.const_of(&e).is_some()
            || Ty::from_suffix(&p.segs[0].name).is_some_and(|t| crate::program::prim_assoc_const(t, &p.segs[1].name).is_some())
    }

    pub(crate) fn no_variant_msg(&self, p: &Path) -> String {
        let segs: Vec<String> = p.segs.iter().map(|s| s.name.clone()).collect();
        match segs.as_slice() {
            [t, v] => format!("no variant named `{v}` in `{t}`"),
            _ => "unsupported path pattern (only enum variants)".into(),
        }
    }

    /// cond が偽なら fail へ。真なら新しいブロックで続ける
    pub(crate) fn require(&mut self, cond: Value, fail: Block) {
        let next = self.b.create_block();
        self.b.ins().brif(cond, next, &[], fail, &[]);
        self.b.switch_to_block(next);
    }

    /// 参照に対して参照でないパターンを当てるときは自動で deref し、束縛モードを Ref にする
    pub(crate) fn pat_deref(&mut self, mut p: Place, mut mode: BindMode) -> (Place, BindMode) {
        loop {
            let (ptr, m, inner) = match p {
                Place::Var(v, Ty::Ref(m, i)) => (self.b.use_var(v), m, *i),
                Place::Mem(a, o, Ty::Ref(m, i)) => (self.b.ins().load(PTR, mf(), a, o), m, *i),
                _ => return (p, mode),
            };
            mode = match mode {
                BindMode::Ref(false) => BindMode::Ref(false),
                _ => BindMode::Ref(m),
            };
            p = Place::Mem(ptr, 0, inner);
        }
    }

    /// 構造体・列挙・タプルの値 p のフィールド (オフセット off、型 fty) の場所。p がメモリにない (パターンが値の型に
    /// 合わない: `let Ok(0) = 42u8;`) なら型の不一致のエラー
    pub(crate) fn field_place(p: Place, fty: Ty, off: u32) -> R<Place> {
        match p {
            Place::Mem(a, o, _) => Ok(Place::Mem(a, o + off as i32, fty)),
            _ => err(SP, "mismatched types: this pattern does not match the type of the value"),
        }
    }

    /// パターンの判定値 (リテラル / 範囲の端) を place の型で作る
    pub(crate) fn pat_const(&mut self, e: &Expr, t: Ty) -> R<Value> {
        // (`i32::MIN..=-1` や const の名前は式として評価する)
        let Expr::Lit(l, _) = e else {
            let (v, vt) = self.expr(e, Some(t))?;
            self.same(SP, vt, t)?;
            return v.ok_or_else(|| "range pattern bound has no value".to_string());
        };
        let (v, vt) = self.lit(l, Some(t))?;
        self.same(SP, vt, t)?;
        Ok(v.unwrap())
    }

    /// enum のタグが vi のバリアントか調べる
    pub(crate) fn test_variant(&mut self, p: Place, sid: u32, vi: usize, span: Sp, fail: Block) -> R<()> {
        if p.ty() != Ty::Struct(sid) {
            let got = self.prog.tn(p.ty());
            return err(span, format!("mismatched types: expected `{got}`, found `{}`", self.prog.tn(Ty::Struct(sid))));
        }
        let sd = &self.prog.structs[sid as usize];
        if sd.variants.len() == 1 {
            return Ok(()); // バリアントが 1 つなら判定不要
        }
        if vi >= sd.variants.len() {
            return err(span, format!("internal: `{}` has no variant {vi}", sd.name));
        }
        let (tag, discr) = (sd.tag, sd.variants[vi].discr);
        let Place::Mem(a, o, _) = p else {
            unreachable!()
        };
        let tv = self.b.ins().load(clif_ty(tag).unwrap(), mf(), a, o);
        let d = self.b.ins().iconst(clif_ty(tag).unwrap(), discr);
        let c = self.b.ins().icmp(IntCC::Equal, tv, d);
        self.require(c, fail);
        Ok(())
    }

    /// The places pattern p tests, from the place of the value it matches (of type t): rustc's shallow fake borrows
    /// of a match with guards. A tuple or a struct is not tested, only what its fields' patterns test (`(None, _)`
    /// tests `.0`); where a part's place is not followed, the place of the whole
    fn tested_places(&self, p: &Pat, place: &borrowck::BPlace, t: Ty, out: &mut Vec<borrowck::BPlace>, depth: u32) {
        let push = |out: &mut Vec<borrowck::BPlace>, pl: &borrowck::BPlace| {
            if !out.contains(pl) {
                out.push(pl.clone());
            }
        };
        let field = |pl: &borrowck::BPlace, f: String| {
            let mut q = pl.clone();
            q.proj.push(borrowck::Proj::Field(f));
            q
        };
        if depth > 8 {
            return push(out, place);
        }
        // (a reference matched by a pattern that is not a reference's: what it points to is tested)
        if let (Ty::Ref(m, i), Pat::Tuple(_) | Pat::TupleStruct(..) | Pat::Struct(..) | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) | Pat::Slice(_)) = (t, p) {
            let mut q = place.clone();
            q.proj.push(if m { borrowck::Proj::Deref(false) } else { borrowck::Proj::Shr });
            return self.tested_places(p, &q, *i, out, depth + 1);
        }
        match p {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident(_, _, n, sub) if n.starts_with(|c: char| c.is_lowercase() || c == '_') => {
                if let Some(s) = sub {
                    self.tested_places(s, place, t, out, depth + 1);
                }
            }
            Pat::Ref(_, i) => match t {
                Ty::Ref(m, it) => {
                    let mut q = place.clone();
                    q.proj.push(if m { borrowck::Proj::Deref(false) } else { borrowck::Proj::Shr });
                    self.tested_places(i, &q, *it, out, depth + 1);
                }
                _ => push(out, place),
            },
            Pat::Or(ps) => {
                for q in ps {
                    self.tested_places(q, place, t, out, depth + 1);
                }
            }
            Pat::Tuple(ps) => match t {
                Ty::Struct(sid) if self.prog.is_tuple(sid) => {
                    let fs = &self.prog.structs[sid as usize].fields;
                    match Self::tuple_fields(ps.iter(), fs.len(), SP) {
                        Ok(v) => {
                            for (i, q) in v {
                                self.tested_places(q, &field(place, i.to_string()), fs[i].1, out, depth + 1);
                            }
                        }
                        Err(_) => push(out, place),
                    }
                }
                _ => push(out, place),
            },
            // (a struct's pattern (not an enum's variant): what its fields' patterns test)
            Pat::TupleStruct(..) | Pat::Struct(..) if matches!(t, Ty::Struct(sid) if self.prog.structs[sid as usize].variants.is_empty() && !self.prog.is_tuple(sid)) => {
                let Ty::Struct(sid) = t else { return };
                let fs = &self.prog.structs[sid as usize].fields;
                match p {
                    Pat::TupleStruct(_, ps) => match Self::tuple_fields(ps.iter(), fs.len(), SP) {
                        Ok(v) => {
                            for (i, q) in v {
                                self.tested_places(q, &field(place, i.to_string()), fs[i].1, out, depth + 1);
                            }
                        }
                        Err(_) => push(out, place),
                    },
                    Pat::Struct(_, named, _) => {
                        for (n, q) in named {
                            match fs.iter().find(|f| f.0 == *n) {
                                Some(f) => self.tested_places(q, &field(place, n.clone()), f.1, out, depth + 1),
                                None => push(out, place),
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => push(out, place),
        }
    }

    /// パターン列 (`V(a, .., z)`) とフィールド列の対応 (`..` は途中を飛ばす)
    pub(crate) fn tuple_fields<'e>(elems: impl Iterator<Item = &'e Pat>, nfields: usize, span: Sp) -> R<Vec<(usize, &'e Pat)>> {
        let elems: Vec<&Pat> = elems.collect();
        let rest = elems.iter().position(|p| matches!(p, Pat::Rest));
        let n_named = elems.len() - rest.is_some() as usize;
        if n_named > nfields || (rest.is_none() && n_named != nfields) {
            return err(span, format!("this pattern has {n_named} fields, but the corresponding variant has {nfields}"));
        }
        let mut out = vec![];
        for (i, p) in elems.iter().enumerate() {
            match rest {
                Some(r) if i == r => {}
                Some(r) if i > r => out.push((nfields - (elems.len() - i), *p)),
                _ => out.push((i, *p)),
            }
        }
        Ok(out)
    }

    /// An identifier pattern that names a const of the scrutinee's type (a simple type: integer, bool, char, `&str`,
    /// a reference to an integer or integer slice)
    /// The part of t that is not structural (no derived PartialEq), if any: numbers, bool, char, str, references,
    /// arrays, tuples are; a struct / enum is when it derives PartialEq and its fields are (Option / Result derive it);
    /// Vec / String / Box (manual impls) are not
    pub(crate) fn non_structural(&self, t: Ty, depth: u32) -> Option<Ty> {
        if depth > 8 {
            return None;
        }
        match t {
            t if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char | Ty::Unit | Ty::Never | Ty::Str) => None,
            Ty::Ref(_, i) | Ty::Array(i, _) | Ty::Slice(i) => self.non_structural(*i, depth + 1),
            Ty::Struct(sid) => {
                let base = self.prog.base_of(sid);
                let sd = &self.prog.structs[sid as usize];
                // (std's types that derive PartialEq there, whatever the prelude's version does)
                let std_structural = matches!(
                    self.prog.structs[base as usize].name.as_str(),
                    "Option" | "Result" | "ManuallyDrop" | "Wrapping" | "Saturating" | "PhantomData" | "Reverse" | "Ordering" | "Bound" | "Range" | "RangeInclusive"
                );
                if !self.prog.is_tuple(base) && !std_structural && !self.prog.derived_peq.contains(&base) {
                    return Some(t);
                }
                // (an enum's variants are not looked into: only the variant the constant holds counts, which is not
                // evaluated here; a struct's fields all are)
                let fields: Vec<Ty> = if sd.is_enum { vec![] } else { sd.fields.iter().map(|f| f.1).collect() };
                fields.into_iter().find_map(|f| self.non_structural(f, depth + 1))
            }
            _ => None,
        }
    }

    /// Whether t holds a fn pointer or a raw pointer (in a field, an element): such a constant does not match reliably
    fn has_fn_or_raw_ptr(&self, t: Ty, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        match t {
            // (a raw pointer may come from an integer (`ptr::null()`), which matches reliably: only fn pointers)
            Ty::Fn(_) => true,
            Ty::Ref(_, i) | Ty::Array(i, _) | Ty::Slice(i) => self.has_fn_or_raw_ptr(*i, depth + 1),
            Ty::Struct(sid) => {
                let sd = &self.prog.structs[sid as usize];
                // (as for non_structural: not an enum's variants, only the one the constant holds would count)
                let fs: Vec<Ty> = if sd.is_enum { vec![] } else { sd.fields.iter().map(|f| f.1).collect() };
                fs.into_iter().any(|f| self.has_fn_or_raw_ptr(f, depth + 1))
            }
            _ => false,
        }
    }

    pub(crate) fn const_pat_ok(&self, pat: &Pat, t: Ty) -> bool {
        let Some(n) = const_ident_pat(self.prog, pat) else { return false };
        if self.lookup(n).is_some() {
            return false;
        }
        let ct = self.prog.consts[self.prog.const_index[n] as usize].ty;
        let simple = |t: Ty| t.is_int() || matches!(t, Ty::Bool | Ty::Char);
        ct == t
            && match t {
                Ty::Ref(_, Ty::Str) => true,
                Ty::Ref(_, Ty::Slice(e)) => simple(**e),
                Ty::Ref(_, i) => simple(*i),
                t => simple(t),
            }
    }

    /// パターンの判定だけをする (合わなければ fail へ飛ぶ)。束縛は pat_bind で行う
    pub(crate) fn pat_test(&mut self, pat: &Pat, p: Place, fail: Block) -> R<()> {
        match pat {
            Pat::Wild | Pat::Rest => Ok(()),
            // `None` のように unit バリアントに解決される識別子は、束縛ではなくパスのパターン
            Pat::Ident(..) if unit_variant_ident(self.prog, pat).is_some() => {
                let path = unit_variant_ident(self.prog, pat).unwrap();
                let (p, _) = self.pat_deref(p, BindMode::Move);
                let Some((sid, vi)) = self.variant_on(&path, p.ty()) else {
                    unreachable!()
                };
                self.test_variant(p, sid, vi, SP, fail)
            }
            Pat::Ident(..) if const_lit_pat(self.prog, pat).is_some() => {
                let lp = const_lit_pat(self.prog, pat).unwrap();
                self.pat_test(&lp, p, fail)
            }
            // (`G::<5> => ..` with `struct G<const N: usize>;`: a unit struct's pattern, checked against the value's
            // type (through references) and always matching)
            Pat::Path(path) if path.segs.last().is_some_and(|s| !s.args.is_empty()) && self.variant_on(path, p.ty()).is_none() => {
                let (pd, _) = self.pat_deref(p, BindMode::Move);
                let (pp, _) = self.turbofish_split(path);
                if self.prog.struct_by_p(&pp, self.self_sid).is_none_or(|sid| !self.prog.structs[sid as usize].fields.is_empty() || self.prog.structs[sid as usize].is_enum) {
                    return err(SP, self.no_variant_msg(path));
                }
                if let Some(pt) = self.pat_turbofish_ty(path, pd.ty())? {
                    self.same(SP, pt, pd.ty())?;
                }
                Ok(())
            }
            // (`Num::ZERO => ..` / `u8::MAX`: a path to a constant, not a variant, is equal to its value)
            Pat::Path(path) if self.variant_on(path, p.ty()).is_none() && self.path_const_pat(path) => {
                let t = p.ty();
                let (v, _) = self.read(p);
                let ce = self.keep_expr(Expr::Path(None, path.clone()));
                let (c, ct) = self.expr(ce, Some(t))?;
                self.same(SP, ct, t)?;
                let cond = self.eq_at(v.unwrap(), c.unwrap(), t, SP)?;
                self.require(cond, fail);
                Ok(())
            }
            // (`X => ..` with `const X: u64 = !0 as u16 as u64;` / `const FOO: &[u8] = b"foo";`: equal to its value)
            Pat::Ident(..) if self.const_pat_ok(pat, p.ty()) => {
                let n = const_ident_pat(self.prog, pat).unwrap().to_string();
                let t = p.ty();
                let (v, _) = self.read(p);
                let ce = self.keep_expr(path_expr(&n));
                let (c, ct) = self.expr(ce, Some(t))?;
                self.same(SP, ct, t)?;
                let cond = self.eq_at(v.unwrap(), c.unwrap(), t, SP)?;
                self.require(cond, fail);
                Ok(())
            }
            // (a constant of a type without a derived PartialEq (or holding one) cannot be a pattern)
            Pat::Ident(_, _, n, None) if self.user_code && self.lookup(n).is_none() && self.prog.const_index.contains_key(n.as_str()) => {
                let c = &self.prog.consts[self.prog.const_index[n.as_str()] as usize];
                // (a static's name is not a pattern (E0530))
                // (with modules, whose scopes are flattened away, the static may be another module's: a binding then)
                if c.stat.is_some() {
                    if self.prog.has_mods {
                        return Ok(());
                    }
                    return err(SP, format!("match bindings cannot shadow statics (E0530): `{n}`"));
                }
                let ct = c.ty;
                if let Ty::Struct(sid) = self.strip_refs(ct) {
                    if self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name) {
                        return err(SP, "cannot use unions in constant patterns".to_string());
                    }
                }
                if let Some(bad) = self.non_structural(ct, 0) {
                    return err(SP, format!("constant of non-structural type `{}` in a pattern", self.prog.tn(bad)));
                }
                if let Ty::Dyn(_) = self.strip_refs(ct) {
                    return err(SP, format!("`{}` cannot be used in patterns", self.prog.tn(self.strip_refs(ct))));
                }
                if self.has_fn_or_raw_ptr(ct, 0) {
                    return err(SP, "function pointers and raw pointers not derived from integers in patterns behave unpredictably".to_string());
                }
                // (a structural constant of the scrutinee's type (`const C: [u8; 2]`, a struct deriving PartialEq):
                // equal to its value, as `==` compares)
                // (`match &[2u8] { C => .. }` with `const C: [u8; 1]`: the scrutinee through its references)
                let mut p = p;
                while p.ty() != ct && matches!(p.ty(), Ty::Ref(..)) {
                    p = self.pat_deref_once(p).0;
                }
                let t = p.ty();
                // (`match 0 { OR => .. }` with `const OR: usize`: rustc types the literal from the constant; here the
                // scrutinee kept its own integer type, and the constant's value is compared in it)
                let int_pair = ct.is_int() && t.is_int();
                if ct != t && !int_pair {
                    return err(SP, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(t), self.prog.tn(ct)));
                }
                let (v, _) = self.read(p);
                let ce = self.keep_expr(path_expr(n));
                let (c, cty) = self.expr(ce, Some(ct))?;
                self.same(SP, cty, ct)?;
                let c = match (c, int_pair && ct != t) {
                    (Some(c), true) => Some(self.convert(c, ct, t)),
                    (c, _) => c,
                };
                if let (Some(v), Some(c)) = (v, c) {
                    let cond = self.eq_at(v, c, t, SP)?;
                    self.require(cond, fail);
                }
                Ok(())
            }
            Pat::Ident(_, _, _, sub) => match sub {
                Some(sub) => self.pat_test(sub, p, fail),
                None => Ok(()),
            },
            Pat::Ref(_, inner) => {
                let Ty::Ref(_, _) = p.ty() else {
                    return err(SP, format!("mismatched types: `{}` is not a reference", self.prog.tn(p.ty())));
                };
                let (q, _) = self.pat_deref_once(p);
                self.pat_test(inner, q, fail)
            }
            Pat::Slice(_) => {
                // (an array's length is known: a pattern of another length is an error (E0527 / E0528))
                let mut at = p.ty();
                while let Ty::Ref(_, i) = at {
                    at = *i;
                }
                let (pre, rest, post) = slice_parts(pat);
                let fixed = (pre.len() + post.len()) as i64;
                if let Ty::Array(_, n) = at {
                    if rest.is_none() && fixed != n as i64 {
                        return err(SP, format!("pattern requires {fixed} elements but array has {n} (E0527)"));
                    }
                    if rest.is_some() && fixed > n as i64 {
                        return err(SP, format!("pattern requires at least {fixed} elements but array has {n} (E0528)"));
                    }
                }
                let (base, len, elem, _) = self.slice_scrutinee(p, SP)?;
                let c = if rest.is_some() {
                    self.b.ins().icmp_imm_u(IntCC::UnsignedGreaterThanOrEqual, len, fixed)
                } else {
                    self.b.ins().icmp_imm_u(IntCC::Equal, len, fixed)
                };
                self.require(c, fail);
                for (q, sub) in self.slice_elems(base, len, elem, &pre, &post) {
                    self.pat_test(sub, q, fail)?;
                }
                Ok(())
            }
            Pat::Or(cases) => {
                let ok = self.b.create_block();
                let n = cases.len();
                for (i, case) in cases.iter().enumerate() {
                    let nf = if i + 1 == n { fail } else { self.b.create_block() };
                    self.pat_test(case, p, nf)?;
                    self.b.ins().jump(ok, &[]);
                    if i + 1 != n {
                        self.b.switch_to_block(nf);
                    }
                }
                self.b.switch_to_block(ok);
                Ok(())
            }
            // バイト列のパターン (`match w { b"pub" => .. }`、w: &[u8] / &[u8; N]): 長さと中身で比べる
            Pat::Lit(l) if lit_kind(l, |x| matches!(x, Lit::ByteStr(_))) => {
                let Expr::Lit(text, _) = &**l else { unreachable!() };
                let Some(Lit::ByteStr(bs)) = lit::lit(text) else { unreachable!() };
                let mut p = p;
                // (&&[u8] などは 1 段ずつ外し、&[u8] / &[u8; N] で止める)
                while let Ty::Ref(_, i) = p.ty() {
                    if matches!(i, Ty::Slice(_) | Ty::Array(..)) {
                        break;
                    }
                    let (v, _) = self.read(p);
                    p = Place::Mem(v.unwrap(), 0, *i);
                }
                let u8s = Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))));
                let fat = match p.ty() {
                    Ty::Ref(_, Ty::Slice(Ty::U8)) => self.read(p).0.unwrap(),
                    Ty::Ref(_, Ty::Array(Ty::U8, n)) => {
                        let (v, _) = self.read(p);
                        let n = self.b.ins().iconst(types::I64, *n as i64);
                        self.make_fat_v(v.unwrap(), n)
                    }
                    t => return err(SP, format!("mismatched types: expected `{}`, found `&[u8]`", self.prog.tn(t))),
                };
                let (c, _) = self.lit(text, None)?;
                let n = self.b.ins().iconst(types::I64, bs.len() as i64);
                let cf = self.make_fat_v(c.unwrap(), n);
                let cond = self.eq_at(fat, cf, u8s, SP)?;
                self.require(cond, fail);
                Ok(())
            }
            // 文字列のパターン (`match s { "add" => .. }`): &str (と &&str) の中身で比べる
            Pat::Lit(l) if lit_kind(l, |x| matches!(x, Lit::Str(_))) => {
                let Expr::Lit(text, _) = &**l else { unreachable!() };
                let sref = Ty::Ref(false, intern(Ty::Str));
                let mut p = p;
                while let Ty::Ref(_, i) = p.ty() {
                    if p.ty() == sref || *i == Ty::Str {
                        break;
                    }
                    let (v, _) = self.read(p);
                    p = Place::Mem(v.unwrap(), 0, *i);
                }
                if !matches!(p.ty(), Ty::Ref(_, Ty::Str)) {
                    return err(SP, format!("mismatched types: expected `{}`, found `&str`", self.prog.tn(p.ty())));
                }
                let (v, _) = self.read(p);
                let (c, _) = self.lit(text, Some(sref))?;
                let cond = self.eq_at(v.unwrap(), c.unwrap(), sref, SP)?;
                self.require(cond, fail);
                Ok(())
            }
            _ => {
                let (p, _) = self.pat_deref(p, BindMode::Move);
                let t = p.ty();
                match pat {
                    Pat::Lit(l) => {
                        let (v, _) = self.read(p);
                        // (負のリテラル `-1` は式として評価する)
                        let (c, ct) = match &**l {
                            Expr::Lit(text, _) => self.lit(text, Some(t))?,
                            e => self.expr(e, Some(t))?,
                        };
                        self.same(SP, ct, t)?;
                        if !(t.is_int() || t == Ty::Bool || t == Ty::Char) {
                            return err(SP, "literal patterns need an integer, char, bool or string");
                        }
                        let cond = self.b.ins().icmp(IntCC::Equal, v.unwrap(), c.unwrap());
                        self.require(cond, fail);
                        Ok(())
                    }
                    Pat::Range(start, end, inclusive) => {
                        if !(t.is_int() || t == Ty::Char) {
                            return err(SP, "range patterns need an integer or char");
                        }
                        let (v, _) = self.read(p);
                        let v = v.unwrap();
                        let s = t.signed();
                        if let Some(lo) = start {
                            let lo = self.pat_const(lo, t)?;
                            let cc = if s { IntCC::SignedGreaterThanOrEqual } else { IntCC::UnsignedGreaterThanOrEqual };
                            let c = self.b.ins().icmp(cc, v, lo);
                            self.require(c, fail);
                        }
                        if let Some(hi) = end {
                            let hi = self.pat_const(hi, t)?;
                            let cc = match (*inclusive, s) {
                                (true, true) => IntCC::SignedLessThanOrEqual,
                                (true, false) => IntCC::UnsignedLessThanOrEqual,
                                (_, true) => IntCC::SignedLessThan,
                                (_, false) => IntCC::UnsignedLessThan,
                            };
                            let c = self.b.ins().icmp(cc, v, hi);
                            self.require(c, fail);
                        }
                        Ok(())
                    }
                    Pat::Path(path) => {
                        let Some((sid, vi)) = self.variant_on(path, t) else {
                            return err(SP, self.no_variant_msg(path));
                        };
                        self.test_variant(p, sid, vi, SP, fail)
                    }
                    Pat::TupleStruct(path, elems) => {
                        let fields = match self.variant_on(path, t) {
                            Some((sid, vi)) => {
                                self.test_variant(p, sid, vi, SP, fail)?;
                                self.prog.structs[sid as usize].variants[vi].fields.clone()
                            }
                            None => self.tuple_struct_fields(path, t, SP)?,
                        };
                        for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP).map_err(|m| self.at(path.pos, m))? {
                            let (_, fty, off) = fields[fi];
                            self.pat_test(sub, Self::field_place(p, fty, off)?, fail)?;
                        }
                        Ok(())
                    }
                    Pat::Tuple(elems) => {
                        let fields = self.tuple_pat_fields(p, elems.len(), SP)?;
                        for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP)? {
                            let (_, fty, off) = fields[fi];
                            self.pat_test(sub, Self::field_place(p, fty, off)?, fail)?;
                        }
                        Ok(())
                    }
                    Pat::Struct(..) => {
                        let fields = self.struct_pat_fields(pat, p, Some(fail))?;
                        for (sub, fp) in fields {
                            self.pat_test(sub, fp, fail)?;
                        }
                        Ok(())
                    }
                    _ => err(SP, "unsupported pattern"),
                }
            }
        }
    }

    /// struct パターン (`P(a, b)` / `S { .. }`) のフィールド。match 対象の型 t のインスタンスで引く
    pub(crate) fn tuple_struct_fields(&self, path: &Path, t: Ty, span: Sp) -> R<Vec<(String, Ty, u32)>> {
        let Some(sid) = self.prog.struct_by_p(&PPath::of(path), self.self_sid) else {
            return err(span, self.no_variant_msg(path));
        };
        match t {
            Ty::Struct(x) if self.prog.base_of(x) == self.prog.base_of(sid) => Ok(self.prog.structs[x as usize].fields.clone()),
            _ => err(span, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(t), self.prog.structs[sid as usize].name)),
        }
    }

    /// タプルパターンのフィールド (match 対象はタプルのインスタンス)
    pub(crate) fn tuple_pat_fields(&self, p: Place, n_pat: usize, span: Sp) -> R<Vec<(String, Ty, u32)>> {
        match p.ty() {
            Ty::Struct(x) if self.prog.is_tuple(x) => Ok(self.prog.structs[x as usize].fields.clone()),
            // `()` のパターン (`Ok(())`)
            Ty::Unit if n_pat == 0 => Ok(vec![]),
            t => err(span, format!("mismatched types: expected a tuple of {n_pat} elements, found `{}`", self.prog.tn(t))),
        }
    }

    pub(crate) fn pat_deref_once(&mut self, p: Place) -> (Place, bool) {
        match p {
            Place::Var(v, Ty::Ref(m, i)) => (Place::Mem(self.b.use_var(v), 0, *i), m),
            // (a `&[T]` / `&str` in memory is the (pointer, length) pair itself: the slice place is that pair)
            Place::Mem(a, o, Ty::Ref(m, i @ (Ty::Slice(_) | Ty::Str))) => (Place::Mem(a, o, *i), m),
            Place::Mem(a, o, Ty::Ref(m, i)) => (Place::Mem(self.b.ins().load(PTR, mf(), a, o), 0, *i), m),
            p => (p, false),
        }
    }

    /// `S { a, b: p, .. }` / `E::V { a, .. }` の各フィールドの (サブパターン, place)。
    /// fail があればバリアントの判定もする
    pub(crate) fn struct_pat_fields<'e>(&mut self, ps: &'e Pat, p: Place, fail: Option<Block>) -> R<Vec<(&'e Pat, Place)>> {
        let Pat::Struct(path, pfields, has_rest) = ps else {
            return err(SP, "internal: struct_pat_fields on a non-struct pattern");
        };
        let fields: Vec<(String, Ty, u32)> = if let Some((sid, vi)) = self.variant_on(path, p.ty()) {
            if let Some(fail) = fail {
                self.test_variant(p, sid, vi, SP, fail)?;
            }
            self.prog.structs[sid as usize].variants[vi].fields.clone()
        } else if self.prog.struct_by_p(&PPath::of(path), self.self_sid).is_some() {
            self.tuple_struct_fields(path, p.ty(), SP)?
        } else {
            return err(SP, "cannot find struct or variant");
        };
        let mut out = vec![];
        for (fid, fpat) in pfields {
            // (`S { 0: x }` の番号のフィールドは受け付けない)
            if fid.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return err(SP, "expected a field name");
            }
            let Some(&(_, fty, off)) = fields.iter().find(|f| f.0 == *fid) else {
                return err(SP, format!("no field `{fid}`"));
            };
            out.push((fpat, Self::field_place(p, fty, off)?));
        }
        if !*has_rest && out.len() != fields.len() {
            return err(SP, "pattern does not mention all fields (add `..`)");
        }
        Ok(out)
    }

    /// 判定に通った後で、パターン中の名前を束縛する
    pub(crate) fn pat_bind(&mut self, pat: &Pat, p: Place, mode: BindMode) -> R<()> {
        match pat {
            // (`let PhantomData::<(A, B)> = x`: a unit struct's turbofish names the type the value has)
            Pat::Path(path) if path.segs.last().is_some_and(|s| !s.args.is_empty()) && self.variant_on(path, p.ty()).is_none() => {
                // (`let G::<5> = &x;`: through the reference, as match ergonomics reads it)
                let (pd, _) = self.pat_deref(p, mode);
                if let Some(pt) = self.pat_turbofish_ty(path, pd.ty())? {
                    self.same(SP, pt, pd.ty())?;
                }
                Ok(())
            }
            Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) => Ok(()),
            Pat::Ident(..) if unit_variant_ident(self.prog, pat).is_some() => Ok(()),
            Pat::Ident(..) if const_lit_pat(self.prog, pat).is_some() => Ok(()),
            Pat::Ident(..) if self.const_pat_ok(pat, p.ty()) => Ok(()),
            // (an aggregate constant, tested by pat_test: not a new name, so a nested match still compares with it)
            Pat::Ident(_, _, n, None)
                if self.user_code
                    && (self.prog.lower_consts || n.trim_start_matches('_').starts_with(|c: char| c.is_uppercase()))
                    && self.prog.const_index.get(n.as_str()).is_some_and(|&ci| self.prog.consts[ci as usize].stat.is_none())
                    && self.lookup(n).is_none() =>
            {
                Ok(())
            }
            Pat::Ident(by_ref, mutable, name, subpat) => {
                if let Some(sub) = subpat {
                    self.pat_bind(sub, p, mode)?;
                }
                let name = name.clone();
                let mode = if *by_ref { BindMode::Ref(*mutable) } else { mode };
                match mode {
                    BindMode::Move => {
                        let (v, t) = self.read(p);
                        self.bk_pat_move(t, &name);
                        self.bk.mut_next = Some(*mutable);
                        self.bind(name.clone(), t, v);
                        self.rg_pat_bind(&name, false);
                    }
                    BindMode::Ref(m) => {
                        // (a value not in memory (`let ref x = 0;`, a zero-sized one): a temporary holds it, as `&`
                        // of a value does)
                        let (addr, t) = match p {
                            Place::Mem(a, o, t) => (self.addr_plus(a, o), t),
                            Place::Zst(t) => (self.temp(t), t),
                            Place::Var(..) => {
                                let (v, t) = self.read(p);
                                let tmp = self.temp(t);
                                self.write(Place::Mem(tmp, 0, t), v);
                                (tmp, t)
                            }
                        };
                        let saved = self.bk_pat_ref(m, *by_ref);
                        // (the binding of `ref x` / `ref mut x`, or one by reference of the default binding mode, is not
                        // `mut` itself: `&mut x` of it is E0596)
                        self.bk.mut_next = Some(false);
                        self.bind(name.clone(), Ty::Ref(m, intern(t)), Some(addr));
                        self.bk.bind_srcs = saved;
                        if !*by_ref {
                            self.rg_pat_bind(&name, true);
                        } else {
                            // (`ref` / `ref mut`: a borrow of the part matched)
                            let pr = self.rg.pat_ref.take();
                            self.rg_pat_bind(&name, true);
                            self.rg.pat_ref = pr;
                        }
                    }
                }
                Ok(())
            }
            Pat::Ref(pm, inner) => {
                let (q, _) = self.pat_deref_once(p);
                let saved = self.bk_pat_proj(borrowck::ref_deref(*pm));
                let rs = (self.rg.pat.clone(), self.rg.pat_ref);
                self.rg_pat_align(q.ty());
                // (an explicit `&` pattern: what it binds is by value again)
                self.rg.pat_ref = None;
                let r = self.pat_bind(inner, q, BindMode::Move);
                self.rg.pat = rs.0;
                    self.rg.pat_ref = rs.1;
                self.bk.pat_path = saved;
                r
            }
            Pat::Or(cases) => {
                if !cases.iter().any(has_bindings) {
                    return Ok(());
                }
                // (the bindings of `|` alternatives are not followed)
                self.rg.pat = None;
                // どの選択肢に合ったかを判定し直し、その選択肢で束縛した値を merge の引数で渡す
                let mut names = vec![];
                crate::ast_closure::pat_idents(&cases[0], &mut names);
                let merge = self.b.create_block();
                let mut tys: Option<Vec<(String, Ty)>> = None;
                for case in cases {
                    let next = self.b.create_block();
                    self.pat_test(case, p, next)?;
                    let o0 = self.owned.len();
                    let ob = self.owned_binds;
                    self.scopes.push(vec![]);
                    self.pat_bind(case, p, mode)?;
                    let mut vals = vec![];
                    let mut tl = vec![];
                    for n in &names {
                        if let Some((loc, t)) = self.lookup(n) {
                            if let Some(v) = self.loc_arg(loc, t) {
                                vals.push(v);
                            }
                            tl.push((n.clone(), t));
                        }
                    }
                    self.scopes.pop();
                    self.owned.truncate(o0);
                    self.owned_binds = ob;
                    match &tys {
                        None => {
                            for (_, t) in &tl {
                                if let Some(ct) = self.param_ty(*t) {
                                    self.b.append_block_param(merge, ct);
                                }
                            }
                            tys = Some(tl);
                        }
                        Some(t0) => {
                            if t0.len() != tl.len() || t0.iter().zip(tl.iter()).any(|(a, b)| a.1 != b.1) {
                                return err(SP, "`|` alternatives bind different types");
                            }
                        }
                    }
                    let args: Vec<Value> = vals.into_iter().collect();
                    self.b.ins().jump(merge, &args);
                    self.b.switch_to_block(next);
                }
                self.b.ins().trap(TrapCode::unwrap_user(2));
                self.b.switch_to_block(merge);
                let params = self.b.block_params(merge).to_vec();
                let mut pi = params.into_iter();
                for (n, t) in tys.unwrap_or_default() {
                    let v = if self.param_ty(t).is_some() { pi.next() } else { None };
                    self.bind(n, t, v);
                }
                Ok(())
            }
            Pat::Slice(_) => {
                // (an array matched by value: each element is a place of its own (`[_, _, x] = a` moves a[2]); a slice's
                // or one through a reference is not tracked)
                let arr = matches!(p.ty(), Ty::Array(..));
                let saved = if arr { self.bk.pat_path.clone() } else { self.bk.pat_path.take() };
                let rs = self.rg.pat.take();
                // (the elements' tree: the array's or the slice's, through the references matched)
                self.rg.pat_elem = rs.clone().and_then(|p| self.rg_slice_elem(p));
                let r = self.pat_bind_slice(pat, p, mode);
                self.rg.pat_elem = None;
                self.bk.pat_path = saved;
                self.rg.pat = rs;
                r
            }
            _ => self.pat_bind_rest(pat, p, mode),
        }
    }

    fn pat_bind_slice(&mut self, pat: &Pat, p: Place, mode: BindMode) -> R<()> {
        match pat {
            Pat::Slice(_) => {
                let (base, len, elem, rmode) = self.slice_scrutinee(p, SP)?;
                let mode = rmode.map(BindMode::Ref).unwrap_or(mode);
                let (pre, rest, post) = slice_parts(pat);
                let n = match p.ty() {
                    Ty::Array(_, n) if self.bk.pat_path.is_some() => Some(n as usize),
                    _ => None,
                };
                for (k, (q, sub)) in self.slice_elems(base, len, elem, &pre, &post).into_iter().enumerate() {
                    self.rg.pat = self.rg.pat_elem.clone();
                    let idx = if k < pre.len() { k } else { n.unwrap_or(0).wrapping_sub(pre.len() + post.len()).wrapping_add(k) };
                    let saved = n.map(|_| self.bk_pat_proj(borrowck::Proj::Field(format!("[{idx}]"))));
                    self.pat_bind(sub, q, mode)?;
                    if let Some(s) = saved {
                        self.bk.pat_path = s;
                    }
                }
                self.rg.pat = None;
                // (`rest @ ..` by value of an array: it takes the elements between, one place for them all (`a[1..19]`), so
                // the work does not grow with the array's length)
                if let (Some(n), Some(Pat::Ident(false, _, _, _)), BindMode::Move) = (n, rest, mode) {
                    if !self.bk_ty(elem).0 && pre.len() < n.saturating_sub(post.len()) {
                        let saved = self.bk_pat_proj(borrowck::Proj::Field(format!("[{}..{}]", pre.len(), n - post.len())));
                        self.bk_pat_move(elem, "");
                        self.bk.pat_path = saved;
                    }
                }
                // `rest @ ..`: 残りの部分スライス (`&[T]`)
                if let Some(Pat::Ident(_, _, rname, _)) = rest {
                    let (es, _) = self.prog.size_align(elem);
                    let start = self.b.ins().iadd_imm_s(base, pre.len() as i64 * es as i64);
                    let n = self.b.ins().iadd_imm_s(len, -((pre.len() + post.len()) as i64));
                    let fat = self.make_fat_v(start, n);
                    let m = matches!(mode, BindMode::Ref(true));
                    self.bind(rname.clone(), Ty::Ref(m, intern(Ty::Slice(intern(elem)))), Some(fat));
                }
                Ok(())
            }
            _ => unreachable!(),
        }
    }

    fn pat_bind_rest(&mut self, pat: &Pat, p: Place, mode: BindMode) -> R<()> {
        match pat {
            Pat::TupleStruct(path, elems) => {
                let (p, mode) = self.pat_deref(p, mode);
                self.rg_pat_align(p.ty());
                self.rg_pat_lifetimes(path);
                let (fields, vname, rvi) = match self.variant_on(path, p.ty()) {
                    Some((sid, vi)) => {
                        let v = &self.prog.structs[sid as usize].variants[vi];
                        (v.fields.clone(), Some(v.name.clone()), Some(vi))
                    }
                    None => (self.tuple_struct_fields(path, p.ty(), SP)?, None, None),
                };
                let deref = matches!(mode, BindMode::Ref(_));
                for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP).map_err(|m| self.at(path.pos, m))? {
                    let (fname, fty, off) = fields[fi].clone();
                    let saved = self.bk_pat_field(deref, vname.as_deref(), &fname);
                    let rs = self.rg_pat_field(p.ty(), rvi, fi);
                    let r = self.pat_bind(sub, Self::field_place(p, fty, off)?, mode);
                    self.rg.pat = rs.0;
                    self.rg.pat_ref = rs.1;
                    self.bk.pat_path = saved;
                    r?;
                }
                Ok(())
            }
            Pat::Tuple(elems) => {
                let (p, mode) = self.pat_deref(p, mode);
                self.rg_pat_align(p.ty());
                let fields = self.tuple_pat_fields(p, elems.len(), SP)?;
                let deref = matches!(mode, BindMode::Ref(_));
                for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP)? {
                    let (fname, fty, off) = fields[fi].clone();
                    let saved = self.bk_pat_field(deref, None, &fname);
                    let rs = self.rg_pat_field(p.ty(), None, fi);
                    let r = self.pat_bind(sub, Self::field_place(p, fty, off)?, mode);
                    self.rg.pat = rs.0;
                    self.rg.pat_ref = rs.1;
                    self.bk.pat_path = saved;
                    r?;
                }
                Ok(())
            }
            Pat::Struct(spath, fps, _) => {
                let (p, mode) = self.pat_deref(p, mode);
                self.rg_pat_align(p.ty());
                self.rg_pat_lifetimes(spath);
                // (the borrow check follows the parts by their names, as for a tuple struct's (`S { x: ax, .. } = a`
                // moves a.x); the region check does not)
                let vname = self.variant_on(spath, p.ty()).map(|(sid, vi)| self.prog.structs[sid as usize].variants[vi].name.clone());
                let deref = matches!(mode, BindMode::Ref(_));
                let rs = self.rg.pat.take();
                let r = (|| {
                    let parts = self.struct_pat_fields(pat, p, None)?;
                    for ((fname, _), (sub, fp)) in fps.iter().zip(parts) {
                        let saved = self.bk_pat_field(deref, vname.as_deref(), fname);
                        let r = self.pat_bind(sub, fp, mode);
                        self.bk.pat_path = saved;
                        r?;
                    }
                    Ok(())
                })();
                self.rg.pat = rs;
                r
            }
            _ => err(SP, "unsupported pattern"),
        }
    }
}

// ---------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------
//
// - drop が要る型 ([`Program::needs_drop`]) の値を持つローカル変数・一時値は、所有者として
//   `owned` / `temps` に drop フラグ (スタック上の 1 byte) と一緒に登録する。
// - ムーブ (値としての読み出し・値で受ける self・move キャプチャ・値で束縛するパターン) でフラグを 0 にする。
//   フィールドだけのムーブ (部分ムーブ) も全体のフラグを 0 にする (残りはリークする)。
// - スコープの終わり・return / break / continue / `?` で、フラグが 1 のものを登録と逆順に drop する。
// - drop 本体は `__exr_drop_in_place::<T>` のインスタンス (drop glue)。codegen が型ごとに
//   1 段だけ展開した本体を作り、入れ子の型は呼び出しにする (再帰的な型でも展開が止まる)。
// - panic では drop しない (unwind しない)。

impl Cx<'_> {
    pub(crate) fn needs_drop(&self, t: Ty) -> bool {
        self.prog.has_drop && self.prog.needs_drop(t)
    }

    pub(crate) fn new_flag(&mut self) -> StackSlot {
        let ss = self.b.create_stack_slot(1, 1);
        // (入口のブロックは最初に作るブロック: Block(0))
        if self.b.current_block() != Some(Block(0)) {
            self.late_flags.push(ss);
        }
        let one = self.b.ins().iconst(types::I8, 1);
        self.b.ins().stack_store(one, ss, 0);
        ss
    }

    pub(crate) fn clear_flag(&mut self, flag: StackSlot) {
        let z = self.b.ins().iconst(types::I8, 0);
        self.b.ins().stack_store(z, flag, 0);
    }

    /// 今のスコープのローカル変数として所有する
    pub(crate) fn own_local(&mut self, loc: Loc, ty: Ty) {
        if !self.needs_drop(ty) {
            return;
        }
        let flag = self.new_flag();
        let depth = self.scopes.len() - 1;
        self.owned.push(Owned { loc, ty, flag, depth });
    }

    /// 一時値 v (集約型ならアドレス) を文の終わりまで所有する。drop が要らなければ None
    pub(crate) fn own_temp(&mut self, v: Value, ty: Ty) -> Option<StackSlot> {
        if !self.needs_drop(ty) {
            return None;
        }
        let loc = if ty.is_agg() {
            Loc::Ptr(v)
        } else {
            let ss = self.slot(ty);
            self.b.ins().stack_store(v, ss, 0);
            Loc::Slot(ss)
        };
        let flag = self.new_flag();
        self.temps.push(Owned { loc, ty, flag, depth: 0 });
        Some(flag)
    }

    /// 0 で埋めた状態が「drop しても何もしない値」になる型か (Box は null、enum は判別値 0 のバリアント)。
    /// `impl Drop` を含む型は 0 にすると drop が走ってしまうので不可
    pub(crate) fn zeroable(&self, t: Ty) -> bool {
        match t {
            Ty::Box(_) => true,
            Ty::Array(e, _) => !self.needs_drop(*e) || self.zeroable(*e),
            Ty::Struct(sid) => {
                // (prelude の Vec / String / Rc / Arc は 0 (null の ptr、長さ 0) でも drop が何もしない)
                let bname = &self.prog.structs[self.prog.base_of(sid) as usize].name;
                if bname == "Vec" || bname == "String" || bname == "Rc" || bname == "Arc" {
                    return true;
                }
                if self.prog.drop_impls.contains(&self.prog.base_of(sid)) {
                    return false;
                }
                let sd = &self.prog.structs[sid as usize];
                let ok = |fs: &[(String, Ty, u32)]| fs.iter().all(|f| !self.needs_drop(f.1) || self.zeroable(f.1));
                if sd.is_enum { sd.variants.iter().find(|v| v.discr == 0).is_some_and(|v| ok(&v.fields)) } else { ok(&sd.fields) }
            }
            _ => !self.needs_drop(t),
        }
    }

    /// Empties the parts of the value of type t at dst that need dropping (t is zeroable): a struct's fields one by
    /// one, so that its other fields keep their values
    pub(crate) fn zero_drop_parts(&mut self, dst: Value, t: Ty) {
        if !self.needs_drop(t) {
            return;
        }
        if let Ty::Struct(sid) = t {
            let sd = &self.prog.structs[sid as usize];
            let bname = &self.prog.structs[self.prog.base_of(sid) as usize].name;
            // (an enum of one variant: its fields, so that a `Copy` one the closure uses keeps its value
            // (`let E::V(n, _) = e;`); the tag stays)
            let fields = if !sd.is_enum {
                Some(sd.fields.clone())
            } else if sd.variants.len() == 1 {
                Some(sd.variants[0].fields.clone())
            } else {
                None
            };
            if let (Some(fs), false) = (fields, matches!(bname.as_str(), "Vec" | "String" | "Rc" | "Arc")) {
                for (_, ft, off) in fs {
                    let d = self.addr_plus(dst, off as i32);
                    self.zero_drop_parts(d, ft);
                }
                return;
            }
        }
        self.zero(dst, t);
    }

    pub(crate) fn zero(&mut self, dst: Value, t: Ty) {
        let (size, _) = self.prog.size_align(t);
        let z = self.b.ins().iconst(types::I8, 0);
        for i in 0..size {
            self.b.ins().store(mf(), z, dst, i as i32);
        }
    }

    /// prelude の Vec<T> のインスタンスなら (ptr のオフセット, len のオフセット, T)
    pub(crate) fn vec_fields(&self, sid: u32) -> Option<(u32, u32, Ty)> {
        let sd = &self.prog.structs[sid as usize];
        let b = sd.base?;
        if self.prog.structs[b as usize].name != "Vec" {
            return None;
        }
        let po = sd.fields.iter().find(|f| f.0 == "ptr")?.2;
        let lo = sd.fields.iter().find(|f| f.0 == "len")?.2;
        Some((po, lo, *sd.args.first()?))
    }

    /// `&Vec<T>` (アドレス a) → `&[T]`、`&String` → `&str` の fat pointer
    pub(crate) fn deref_to_slice(&mut self, a: Value, sid: u32, m: bool) -> Option<(Value, Ty)> {
        if let Some((po, lo, e)) = self.vec_fields(sid) {
            let p = self.b.ins().load(PTR, mf(), a, po as i32);
            let n = self.b.ins().load(types::I64, mf(), a, lo as i32);
            return Some((self.make_fat_v(p, n), Ty::Ref(m, intern(Ty::Slice(intern(e))))));
        }
        let sd = &self.prog.structs[sid as usize];
        if sd.base.is_none() && sd.name == "String" {
            let (_, vt, vo) = sd.fields.iter().find(|f| f.0 == "vec")?.clone();
            let Ty::Struct(vs) = vt else { return None };
            let va = self.addr_plus_v(a, vo);
            let (fat, _) = self.deref_to_slice(va, vs, m)?;
            return Some((fat, Ty::Ref(m, intern(Ty::Str))));
        }
        // (a `Cow<str>` / `Cow<[T]>`: what its Deref gives, the text or the slice it borrows or owns; never `&mut`)
        let base = self.prog.base_of(sid);
        let bname = self.prog.structs[base as usize].name.clone();
        if !m && matches!(bname.as_str(), "__CowStr" | "Cow") && self.prog.index.contains_key(&format!("{bname}::deref")) {
            let targs = self.prog.structs[sid as usize].args.clone();
            let r = Ty::Ref(false, intern(Ty::Struct(sid)));
            let (v, t) = self.call_named(&format!("{bname}::deref"), targs, &[(a, r)], SP).ok()?;
            return Some((v?, t));
        }
        None
    }

    pub(crate) fn addr_plus_v(&mut self, a: Value, off: u32) -> Value {
        self.addr_plus(a, off as i32)
    }

    /// prelude の関数 (ジェネリックなら型引数 targs のインスタンス) を、評価済みの引数で呼ぶ
    /// 受け手の struct の型引数で impl のメソッド (`Rc::<T>::deref`) を値の引数で呼ぶ
    pub(crate) fn call_fn_vals(&mut self, id: u32, args: &[(Value, Ty)], span: Sp) -> R<V> {
        let targs = match args.first().map(|a| self.strip_refs(a.1)) {
            Some(Ty::Struct(sid)) if !self.prog.sigs[id as usize].generics.is_empty() => self.prog.structs[sid as usize].args.clone(),
            _ => vec![],
        };
        let name = self.prog.names[id as usize].clone();
        self.call_named(&name, targs, args, span)
    }

    pub(crate) fn call_named(&mut self, name: &str, targs: Vec<Ty>, args: &[(Value, Ty)], span: Sp) -> R<V> {
        let Some(&id) = self.prog.index.get(name) else {
            return err(span, format!("internal: `{name}` is missing from the prelude"));
        };
        let iid = if targs.is_empty() {
            id
        } else {
            let mut g = self.insts.lock().unwrap();
            let i = g.request(self.prog, id, targs).or_else(|e| err(span, e))?;
            drop(g);
            self.requested.push(i);
            i
        };
        let sig = self.insts.lock().unwrap().sig(self.prog, iid).clone();
        let mut cv = vec![];
        let sret = if is_sret(&sig) {
            let a = self.temp_fresh(sig.ret);
            cv.push(a);
            Some(a)
        } else {
            None
        };
        for (&(v, t), &pt) in args.iter().zip(&sig.params) {
            self.same(span, t, pt)?;
            self.push_arg(&mut cv, v, pt);
        }
        let f = self.fref(iid, &sig);
        let call = self.b.ins().call(f, &cv);
        if let Some(a) = sret {
            return Ok((Some(a), sig.ret));
        }
        let r = self.b.inst_results(call).first().copied();
        Ok((r, sig.ret))
    }

    /// 二項演算の被演算子。比較 (cmp) ならムーブせずに読み、一時値は所有する
    pub(crate) fn operand(&mut self, e: &Expr, hint: Option<Ty>, cmp: bool) -> R<V> {
        if !cmp {
            return self.expr(e, hint);
        }
        // `a[..2] == [1, 2]`: サイズ不定のスライスは参照で比べる (`[T]` の PartialEq の &self)
        if let Expr::Index(_, ix, _) = strip_paren(e) {
            if matches!(&**ix, Expr::Range(..)) {
                let r = self.keep_expr(Expr::Ref(false, Box::new(e.clone())));
                return self.expr(r, None);
            }
        }
        // (`None` などローカル変数でない名前は値として評価する)
        let non_local = matches!(strip_paren(e), Expr::Path(_, p)
            if ident_of(p).is_some_and(|id| self.lookup(id).is_none()));
        if is_place_expr(e) && !non_local {
            let r = self.read_no_move(e)?;
            if self.needs_drop(r.1) || hint.is_none() {
                return Ok(r);
            }
            // (整数リテラルの型合わせなど、期待型の変換が要る場合は通常の評価)
            if hint == Some(r.1) {
                return Ok(r);
            }
            return self.expr(e, hint);
        }
        let r = self.expr(e, hint)?;
        if let (Some(v), t) = r {
            if t != Ty::Never {
                self.own_temp(v, t);
            }
        }
        Ok(r)
    }

    pub(crate) fn strip_refs(&self, mut t: Ty) -> Ty {
        while let Ty::Ref(_, i) = t {
            t = *i;
        }
        t
    }

    /// 値を読むがムーブはしない (deref・呼び出しの対象など)
    pub(crate) fn read_no_move(&mut self, e: &Expr) -> R<V> {
        let se = strip_paren(e);
        if is_place_expr(e) && !(matches!(se, Expr::Path(..)) && (self.fn_item(se).is_some() || self.const_of(se).is_some())) {
            let p = self.place(e)?;
            Ok(self.read(p))
        } else {
            self.expr(e, None)
        }
    }

    /// アドレス a にある一時値を所有する
    pub(crate) fn own_temp_at(&mut self, a: Value, ty: Ty) {
        if !self.needs_drop(ty) {
            return;
        }
        let flag = self.new_flag();
        self.temps.push(Owned { loc: Loc::Ptr(a), ty, flag, depth: 0 });
    }

    /// ローカル変数 (の場所) の drop フラグ
    pub(crate) fn flag_of_loc(&self, loc: Loc) -> Option<StackSlot> {
        self.owned.iter().rev().find(|o| o.loc == loc).map(|o| o.flag)
    }

    /// mark_moved が下ろすフラグ (下ろさずに返す)
    pub(crate) fn moved_flag(&self, e: &Expr) -> Option<StackSlot> {
        let mut e = e;
        loop {
            match e {
                Expr::Paren(p) => e = p,
                Expr::Field(b, _, _) => e = b,
                Expr::Unary(op, x, _) if op == "*" => e = x,
                Expr::Path(_, p) => {
                    let (loc, t) = self.lookup(ident_of(p)?)?;
                    if matches!(t, Ty::Ref(..) | Ty::Ptr(..)) {
                        return None;
                    }
                    return self.flag_of_loc(loc);
                }
                _ => return None,
            }
        }
    }

    /// 式 e (ローカル変数か、そのフィールド) から値をムーブした: 変数のフラグを下ろす
    pub(crate) fn mark_moved(&mut self, e: &Expr) {
        let mut e = e;
        let mut via_field = false;
        loop {
            match e {
                Expr::Paren(p) => e = p,
                // 参照越しのフィールドからはムーブできない (Copy 以外は rustc が弾く)
                Expr::Field(b, _, _) => {
                    via_field = true;
                    e = b
                }
                // `*b` (b: Box<T>) から中身をムーブした: b も drop しない (Box の領域はリークする)
                Expr::Unary(op, x, _) if op == "*" => {
                    via_field = true;
                    e = x
                }
                Expr::Path(_, p) => {
                    let Some(id) = ident_of(p) else { return };
                    if let Some((loc, t)) = self.lookup(id) {
                        if matches!(t, Ty::Ref(..) | Ty::Ptr(..)) {
                            return;
                        }
                        // (Box の中身のフィールドをムーブしたときも Box ごとムーブ済みにする: 領域はリークする)
                        let _ = via_field;
                        if let Some(f) = self.flag_of_loc(loc) {
                            self.clear_flag(f);
                        }
                    }
                    return;
                }
                _ => return,
            }
        }
    }

    /// パターンが match 対象 e から値でムーブした (flag は e が一時値のときのフラグ)
    pub(crate) fn release(&mut self, e: &Expr, flag: Option<StackSlot>) -> R<()> {
        self.deref_move_check(e)?;
        self.release_inner(e, flag)
    }

    /// A move of a field (`s.f`, `s.a.b`) out of a value whose type implements `Drop` (E0509): its `drop` would see
    /// the field moved. A move out of a static (E0507)
    pub(crate) fn drop_move_check(&mut self, e: &Expr) -> R<()> {
        if self.bk.level == 0 {
            return Ok(());
        }
        if let Some(n) = self.drop_owner(e) {
            return err(expr_start(e), format!("cannot move out of type `{n}`, which implements the `Drop` trait (E0509)"));
        }
        // (out of what an overloaded `Deref` gives (`x.field` with x: `Rc<Bar>`, `*w` with w: W: Deref): its
        // `deref` returns a reference, E0507. Not out of a `Box`, which the program owns)
        let mut cur = strip_paren(e);
        loop {
            let (b, fname) = match cur {
                Expr::Field(b, f, _) => (strip_paren(b), Some(f)),
                Expr::Unary(op, b, _) if op == "*" => (strip_paren(b), None),
                _ => break,
            };
            // (through references too: `r.value` with r: `&Rc<S>`)
            if let Some(Ty::Struct(sid)) = self.peek(b).map(|t| self.strip_refs(t)) {
                let through = fname.is_none_or(|f| self.prog.field(sid, f).is_none());
                if through && self.user_deref_sid(b, sid).is_some() {
                    let n = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
                    let msg = if matches!(n.as_str(), "Rc" | "Arc") { format!("cannot move out of an `{n}` (E0507)") } else { format!("cannot move out of dereference of `{n}` (E0507)") };
                    return err(expr_start(e), msg);
                }
            }
            if fname.is_none() {
                break;
            }
            cur = b;
        }
        let mut cur = strip_paren(e);
        while let Expr::Field(b, _, _) = cur {
            cur = strip_paren(b);
        }
        // (a static (or a field of one) moved out: E0507, also by a module's path (`m::BAR`). Not a constant, each
        // of whose uses is a new value)
        if let Expr::Path(None, p) = cur {
            let ci = match ident_of(p) {
                Some(id) if self.lookup(id).is_none() => self.prog.const_index.get(id).copied(),
                Some(_) => None,
                None => self.static_of(cur),
            };
            if let (Some(ci), Some(id)) = (ci, p.segs.last()) {
                if self.prog.consts[ci as usize].stat.is_some() {
                    return err(expr_start(e), format!("cannot move out of static item `{}` (E0507)", id.name));
                }
            }
        }
        Ok(())
    }

    /// The type implementing `Drop` that e (a field, `d.f.g`) is a part of: the innermost such owner's name
    pub(crate) fn drop_owner(&mut self, e: &Expr) -> Option<String> {
        let mut cur = strip_paren(e);
        while let Expr::Field(b, _, _) = cur {
            // (`b.f` with b: Box<D>: a field of the D the box owns)
            let mut t = self.peek(b);
            while let Some(Ty::Box(i)) = t {
                t = Some(*i);
            }
            if let Some(Ty::Struct(sid)) = t {
                if let Some(n) = self.user_drop_name(sid) {
                    return Some(n);
                }
            }
            cur = strip_paren(b);
        }
        None
    }

    /// A pattern that binds by value a part of a value whose type implements `Drop` (`let S { f: s } = ..;`, also
    /// deeper: `S { f: Some(s) }`): E0509 at the scrutinee `sp`, as rustc's "cannot move out of here". `owner`: the
    /// type implementing `Drop` the scrutinee (t) is a part of
    pub(crate) fn drop_pat_check(&mut self, pat: &Pat, t: Ty, sp: Sp, owner: Option<&str>) -> R<()> {
        if let (Some(n), Pat::Ident(false, _, _, _)) = (owner, pat) {
            if unit_variant_ident(self.prog, pat).is_none() && const_lit_pat(self.prog, pat).is_none() && !borrowck::is_copy(self.prog, t, 0) {
                return err(sp, format!("cannot move out of type `{n}`, which implements the `Drop` trait (E0509)"));
            }
        }
        // (`D { f: [s] }`: an array's elements; `rest @ ..` binds the rest by value)
        if let (Pat::Slice(ps), Ty::Array(elem, _)) = (pat, t) {
            for p in ps {
                match p {
                    Pat::Rest => {}
                    Pat::Ident(_, _, _, Some(sub)) if matches!(**sub, Pat::Rest) => self.drop_pat_check(p, t, sp, owner)?,
                    _ => self.drop_pat_check(p, *elem, sp, owner)?,
                }
            }
            return Ok(());
        }
        let Ty::Struct(sid) = t else { return Ok(()) };
        let sd = &self.prog.structs[sid as usize];
        let fields: Vec<(&Pat, Ty)> = match pat {
            Pat::Ident(_, _, _, Some(sub)) => return self.drop_pat_check(sub, t, sp, owner),
            Pat::Or(ps) => {
                for p in ps {
                    self.drop_pat_check(p, t, sp, owner)?;
                }
                return Ok(());
            }
            // (both sides of a `..`: `D(.., s)`)
            Pat::Tuple(ps) if !sd.is_enum => match Self::tuple_fields(ps.iter(), sd.fields.len(), sp) {
                Ok(fs) => fs.into_iter().filter_map(|(i, p)| sd.fields.get(i).map(|f| (p, f.1))).collect(),
                Err(_) => return Ok(()),
            },
            Pat::Struct(path, fs, _) => {
                let defs = if sd.is_enum {
                    let vn = path.segs.last().map(|s| s.name.as_str()).unwrap_or("");
                    match sd.variants.iter().find(|v| v.name == vn) {
                        Some(v) => &v.fields,
                        None => return Ok(()),
                    }
                } else {
                    &sd.fields
                };
                fs.iter().filter_map(|(n, p)| defs.iter().find(|f| f.0 == *n).map(|f| (p, f.1))).collect()
            }
            Pat::TupleStruct(path, ps) => {
                let defs = if sd.is_enum {
                    let vn = path.segs.last().map(|s| s.name.as_str()).unwrap_or("");
                    match sd.variants.iter().find(|v| v.name == vn) {
                        Some(v) => &v.fields,
                        None => return Ok(()),
                    }
                } else {
                    &sd.fields
                };
                // (both sides of a `..`: `D(.., s)`)
                match Self::tuple_fields(ps.iter(), defs.len(), sp) {
                    Ok(fs) => fs.into_iter().filter_map(|(i, p)| defs.get(i).map(|f| (p, f.1))).collect(),
                    Err(_) => return Ok(()),
                }
            }
            _ => return Ok(()),
        };
        let drop = self.user_drop_name(sid);
        let owner = drop.as_deref().or(owner).map(str::to_string);
        for (p, ft) in fields {
            self.drop_pat_check(p, ft, sp, owner.as_deref())?;
        }
        Ok(())
    }

    /// The name of a struct of the program's that implements `Drop` (not the prelude's, whose fields are not seen)
    pub(crate) fn user_drop_name(&self, sid: u32) -> Option<String> {
        let base = self.prog.base_of(sid);
        let prelude = self.prog.struct_item.get(base as usize).is_some_and(|&i| i >= self.prog.prelude_from);
        if prelude || self.prog.closure_of_struct.contains_key(&base) || !self.prog.impls.contains(&(base, "Drop".to_string())) {
            return None;
        }
        Some(self.prog.tn(Ty::Struct(sid)))
    }

    /// a pattern moved a value out of `*rc` (an Rc / a Deref type of the program's): E0507, as `let x = *rc;`
    pub(crate) fn deref_move_check(&mut self, e: &Expr) -> R<()> {
        if let Expr::Unary(op, x, _) = strip_paren(e) {
            if op == "*" && self.bk_on() && self.bk.level >= 1 {
                if let Some(Ty::Struct(sid)) = self.peek(x).map(|t| self.strip_refs(t)) {
                    if self.user_deref_sid(x, sid).is_some() {
                        let n = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
                        let what = if matches!(n.as_str(), "Rc" | "Arc") { format!("an `{n}`") } else { format!("dereference of `{}`", self.prog.tn(Ty::Struct(sid))) };
                        return err(expr_start(e), format!("cannot move out of {what} (E0507)"));
                    }
                }
            }
        }
        Ok(())
    }

    fn release_inner(&mut self, e: &Expr, flag: Option<StackSlot>) -> R<()> {
        // (a pattern moved out of a field of a local (`match a.1 { Some(v) => .. }`): the field becomes "empty" (0), as
        // `let v = a.1;` leaves it, so the owner's drop and a later `a.1 = ..` free nothing twice)
        if let Expr::Field(b, _, _) = strip_paren(e) {
            let mut root: &Expr = b;
            while let Expr::Field(r, _, _) = strip_paren(root) {
                root = r;
            }
            let local = matches!(strip_paren(root), Expr::Path(_, p) if ident_of(p).and_then(|id| self.lookup(id)).is_some_and(|(_, t)| !matches!(t, Ty::Ref(..) | Ty::Ptr(..))));
            if local {
                // (reaching the place again is no access of the program's)
                let on = std::mem::replace(&mut self.bk.on, false);
                let pl = self.place_inner(e);
                self.bk.on = on;
                if let Ok(Place::Mem(a, o, t)) = pl {
                    if self.needs_drop(t) && self.zeroable(t) {
                        let dst = self.addr_plus(a, o);
                        self.zero(dst, t);
                        return Ok(());
                    }
                }
            }
        }
        if is_place_expr(e) {
            self.mark_moved(e);
            // (a pattern moved out of a capture held by value: its slot becomes empty, as for moving the capture
            // out; the values bound were read already)
            if let Expr::Path(_, p) = strip_paren(e) {
                if let Some((loc, t)) = ident_of(p).and_then(|id| self.lookup(id).filter(|&(l, _)| self.is_value_cap(id, l))) {
                    if self.needs_drop(t) && self.zeroable(t) {
                        if let Some(dst) = self.loc_addr(loc) {
                            self.zero(dst, t);
                        }
                    }
                }
            }
        } else if let Some(f) = flag {
            self.clear_flag(f);
        }
        Ok(())
    }

    /// パターンの束縛のスコープ (いちばん内側) を除いて release する:
    /// `let nn = ..; if let Some(nn) = nn { .. }` の match 対象 `nn` は外側の変数 (内側の束縛と同じ名前)
    pub(crate) fn release_outer(&mut self, e: &Expr, flag: Option<StackSlot>) -> R<()> {
        let top = self.scopes.pop();
        let r = self.release(e, flag);
        if let Some(t) = top {
            self.scopes.push(t);
        }
        r
    }

    /// 代入で上書きする前に古い値を drop する。ローカル変数ならフラグを見て、代入後に立て直すフラグを返す
    pub(crate) fn drop_before_assign(&mut self, left: &Expr, p: Place) -> Option<StackSlot> {
        if let Expr::Path(_, pp) = strip_paren(left) {
            if let Some(id) = ident_of(pp) {
                if let Some((loc, ty)) = self.lookup(id) {
                    if let Some(flag) = self.flag_of_loc(loc) {
                        self.emit_owned_drop(Owned { loc, ty, flag, depth: 0 });
                        return Some(flag);
                    }
                }
            }
        }
        // フィールド・`*p`・`a[i]`: 初期化済みなので必ず drop する
        let addr = match p {
            Place::Mem(a, o, _) => self.addr_plus(a, o),
            Place::Var(v, t) => {
                let x = self.b.use_var(v);
                let ss = self.slot(t);
                self.b.ins().stack_store(x, ss, 0);
                self.b.ins().stack_addr(PTR, ss, 0)
            }
            Place::Zst(_) => return None,
        };
        self.drop_in_place(addr, p.ty());
        None
    }

    /// name (bound at loc) is a capture of the closure being lowered, held by value
    pub(crate) fn is_value_cap(&self, name: &str, loc: Loc) -> bool {
        // (the capture's binding is the first of that name: a later `let` shadows it)
        let first = self.scopes.iter().flat_map(|s| s.iter()).find(|b| b.0 == name).map(|b| b.1);
        first == Some(loc) && self.caps.iter().any(|c| c.0 == name && !c.2)
    }

    pub(crate) fn mark_moved_loc(&mut self, loc: Loc) {
        if let Some(f) = self.flag_of_loc(loc) {
            self.clear_flag(f);
        }
    }

    /// フラグが立っていれば drop する
    pub(crate) fn emit_owned_drop(&mut self, o: Owned) {
        let f = self.b.ins().stack_load(types::I8, o.flag, 0);
        let yes = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(f, yes, &[], done, &[]);
        self.b.switch_to_block(yes);
        self.clear_flag(o.flag);
        let addr = match o.loc {
            Loc::Slot(ss) => self.b.ins().stack_addr(PTR, ss, 0),
            // (codegen.rs はスタックの一時値のアドレスを作り直していた: 支配しない枝で作られた値のため。
            //  minclif の値はフレーム上に置かれるので、そのまま使える)
            Loc::Ptr(a) => a,
            Loc::Var(v) => {
                let x = self.b.use_var(v);
                let ss = self.slot(o.ty);
                self.b.ins().stack_store(x, ss, 0);
                self.b.ins().stack_addr(PTR, ss, 0)
            }
            Loc::Zst(_) => self.b.ins().iconst(PTR, 0),
        };
        self.drop_in_place(addr, o.ty);
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
    }

    /// addr にある t の値を drop する (drop glue の呼び出し)
    pub(crate) fn drop_in_place(&mut self, addr: Value, t: Ty) {
        if !self.needs_drop(t) {
            return;
        }
        let Some(&g) = self.prog.index.get(crate::prelude::DROP_GLUE) else {
            return;
        };
        let iid = {
            let mut ins = self.insts.lock().unwrap();
            match ins.request(self.prog, g, vec![t]) {
                Ok(i) => i,
                Err(_) => return,
            }
        };
        self.requested.push(iid);
        let sig = self.insts.lock().unwrap().sig(self.prog, iid).clone();
        let f = self.fref(iid, &sig);
        self.b.ins().call(f, &[addr]);
    }

    /// スコープを閉じる: そのスコープのローカル変数を逆順に drop する
    pub(crate) fn pop_scope(&mut self) {
        self.bk_dead_scope(SP);
        let depth = self.scopes.len() - 1;
        while self.owned.last().is_some_and(|o| o.depth >= depth) {
            let o = self.owned.pop().unwrap();
            self.emit_owned_drop(o);
        }
        self.scopes.pop();
    }

    /// 早期脱出 (return / break / continue): 深さ depth 以上のローカル変数と temps[t..] を drop する
    pub(crate) fn drop_for_exit(&mut self, depth: usize, t: usize) {
        let temps: Vec<Owned> = self.temps[t.min(self.temps.len())..].to_vec();
        for o in temps.into_iter().rev() {
            self.emit_owned_drop(o);
        }
        let locals: Vec<Owned> = self.owned.iter().filter(|o| o.depth >= depth).copied().collect();
        for o in locals.into_iter().rev() {
            self.emit_owned_drop(o);
        }
    }

    /// 文の終わり: temps[t..] を drop して捨てる
    pub(crate) fn drop_temps(&mut self, t: usize) {
        while self.temps.len() > t {
            let o = self.temps.pop().unwrap();
            self.emit_owned_drop(o);
        }
    }

    /// ブロックの末尾の式・match の arm の一時値は、その場で drop する (Rust 2024 と同じ)。
    /// 分岐の中で作った値を文の終わりで触ると支配関係が崩れるため。結果そのものの一時値は残す
    pub(crate) fn drop_tail_temps(&mut self, t: usize, result: V) {
        if result.1 == Ty::Never {
            self.temps.truncate(t.min(self.temps.len()));
            return;
        }
        let keep: Vec<Owned> =
            self.temps[t.min(self.temps.len())..].iter().filter(|o| result.0.is_some_and(|v| o.loc == Loc::Ptr(v))).copied().collect();
        let rest: Vec<Owned> = self.temps.drain(t.min(self.temps.len())..).collect();
        for o in rest.into_iter().rev() {
            if !keep.iter().any(|k| k.loc == o.loc) {
                self.emit_owned_drop(o);
            }
        }
        self.temps.extend(keep);
    }

    /// let の初期化式の一時値は、ブロックの終わりまで延ばす (rustc の一時値の寿命延長の近似)
    pub(crate) fn extend_temps(&mut self, t: usize) {
        let depth = self.scopes.len() - 1;
        let ts: Vec<Owned> = self.temps.drain(t.min(self.temps.len())..).collect();
        for mut o in ts {
            o.depth = depth;
            self.owned.push(o);
        }
    }

    pub(crate) fn free(&mut self, p: Value) {
        if let Some(&fr) = self.prog.index.get(crate::prelude::BOX_FREE) {
            let sig = self.prog.sigs[fr as usize].clone();
            let f = self.fref(fr, &sig);
            self.b.ins().call(f, &[p]);
        }
    }

    /// vtable の中で drop glue が入っている位置 (メソッドの後ろ)
    pub(crate) fn vtable_drop_index(&self, tr: &str) -> usize {
        // (the trait's methods and its supertraits', as the vtable holds them)
        if crate::ast_summary::parse_fn_bound(tr).is_some() {
            1
        } else if self.prog.traits.contains_key(tr) {
            self.prog.dyn_methods(tr).len()
        } else {
            let base = tr.split('<').next().unwrap_or(tr).trim();
            self.prog.dyn_methods(base).len()
        }
    }

    /// vtable の drop の欄: t の drop glue (要らなければ u32::MAX = null)
    pub(crate) fn vtable_drop_entry(&mut self, t: Ty) -> u32 {
        if !self.needs_drop(t) {
            return u32::MAX;
        }
        let Some(&g) = self.prog.index.get(crate::prelude::DROP_GLUE) else {
            return u32::MAX;
        };
        let mut ins = self.insts.lock().unwrap();
        match ins.request(self.prog, g, vec![t]) {
            Ok(i) => {
                drop(ins);
                self.requested.push(i);
                i
            }
            Err(_) => u32::MAX,
        }
    }

    /// `__exr_drop_in_place::<T>(p: *mut T)` の本体
    pub(crate) fn drop_glue_body(&mut self, sig: &FnSig) -> R<()> {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        self.b.seal_block(entry);
        let p = self.b.func_params()[0];
        let Some(Ty::Ptr(_, t)) = sig.params.first() else {
            return Err("internal: bad drop glue signature".into());
        };
        let t = **t;
        match t {
            Ty::Box(Ty::Dyn(tr)) => {
                // 中身の型は vtable の drop 欄が知っている。data が 0 ならムーブ済み
                let data = self.b.ins().load(PTR, mf(), p, 0);
                let live = self.b.create_block();
                let out = self.b.create_block();
                self.b.ins().brif(data, live, &[], out, &[]);
                self.b.switch_to_block(live);
                let vt = self.b.ins().load(PTR, mf(), p, 8);
                let idx = self.vtable_drop_index(tr);
                let dfn = self.b.ins().load(PTR, mf(), vt, 8 * idx as i32);
                let call = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().brif(dfn, call, &[], done, &[]);
                self.b.switch_to_block(call);
                let mut s = Signature::default();
                s.params.push(AbiParam::new(PTR));
                let sr = self.b.import_signature(s);
                self.b.ins().call_indirect(sr, dfn, &[data]);
                self.b.ins().jump(done, &[]);
                self.b.switch_to_block(done);
                self.free(data);
                self.b.ins().jump(out, &[]);
                self.b.switch_to_block(out);
            }
            Ty::Box(Ty::Slice(e)) => {
                // (pointer, length): each element, then the allocation. A null pointer is moved out
                let ptr = self.b.ins().load(PTR, mf(), p, 0);
                let live = self.b.create_block();
                let out = self.b.create_block();
                self.b.ins().brif(ptr, live, &[], out, &[]);
                self.b.switch_to_block(live);
                if self.needs_drop(**e) {
                    let n = self.b.ins().load(types::I64, mf(), p, 8);
                    let (size, _) = self.prog.size_align(**e);
                    let i = self.b.declare_var(types::I64);
                    let z = self.b.ins().iconst(types::I64, 0);
                    self.b.def_var(i, z);
                    let head = self.b.create_block();
                    let body = self.b.create_block();
                    let done = self.b.create_block();
                    self.b.ins().jump(head, &[]);
                    self.b.switch_to_block(head);
                    let iv = self.b.use_var(i);
                    let more = self.b.ins().icmp(IntCC::UnsignedLessThan, iv, n);
                    self.b.ins().brif(more, body, &[], done, &[]);
                    self.b.switch_to_block(body);
                    let off = self.b.ins().imul_imm_s(iv, size as i64);
                    let a = self.b.ins().iadd(ptr, off);
                    self.drop_in_place(a, **e);
                    let next = self.b.ins().iadd_imm_s(iv, 1);
                    self.b.def_var(i, next);
                    self.b.ins().jump(head, &[]);
                    self.b.switch_to_block(done);
                }
                self.free(ptr);
                self.b.ins().jump(out, &[]);
                self.b.switch_to_block(out);
            }
            Ty::Box(inner) => {
                let ptr = self.b.ins().load(PTR, mf(), p, 0);
                let live = self.b.create_block();
                let out = self.b.create_block();
                self.b.ins().brif(ptr, live, &[], out, &[]);
                self.b.switch_to_block(live);
                self.drop_in_place(ptr, *inner);
                self.free(ptr);
                self.b.ins().jump(out, &[]);
                self.b.switch_to_block(out);
            }
            Ty::Array(e, n) => {
                let (size, _) = self.prog.size_align(*e);
                // 要素ごとに drop (長さは静的に分かるので展開しないループ)
                let i = self.b.declare_var(types::I64);
                let z = self.b.ins().iconst(types::I64, 0);
                self.b.def_var(i, z);
                let head = self.b.create_block();
                let body = self.b.create_block();
                let done = self.b.create_block();
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(head);
                let iv = self.b.use_var(i);
                let more = self.b.ins().icmp_imm_u(IntCC::UnsignedLessThan, iv, n as i64);
                self.b.ins().brif(more, body, &[], done, &[]);
                self.b.switch_to_block(body);
                let off = self.b.ins().imul_imm_s(iv, size as i64);
                let a = self.b.ins().iadd(p, off);
                self.drop_in_place(a, *e);
                let next = self.b.ins().iadd_imm_s(iv, 1);
                self.b.def_var(i, next);
                self.b.ins().jump(head, &[]);
                self.b.switch_to_block(done);
            }
            Ty::Struct(sid) => {
                // 先に `impl Drop` の drop(&mut self)、それからフィールド
                if self.prog.drop_impls.contains(&self.prog.base_of(sid)) {
                    if let Some(m) = self.prog.method(sid, "drop") {
                        let recv = [Ty::Ref(true, intern(t))];
                        let mut fid = m;
                        let ms = &self.prog.sigs[m as usize];
                        if ms.is_generic() {
                            let mut sol = vec![None; ms.generics.len()];
                            if let Some(st) = ms.self_ty {
                                self.prog.match_ty(st, t, &mut sol);
                            }
                            fid = self.resolve_call(m, &recv, sol, None, SP)?.0;
                        }
                        let fs = self.insts.lock().unwrap().sig(self.prog, fid).clone();
                        let f = self.fref(fid, &fs);
                        self.b.ins().call(f, &[p]);
                    }
                }
                let sd = &self.prog.structs[sid as usize];
                if sd.is_enum {
                    let tag_ty = sd.tag;
                    let vs: Vec<(i64, Vec<(Ty, u32)>)> = sd
                        .variants
                        .iter()
                        .map(|v| (v.discr, v.fields.iter().filter(|f| self.needs_drop(f.1)).map(|f| (f.1, f.2)).collect()))
                        .collect();
                    let tag = self.b.ins().load(clif_ty(tag_ty).unwrap(), mf(), p, 0);
                    for (d, fs) in vs {
                        if fs.is_empty() {
                            continue;
                        }
                        let yes = self.b.create_block();
                        let next = self.b.create_block();
                        let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, d);
                        self.b.ins().brif(c, yes, &[], next, &[]);
                        self.b.switch_to_block(yes);
                        for (ft, off) in fs {
                            let a = self.addr_plus(p, off as i32);
                            self.drop_in_place(a, ft);
                        }
                        self.b.ins().jump(next, &[]);
                        self.b.switch_to_block(next);
                    }
                } else {
                    let fs: Vec<(Ty, u32)> = sd.fields.iter().filter(|f| self.needs_drop(f.1)).map(|f| (f.1, f.2)).collect();
                    for (ft, off) in fs {
                        let a = self.addr_plus(p, off as i32);
                        self.drop_in_place(a, ft);
                    }
                }
            }
            _ => {}
        }
        self.b.ins().return_(&[]);
        Ok(())
    }
}

fn has_or(p: &Pat) -> bool {
    match p {
        Pat::Or(_) => true,
        Pat::Ident(_, _, _, Some(q)) | Pat::Ref(_, q) => has_or(q),
        Pat::Tuple(v) | Pat::TupleStruct(_, v) | Pat::Slice(v) => v.iter().any(has_or),
        Pat::Struct(_, fs, _) => fs.iter().any(|f| has_or(&f.1)),
        _ => false,
    }
}

/// The alternatives of a pattern with or-patterns inside, left to right (`(a | b, c | d)`: `(a, c)`, `(a, d)`, ..)
fn or_alts(p: &Pat) -> Vec<Pat> {
    // (the product of the parts' alternatives, the first part varying slowest)
    fn product(parts: &[Pat]) -> Vec<Vec<Pat>> {
        let mut out: Vec<Vec<Pat>> = vec![vec![]];
        for q in parts {
            let alts = or_alts(q);
            let mut next = vec![];
            for o in &out {
                for a in &alts {
                    let mut v = o.clone();
                    v.push(a.clone());
                    next.push(v);
                }
            }
            out = next;
            if out.len() > 64 {
                break;
            }
        }
        out
    }
    match p {
        Pat::Or(v) => v.iter().flat_map(or_alts).collect(),
        Pat::Ident(r, m, n, Some(q)) => or_alts(q).into_iter().map(|a| Pat::Ident(*r, *m, n.clone(), Some(Box::new(a)))).collect(),
        Pat::Ref(m, q) => or_alts(q).into_iter().map(|a| Pat::Ref(*m, Box::new(a))).collect(),
        Pat::Tuple(v) => product(v).into_iter().map(Pat::Tuple).collect(),
        Pat::TupleStruct(path, v) => product(v).into_iter().map(|x| Pat::TupleStruct(path.clone(), x)).collect(),
        Pat::Slice(v) => product(v).into_iter().map(Pat::Slice).collect(),
        Pat::Struct(path, fs, rest) => {
            let ps: Vec<Pat> = fs.iter().map(|f| f.1.clone()).collect();
            product(&ps)
                .into_iter()
                .map(|x| Pat::Struct(path.clone(), fs.iter().map(|f| f.0.clone()).zip(x).collect(), *rest))
                .collect()
        }
        p => vec![p.clone()],
    }
}

/// a number literal without a suffix, or a tuple of them (`(0, 0)`): its type is not fixed by itself
fn lit_scrutinee(e: &Expr) -> bool {
    match strip_paren(e) {
        e @ (Expr::Lit(..) | Expr::Unary(..)) => lit_suffix(e) == Some(false),
        Expr::Tuple(xs) => !xs.is_empty() && xs.iter().all(lit_scrutinee),
        _ => false,
    }
}

/// a range pattern's start as a pattern of its own (`usize::MIN..=5`: `usize::MIN`)
fn range_end_pat(e: &Expr) -> Option<Pat> {
    match strip_paren(e) {
        Expr::Path(None, p) => Some(Pat::Path(p.clone())),
        e @ Expr::Lit(..) => Some(Pat::Lit(Box::new(e.clone()))),
        _ => None,
    }
}

/// a number literal (or its negation): whether it has a type suffix. None: not a number literal
fn lit_suffix(e: &Expr) -> Option<bool> {
    match strip_paren(e) {
        Expr::Lit(t, _) => match super::lit::lit(t)? {
            super::lit::Lit::Int(i) => Some(!i.suffix().is_empty()),
            super::lit::Lit::Float(f) => Some(!f.suffix().is_empty()),
            _ => None,
        },
        Expr::Unary(op, x, _) if op == "-" => lit_suffix(x),
        _ => None,
    }
}

/// A closure in e (a match guard) whose body moves one of names out: that capture, and where it moves
fn guard_closure_move(e: &Expr, names: &[String], by: crate::ast_closure::MovesBy) -> Option<(String, crate::parse::Pos)> {
    if names.is_empty() {
        return None;
    }
    // (a `move` closure takes what it names by value when it is made: one of names it names is moved there, even if
    // its body only borrows it)
    if let Expr::Closure(true, _, _, _, at) = e {
        if let Some(n) = crate::ast_closure::free_vars(e).into_iter().find(|n| names.contains(n)) {
            return Some((n, *at));
        }
    }
    if let Expr::Closure(..) = e {
        return crate::ast_closure::move_site_by(e, names, by);
    }
    // (a `let` in a block of the guard binds a name again: past it, the name is not the arm's binding)
    if let Expr::Block(_, _, b) = e {
        let mut cur = names.to_vec();
        for st in &b.stmts {
            match st {
                crate::parse::Stmt::Let(p, _, init, els, ..) => {
                    if let Some(f) = init.as_ref().and_then(|x| guard_closure_move(x, &cur, by)) {
                        return Some(f);
                    }
                    if let Some(f) = els.as_ref().and_then(|eb| guard_closure_move(&Expr::Block(None, false, eb.clone()), &cur, by)) {
                        return Some(f);
                    }
                    let mut bound = vec![];
                    crate::ast_closure::pat_idents(p, &mut bound);
                    cur.retain(|n| !bound.contains(n));
                }
                crate::parse::Stmt::Expr(x, _) => {
                    if let Some(f) = guard_closure_move(x, &cur, by) {
                        return Some(f);
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    let mut found = None;
    crate::ast_closure::each_child_pub(e, &mut |c| {
        if found.is_none() {
            found = guard_closure_move(c, names, by);
        }
    });
    found
}
