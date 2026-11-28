use super::*;
use crate::parse::{Block as PBlock, Expr, Pat, Path, Stmt};
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
    /// The scrutinee's place for patterns `pats` (bk_scrutinee): a mutable place when one of them binds by `ref mut`, so
    /// that a scrutinee through a user `Deref` (`match *b { ref mut n => .. }`, `let ref mut z = *y;`) calls `deref_mut`
    pub(crate) fn scrut_place(&mut self, e: &Expr, expected: Option<Ty>, pats: &[&Pat]) -> R<Option<Place>> {
        // (only a place reached through something (`*b`, `p.x`) can be through a Deref: a local's pattern is not read)
        if !matches!(strip_paren(e), Expr::Path(..)) && is_place_expr(e) && pats.iter().any(|p| binds_ref_mut(p)) {
            self.scrut_mut = true;
        }
        let r = self.bk_scrutinee(e, expected, pats);
        self.scrut_mut = false;
        r
    }

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
            let m = self.bk.place_mut || std::mem::take(&mut self.scrut_mut);
            let pm = std::mem::replace(&mut self.bk.place_mut, m);
            let p = self.place(e);
            self.bk.place_mut = pm;
            let p = p?;
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
        // (arms that are distinct closures with no captures and one signature, with nothing expected: rustc coerces
        // them to that fn pointer)
        // (inference may have given the match its first arm's closure type: that is no fn pointer either)
        let lub = || self.closures_lub(&arms.iter().map(|a| &a.body).collect::<Vec<_>>());
        let lubbed = match expected {
            None => lub(),
            Some(Ty::Struct(s)) if self.prog.closure_of_struct.contains_key(&self.prog.base_of(s)) => lub(),
            _ => None,
        };
        if lubbed.is_some() {
            self.lub_fnptr += 1;
            let r = self.match_(m, lubbed);
            self.lub_fnptr -= 1;
            return r;
        }
        // (E0408: before anything of the arms' types, as rustc's resolver checks it, and before an arm with a guard
        // becomes one arm per alternative)
        if self.user_code {
            for a in arms.iter().filter(|a| has_or(&a.pat)) {
                self.or_pat_names(&a.pat, &mut vec![])?;
            }
        }
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
                spos: Default::default(),
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
            // arms' patterns and guards, which the call's own arguments do not give; a method's too, `match
            // js.try_decode() { Ok(d) => d, .. }` with `fn try_decode<T>(self) -> Result<Box<[T]>, &str>`)
            e @ (Expr::Call(..) | Expr::MethodCall(..)) if self.lets.contains_key(&(e as *const Expr as usize)) => {
                let t = self.lets[&(e as *const Expr as usize)];
                (!self.prog.has_param(t) && !has_hole(self.prog, t)).then_some(t)
            }
            _ => self.bare_none_ty(scrut, &pats)?,
        };
        let Some(place) = self.scrut_place(scrut, scrut_exp, &pats)? else {
            return Ok((None, Ty::Never));
        };
        let scrut_drop = self.bk.scrut_drop.take();
        // (E0004: the arms must cover the scrutinee's type; user code only)
        if self.user_code {
            // (first E0170: such a binding is a catch-all arm in rustc, which zustc reads as the variant)
            if let Some(e) = self.variant_binding(arms, place.ty()) {
                return Err(e);
            }
            if let Some(w) = self.non_exhaustive(arms, place.ty()) {
                // (no arms at all: rustc names the type, `match () {}`)
                if arms.is_empty() {
                    let t = self.prog.tn(place.ty()).replace('#', "");
                    return Err(self.at(expr_start(scrut), format!("non-exhaustive patterns: type `{t}` is non-empty (E0004)")));
                }
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
        // (the variant every arm that goes on matched: Some(None) when they differ or one matches any. Only of an enum
        // in a local that is dropped)
        let mut on_vars: Option<Option<(u32, usize)>> = None;
        let track = matches!(strip_paren(scrut), Expr::Path(..))
            && matches!(place.ty(), Ty::Struct(sid) if self.prog.structs[sid as usize].is_enum)
            && self.moved_root(scrut).is_some();
        // (the scrutinee's flag, for a mark in known_var during each arm's body: an assignment to the scrutinee or a
        // `&mut` of it there (forget_variant) removes it, and that arm's variant is not known after it)
        let track_f = if track { self.moved_root(scrut).map(|r| r.2) } else { None };
        const MARK: (u32, usize) = (u32::MAX, usize::MAX);
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
            let pm = self.bk.pat_moves;
            let bk_l0 = self.bk.locals.len() as u32;
            self.bk.scrut = bk_ss.clone();
            self.bk.scrut_place = bk_sp.clone();
            self.rg.scrut = rg_ss.clone();
            self.bk_bind_begin();
            self.bk.guard_arm = arm.guard.is_some();
            self.bk.arm_at = Some(arm.pos).filter(|p| p.line != 0);
            let pv0 = (self.pat_moved.len(), self.pat_vars.len());
            let r = self.pat_bind(&arm.pat, place, BindMode::Move);
            self.bk.arm_at = None;
            self.bk.guard_arm = false;
            self.bk_bind_end();
            r?;
            if self.owned_binds > ob {
                self.rel_parts = self.pat_parts(place, pv0);
                let r = self.release_outer(scrut, sflag);
                self.rel_parts = None;
                r?;
            } else if self.bk.pat_moves > pm {
                self.deref_move_check(scrut)?;
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
                // (a `ref mut` binding is a shared reference in the guard: assigning through it or borrowing it
                // mutably there, in a closure too, is E0594 / E0596)
                let mut rm = vec![];
                ref_mut_names(&arm.pat, &mut rm);
                if let Some((at, n, assign)) = guard_mut_use(g, &rm) {
                    let msg = if assign { format!("cannot assign to `*{n}`, as it is immutable for the pattern guard (E0594)") } else { format!("cannot borrow `*{n}` as mutable, as it is immutable for the pattern guard (E0596)") };
                    return Err(self.at(at, msg));
                }
            }
            // (the locals the arm's pattern bound, which the guard sees by shared reference: see bk_ev)
            // (pushed on those of the guards it is in: theirs are seen so in it too)
            let guard_saved = if arm.guard.is_some() && self.bk_on() {
                let hi = self.bk.locals.len() as u32;
                let n = self.bk.guard_binds.len();
                self.bk.guard_binds.push((bk_l0, hi));
                Some(n)
            } else {
                None
            };
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
                        let Some(lplace) = ({ let ex = self.bare_none_ty(lexpr, &[&**lpat])?; self.scrut_place(lexpr, ex, &[&**lpat])? }) else {
                            break;
                        };
                        // (a pattern that is not of the value's type: at the pattern, `Ok(x) if let Err(_) = x`)
                        self.pat_test(lpat, lplace, next).map_err(|m| self.at(crate::parse::pat_pos(lpat), m))?;
                        self.bk_bind_begin();
                        let r = self.pat_bind(lpat, lplace, BindMode::Move);
                        self.bk_bind_end();
                        r?;
                        continue;
                    }
                    // (a guard is a terminating scope: its temporaries go away before the arm or the next one runs, also
                    // their drops: `_ if D(&x).ok() => { x = 2; }` drops the D before the arm, seeing x = 1)
                    let (t0, bk0) = (self.temps.len(), self.bk.stmt_temps.len());
                    let (c, ct) = self.expr(c, Some(Ty::Bool))?;
                    self.same(SP, ct, Ty::Bool)?;
                    if c.is_some() {
                        self.drop_temps(t0);
                    }
                    self.bk_temps_end(bk0);
                    if let Some(c) = c {
                        self.require(c, next);
                    }
                }
            }
            if let Some(n) = guard_saved {
                self.bk.guard_binds.truncate(n);
                if let Some((sp, m)) = self.bk.guard_err.take() {
                    return Err(self.at(sp, m));
                }
            }
            if guard_place.is_some() {
                self.bk_ev(borrowck::Ev::Guard(None));
            }
            // (an arm that takes the whole scrutinee moves it: its drop holds nothing on this arm's path)
            if let Some(id) = scrut_drop.filter(|_| self.takes_whole(&arm.pat)) {
                self.bk_ev(borrowck::Ev::Def(id, vec![]));
            }
            let t0 = self.temps.len();
            if let Some(f) = track_f {
                self.known_var.push((f, MARK.0, MARK.1));
            }
            let (v, t) = self.expr(&arm.body, result_ty.or(expected))?;
            let kept = track_f.and_then(|f| self.known_var.iter().position(|k| *k == (f, MARK.0, MARK.1)));
            if let Some(i) = kept {
                self.known_var.remove(i);
            }
            if track && t != Ty::Never {
                let var = kept.and_then(|_| self.pat_variant(&arm.pat, place.ty()));
                on_vars = match on_vars {
                    Some(Some(x)) if var != Some(x) => Some(None),
                    Some(x) => Some(x),
                    None => Some(var),
                };
            }
            // (the fn's tail: what an arm returns by naming one of its bindings is returned here, while the binding is
            // in scope (`S(&mut ref mut r) => r`: E0713 through a `Drop` value); after the match, the return of the
            // match no longer finds it)
            if self.bk.tail == m as *const Expr as usize && self.bk_on() && self.bk_ty(self.ret).1 {
                self.bk_ret(&arm.body);
            }
            self.rg_keep(&arm.body);
            self.drop_tail_temps(t0, (v, t));
            self.pop_scope();
            // (an arm whose type is not that of the arms before it: at its body's tail, as rustc points; an empty
            // block's, `None => {}`, at the arm)
            let at = Some(branch_pos(&arm.body)).filter(|p| p.line != 0).unwrap_or(arm.pos);
            self.finish_arm(v, t, merge, &mut result_ty, &mut param, at)?;
            self.b.switch_to_block(next);
        }
        // どの arm にも合わない (rustc が網羅性を保証するので到達しない)
        self.b.ins().trap(TrapCode::unwrap_user(2));
        self.b.switch_to_block(merge);
        if let Some(Some((sid, vi))) = on_vars {
            if let Some((_, _, f)) = self.moved_root(scrut) {
                self.known_var.retain(|k| k.0 != f);
                self.known_var.push((f, sid, vi));
            }
        }
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
        let Some(place) = ({ let ex = self.bare_none_ty(lexpr, &[&**lpat])?; self.scrut_place(lexpr, ex, &[&**lpat])? }) else {
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
        let pm = self.bk.pat_moves;
        self.bk_bind_begin();
        let pv0 = (self.pat_moved.len(), self.pat_vars.len());
        let r = self.pat_bind(lpat, place, BindMode::Move);
        self.bk_bind_end();
        r?;
        if self.owned_binds > ob {
            self.rel_parts = self.pat_parts(place, pv0);
            let r = self.release_outer(lexpr, sflag);
            self.rel_parts = None;
            r?;
        } else if self.bk.pat_moves > pm {
            self.deref_move_check(lexpr)?;
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
        let at = else_branch.as_deref().map_or(SP, branch_pos);
        self.finish_arm(ev, et, merge, &mut result_ty, &mut param, at)?;
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
        self.presplit_loop(Some(e), body);
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
        let bk_t0 = self.bk.stmt_temps.len();
        let Some(place) = ({ let ex = self.bare_none_ty(e, &[pat])?; self.scrut_place(e, ex, &[pat])? }) else {
            return Ok((None, Ty::Never));
        };
        let sflag = self.last_scrut;
        self.pat_test(pat, place, exit)?;
        self.scopes.push(vec![]);
        let ob = self.owned_binds;
        let pm = self.bk.pat_moves;
        self.bk_bind_begin();
        let pv0 = (self.pat_moved.len(), self.pat_vars.len());
        let r = self.pat_bind(pat, place, BindMode::Move);
        self.bk_bind_end();
        r?;
        if self.owned_binds > ob {
            self.rel_parts = self.pat_parts(place, pv0);
            let r = self.release_outer(e, sflag);
            self.rel_parts = None;
            r?;
        } else if self.bk.pat_moves > pm {
            self.deref_move_check(e)?;
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
        // (the scrutinee's temporaries and the body's go away at the end of each iteration; on the way out, those of
        // the last test stay the statement's)
        let last: Vec<u32> = self.bk.stmt_temps.get(bk_t0..).map(|t| t.to_vec()).unwrap_or_default();
        if bt != Ty::Never {
            // (a body with a value: at its tail, as rustc points)
            self.same(block_end_pos(body), bt, Ty::Unit)?;
            self.drop_for_exit(usize::MAX, t0);
            self.bk_iteration_end(bk_t0);
            self.b.ins().jump(header, &[]);
        } else {
            self.b.ins().trap(TrapCode::unwrap_user(1));
        }
        self.bk.stmt_temps.truncate(bk_t0);
        self.bk.stmt_temps.extend(last);
        self.b.switch_to_block(exit);
        // (the loop is a terminating scope: those of the last test go away with it, before the locals of a block
        // whose tail it is)
        self.bk_temps_end(bk_t0);
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

    /// Where an error of a `x @ sub` pattern is reported: at the arm, at the `let`'s pattern, or at the scrutinee (the
    /// pattern has no position of its own)
    pub(crate) fn at_pat_pos(&self) -> Sp {
        self.at_pat_at.or(self.bk.arm_at).or(self.bk.scrut_place.as_ref().map(|x| x.1)).filter(|s| s.line != 0).unwrap_or(SP)
    }

    /// A `ref mut` binding where the default binding mode is by shared reference (`if let Some(ref mut v) = o` with
    /// o: `&Option<i32>`): a mutable borrow through the `&`, rustc's E0596 at the binding (edition 2024 also rejects
    /// the `ref mut` written there). At the arm, or at the scrutinee
    pub(crate) fn ref_mut_through_shared(&mut self) -> String {
        let at = self.bk.arm_at.or(self.bk.scrut_place.as_ref().map(|x| x.1)).filter(|s| s.line != 0).unwrap_or(SP);
        self.at(at, "cannot borrow data in a `&` reference as mutable (E0596)".to_string())
    }

    /// 参照に対して参照でないパターンを当てるときは自動で deref し、束縛モードを Ref にする
    pub(crate) fn pat_deref(&mut self, mut p: Place, mut mode: BindMode) -> (Place, BindMode) {
        loop {
            let (ptr, m, inner) = match p {
                // (a reference to a struct with an unsized tail: the place of its pair is the struct's)
                Place::Var(v, Ty::Ref(m, i)) if i.is_dst_struct() => (self.b.use_var(v), m, *i),
                Place::Mem(a, o, Ty::Ref(m, i)) if i.is_dst_struct() => (self.addr_plus(a, o), m, *i),
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

    /// A field (type fty, offset off) of a struct with an unsized tail, whose place is its pair at a + o: a sized
    /// field is at the data address + off; the tail is a pair of its own (its address, the same length or vtable).
    /// A tail that is (or ends in) `dyn Tr` is placed by the concrete type's alignment, which the vtable holds
    pub(crate) fn dst_field(&mut self, a: Value, o: i32, fty: Ty, off: u32) -> Place {
        let data = self.b.ins().load(PTR, mf(), a, o);
        if !fty.is_unsized() {
            return Place::Mem(data, off as i32, fty);
        }
        let meta = self.b.ins().load(types::I64, mf(), a, o + 8);
        let p = self.dst_tail_addr(data, meta, fty, off);
        Place::Mem(self.make_fat_v(p, meta), 0, fty)
    }

    /// The address of an unsized tail (type fty, static offset off) of the struct at data, with meta the pair's
    /// second word: a `dyn` tail's offset is rounded up to its type's alignment, from the vtable
    fn dst_tail_addr(&mut self, data: Value, meta: Value, fty: Ty, off: u32) -> Value {
        match self.tail_dyn_align(meta, fty) {
            // (off rounded up to the alignment: (off + al - 1) & -al)
            Some(al) => {
                let x = self.b.ins().iadd_imm(al, off as i64 - 1);
                let neg = self.b.ins().ineg(al);
                let off = self.b.ins().band(x, neg);
                self.b.ins().iadd(data, off)
            }
            None => self.b.ins().iadd_imm(data, off as i64),
        }
    }

    /// The size and alignment of a value of the unsized type t, with meta its pointer's second word (a length or a
    /// vtable): a struct's tail is placed at its offset rounded up to the tail's alignment, and the whole is rounded
    /// up to the struct's alignment
    pub(crate) fn unsized_size_align(&mut self, meta: Value, t: Ty) -> (Value, Value) {
        match t {
            Ty::Slice(e) => {
                let (es, ea) = self.prog.size_align(*e);
                let size = self.b.ins().imul_imm_s(meta, es as i64);
                (size, self.b.ins().iconst(types::I64, ea as i64))
            }
            Ty::Str => (meta, self.b.ins().iconst(types::I64, 1)),
            Ty::Dyn(tr) => {
                let idx = self.vtable_drop_index(tr.as_str());
                let size = self.b.ins().load(types::I64, mf(), meta, 8 * (idx as i32 + 1));
                let align = self.b.ins().load(types::I64, mf(), meta, 8 * (idx as i32 + 2));
                (size, align)
            }
            Ty::Struct(s) if t.is_dst_struct() => {
                let (tt, toff, sa) = {
                    let d = &self.prog.structs[s as usize];
                    let Some(&(_, tt, toff)) = d.fields.last() else { return (self.b.ins().iconst(types::I64, 0), self.b.ins().iconst(types::I64, 1)) };
                    (tt, toff, d.align)
                };
                let (ts, ta) = self.unsized_size_align(meta, tt);
                let sa = self.b.ins().iconst(types::I64, sa as i64);
                let align = self.b.ins().umax(ta, sa);
                let round = |me: &mut Self, x: Value, a: Value| {
                    let y = me.b.ins().iadd(x, a);
                    let y = me.b.ins().iadd_imm(y, -1);
                    let n = me.b.ins().ineg(a);
                    me.b.ins().band(y, n)
                };
                let toff = self.b.ins().iconst(types::I64, toff as i64);
                let off = round(self, toff, ta);
                let end = self.b.ins().iadd(off, ts);
                (round(self, end, align), align)
            }
            _ => {
                let (sz, al) = self.prog.size_align(t);
                (self.b.ins().iconst(types::I64, sz as i64), self.b.ins().iconst(types::I64, al as i64))
            }
        }
    }

    /// Drops the fields of the struct t with an unsized tail, at data, with meta its pair's second word (the
    /// contents of a `Box<Foo<[T]>>` / `Box<Foo<dyn Tr>>`)
    fn drop_dst_struct(&mut self, data: Value, meta: Value, t: Ty) -> R<()> {
        let Ty::Struct(s) = t else { return Ok(()) };
        // (first its own `impl Drop`, with `&mut self` the (data, metadata) pair, then its fields)
        if self.prog.drop_impls.contains(&self.prog.base_of(s)) {
            if let Some(m) = self.prog.method(s, "drop") {
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
                let pair = self.make_fat_v(data, meta);
                self.b.ins().call(f, &[pair]);
            }
        }
        let fs = self.prog.structs[s as usize].fields.clone();
        let Some(((_, tt, toff), prefix)) = fs.split_last() else { return Ok(()) };
        for &(_, ft, off) in prefix {
            if self.needs_drop(ft) {
                let a = self.b.ins().iadd_imm(data, off as i64);
                self.drop_in_place(a, ft);
            }
        }
        let tail = self.dst_tail_addr(data, meta, *tt, *toff);
        match *tt {
            Ty::Slice(e) if self.needs_drop(*e) => self.drop_elems(tail, meta, *e),
            Ty::Dyn(tr) => self.vtable_drop_call(meta, tr.as_str(), tail),
            t if t.is_dst_struct() => self.drop_dst_struct(tail, meta, t)?,
            _ => {}
        }
        Ok(())
    }

    /// Calls the drop entry of the vtable vt (trait tr) on data, if it has one
    fn vtable_drop_call(&mut self, vt: Value, tr: &str, data: Value) {
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
    }

    /// Drops n elements of type e from ptr
    fn drop_elems(&mut self, ptr: Value, n: Value, e: Ty) {
        let (size, _) = self.prog.size_align(e);
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
        self.drop_in_place(a, e);
        let next = self.b.ins().iadd_imm_s(iv, 1);
        self.b.def_var(i, next);
        self.b.ins().jump(head, &[]);
        self.b.switch_to_block(done);
    }

    /// The alignment of an unsized tail t that is (or ends in) `dyn Tr`, from the vtable vt; None for a slice tail
    /// (its alignment is known statically)
    fn tail_dyn_align(&mut self, vt: Value, t: Ty) -> Option<Value> {
        match t {
            Ty::Dyn(tr) => {
                let idx = self.vtable_drop_index(tr.as_str()) + 2;
                Some(self.b.ins().load(types::I64, mf(), vt, 8 * idx as i32))
            }
            Ty::Struct(s) if t.is_dst_struct() => {
                let (inner, sa) = {
                    let d = &self.prog.structs[s as usize];
                    (d.fields.last()?.1, d.align)
                };
                let a = self.tail_dyn_align(vt, inner)?;
                let sa = self.b.ins().iconst(types::I64, sa as i64);
                Some(self.b.ins().umax(a, sa))
            }
            _ => None,
        }
    }

    /// 構造体・列挙・タプルの値 p のフィールド (オフセット off、型 fty) の場所。p がメモリにない (パターンが値の型に
    /// 合わない: `let Ok(0) = 42u8;`) なら型の不一致のエラー
    pub(crate) fn field_place(&mut self, p: Place, fty: Ty, off: u32) -> R<Place> {
        match p {
            Place::Mem(a, o, t) if t.is_dst_struct() => Ok(self.dst_field(a, o, fty, off)),
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
            Pat::Or(ps, _) => {
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
                        // (a union's field is marked, as an expression's is: a guard's write to another field of the
                        // union is to the tested memory (E0510))
                        let union = self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name);
                        for (n, q) in named {
                            let fname = if union { format!("{}{n}", borrowck::UNION_FIELD) } else { n.clone() };
                            match fs.iter().find(|f| f.0 == *n) {
                                Some(f) => self.tested_places(q, &field(place, fname), f.1, out, depth + 1),
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

    /// bindings_with_variant_name (E0170, deny by default): an arm whose pattern is only a binding by value (behind `&`s)
    /// of a name the scrutinee's enum has a unit variant of, `Nil => ..` where `Stack::Nil` is not in scope, at the
    /// arm (rustc 1.100's message)
    #[cold]
    #[inline(never)]
    fn variant_binding(&self, arms: &[crate::parse::Arm], t: Ty) -> Option<String> {
        let Ty::Struct(sid) = self.strip_refs(t) else { return None };
        let sd = &self.prog.structs[sid as usize];
        let base = self.prog.base_of(sid);
        let name = &self.prog.structs[base as usize].name;
        // (the program's own enums, whose variants no `use` may bring into scope: zustc resolves a variant's name
        // alone as the variant, as if it were imported)
        let prelude = self.prog.struct_item.get(base as usize).is_none_or(|&ii| ii >= self.prog.prelude_from);
        if !sd.is_enum || prelude || self.prog.use_prefixes.iter().any(|u| u == name) {
            return None;
        }
        for arm in arms {
            let mut p = &arm.pat;
            while let Pat::Ref(_, q) = p {
                p = q;
            }
            let Pat::Ident(false, false, n, None) = p else { continue };
            if !sd.variants.iter().any(|v| v.name == *n && v.kind == crate::types::VariantKind::Unit) {
                continue;
            }
            // (a constant of the name is no binding)
            if self.prog.const_index.contains_key(n.as_str()) {
                continue;
            }
            return Some(self.at(arm.pos, format!("pattern binding `{n}` is named the same as one of the variants of the type `{name}` (E0170)")));
        }
        None
    }

    /// E0533: a path pattern of a tuple or struct variant, at the path
    #[cold]
    fn unit_pat_err(&self, path: &crate::parse::Path, k: crate::types::VariantKind) -> String {
        let name = path.segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("::");
        let kind = if k == crate::types::VariantKind::Tuple { "tuple variant" } else { "struct variant" };
        self.at(path.pos, format!("expected unit struct, unit variant or constant, found {kind} `{name}` (E0533)"))
    }

    /// A tuple struct pattern (`V(..)`) of a unit variant (E0532) or a struct variant (E0164; E0532 by its name alone),
    /// at its path, with rustc 1.100's message
    fn tuple_pat_of_variant(&self, path: &crate::parse::Path, sid: u32, vi: usize) -> R<()> {
        // (a type the program has no variants of, as an unresolved projection's: its own error comes later)
        let (kind, code) = match self.prog.structs[sid as usize].variants.get(vi).map(|v| v.kind) {
            Some(crate::types::VariantKind::Unit) => ("unit variant", "E0532"),
            Some(crate::types::VariantKind::Struct) => ("struct variant", "E0164"),
            _ => return Ok(()),
        };
        let name = path.segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("::");
        // (a struct variant by its name alone (`use E::V;`) is none in the value namespace)
        if code == "E0164" && path.segs.len() == 1 {
            return Err(self.at(path.pos, format!("cannot find tuple struct or tuple variant `{name}` in this scope (E0532)")));
        }
        Err(self.at(path.pos, format!("expected tuple struct or tuple variant, found {kind} `{name}` ({code})")))
    }

    /// パターン列 (`V(a, .., z)`) とフィールド列の対応 (`..` は途中を飛ばす)。数が合わなければ "this pattern has N fields, but
    /// the corresponding variant has M" (e0023 makes it rustc's where the pattern's path is known)
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

    /// A variant's pattern with type arguments (`None::<isize>`, `Option::<isize>::None`) against a value of the enum's
    /// instance sid: the arguments written are the value's, else mismatched types (E0308)
    fn variant_turbofish(&self, path: &crate::parse::Path, sid: u32) -> R<()> {
        let n = path.segs.len();
        let Some(seg) = path.segs[n.saturating_sub(2)..].iter().rev().find(|s| s.args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(_)))) else {
            return Ok(());
        };
        let base = self.prog.base_of(sid);
        let have = self.prog.structs[sid as usize].args.clone();
        // (fewer arguments than parameters, `E::<u8>::V(..)` of `enum E<T, U = ()>`: the rest are defaults zustc does
        // not read here, not checked)
        // (a const argument counts as one of them; more than the parameters is E0107)
        let written = seg.args.iter().filter(|a| matches!(a, crate::parse::GArg::Ty(_) | crate::parse::GArg::Const(_))).count();
        if written > have.len() {
            let n = have.len();
            return err(path.pos, format!("enum takes {n} generic argument{} but {written} generic arguments were supplied (E0107)", if n == 1 { "" } else { "s" }));
        }
        if written != have.len() {
            return Ok(());
        }
        let ts = self.turbofish_targs(base, &seg.args)?;
        if ts.len() == have.len() && ts.iter().zip(&have).any(|(t, &h)| t.is_some_and(|t| t != h && !self.prog.has_param(h) && !self.prog.has_param(t))) {
            let want: Vec<Ty> = ts.iter().zip(&have).map(|(t, &h)| t.unwrap_or(h)).collect();
            let found = self.prog.instantiate(base, want)?;
            return err(path.pos, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(Ty::Struct(sid)), self.prog.tn(Ty::Struct(found))));
        }
        Ok(())
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
            Pat::Ref(pm, inner) => {
                let Ty::Ref(_, _) = p.ty() else {
                    return err(SP, format!("mismatched types: `{}` is not a reference", self.prog.tn(p.ty())));
                };
                if let Some(m) = self.ref_pat_mutability_err(*pm, p.ty()) {
                    return err(SP, m);
                }
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
            Pat::Or(cases, _) => {
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
                // (the program's: a literal of a reference's type takes no reference off (rustc's match ergonomics), so a
                // `&&[u8]` or a `&mut [u8; N]` is another type (E0308))
                if self.user_code && !matches!(p.ty(), Ty::Ref(false, Ty::Slice(Ty::U8) | Ty::Array(Ty::U8, _))) {
                    return err(SP, format!("mismatched types: expected `{}`, found `&[u8; {}]`", self.prog.tn(p.ty()), bs.len()));
                }
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
                // (the program's: no reference is taken off a `&&str` (E0308), as for a byte string)
                if self.user_code && matches!(p.ty(), Ty::Ref(_, Ty::Ref(..))) {
                    return err(SP, format!("mismatched types: expected `{}`, found `&str`", self.prog.tn(p.ty())));
                }
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
                        // (E0533: `E::V` of a tuple or struct variant, which a path pattern cannot be)
                        if self.user_code {
                            let kind = self.prog.structs[sid as usize].variants.get(vi).map(|v| v.kind);
                            if let Some(k @ (crate::types::VariantKind::Tuple | crate::types::VariantKind::Struct)) = kind {
                                return Err(self.unit_pat_err(path, k));
                            }
                        }
                        self.variant_turbofish(path, sid)?;
                        self.test_variant(p, sid, vi, SP, fail)
                    }
                    Pat::TupleStruct(path, elems) => {
                        let on = self.variant_on(path, t);
                        let fields = match on {
                            Some((sid, vi)) => {
                                self.tuple_pat_of_variant(path, sid, vi)?;
                                self.variant_turbofish(path, sid)?;
                                self.test_variant(p, sid, vi, SP, fail)?;
                                self.prog.structs[sid as usize].variants[vi].fields.clone()
                            }
                            None => self.tuple_struct_fields(path, t, SP)?,
                        };
                        for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP).map_err(|m| self.at(path.pos, e0023(m, on.is_some())))? {
                            let (_, fty, off) = fields[fi];
                            let fp = self.field_place(p, fty, off)?;
                            self.pat_test(sub, fp, fail)?;
                        }
                        Ok(())
                    }
                    Pat::Tuple(elems) => {
                        let fields = self.tuple_pat_fields(p, elems.len(), SP)?;
                        for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP)? {
                            let (_, fty, off) = fields[fi];
                            let fp = self.field_place(p, fty, off)?;
                            self.pat_test(sub, fp, fail)?;
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
    /// The struct a pattern's path names, with or without a turbofish (`Foo::<'static> { field }`: the value's type
    /// gives the instance, the region check the lifetimes)
    pub(crate) fn pat_struct_by(&self, path: &Path) -> Option<u32> {
        self.prog.struct_by_p(&PPath::of(path), self.self_sid).or_else(|| {
            let (pp, targs) = self.turbofish_split(path);
            targs.and_then(|_| self.prog.struct_by_p(&pp, self.self_sid))
        })
    }

    pub(crate) fn tuple_struct_fields(&self, path: &Path, t: Ty, span: Sp) -> R<Vec<(String, Ty, u32)>> {
        let Some(sid) = self.pat_struct_by(path) else {
            return err(span, self.no_variant_msg(path));
        };
        // (`S::<u16>(_)` matching an `S<u8>`: the turbofish's arguments are the value's)
        if let (Some(args), Ty::Struct(_)) = (self.turbofish_split(path).1, t) {
            if !args.is_empty() && args.iter().any(|a| matches!(a, crate::parse::GArg::Ty(_))) && self.prog.struct_by_p(&PPath::of(path), self.self_sid).is_none() {
                let ts = self.turbofish_targs(sid, args)?;
                let n = self.prog.structs[self.prog.base_of(sid) as usize].generics.len();
                if ts.len() != n {
                    return err(span, format!("struct takes {n} generic arguments but {} were supplied (E0107)", ts.len()));
                }
                self.turbofish_vs_expected(sid, &ts, Some(t))?;
            }
        }
        match t {
            Ty::Struct(x) if self.prog.base_of(x) == self.prog.base_of(sid) => Ok(self.prog.structs[x as usize].fields.clone()),
            // (at the pattern's path: a sub-pattern `Tau { t: x }` on its own line is where rustc points)
            _ => Err(self.at(path.pos, format!("mismatched types: expected `{}`, found `{}`", self.prog.tn(t), self.prog.structs[sid as usize].name))),
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
            // (a `&[T]` / `&str` / `&Foo<[T]>` in memory is the (pointer, length) pair itself: the place of the
            // unsized value is that pair)
            Place::Mem(a, o, Ty::Ref(m, i)) if i.is_unsized() => (Place::Mem(a, o, *i), m),
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
        } else if self.pat_struct_by(path).is_some() {
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
            out.push((fpat, self.field_place(p, fty, off)?));
        }
        // (a union's pattern names exactly one field, without `..`; matching the field reads it, which needs `unsafe`
        // (E0133), unless its pattern is `_` (or `_ | _`). The pattern's shape is the type check's, before E0133)
        let union = matches!(p.ty(), Ty::Struct(sid) if self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name));
        if union {
            if out.len() != 1 {
                return Err(self.at(path.pos, "union patterns should have exactly one field".into()));
            }
            if *has_rest {
                return Err(self.at(path.pos, "`..` cannot be used in union patterns".into()));
            }
            fn wild(p: &Pat) -> bool {
                match p {
                    Pat::Wild => true,
                    Pat::Or(ps, _) => ps.iter().all(wild),
                    _ => false,
                }
            }
            if self.unsafe_ok == 0 && self.user_code && !wild(out[0].0) {
                return Err(self.at(path.pos, "access to union field is unsafe and requires unsafe function or block (E0133)".into()));
            }
            return Ok(out);
        }
        if !*has_rest && out.len() != fields.len() {
            return err(SP, "pattern does not mention all fields (add `..`)");
        }
        Ok(out)
    }

    /// n names the unit struct t is (no fields, no variants)
    /// A `&` pattern on a `&mut` value, or a `&mut` pattern on a `&` one: the types differ in mutability (E0308)
    fn ref_pat_mutability_err(&self, pat_mut: bool, t: Ty) -> Option<String> {
        let Ty::Ref(m, i) = t else { return None };
        if !self.user_code || m == pat_mut {
            return None;
        }
        let (want, found) = if m { ("&mut ", "&_") } else { ("&", "&mut _") };
        Some(format!("mismatched types: expected `{want}{}`, found `{found}` (types differ in mutability)", self.prog.tn(*i)))
    }

    /// Whether n names a unit struct of the program (`struct S;`, not generic)
    pub(crate) fn unit_struct_named(&self, n: &str) -> bool {
        !self.prog.unit_structs.is_empty() && self.prog.unit_structs.contains(n) && self.prog.struct_index.get(n).is_some_and(|&sid| self.prog.structs[sid as usize].generics.is_empty())
    }

    pub(crate) fn unit_struct_pat(&self, n: &str, t: Ty) -> bool {
        let Ty::Struct(sid) = t else { return false };
        let sd = &self.prog.structs[self.prog.base_of(sid) as usize];
        sd.name == n && sd.fields.is_empty() && sd.variants.is_empty() && !self.prog.is_tuple(sid)
    }

    /// A bare name of a pattern that pat_bind takes for a unit variant, a constant or a unit struct, whatever the
    /// value's type: no binding
    fn pat_ident_names_item(&self, p: &Pat) -> bool {
        let Pat::Ident(false, false, n, None) = p else { return false };
        unit_variant_ident(self.prog, p).is_some()
            || const_lit_pat(self.prog, p).is_some()
            || ((self.prog.lower_consts || n.trim_start_matches('_').starts_with(|c: char| c.is_uppercase()))
                && self.prog.const_index.get(n.as_str()).is_some_and(|&ci| self.prog.consts[ci as usize].stat.is_none())
                && self.lookup(n).is_none())
            || self.prog.struct_by_p(&PPath::of(&path_of(n)), self.self_sid).is_some_and(|sid| {
                let sd = &self.prog.structs[sid as usize];
                !sd.is_enum && sd.fields.is_empty() && sd.variants.is_empty() && !self.prog.is_tuple(sid)
            })
    }

    /// E0408: the alternatives of each or-pattern in p bind the same names, checked as rustc's resolver does before
    /// any type (an or-pattern inside an alternative first, then the alternatives). The names p binds go to out,
    /// each once, in the order rustc's binding map has them
    pub(crate) fn or_pat_names(&self, p: &Pat, out: &mut Vec<String>) -> R<()> {
        match p {
            Pat::Ident(_, _, n, sub) => {
                if !self.pat_ident_names_item(p) && !out.contains(n) {
                    out.push(n.clone());
                }
                if let Some(s) = sub {
                    self.or_pat_names(s, out)?;
                }
            }
            Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Slice(ps) => {
                for q in ps {
                    self.or_pat_names(q, out)?;
                }
            }
            Pat::Struct(_, fs, _) => {
                for (_, q) in fs {
                    self.or_pat_names(q, out)?;
                }
            }
            Pat::Ref(_, q) => self.or_pat_names(q, out)?,
            Pat::Or(alts, at) => {
                let mut each = Vec::with_capacity(alts.len());
                for a in alts {
                    let mut v = vec![];
                    self.or_pat_names(a, &mut v)?;
                    each.push(v);
                }
                // (rustc's first error: the first alternative that lacks a name another binds, with the first such
                // name of the others in order)
                for (i, v) in each.iter().enumerate() {
                    let mut others = each.iter().enumerate().filter(|&(j, _)| j != i).flat_map(|(_, w)| w);
                    if let Some(n) = others.find(|n| !v.contains(n)) {
                        let sp = at.get(i).copied().unwrap_or_default();
                        return Err(self.at(sp, format!("variable `{n}` is not bound in all patterns (E0408)")));
                    }
                }
                for n in each.into_iter().flatten() {
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// 判定に通った後で、パターン中の名前を束縛する
    pub(crate) fn pat_bind(&mut self, pat: &Pat, p: Place, mode: BindMode) -> R<()> {
        // (a destructuring assignment, `(x, y) = &(1, 2);` (parse::destructure_assign's `let` of `__exr_da` names): no
        // default binding mode takes a reference off its right side (E0308))
        if let (Pat::Tuple(_) | Pat::Slice(_), Ty::Ref(..)) = (pat, p.ty()) {
            if destructure_assign_pat(pat) {
                let shape = if matches!(pat, Pat::Tuple(_)) { "a tuple" } else { "an array" };
                return err(SP, format!("mismatched types: expected `{}`, found {shape}", self.prog.tn(p.ty())));
            }
        }
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
            // (a unit struct's name, of the value's type: a pattern that binds and moves nothing (`let Unit = *w;`
            // through a Deref, `let Unit = u;` before a use of u); through references too, by the default binding mode
            // (`let U = &U;`))
            Pat::Ident(false, false, n, None) if self.unit_struct_pat(n, self.strip_refs(p.ty())) => Ok(()),
            // (an aggregate constant, tested by pat_test: not a new name, so a nested match still compares with it)
            Pat::Ident(_, _, n, None)
                if self.user_code
                    && (self.prog.lower_consts || n.trim_start_matches('_').starts_with(|c: char| c.is_uppercase()))
                    && self.prog.const_index.get(n.as_str()).is_some_and(|&ci| self.prog.consts[ci as usize].stat.is_none())
                    && self.lookup(n).is_none() =>
            {
                Ok(())
            }
            // (the name of a unit struct is that struct, not a new name, for a value of another type too: `let Test = 1;`,
            // `let Thing { foo } = t;` with a `struct foo;` (E0308))
            Pat::Ident(false, false, n, None) if self.user_code && self.unit_struct_named(n) => {
                err(SP, format!("mismatched types: expected `{}`, found `{n}`", self.prog.tn(p.ty())))
            }
            Pat::Ident(by_ref, mutable, name, subpat) => {
                // (`x @ sub`: sub binds first, and what it binds is checked against x below (at_conflict))
                let inner = match subpat {
                    Some(sub) => {
                        self.at_binds.push(vec![]);
                        let r = self.pat_bind(sub, p, mode);
                        let inner = self.at_binds.pop().unwrap_or_default();
                        r?;
                        Some(inner)
                    }
                    None => None,
                };
                let name = name.clone();
                // (`mut x` where the default binding mode is by reference: by value again, before edition 2024
                // (`for (n, mut m) in &tups` moves out of the shared reference, E0507; `let (_, mut m) = &(1u8, 2u8);`
                // makes m a u8))
                if *by_ref && *mutable && matches!(mode, BindMode::Ref(false)) {
                    return Err(self.ref_mut_through_shared());
                }
                let mode = match mode {
                    _ if *by_ref => BindMode::Ref(*mutable),
                    BindMode::Ref(m) if *mutable && self.prog.edition < 2024 => {
                        // (a value that is not Copy is moved out of the reference the pattern matched through, a `&mut`
                        // too, which is_copy takes for Copy as it does every reference: `let (mut r,) = &(&mut n,);`)
                        if !borrowck::is_copy(self.prog, p.ty(), 0) || holds_mut_ref(self.prog, p.ty(), 0) {
                            let what = if m { "a mutable reference" } else { "a shared reference" };
                            let at = self.bk.scrut_place.as_ref().map(|x| x.1).filter(|s| s.line != 0).unwrap_or(SP);
                            return Err(self.at(at, format!("cannot move out of {what} (E0507)")));
                        }
                        BindMode::Move
                    }
                    m => m,
                };
                if inner.is_some() || !self.at_binds.is_empty() {
                    let kind = match mode {
                        BindMode::Move if borrowck::is_copy(self.prog, p.ty(), 0) && !holds_mut_ref(self.prog, p.ty(), 0) => AtBind::Copy,
                        BindMode::Move => AtBind::Move,
                        BindMode::Ref(m) => AtBind::Ref(m),
                    };
                    if let Some(inner) = &inner {
                        let whole = matches!(subpat.as_deref(), Some(Pat::Ident(..)));
                        // (the moves are the borrow check's (E0382), which code after a `return;` is not given to:
                        // rustc's check of the pattern holds there too)
                        if let Some(m) = at_conflict(&name, kind, inner, whole).filter(|m| !(self.dead_stmt && m.ends_with("(E0382)"))) {
                            return Err(self.at(self.at_pat_pos(), m));
                        }
                    }
                    if let Some(top) = self.at_binds.last_mut() {
                        top.extend(inner.unwrap_or_default());
                        top.push((kind, name.clone()));
                    }
                }
                match mode {
                    BindMode::Move => {
                        let (v, t) = self.read(p);
                        self.bk_pat_move(t, &name);
                        self.bk.mut_next = Some(*mutable);
                        let ob = self.owned_binds;
                        self.bind(name.clone(), t, v);
                        // (a value that needs dropping, which bind owned: a part the pattern moved out of the place)
                        if self.owned_binds > ob {
                            self.pat_moved.push(match p {
                                Place::Mem(a, o, t) => (Some(a), o, t),
                                _ => (None, 0, t),
                            });
                        }
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
                // (`let &x = 5u32;`, `fn f(&a: Foo)`: a reference's pattern on a value that is not one (E0308), as
                // pat_test says for a refutable one)
                let t = p.ty();
                if matches!(t, Ty::Struct(_) | Ty::Array(..) | Ty::Unit | Ty::Bool | Ty::Char | Ty::F32 | Ty::F64) || t.is_int() {
                    return err(SP, format!("mismatched types: expected `{}`, found `&_`", self.prog.tn(t)));
                }
                if let Some(m) = self.ref_pat_mutability_err(*pm, t) {
                    return err(SP, m);
                }
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
            Pat::Or(cases, at) => {
                if !cases.iter().any(has_bindings) {
                    return Ok(());
                }
                // (E0408 of a `let`, an `if let`, a `for` ..; a match checks its arms before it lowers them)
                if self.user_code {
                    self.or_pat_names(pat, &mut vec![])?;
                }
                // (the bindings of `|` alternatives are not followed)
                self.rg.pat = None;
                // どの選択肢に合ったかを判定し直し、その選択肢で束縛した値を merge の引数で渡す
                let mut names = vec![];
                crate::ast_closure::pat_idents(&cases[0], &mut names);
                let merge = self.b.create_block();
                let mut tys: Option<Vec<(String, Ty)>> = None;
                for (k, case) in cases.iter().enumerate() {
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
                            // (E0308: a name has the first alternative's type in each; at the alternative, a binding
                            // having no position of its own)
                            if t0.len() != tl.len() {
                                // (at the alternative, as rustc: `Some(y) | None`)
                                let sp = [crate::parse::pat_pos(case), crate::parse::pat_pos(&cases[0])].into_iter().find(|s| s.line != 0).unwrap_or(SP);
                                return Err(self.at(sp, "`|` alternatives bind different types".to_string()));
                            }
                            if let Some((a, b)) = t0.iter().zip(tl.iter()).find(|(a, b)| a.1 != b.1) {
                                let (want, got) = (self.prog.tn(a.1), self.prog.tn(b.1));
                                let sp = Some(crate::parse::pat_pos(case)).filter(|s| s.line != 0).or_else(|| at.get(k).copied()).unwrap_or_default();
                                return Err(self.at(sp, format!("mismatched types: expected `{want}`, found `{got}`")));
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
                // (an array or a slice matched as a place: each element is a place of its own (`[_, _, x] = a` moves
                // a[2]; `[ref a, ..] = *s` borrows s's first); one through a reference is not tracked)
                let arr = matches!(p.ty(), Ty::Array(..) | Ty::Slice(_));
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
                // (a slice's elements are counted from the pattern's ends, with the least length it needs (m): `[1;m]`,
                // `[-1;m]` (the last), as rustc's MIR does: `[_, ref mut b, ..]` and `[.., ref c, _, _]` may be one
                // element of a slice of 3)
                let slice = matches!(p.ty(), Ty::Slice(_)) && self.bk.pat_path.is_some();
                let m = pre.len() + post.len();
                for (k, (q, sub)) in self.slice_elems(base, len, elem, &pre, &post).into_iter().enumerate() {
                    self.rg.pat = self.rg.pat_elem.clone();
                    let name = match n {
                        Some(n) => Some(format!("[{}]", if k < pre.len() { k } else { n.wrapping_sub(m).wrapping_add(k) })),
                        None if slice && k < pre.len() => Some(format!("[{k};{m}]")),
                        None if slice => Some(format!("[-{};{m}]", m - k)),
                        None => None,
                    };
                    let saved = name.map(|f| self.bk_pat_proj(borrowck::Proj::Field(f)));
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
                if let Some(Pat::Ident(by_ref, mutable, rname, _)) = rest {
                    if *by_ref && *mutable && matches!(mode, BindMode::Ref(false)) {
                        return Err(self.ref_mut_through_shared());
                    }
                    let (es, _) = self.prog.size_align(elem);
                    // (`mut rest @ ..` of an array where the default binding mode is by reference: by value again before
                    // edition 2024, as an identifier's binding is: the elements between, copied into an array of
                    // their own (`let [mut b @ ..] = &[1u8];` makes b a `[u8; 1]`). Of `Copy` elements: others would
                    // move out of the reference)
                    let arr_n = match p.ty() {
                        Ty::Array(_, n) => Some(n as usize),
                        Ty::Ref(_, Ty::Array(_, n)) => Some(*n as usize),
                        _ => None,
                    };
                    if let (false, true, BindMode::Ref(_), true, Some(n)) = (*by_ref, *mutable, mode, self.prog.edition < 2024, arr_n) {
                        if borrowck::is_copy(self.prog, elem, 0) && !holds_mut_ref(self.prog, elem, 0) && pre.len() + post.len() <= n {
                            let k = n - pre.len() - post.len();
                            let at = Ty::Array(intern(elem), k as _);
                            let start = self.b.ins().iadd_imm_s(base, pre.len() as i64 * es as i64);
                            let tmp = self.temp(at);
                            self.copy(tmp, start, at);
                            if let Some(top) = self.at_binds.last_mut() {
                                top.push((AtBind::Copy, rname.clone()));
                            }
                            self.bk.mut_next = Some(true);
                            self.bind(rname.clone(), at, Some(tmp));
                            return Ok(());
                        }
                    }
                    let start = self.b.ins().iadd_imm_s(base, pre.len() as i64 * es as i64);
                    let len_v = self.b.ins().iadd_imm_s(len, -((pre.len() + post.len()) as i64));
                    let fat = self.make_fat_v(start, len_v);
                    // (`ref mut tail @ ..` is `&mut [T]`; without `ref`, the mode the pattern is matched in)
                    let m = if *by_ref { *mutable } else { matches!(mode, BindMode::Ref(true)) };
                    // (`ref tail @ ..` of an array: a borrow of the elements between, which tail holds (`a[2] = 0` while
                    // it lives is E0506); of a slice: `[1..-2]`, the elements but the first and the last two)
                    let between = match n {
                        Some(n) if pre.len() < n.saturating_sub(post.len()) => Some(format!("[{}..{}]", pre.len(), n - post.len())),
                        None if slice => Some(format!("[{}..-{}]", pre.len(), post.len())),
                        _ => None,
                    };
                    let held = match (between, *by_ref) {
                        (Some(f), true) => {
                            let path = self.bk_pat_proj(borrowck::Proj::Field(f));
                            let srcs = self.bk_pat_ref(m, true);
                            Some((path, srcs))
                        }
                        _ => None,
                    };
                    self.bind(rname.clone(), Ty::Ref(m, intern(Ty::Slice(intern(elem)))), Some(fat));
                    // (in an `x @ sub`, how it binds, as pat_bind notes the other bindings (at_conflict): `ref mut a @
                    // [ref b @ ..]`)
                    if let Some(top) = self.at_binds.last_mut() {
                        // (`mut rest @ ..` where the default binding mode is by reference: by value again before
                        // edition 2024, as an identifier's binding is)
                        let kind = if *by_ref || (matches!(mode, BindMode::Ref(_)) && !(*mutable && self.prog.edition < 2024)) {
                            AtBind::Ref(m)
                        } else if borrowck::is_copy(self.prog, elem, 0) && !holds_mut_ref(self.prog, elem, 0) {
                            AtBind::Copy
                        } else {
                            AtBind::Move
                        };
                        top.push((kind, rname.clone()));
                    }
                    if let Some((path, srcs)) = held {
                        self.bk.bind_srcs = srcs;
                        self.bk.pat_path = path;
                    }
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
                        if let Place::Mem(a, o, _) = p {
                            self.pat_vars.push((Some(a), o, sid, vi));
                        }
                        self.tuple_pat_of_variant(path, sid, vi)?;
                        let v = &self.prog.structs[sid as usize].variants[vi];
                        (v.fields.clone(), Some(v.name.clone()), Some(vi))
                    }
                    None => (self.tuple_struct_fields(path, p.ty(), SP)?, None, None),
                };
                let deref = matches!(mode, BindMode::Ref(_));
                for (fi, sub) in Self::tuple_fields(elems.iter(), fields.len(), SP).map_err(|m| self.at(path.pos, e0023(m, rvi.is_some())))? {
                    let (fname, fty, off) = fields[fi].clone();
                    let saved = self.bk_pat_field(deref, vname.as_deref(), &fname);
                    let rs = self.rg_pat_field(p.ty(), rvi, fi);
                    let fp = self.field_place(p, fty, off)?;
                    let r = self.pat_bind(sub, fp, mode);
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
                    let fp = self.field_place(p, fty, off)?;
                    let r = self.pat_bind(sub, fp, mode);
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
                let var = self.variant_on(spath, p.ty());
                if let (Some((sid, vi)), Place::Mem(a, o, _)) = (var, p) {
                    self.pat_vars.push((Some(a), o, sid, vi));
                }
                let vname = var.map(|(sid, vi)| self.prog.structs[sid as usize].variants[vi].name.clone());
                let deref = matches!(mode, BindMode::Ref(_));
                // (a union's field is marked, as an expression's is (borrowck::UNION_FIELD): a loan of `u.a` by
                // `U { a: ref x }` overlaps a write to `u.a` or `u.b`)
                let union = matches!(p.ty(), Ty::Struct(sid) if self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name));
                let rs = self.rg.pat.take();
                let r = (|| {
                    let parts = self.struct_pat_fields(pat, p, None)?;
                    for ((fname, _), (sub, fp)) in fps.iter().zip(parts) {
                        let uname = format!("{}{fname}", borrowck::UNION_FIELD);
                        let saved = self.bk_pat_field(deref, vname.as_deref(), if union { &uname } else { fname });
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

/// Whether pattern p binds a name by `ref mut`
fn binds_ref_mut(p: &Pat) -> bool {
    match p {
        Pat::Ident(by_ref, m, _, sub) => (*by_ref && *m) || sub.as_deref().is_some_and(binds_ref_mut),
        Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Or(ps, _) | Pat::Slice(ps) => ps.iter().any(binds_ref_mut),
        Pat::Struct(_, fs, _) => fs.iter().any(|f| binds_ref_mut(&f.1)),
        Pat::Ref(_, q) => binds_ref_mut(q),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Drop
// ---------------------------------------------------------------------------
//
// - drop が要る型 ([`Program::needs_drop`]) の値を持つローカル変数・一時値は、所有者として
//   `owned` / `temps` に drop フラグ (スタック上の 1 byte) と一緒に登録する。
// - ムーブ (値としての読み出し・値で受ける self・move キャプチャ・値で束縛するパターン) でフラグを 0 にする。
//   A move out of a part of a local (a partial move) splits its flag into flags of its other parts, which are
//   dropped in its place (split_moved); what zustc does not split (an array's element, what a Box holds) leaks.
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
        self.owned.push(Owned { loc, ty, flag, depth, off: 0 });
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
        self.temps.push(Owned { loc, ty, flag, depth: 0, off: 0 });
        Some(flag)
    }

    /// 0 で埋めた状態が「drop しても何もしない値」になる型か (Box は null、enum は判別値 0 のバリアント)。
    /// `impl Drop` を含む型は 0 にすると drop が走ってしまうので不可
    pub(crate) fn zeroable(&self, t: Ty) -> bool {
        match t {
            Ty::Box(_) => true,
            Ty::Array(e, _) => !self.needs_drop(*e) || self.zeroable(*e),
            Ty::Struct(sid) => {
                // (prelude の Vec / String / Rc / Arc は 0 (null の ptr、長さ 0) でも drop が何もしない。the threads' handles too:
                // a channel's ends (an Arc inside), a Thread and a JoinHandle check for null in their drops)
                let bname = &self.prog.structs[self.prog.base_of(sid) as usize].name;
                if bname == "Vec" || bname == "String" || bname == "Rc" || bname == "Arc" {
                    return true;
                }
                if matches!(bname.as_str(), "__Sndr" | "__SyncSndr" | "__Rcvr__" | "Thread" | "JoinHandle") && self.prog.struct_item.get(self.prog.base_of(sid) as usize).is_some_and(|&i| i >= self.prog.prelude_from) {
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
            // (the threads' handles whole: a Thread's or JoinHandle's pointer needs no drop of its own, but their drops
            // read it)
            let whole = matches!(bname.as_str(), "Vec" | "String" | "Rc" | "Arc")
                || (matches!(bname.as_str(), "__Sndr" | "__SyncSndr" | "__Rcvr__" | "Thread" | "JoinHandle")
                    && self.prog.struct_item.get(self.prog.base_of(sid) as usize).is_some_and(|&i| i >= self.prog.prelude_from));
            if let (Some(fs), false) = (fields, whole) {
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
            Some(t @ Ty::Struct(sid)) if !self.prog.sigs[id as usize].generics.is_empty() => {
                // (the impl's parameters from its self type matched against the receiver's: `impl<T, H> Deref for
                // W<H, T>` lists them in another order than W's arguments)
                let sig = &self.prog.sigs[id as usize];
                let mut sol = vec![None; sig.generics.len()];
                match sig.self_ty {
                    Some(st) if self.prog.match_ty(st, t, &mut sol) => match sol.iter().copied().collect::<Option<Vec<Ty>>>() {
                        Some(v) => v,
                        None => self.prog.structs[sid as usize].args.clone(),
                    },
                    _ => self.prog.structs[sid as usize].args.clone(),
                }
            }
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
        self.temps.push(Owned { loc: Loc::Ptr(a), ty, flag, depth: 0, off: 0 });
    }

    /// ローカル変数 (の場所) の drop フラグ
    pub(crate) fn flag_of_loc(&self, loc: Loc) -> Option<StackSlot> {
        // (not a part's (split_moved), which has its local's place)
        self.owned.iter().rev().find(|o| o.loc == loc && (self.parts.is_empty() || !self.parts.iter().any(|p| p.3 == o.flag))).map(|o| o.flag)
    }

    /// The local mark_moved moves out of (its place, type and drop flag), without touching its flag
    pub(crate) fn moved_root(&self, e: &Expr) -> Option<(Loc, Ty, StackSlot)> {
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
                    return self.flag_of_loc(loc).map(|f| (loc, t, f));
                }
                _ => return None,
            }
        }
    }

    /// A value was moved out of e (a local, or a field of one): the local is not dropped, or (a part of it moved out,
    /// split_moved) only its other parts are
    pub(crate) fn mark_moved(&mut self, e: &Expr) {
        let parts = self.rel_parts.take();
        let e0 = e;
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
                            if !self.split_moved(e0, loc, t, f, parts) {
                                // (all of it moved: the flags of the parts an earlier move split out too)
                                self.clear_flag(f);
                                let pfs: Vec<StackSlot> = self.parts.iter().filter(|p| p.0 == f).map(|p| p.3).collect();
                                for pf in pfs {
                                    self.clear_flag(pf);
                                }
                            }
                        }
                    }
                    return;
                }
                _ => return,
            }
        }
    }

    /// The variant (enum, variant) a match arm's pattern names, if it names one: `None`, `E::A(..)`, `E::S { .. }`
    fn pat_variant(&self, pat: &Pat, t: Ty) -> Option<(u32, usize)> {
        match pat {
            Pat::Path(p) | Pat::TupleStruct(p, _) | Pat::Struct(p, _, _) => self.variant_on(p, t),
            Pat::Ident(..) => self.variant_on(&unit_variant_ident(self.prog, pat)?, t),
            _ => None,
        }
    }

    /// A local e (or a part of it) is assigned or borrowed `&mut`: no variant is known of it any more (known_var)
    pub(crate) fn forget_variant(&mut self, e: &Expr) {
        let mut cur = strip_paren(e);
        while let Expr::Field(b, _, _) | Expr::Index(b, _, _) = cur {
            cur = strip_paren(b);
        }
        if let Some((_, _, f)) = self.moved_root(cur) {
            self.known_var.retain(|k| k.0 != f);
        }
    }

    /// The parts (offset in `place`, type) that the pattern bound after pat_moved's entry `from` moved out of, or None
    /// when one is not in place's memory (behind a reference or a Box)
    pub(crate) fn pat_parts(&mut self, place: Place, from: (usize, usize)) -> Option<PatParts> {
        let moved = self.pat_moved.split_off(from.0.min(self.pat_moved.len()));
        let vars = self.pat_vars.split_off(from.1.min(self.pat_vars.len()));
        let Place::Mem(a, o, _) = place else { return None };
        let parts = moved.iter().map(|&(b, mo, t)| (b == Some(a)).then_some((mo - o, t))).collect::<Option<Vec<_>>>()?;
        Some((parts, vars.iter().filter(|v| v.0 == Some(a)).map(|v| (v.1 - o, v.2, v.3)).collect()))
    }

    /// A part of a local was moved out of (e: the local or a field of it, its parts: what a pattern moved out of e, or
    /// all of e): the local's flag (f) is split into flags of its other parts, which are dropped in its place, field
    /// by field, as rustc drops what a partial move leaves. False when it is not split (all of it moved; a local not in
    /// memory; a part through a Box): the caller clears f, and what is left is not dropped
    pub(crate) fn split_moved(&mut self, e: &Expr, loc: Loc, t: Ty, f: StackSlot, parts: Option<PatParts>) -> bool {
        if !matches!(loc, Loc::Slot(_) | Loc::Ptr(_)) || parts.is_none() && matches!(strip_paren(e), Expr::Path(..)) {
            return false;
        }
        let Some(depth) = self.owned.iter().rev().find(|o| o.flag == f).map(|o| o.depth) else { return false };
        let Some((off, pt, whole)) = self.local_part(e, t) else { return false };
        let mut vars = vec![];
        let moved: Vec<(i32, Ty)> = match parts {
            Some((v, vs)) => {
                vars = vs.into_iter().map(|(o, sid, vi)| (off + o, sid, vi)).collect();
                v.into_iter().map(|(o, k)| (off + o, k)).filter(|&(_, k)| self.needs_drop(k)).collect()
            }
            None if whole => return false,
            None => [(off, pt)].into_iter().filter(|&(_, k)| self.needs_drop(k)).collect(),
        };
        // (a part that needs no dropping: the local is dropped as before, what drops nothing of it included)
        if moved.is_empty() {
            return true;
        }
        if moved.iter().any(|&(o, k)| o == 0 && k == t) {
            return false;
        }
        if let Some(&(_, sid, vi)) = self.known_var.iter().find(|k| k.0 == f) {
            if vars.iter().any(|v| v.0 == 0 && v.1 == sid && v.2 != vi) {
                return false;
            }
        }
        self.part_vars = vars;
        // (not in a loop the local outlives, unless the flags of the parts are there from before it (presplit_loop):
        // code lowered before the split, which knows no flag the split makes, runs again after it on the next pass)
        if self.in_loop_since(depth) && !self.parts_ready(f, t, &moved, true) {
            self.part_vars.clear();
            return false;
        }
        self.split_part((f, loc, depth), f, 0, t, &moved, true);
        self.part_vars.clear();
        true
    }

    /// Whether a loop entered after scope depth `depth` is being lowered (a local of that depth outlives its passes)
    fn in_loop_since(&self, depth: usize) -> bool {
        self.loops.iter().any(|l| !l.is_block && l.depth > depth)
    }

    /// Whether split_part of the local of type t whose flag is f, for moved, makes no flag that is not there yet
    fn parts_ready(&self, f: StackSlot, t: Ty, moved: &[(i32, Ty)], clear: bool) -> bool {
        let mut need = vec![];
        let _ = self.part_plan(f, 0, t, moved, clear, &mut need);
        need.is_empty()
    }

    /// The flags (offset, type) split_part of the part (off, t) of the local whose flag is f would make (those not
    /// made yet, into need), or None where it splits nothing (and clears the flag of what is left)
    fn part_plan(&self, f: StackSlot, off: i32, t: Ty, moved: &[(i32, Ty)], clear: bool, need: &mut Vec<(i32, Ty)>) -> Option<()> {
        let kids = self.part_kids(t, off, moved)?;
        let exact = |o: i32, k: Ty| moved.iter().any(|&(mo, mt)| mo == o && mt == k);
        if kids.iter().all(|&(o, k)| exact(o, k) && clear) {
            return Some(());
        }
        for &(o, k) in &kids {
            if exact(o, k) && clear {
                continue;
            }
            if !self.parts.iter().any(|p| p.0 == f && p.1 == o && p.2 == k) && !need.contains(&(o, k)) {
                need.push((o, k));
            }
            if exact(o, k) {
                continue;
            }
            let size = self.prog.size_align(k).0 as i32;
            let inner: Vec<(i32, Ty)> = moved.iter().copied().filter(|&(mo, _)| o <= mo && mo < o + size).collect();
            if !inner.is_empty() {
                self.part_plan(f, o, k, &inner, clear, need)?;
            }
        }
        Some(())
    }

    /// Before a loop (its condition, its body): the flags of the parts of the locals it outlives that a field of one
    /// moved out of or assigned in it would split them into, made now (split_part does not make one in such a loop,
    /// in_loop_since). They are zero (each use of a part splits the local's flag at run time, as outside a loop)
    #[inline]
    pub(crate) fn presplit_loop(&mut self, cond: Option<&Expr>, body: &PBlock) {
        if !self.owned.is_empty() {
            self.presplit_owned(cond, body);
        }
    }

    #[inline(never)]
    fn presplit_owned(&mut self, cond: Option<&Expr>, body: &PBlock) {
        // (only with a local that can be split: a tuple or a struct of the program's, without a `Drop` of its own,
        // with a part to drop)
        let splittable = |o: &Owned| {
            let Ty::Struct(sid) = o.ty else { return false };
            let b = self.prog.base_of(sid);
            matches!(o.loc, Loc::Slot(_) | Loc::Ptr(_))
                && !self.prog.structs[sid as usize].is_enum
                && self.prog.struct_item.get(b as usize).is_none_or(|&i| i < self.prog.prelude_from)
                && !self.prog.drop_impls.contains(&b)
                && self.needs_drop(o.ty)
        };
        if !self.owned.iter().any(splittable) {
            return;
        }
        if let Some(c) = cond {
            self.presplit_scan(c);
        }
        self.presplit_block(body);
    }

    fn presplit_block(&mut self, b: &PBlock) {
        for s in &b.stmts {
            match s {
                Stmt::Let(pat, _, init, els, ..) => {
                    if let Some(e) = init {
                        self.presplit_pat(pat, e);
                        self.presplit_scan(e);
                    }
                    if let Some(b) = els {
                        self.presplit_block(b);
                    }
                }
                Stmt::Expr(e, _) => self.presplit_scan(e),
                Stmt::Item(_) => {}
            }
        }
    }

    /// The fields of locals in e that may be moved out of or assigned (not a method's receiver, what `&` borrows, an
    /// index's base, a comparison's operand, what `*` or `+=` takes)
    fn presplit_scan(&mut self, e: &Expr) {
        let field = |x: &Expr| matches!(strip_paren(x), Expr::Field(..));
        match e {
            Expr::Field(b, _, _) => {
                let mut cur = strip_paren(b);
                while let Expr::Field(x, _, _) = cur {
                    cur = strip_paren(x);
                }
                match cur {
                    Expr::Path(..) => self.presplit_field(e),
                    _ => self.presplit_scan(cur),
                }
            }
            Expr::MethodCall(r, _, _, args, _) => {
                if !field(r) {
                    self.presplit_scan(r);
                }
                for a in args {
                    self.presplit_scan(a);
                }
            }
            Expr::Ref(_, x) | Expr::RawRef(_, x) | Expr::Unary(_, x, _) if field(x) => {}
            Expr::Index(x, i, _) | Expr::AssignOp(_, x, i, _) if field(x) => self.presplit_scan(i),
            Expr::Binary(op, l, r, _) if matches!(op.as_str(), "==" | "!=" | "<" | "<=" | ">" | ">=") => {
                for x in [l, r] {
                    if !field(x) {
                        self.presplit_scan(x);
                    }
                }
            }
            Expr::Closure(..) => {}
            Expr::Macro(p, g, _) => self.presplit_macro(p, g),
            _ => {
                match e {
                    Expr::Match(x, arms) => {
                        for a in arms {
                            self.presplit_pat(&a.pat, x);
                        }
                    }
                    Expr::Let(pat, x) => self.presplit_pat(pat, x),
                    _ => {}
                }
                crate::ast_closure::each_child_pub(e, &mut |c| self.presplit_scan(c))
            }
        }
    }

    /// presplit_scan of a macro call's arguments, when they may name a field (`dbg!({ x.1 = v; })`, `vec![x.0]`): a
    /// format macro's argument that is a field is only borrowed
    #[inline(never)]
    fn presplit_macro(&mut self, p: &Path, g: &str) {
        if !g.contains('.') {
            return;
        }
        let name = p.segs.last().map_or("", |s| s.name.as_str());
        if matches!(name, "matches" | "stringify" | "concat" | "include_str" | "include_bytes" | "cfg" | "env") {
            return;
        }
        let Some(args) = crate::ast_closure::macro_args_at(g, Default::default()) else { return };
        let fmt = matches!(
            name,
            "println" | "print" | "eprintln" | "eprint" | "format" | "format_args" | "write" | "writeln" | "panic" | "assert"
                | "assert_eq" | "assert_ne" | "debug_assert" | "debug_assert_eq" | "debug_assert_ne" | "unreachable" | "todo"
                | "unimplemented"
        );
        for a in &args {
            if !(fmt && matches!(strip_paren(a), Expr::Field(..))) {
                self.presplit_scan(a);
            }
        }
    }

    /// presplit_loop of what a pattern moves out of a local or a field of one (`let (a, _) = x`, `match x.0 { S { a, .. } => .. }`)
    fn presplit_pat(&mut self, pat: &Pat, x: &Expr) {
        if matches!(pat, Pat::Ident(..) | Pat::Wild) || !matches!(strip_paren(x), Expr::Path(..) | Expr::Field(..)) {
            return;
        }
        let Some((loc, t, f)) = self.moved_root(x) else { return };
        if !matches!(loc, Loc::Slot(_) | Loc::Ptr(_)) {
            return;
        }
        let Some(depth) = self.owned.iter().rev().find(|o| o.flag == f).map(|o| o.depth) else { return };
        let Some((off, pt, _)) = self.local_part(x, t) else { return };
        let mut moved = vec![];
        if self.pat_move_parts(pat, pt, off, &mut moved).is_none() {
            return;
        }
        moved.retain(|&(_, k)| self.needs_drop(k));
        if moved.is_empty() || moved.iter().any(|&(o, k)| o == 0 && k == t) {
            return;
        }
        let mut need = vec![];
        if self.part_plan(f, 0, t, &moved, false, &mut need).is_none() {
            return;
        }
        for (o, k) in need {
            self.part_flag((f, loc, depth), o, k);
        }
    }

    /// The parts (offset, type) of a place of type t at off that pat binds by value: None for a pattern of an enum, a
    /// slice or alternatives, which are not looked into
    fn pat_move_parts(&self, pat: &Pat, t: Ty, off: i32, out: &mut Vec<(i32, Ty)>) -> Option<()> {
        match pat {
            Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) | Pat::Ident(true, ..) => Some(()),
            Pat::Ident(false, ..) => {
                out.push((off, t));
                Some(())
            }
            Pat::Tuple(ps) | Pat::TupleStruct(_, ps) => {
                let Ty::Struct(sid) = t else { return None };
                let sd = &self.prog.structs[sid as usize];
                if sd.is_enum {
                    return None;
                }
                let rest = ps.iter().position(|p| matches!(p, Pat::Rest));
                for (i, p) in ps.iter().enumerate() {
                    if matches!(p, Pat::Rest) {
                        continue;
                    }
                    let fi = match rest {
                        Some(r) if i > r => sd.fields.len().checked_sub(ps.len() - i)?,
                        _ => i,
                    };
                    let (ft, fo) = self.prog.field(sid, &fi.to_string())?;
                    self.pat_move_parts(p, ft, off + fo as i32, out)?;
                }
                Some(())
            }
            Pat::Struct(_, fs, _) => {
                let Ty::Struct(sid) = t else { return None };
                if self.prog.structs[sid as usize].is_enum {
                    return None;
                }
                for (n, p) in fs {
                    let (ft, fo) = self.prog.field(sid, n)?;
                    self.pat_move_parts(p, ft, off + fo as i32, out)?;
                }
                Some(())
            }
            _ => None,
        }
    }

    /// presplit_loop of a field e (`x.a`, `x.a.b`) of a local
    fn presplit_field(&mut self, e: &Expr) {
        let Some((loc, t, f)) = self.moved_root(e) else { return };
        if !matches!(loc, Loc::Slot(_) | Loc::Ptr(_)) {
            return;
        }
        let Some(depth) = self.owned.iter().rev().find(|o| o.flag == f).map(|o| o.depth) else { return };
        let Some((off, pt, false)) = self.local_part(e, t) else { return };
        if (off == 0 && pt == t) || !self.needs_drop(pt) {
            return;
        }
        let mut need = vec![];
        if self.part_plan(f, 0, t, &[(off, pt)], false, &mut need).is_none() {
            return;
        }
        for (o, k) in need {
            self.part_flag((f, loc, depth), o, k);
        }
    }

    /// The offset and type of the place e (a local, or its fields `x.a.b`) in the local of type t, and whether it is
    /// the local itself. None through anything else (`*b`, an index)
    fn local_part(&self, e: &Expr, t: Ty) -> Option<(i32, Ty, bool)> {
        let mut names = vec![];
        let mut cur = strip_paren(e);
        while let Expr::Field(b, n, _) = cur {
            names.push(n.as_str());
            cur = strip_paren(b);
        }
        if !matches!(cur, Expr::Path(..)) {
            return None;
        }
        let (mut off, mut pt) = (0i32, t);
        for n in names.iter().rev() {
            let Ty::Struct(sid) = pt else { return None };
            if self.prog.structs[sid as usize].is_enum {
                return None;
            }
            let (ft, fo) = self.prog.field(sid, n)?;
            off += fo as i32;
            pt = ft;
        }
        Some((off, pt, names.is_empty()))
    }

    /// Splits the part (off, t) of a local (root: its flag, place, scope depth), whose flag is cont, down to each part
    /// in moved: the fields of t that need dropping get flags of their own, set where cont is, and cont is cleared.
    /// The flag of a part in moved is then cleared (clear: a move) or left (an assignment to it). A part zustc does
    /// not split (an array's element, what a Box holds, an enum with no one variant that holds the parts) is not
    /// dropped, as after a move of all of it
    fn split_part(&mut self, root: (StackSlot, Loc, usize), cont: StackSlot, off: i32, t: Ty, moved: &[(i32, Ty)], clear: bool) {
        let Some(kids) = self.part_kids(t, off, moved) else {
            self.clear_flag(cont);
            return;
        };
        // (a part moved out needs no flag: one a split before made is cleared. When every part is moved out, the
        // local's flag is all there is to clear: `match o { Some(v) => .. }`)
        let exact = |o: i32, k: Ty| moved.iter().any(|&(mo, mt)| mo == o && mt == k);
        let flags: Vec<Option<StackSlot>> = kids
            .iter()
            .map(|&(o, k)| match exact(o, k) && clear {
                true => self.parts.iter().find(|p| p.0 == root.0 && p.1 == o && p.2 == k).map(|p| p.3),
                false => Some(self.part_flag(root, o, k)),
            })
            .collect();
        if kids.iter().all(|&(o, k)| exact(o, k) && clear) {
            for f in flags.into_iter().flatten() {
                self.clear_flag(f);
            }
            for &(o, k) in &kids {
                self.clear_inner_parts(root.0, o, k);
            }
            self.clear_flag(cont);
            return;
        }
        let c = self.b.ins().stack_load(types::I8, cont, 0);
        for (&(o, k), f) in kids.iter().zip(&flags) {
            if let (Some(f), false) = (*f, exact(o, k) && clear) {
                let x = self.b.ins().stack_load(types::I8, f, 0);
                let y = self.b.ins().bor(x, c);
                self.b.ins().stack_store(y, f, 0);
            }
        }
        self.clear_flag(cont);
        for (&(o, k), &f) in kids.iter().zip(&flags) {
            let Some(f) = f else { continue };
            if exact(o, k) {
                if clear {
                    self.clear_flag(f);
                    self.clear_inner_parts(root.0, o, k);
                }
                continue;
            }
            let size = self.prog.size_align(k).0 as i32;
            let inner: Vec<(i32, Ty)> = moved.iter().copied().filter(|&(mo, _)| o <= mo && mo < o + size).collect();
            if !inner.is_empty() {
                self.split_part(root, f, o, k, &inner, clear);
            }
        }
    }

    /// A part (off, t) of the local whose flag is f0 moved out as a whole: the flags of its own parts that an earlier
    /// split made (`let a = x.0.0; x.0.0 = ..;` before `let b = x.0;`) are cleared too, so that they are not dropped
    /// where the local is as well as by the new owner
    fn clear_inner_parts(&mut self, f0: StackSlot, off: i32, t: Ty) {
        if !self.parts.iter().any(|p| p.0 == f0 && p.1 >= off && (p.1, p.2) != (off, t)) {
            return;
        }
        let size = self.prog.size_align(t).0 as i32;
        let inner: Vec<StackSlot> = self
            .parts
            .iter()
            .filter(|p| p.0 == f0 && off <= p.1 && p.1 + self.prog.size_align(p.2).0 as i32 <= off + size && (p.1, p.2) != (off, t))
            .map(|p| p.3)
            .collect();
        for f in inner {
            self.clear_flag(f);
        }
    }

    /// The fields (offset, type) of the part (off, t) that need dropping, when t is a struct or tuple without a `Drop`
    /// of its own, or an enum of which one variant holds every part in moved (that variant's)
    fn part_kids(&self, t: Ty, off: i32, moved: &[(i32, Ty)]) -> Option<Vec<(i32, Ty)>> {
        let Ty::Struct(sid) = t else { return None };
        let base = self.prog.base_of(sid);
        if self.prog.drop_impls.contains(&base) || self.prog.unions.contains(&self.prog.structs[base as usize].name) || t.is_dst_struct() {
            return None;
        }
        let holds = |fs: &[(String, Ty, u32)]| {
            moved.iter().all(|&(mo, mt)| {
                fs.iter().any(|f| {
                    let o = off + f.2 as i32;
                    (mo == o && mt == f.1) || (o <= mo && mo < o + self.prog.size_align(f.1).0 as i32)
                })
            })
        };
        let sd = &self.prog.structs[sid as usize];
        // (an enum: the variant its pattern matched, or the one variant that holds the parts)
        let matched = self.part_vars.iter().find(|v| v.0 == off && v.1 == sid).map(|v| v.2);
        let fields = if let (true, Some(vi)) = (sd.is_enum, matched) {
            &sd.variants.get(vi)?.fields
        } else if sd.is_enum {
            let mut vs = sd.variants.iter().filter(|v| holds(&v.fields));
            let v = vs.next()?;
            if vs.next().is_some() {
                return None;
            }
            &v.fields
        } else if holds(&sd.fields) {
            &sd.fields
        } else {
            return None;
        };
        Some(fields.iter().filter(|f| self.needs_drop(f.1)).map(|f| (off + f.2 as i32, f.1)).collect())
    }

    /// The flag of the part (off, t) of a local (root), made the first time: zero on entry, owned next to the local,
    /// so that its parts are dropped where it is, in the order of their offsets
    fn part_flag(&mut self, root: (StackSlot, Loc, usize), off: i32, t: Ty) -> StackSlot {
        let (f0, loc, depth) = root;
        if let Some(p) = self.parts.iter().find(|p| p.0 == f0 && p.1 == off && p.2 == t) {
            return p.3;
        }
        let ss = self.b.create_stack_slot(1, 1);
        self.late_flags.push(ss);
        if let Some(pos) = self.owned.iter().rposition(|o| o.flag == f0) {
            let mut i = pos + 1;
            while i < self.owned.len() && self.owned[i].off > off && self.parts.iter().any(|p| p.0 == f0 && p.3 == self.owned[i].flag) {
                i += 1;
            }
            self.owned.insert(i, Owned { loc, ty: t, flag: ss, depth, off });
        }
        self.parts.push((f0, off, t, ss));
        ss
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
            // (out of what a raw pointer points to (`*x`, `(*x).a`): E0507, as it is not owned)
            if fname.is_none() && matches!(self.peek(b), Some(Ty::Ptr(..))) {
                return err(expr_start(e), format!("cannot move out of `{}`, which is behind a raw pointer (E0507)", raw_place_text(e)));
            }
            // (out of what a reference that is no place points to (`*&x`, `*v.first().unwrap()`, `*id(&s)`): the
            // reference is a temporary, E0507 as rustc says it. A place's is the borrow check's)
            if fname.is_none() && !is_place_expr(b) {
                if let Some(Ty::Ref(m, _)) = self.peek(b) {
                    return err(expr_start(e), format!("cannot move out of a {} reference (E0507)", if m { "mutable" } else { "shared" }));
                }
            }
            // (through references too: `r.value` with r: `&Rc<S>`)
            if let Some(Ty::Struct(sid)) = self.peek(b).map(|t| self.strip_refs(t)) {
                let through = fname.is_none_or(|f| self.prog.field(sid, f).is_none());
                if through && self.user_deref_sid(b, sid).is_some() {
                    let n = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
                    let msg = if matches!(n.as_str(), "Rc" | "Arc") {
                        format!("cannot move out of an `{n}` (E0507)")
                    } else {
                        format!("cannot move out of dereference of `{}` (E0507)", self.prog.tn(Ty::Struct(sid)))
                    };
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
            Pat::Or(ps, _) => {
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
        if !self.bk_on() || self.bk.level == 0 {
            return Ok(());
        }
        // (`*x`, and a field reached through an overloaded deref (`if let Some(v) = wrap.field` with wrap: W: Deref),
        // also deeper (`w.a.b`): what the pattern moves is behind the reference `deref` returns, E0507)
        let mut cur = strip_paren(e);
        loop {
            let (x, fname) = match cur {
                Expr::Field(b, f, _) => (strip_paren(b), Some(f)),
                Expr::Unary(op, b, _) if op == "*" => (strip_paren(b), None),
                _ => break,
            };
            // (a pattern that moves out of what a raw pointer points to: `let (a, s) = *x;`)
            if fname.is_none() && matches!(self.peek(x), Some(Ty::Ptr(..))) {
                return err(expr_start(e), format!("cannot move out of `{}`, which is behind a raw pointer (E0507)", raw_place_text(e)));
            }
            if let Some(Ty::Struct(sid)) = self.peek(x).map(|t| self.strip_refs(t)) {
                let through = fname.is_none_or(|f| self.prog.field(sid, f).is_none());
                if through && self.user_deref_sid(x, sid).is_some() {
                    let n = self.prog.structs[self.prog.base_of(sid) as usize].name.clone();
                    let what = if matches!(n.as_str(), "Rc" | "Arc") { format!("an `{n}`") } else { format!("dereference of `{}`", self.prog.tn(Ty::Struct(sid))) };
                    return err(expr_start(e), format!("cannot move out of {what} (E0507)"));
                }
            }
            if fname.is_none() {
                break;
            }
            cur = x;
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
                        self.known_var.retain(|k| k.0 != flag);
                        self.emit_owned_drop(Owned { loc, ty, flag, depth: 0, off: 0 });
                        // (what a partial move left of it)
                        let mut ps: Vec<(i32, Ty, StackSlot)> = self.parts.iter().filter(|p| p.0 == flag).map(|p| (p.1, p.2, p.3)).collect();
                        ps.sort_by_key(|p| p.0);
                        for (off, t, f) in ps {
                            self.emit_owned_drop(Owned { loc, ty: t, flag: f, depth: 0, off });
                        }
                        return Some(flag);
                    }
                }
            }
        }
        // (a field of a local that a partial move split: the field gets a flag of its own, and its old value is dropped
        // when the flag says it is there)
        if let Some(r) = self.assign_part(left) {
            return Some(r);
        }
        // (`*b = v` of a Box local: what b holds is dropped only when it is there (`*b;` moved it out, which clears b's
        // flag); b holds a value again after it, as rustc re-initializes `*b`)
        if let (Expr::Unary(op, inner, _), Place::Mem(a, o, t)) = (strip_paren(left), p) {
            if op == "*" {
                if let Expr::Path(_, pp) = strip_paren(inner) {
                    if let Some((loc, Ty::Box(_))) = ident_of(pp).and_then(|id| self.lookup(id)) {
                        if let Some(flag) = self.flag_of_loc(loc) {
                            let addr = self.addr_plus(a, o);
                            self.emit_owned_drop(Owned { loc: Loc::Ptr(addr), ty: t, flag, depth: 0, off: 0 });
                            return Some(flag);
                        }
                    }
                }
            }
        }
        // (a field of a local: dropped only while the local's flag says it is all there. A move out of it in a loop it
        // outlives (split_moved) cleared the flag, and the moved field is not dropped again; the rest leaks)
        if let (Expr::Field(..), Place::Mem(a, o, t)) = (strip_paren(left), p) {
            if let Some((Loc::Slot(_) | Loc::Ptr(_), _, f)) = self.moved_root(left) {
                if self.needs_drop(t) {
                    let addr = self.addr_plus(a, o);
                    let x = self.b.ins().stack_load(types::I8, f, 0);
                    let yes = self.b.create_block();
                    let done = self.b.create_block();
                    self.b.ins().brif(x, yes, &[], done, &[]);
                    self.b.switch_to_block(yes);
                    self.drop_in_place(addr, t);
                    self.b.ins().jump(done, &[]);
                    self.b.switch_to_block(done);
                }
                return None;
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

    /// `x.a = v` with x a local split by a partial move (split_moved): x.a's own flag, split out of x's, after its old
    /// value is dropped if it is there
    fn assign_part(&mut self, left: &Expr) -> Option<StackSlot> {
        if self.parts.is_empty() || !matches!(strip_paren(left), Expr::Field(..)) {
            return None;
        }
        let (loc, t, f) = self.moved_root(left)?;
        if !matches!(loc, Loc::Slot(_) | Loc::Ptr(_)) || !self.parts.iter().any(|p| p.0 == f) {
            return None;
        }
        let depth = self.owned.iter().rev().find(|o| o.flag == f)?.depth;
        let (off, pt, _) = self.local_part(left, t)?;
        if !self.needs_drop(pt) {
            return None;
        }
        // (in a loop the local outlives, only parts whose flags are there from before it: no flag is made there
        // (split_moved))
        if self.in_loop_since(depth) && !self.parts_ready(f, t, &[(off, pt)], false) {
            return None;
        }
        self.split_part((f, loc, depth), f, 0, t, &[(off, pt)], false);
        let pf = self.parts.iter().find(|p| p.0 == f && p.1 == off && p.2 == pt)?.3;
        // (the parts of x.a an earlier move split out: those still there are dropped, in order, and their flags
        // cleared, so that the new value has x.a's flag alone)
        let size = self.prog.size_align(pt).0 as i32;
        let mut inner: Vec<(i32, Ty, StackSlot)> =
            self.parts.iter().filter(|p| p.0 == f && off <= p.1 && p.1 < off + size && p.3 != pf).map(|p| (p.1, p.2, p.3)).collect();
        inner.sort_by_key(|p| p.0);
        for (o, k, flag) in inner {
            self.emit_owned_drop(Owned { loc, ty: k, flag, depth: 0, off: o });
        }
        self.emit_owned_drop(Owned { loc, ty: pt, flag: pf, depth: 0, off });
        Some(pf)
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
        let addr = self.addr_plus(addr, o.off);
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

    /// The temporaries of a statement from t: the one whose flag is `value` (the value a `ref` binding refers to) lives
    /// to the block's end, the others are dropped now
    pub(crate) fn extend_value_temp(&mut self, t: usize, value: Option<StackSlot>) {
        let ts: Vec<Owned> = self.temps.drain(t.min(self.temps.len())..).collect();
        let depth = self.scopes.len() - 1;
        let (keep, rest): (Vec<Owned>, Vec<Owned>) = ts.into_iter().partition(|o| value == Some(o.flag));
        for o in rest.into_iter().rev() {
            self.emit_owned_drop(o);
        }
        for mut o in keep {
            o.depth = depth;
            self.owned.push(o);
        }
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
            // (a box of a struct with an unsized tail: (pointer, length or vtable), as `Box<[T]>` / `Box<dyn Tr>`)
            Ty::Box(inner) if inner.is_dst_struct() => {
                let ptr = self.b.ins().load(PTR, mf(), p, 0);
                let live = self.b.create_block();
                let out = self.b.create_block();
                self.b.ins().brif(ptr, live, &[], out, &[]);
                self.b.switch_to_block(live);
                let meta = self.b.ins().load(types::I64, mf(), p, 8);
                self.drop_dst_struct(ptr, meta, *inner)?;
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
            // (`drop_in_place::<Wrap<[T]>>(p)`: p is the address of the (data, metadata) pair)
            Ty::Struct(_) if t.is_dst_struct() => {
                let data = self.b.ins().load(PTR, mf(), p, 0);
                let meta = self.b.ins().load(types::I64, mf(), p, 8);
                self.drop_dst_struct(data, meta, t)?;
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
        Pat::Or(_, _) => true,
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
        Pat::Or(v, _) => v.iter().flat_map(or_alts).collect(),
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
/// The names a pattern binds by `ref mut`
fn ref_mut_names(p: &Pat, out: &mut Vec<String>) {
    match p {
        Pat::Ident(by_ref, m, n, sub) => {
            if *by_ref && *m {
                out.push(n.clone());
            }
            if let Some(q) = sub {
                ref_mut_names(q, out);
            }
        }
        Pat::Ref(_, q) => ref_mut_names(q, out),
        Pat::Or(v, _) | Pat::Tuple(v) | Pat::TupleStruct(_, v) | Pat::Slice(v) => v.iter().for_each(|q| ref_mut_names(q, out)),
        Pat::Struct(_, fs, _) => fs.iter().for_each(|f| ref_mut_names(&f.1, out)),
        _ => {}
    }
}

/// Where a guard assigns through one of names (`*r = ..`) or borrows it mutably (`&mut *r`, `&mut r`), in a closure
/// too: the position, the name, and whether it assigns. A block's `let` that binds a name again ends it there
fn guard_mut_use(e: &Expr, names: &[String]) -> Option<(crate::parse::Pos, String, bool)> {
    if names.is_empty() {
        return None;
    }
    // (a closure's parameters are its own: `|r: &mut i32| { *r = 2; true }` writes no arm's `r`)
    if let Expr::Closure(_, inputs, _, body, _) = e {
        let mut bound = vec![];
        for (p, _) in inputs {
            crate::ast_closure::pat_idents(p, &mut bound);
        }
        let cur: Vec<String> = names.iter().filter(|n| !bound.contains(n)).cloned().collect();
        return guard_mut_use(body, &cur);
    }
    let named = |x: &Expr| -> Option<String> {
        let mut x = strip_paren(x);
        if let Expr::Unary(op, i, _) = x {
            if op == "*" {
                x = strip_paren(i);
            }
        }
        match x {
            Expr::Path(None, p) if p.segs.len() == 1 && names.contains(&p.segs[0].name) => Some(p.segs[0].name.clone()),
            _ => None,
        }
    };
    match e {
        Expr::Assign(l, _) | Expr::AssignOp(_, l, _, _) if matches!(strip_paren(l), Expr::Unary(op, ..) if op == "*") => {
            if let Some(n) = named(l) {
                return Some((borrowck::pos(e), n, true));
            }
        }
        Expr::Ref(true, x) => {
            if let Some(n) = named(x) {
                return Some((borrowck::pos(e), n, false));
            }
        }
        Expr::Block(_, _, b) => {
            let mut cur = names.to_vec();
            for st in &b.stmts {
                match st {
                    crate::parse::Stmt::Let(p, _, init, ..) => {
                        if let Some(f) = init.as_ref().and_then(|x| guard_mut_use(x, &cur)) {
                            return Some(f);
                        }
                        let mut bound = vec![];
                        crate::ast_closure::pat_idents(p, &mut bound);
                        cur.retain(|n| !bound.contains(n));
                    }
                    crate::parse::Stmt::Expr(x, _) => {
                        if let Some(f) = guard_mut_use(x, &cur) {
                            return Some(f);
                        }
                    }
                    _ => {}
                }
            }
            return None;
        }
        _ => {}
    }
    let mut found = None;
    crate::ast_closure::each_child_pub(e, &mut |c| {
        if found.is_none() {
            found = guard_mut_use(c, names);
        }
    });
    found
}

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

/// How rustc names a place through a raw pointer: `*x`, and `x.a` for `(*x).a`
pub(crate) fn raw_place_text(e: &Expr) -> String {
    fn go(e: &Expr, top: bool) -> String {
        match strip_paren(e) {
            Expr::Path(_, p) => p.segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("::"),
            Expr::Field(b, f, _) => format!("{}.{f}", go(b, false)),
            Expr::Index(b, _, _) => format!("{}[_]", go(b, false)),
            Expr::Unary(op, b, _) if op == "*" => if top { format!("*{}", go(b, false)) } else { go(b, false) },
            _ => "..".into(),
        }
    }
    go(e, true)
}

impl Cx<'_> {
    /// e is a place written through a `*const` pointer (`*p`, `(*p).x`, `(*p)[0]`): E0594's text
    pub(crate) fn const_ptr_write(&self, e: &Expr) -> Option<String> {
        let mut cur = strip_paren(e);
        loop {
            cur = match cur {
                Expr::Field(b, ..) | Expr::Index(b, ..) => strip_paren(b),
                Expr::Unary(op, b, _) if op == "*" => {
                    match self.peek(b) {
                        Some(Ty::Ptr(false, _)) => return Some(format!("cannot assign to `{}`, which is behind a `*const` pointer (E0594)", raw_place_text(e))),
                        // (through a `*mut` (`**p = 1` with `p: *const *mut i32`): what it points to is writable, whatever
                        // reached the pointer)
                        Some(Ty::Ptr(true, _)) => return None,
                        _ => strip_paren(b),
                    }
                }
                _ => return None,
            };
        }
    }
}

/// t is a `&mut`, or holds one in an array, a tuple, a struct's field or an enum's variant (`Option<&mut T>`, whose
/// derived Copy is_copy takes to hold, as it takes every reference for Copy)
/// rustc's check of `x @ sub` (its THIR's, before the borrow check), with kind how x binds and inner what sub binds:
/// x moved and sub borrowing ("borrow of moved value"); `ref x` and sub moving ("cannot move out of value because it
/// is borrowed"), or borrowing mutably where either is `ref mut`. Then the borrow check's E0382: x moved after sub moved
/// a part of it (`a @ Some(b)` of an `Option<String>`), or all of it (`a @ b`, whole)
pub(crate) fn at_conflict(name: &str, kind: AtBind, inner: &[(AtBind, String)], whole: bool) -> Option<String> {
    let first = |f: &dyn Fn(AtBind) -> bool| inner.iter().find(|(k, _)| f(*k)).map(|(_, n)| n.as_str());
    match kind {
        AtBind::Copy => None,
        AtBind::Move => {
            if let Some(b) = first(&|k| matches!(k, AtBind::Ref(_))) {
                return Some(format!("borrow of moved value: the value is moved into `{name}`, and `{b}` borrows it after the move"));
            }
            first(&|k| k == AtBind::Move).map(|b| {
                if whole {
                    format!("use of moved value: `{name}` (the value was moved into `{b}`) (E0382)")
                } else {
                    format!("use of partially moved value: `{name}` (a part of it was moved into `{b}`) (E0382)")
                }
            })
        }
        AtBind::Ref(outer_mut) => {
            if let Some(b) = first(&|k| outer_mut && k == AtBind::Ref(true)) {
                return Some(format!("cannot borrow value as mutable more than once at a time: `{name}` and `{b}`"));
            }
            if let Some(b) = first(&|k| matches!(k, AtBind::Ref(m) if m != outer_mut)) {
                return Some(if outer_mut {
                    format!("cannot borrow value as immutable because it is also borrowed as mutable: `{name}` and `{b}`")
                } else {
                    format!("cannot borrow value as mutable because it is also borrowed as immutable: `{name}` and `{b}`")
                });
            }
            first(&|k| k == AtBind::Move).map(|b| format!("cannot move out of value because it is borrowed: `{name}` borrows it, and `{b}` moves out of it"))
        }
    }
}

pub(crate) fn holds_mut_ref(prog: &crate::program::Program, t: Ty, depth: u32) -> bool {
    if depth > 16 {
        return false;
    }
    match t {
        Ty::Ref(true, _) => true,
        Ty::Array(e, _) => holds_mut_ref(prog, *e, depth + 1),
        Ty::Struct(s) => {
            let sd = &prog.structs[s as usize];
            sd.placeholder.is_none()
                && (sd.fields.iter().any(|f| holds_mut_ref(prog, f.1, depth + 1)) || sd.variants.iter().any(|v| v.fields.iter().any(|f| holds_mut_ref(prog, f.1, depth + 1))))
        }
        _ => false,
    }
}

/// rustc's E0023 of tuple_fields' "this pattern has N fields, but the corresponding variant has M": of a tuple variant
/// or a tuple struct, "field" for one (rustc 1.100)
fn e0023(m: String, variant: bool) -> String {
    let Some((n, r)) = m.strip_prefix("this pattern has ").and_then(|r| r.split_once(" fields, but the corresponding variant has ")) else { return m };
    let (Ok(a), Ok(b)) = (n.parse::<usize>(), r.parse::<usize>()) else { return m };
    let f = |k: usize| if k == 1 { "field" } else { "fields" };
    format!("this pattern has {a} {}, but the corresponding tuple {} has {b} {} (E0023)", f(a), if variant { "variant" } else { "struct" }, f(b))
}

/// The pattern of a destructuring assignment's `let` (parse::destructure_assign): its names are `__exr_da..`
fn destructure_assign_pat(p: &Pat) -> bool {
    match p {
        Pat::Tuple(ps) | Pat::Slice(ps) => ps.iter().any(destructure_assign_pat),
        Pat::Ident(_, _, n, _) => n.starts_with("__exr_da"),
        _ => false,
    }
}
