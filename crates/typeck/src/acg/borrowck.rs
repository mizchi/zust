//! Borrow checking on the CFG that acg emits.
//!
//! zustc has no MIR: acg lowers the AST straight to minclif. While it does so it also records, per minclif block,
//! the events that matter to the borrow checker (a local is bound, read, moved, written, borrowed, goes out of scope),
//! and the jumps between blocks. After the function body is lowered, [`Bk::check`] runs two dataflow analyses over
//! that CFG:
//!
//! 1. Liveness of locals (backward): a local is live at a point if a later read of it can be reached without
//!    an assignment in between.
//! 2. Moves and loans (forward): the paths that may be moved out or uninitialized, and for each local the set of
//!    loans its value may hold. A loan is live at a point if a live local may hold it (the Polonius view: loans flow
//!    through origins, here one origin per local, and a loan matters only while an origin holding it is live).
//!    An access that conflicts with a live loan is an error, as is any access to a path that may be moved.
//!
//! What flows into a local is approximated from the initializer: every loan created while evaluating it and every
//! loan held by a local (of a type that can hold references) read while evaluating it. Loans that are never stored
//! in a local (`v.push(v.len())`, `f(&mut x)`) are not live after the expression, which gives two-phase borrows and
//! the conditional-return cases (NLL problem case 3) for free; the price is that conflicts inside one expression are
//! not found.

use super::{Loc, Sp};
use crate::fxhash::FxMap as HashMap;
use crate::ir::Block;
use crate::program::Program;
use crate::types::Ty;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Proj {
    Field(String),
    /// `*x` (true: through a Box, which the local owns; false: through a `&mut`)
    Deref(bool),
    /// `*x` through a `&`
    Shr,
    Index,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BPlace {
    pub local: u32,
    pub proj: Vec<Proj>,
}

/// The name of a temporary's hidden local
pub const TEMP: &str = "<temporary>";

/// The deref of a `&mut` / `&`
pub fn ref_deref(m: bool) -> Proj {
    if m { Proj::Deref(false) } else { Proj::Shr }
}

impl BPlace {
    /// The part of the place the local owns (up to the first deref of a reference)
    fn owned_len(&self) -> usize {
        self.proj.iter().position(|p| matches!(p, Proj::Deref(false) | Proj::Shr)).unwrap_or(self.proj.len())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Acc {
    Read,
    Move,
    /// Assignment (the old value is overwritten)
    Write,
    /// `&` / `&mut` (the loan)
    Borrow(u32),
    /// `x += 1` (reads and writes; the value must be initialized)
    Modify,
    /// The local is dropped at the end of its scope and its type's `Drop` may use what it borrows (drop check): it
    /// is live until then. Not a use of a moved value (a moved one is not dropped), nor a conflict with its loans
    Drop,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src {
    Loan(u32),
    Local(u32),
}

#[derive(Clone, Debug)]
pub enum Ev {
    /// The local comes into scope (initialized or not)
    Bind(u32, bool),
    /// The local goes out of scope
    Dead(u32, Sp),
    Acc(BPlace, Acc, Sp),
    /// The local now holds exactly these loans (assignment of the whole local)
    Def(u32, Vec<Src>),
    /// The local also holds these loans (assignment to a part of it)
    Flow(u32, Vec<Src>),
    /// The function returns a value holding these loans
    Ret(Vec<Src>, Sp),
    /// These loans are stored behind a reference the function was given
    Escape(Vec<Src>),
    /// The local keeps these loans (what its initializer borrows directly): they must not be of a dead local
    Keep(u32, Vec<Src>),
}

#[derive(Clone, Debug)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
    pub copy: bool,
    /// The type may hold references (so the local may hold loans)
    pub refs: bool,
    /// `let mut` (None: not known)
    pub mutable: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct Loan {
    pub place: BPlace,
    pub mutable: bool,
    pub sp: Sp,
    /// Where a mutability error is reported (a closure's loan: the use in its body)
    pub at: Sp,
    /// Made without an access of its own (`for x in &mut v`): the accesses acg makes at its position are its own
    pub quiet: bool,
    /// A closure's borrow of what it captures by reference
    pub capture: bool,
}

/// What the checker found (the first error by source position is reported)
#[derive(Clone, Debug)]
pub struct BkError {
    pub sp: Sp,
    pub msg: String,
}

#[derive(Default)]
pub struct Bk {
    /// Record events (user code only, not while inlining)
    pub on: bool,
    /// What to check (ZUSTC_BORROWCK: 0 = nothing, 1 = moves only, 2 (default) = moves and loans)
    pub level: u8,
    pub locals: Vec<Local>,
    pub by_loc: HashMap<Loc, u32>,
    pub evs: Vec<(Block, Ev)>,
    pub edges: Vec<(Block, Block)>,
    pub loans: Vec<Loan>,
    /// Sources flowing into the value being computed (one frame per `let` initializer / assigned value)
    pub frames: Vec<Vec<Src>>,
    /// Nesting of `place()`: only the outermost records an access
    pub place_depth: u32,
    /// The next outermost `place()` is the left side of an assignment
    pub write_next: bool,
    /// The next bound local has no initializer (`let x;`)
    pub uninit_next: bool,
    /// Mutability of the next bound local
    pub mut_next: Option<bool>,
    /// What the locals bound by a pattern hold (the loans of the scrutinee)
    pub bind_srcs: Option<Vec<Src>>,
    /// What the last scrutinee holds
    pub scrut: Vec<Src>,
    /// The last scrutinee as a place, and its position
    pub scrut_place: Option<(BPlace, Sp)>,
    /// The expression being lowered moves a `&mut T` it reads (`let b = a;`, `{a}`), not reborrows it
    pub exp_none: bool,
    /// The next expression lowered is such a place
    pub move_mut_next: bool,
    /// The place the pattern being bound matches (the scrutinee, then its parts)
    pub pat_path: Option<BPlace>,
    /// The method call an operator was rewritten to (its address)
    pub op_call: usize,
    /// The method call a compound assignment was rewritten to (`a += b` → `a.add_assign(b)`): its `&mut self` is
    /// two-phase, a shared reservation while the right side is evaluated
    pub op_assign_call: usize,
    /// A compound assignment's `&mut self` loan and the call temporary of its reservation: the next call_fn takes the
    /// loan once its arguments are evaluated, before their loans end (`s += &s` conflicts), ending the reservation first
    pub activate: Option<(Ev, u32)>,
    /// The `index_mut` call acg makes to read an indexed place (its receiver is seen as shared)
    pub shared_recv: usize,
    /// Binding the pattern of an arm with a guard: a binding by value does not move until the guard holds (the next
    /// arm may bind the same place again)
    pub guard_arm: bool,
    /// The place being lowered is written or borrowed mutably (`*x = ..`, `&mut *x`): a `deref_mut` acg makes
    /// for it is a mutable borrow
    pub place_mut: bool,
    /// The tail expression of the function body (its address)
    pub tail: usize,
    /// Loan made for an expression (`&x`: the address of x; `x.m()` with a borrowed receiver: of the call)
    pub loan_of: HashMap<usize, u32>,
    /// The next `&e` is an argument the call's result does not borrow from (its loan does not flow on)
    pub no_loan_src: bool,
    /// Hidden locals holding the loans of the arguments of the calls being lowered (live until the call)
    pub call_temps: Vec<u32>,
    pub zst: u32,
    /// Line of the item (positions in messages are absolute)
    pub line_base: u32,
    /// The locals that are parameters
    pub params: Vec<u32>,
    /// Type → (Copy, may hold references)
    pub ty_info: HashMap<Ty, (bool, bool)>,
    /// Hidden locals of temporaries borrowed in the statements being lowered (they go out of scope at the end of
    /// their statement)
    pub stmt_temps: Vec<u32>,
    /// The temporaries of the `let` initializers being lowered that are followed (see direct_temps): their addresses
    pub direct: Vec<usize>,
    /// Loans the region check found must outlive the function (a reborrow through a parameter flowing into a
    /// universal region): live from where each is made to the end (sorted)
    pub forever: Vec<u32>,
    /// The loans of each closure's captures by reference, by the closure's address
    pub cap_loans: HashMap<usize, Vec<u32>>,
    /// Loans the region check found must outlive a region of locals' types (sorted by loan): those locals hold them
    pub held_by: Vec<(u32, Vec<u32>)>,
    /// The loan of each method call's receiver, by the call's address (while the region check is on)
    pub recv_loans: HashMap<usize, u32>,
    /// The scrutinee being lowered is a `let`'s initializer (its temporary is extended)
    pub let_scrut: bool,
    /// The temporaries of the `let` initializers being lowered that the `let` extends (`let x = &f();`): their
    /// addresses (see direct)
    pub extended: Vec<usize>,
    /// Hidden locals of extended temporaries, with the depth of the scope they go out of scope with
    pub scope_temps: Vec<(usize, u32)>,
}

/// The temporaries of a `let` initializer that are followed: the operands of `&` passed directly to its outermost call
/// (`let x = defer(&vec![..]);`). What a call returns borrows from those exactly as its signature says; other
/// temporaries are left alone (the loans that flow into a local are approximated, and a temporary whose lifetime the
/// `let` extends (`let x = &f();`, `S { r: &f() }`) lives as long as the block)
pub fn direct_temps(e: &Expr, out: &mut Vec<usize>) {
    fn strip(mut e: &Expr) -> &Expr {
        while let Expr::Paren(p) = e {
            e = p;
        }
        e
    }
    let args = match strip(e) {
        Expr::Call(_, a) => a,
        Expr::MethodCall(_, _, _, a, _) => a,
        _ => return,
    };
    for a in args {
        direct_ref(a, out);
    }
}

/// `&e` with e a temporary that is never promoted to a static: the result of a call of a fn (not a struct's or
/// variant's constructor), of a method, of `vec!` / `format!` (its addresses, for direct_temps)
pub fn direct_ref(a: &Expr, out: &mut Vec<usize>) {
    fn strip(mut e: &Expr) -> &Expr {
        while let Expr::Paren(p) = e {
            e = p;
        }
        e
    }
    if let Expr::Ref(_, x) = strip(a) {
        let made = match strip(x) {
            Expr::MethodCall(..) => true,
            Expr::Call(f, _) => matches!(strip(f), Expr::Path(_, p) if p.segs.last().is_some_and(|s| s.name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_'))),
            Expr::Macro(p, ..) => p.segs.last().is_some_and(|s| s.name == "vec" || s.name == "format"),
            _ => false,
        };
        if made {
            out.push(&**x as *const Expr as usize);
            out.push(strip(x) as *const Expr as usize);
        }
    }
}

pub fn level() -> u8 {
    match std::env::var("ZUSTC_BORROWCK") {
        Ok(v) => v.parse().unwrap_or(2),
        Err(_) => 2,
    }
}

/// `Copy` for the borrow checker. Unlike `Program::implements`, a derived `Copy` on a generic type also needs its
/// type arguments to be `Copy`, and `&mut T` counts as `Copy` (it is reborrowed where it is passed, which is what
/// almost every use does)
pub fn is_copy(prog: &Program, t: Ty, depth: u32) -> bool {
    if depth > 16 {
        return true;
    }
    match t {
        Ty::Ref(..) | Ty::Ptr(..) | Ty::Fn(_) | Ty::Never | Ty::Unit | Ty::Param(_) => true,
        Ty::Array(e, _) => is_copy(prog, *e, depth + 1),
        Ty::Slice(_) | Ty::Str | Ty::Dyn(_) | Ty::Box(_) => false,
        Ty::Struct(sid) => {
            let sd = &prog.structs[sid as usize];
            if sd.symbolic {
                return true;
            }
            if prog.closure_of_struct.contains_key(&prog.base_of(sid)) || prog.is_tuple(sid) {
                return sd.fields.iter().all(|f| is_copy(prog, f.1, depth + 1));
            }
            // (a derived `Copy` on `Option<T>` needs `T: Copy`: look at the fields of the instance)
            prog.impls.contains(&(prog.base_of(sid), "Copy".to_string()))
                && sd.fields.iter().all(|f| is_copy(prog, f.1, depth + 1))
                && sd.variants.iter().all(|v| v.fields.iter().all(|f| is_copy(prog, f.1, depth + 1)))
        }
        _ => true,
    }
}

/// The prelude's iterator and view types: raw pointers into what they were made from, so they hold the loan of it
const VIEWS: &[&str] = &[
    "Windows", "Chunks", "ChunksExact", "Iter", "IterMut", "Chars", "DebugStruct", "DebugTuple", "DebugMap", "MutexGuard", "HmIter",
    "HmIterMut", "BtIter", "BtIterMut", "DqIter", "Split", "SplitStr", "MatchIndices", "RSplitStr", "CharIndices", "Bytes", "SplitFn",
];

pub fn is_view(prog: &Program, sid: u32) -> bool {
    let b = prog.base_of(sid);
    VIEWS.contains(&prog.structs[b as usize].name.as_str()) && prog.struct_item.get(b as usize).is_some_and(|&i| i >= prog.prelude_from)
}

/// Whether a value of type t may hold a reference (and so a loan)
pub fn has_refs(prog: &Program, t: Ty, _depth: u32) -> bool {
    fn go(prog: &Program, t: Ty, seen: &mut Vec<u32>, memo: &mut HashMap<u32, bool>) -> bool {
        match t {
            Ty::Ref(..) => true,
            Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => go(prog, *i, seen, memo),
            Ty::Dyn(_) | Ty::Param(_) => true,
            Ty::Struct(sid) => {
                // (a recursive type: what it holds is what the rest of it holds)
                if seen.contains(&sid) {
                    return false;
                }
                if let Some(&r) = memo.get(&sid) {
                    return r;
                }
                if seen.len() > 64 {
                    return true;
                }
                seen.push(sid);
                let sd = &prog.structs[sid as usize];
                // (a closure holds what it captures by reference as raw pointers; one with no `move` that holds a copy
                // of a value it uses borrows it, as rustc's closures capture what they only read by reference)
                let cd = prog.closure_of_struct.get(&prog.base_of(sid)).and_then(|c| prog.closures.get(c));
                // (`by_move` is also set when every capture is by value: the `move` keyword is not known here)
                let clo = cd.is_some_and(|cd| sd.fields.iter().any(|f| matches!(f.1, Ty::Ptr(..))) || cd.moved.iter().any(|&m| m))
                    || is_view(prog, sid);
                let r = clo
                    || sd.symbolic
                    || sd.fields.iter().any(|f| go(prog, f.1, seen, memo))
                    || sd.variants.iter().any(|v| v.fields.iter().any(|f| go(prog, f.1, seen, memo)))
                    || sd.args.iter().any(|&a| go(prog, a, seen, memo));
                seen.pop();
                memo.insert(sid, r);
                r
            }
            _ => false,
        }
    }
    go(prog, t, &mut vec![], &mut HashMap::default())
}

fn at(lb: u32, sp: Sp) -> String {
    format!("{}:{}", lb + sp.line, sp.col + 1)
}

fn path_str(locals: &[Local], p: &BPlace) -> String {
    let mut s = locals[p.local as usize].name.clone();
    for (i, pr) in p.proj.iter().enumerate() {
        match pr {
            Proj::Field(f) => {
                s.push('.');
                s.push_str(f);
            }
            Proj::Deref(_) | Proj::Shr => {
                // (auto-deref before a field is not written)
                if !matches!(p.proj.get(i + 1), Some(Proj::Field(_))) {
                    s.insert(0, '*');
                }
            }
            Proj::Index => s.push_str("[..]"),
        }
    }
    s
}

/// a is a prefix of b
fn prefix(a: &[Proj], b: &[Proj]) -> bool {
    a.len() <= b.len() && a.iter().zip(b).all(|(x, y)| x == y || (*x == Proj::Index && *y == Proj::Index))
}

/// The two places overlap (one contains the other)
fn overlap(a: &BPlace, b: &BPlace) -> bool {
    a.local == b.local && (prefix(&a.proj, &b.proj) || prefix(&b.proj, &a.proj))
}

/// A path that may be moved out (or was never initialized)
#[derive(Clone, PartialEq, Eq, Debug)]
struct Moved {
    place: BPlace,
    uninit: bool,
    sp: Sp,
}

#[derive(Clone, PartialEq, Default, Debug)]
struct State {
    moved: Vec<Moved>,
    /// Immutable locals that may have been assigned (sorted): assigning them again is an error
    inited: Vec<u32>,
    /// local → loans it may hold (sorted)
    holds: Vec<(u32, Vec<u32>)>,
    /// Loans of locals that went out of scope (a value made in a block may still carry one out)
    dead: Vec<u32>,
    /// Loans stored behind a reference the function was given (`*x = &local`): they outlive the function
    escaped: Vec<u32>,
}

fn add_sorted(v: &mut Vec<u32>, x: u32) -> bool {
    match v.binary_search(&x) {
        Ok(_) => false,
        Err(i) => {
            v.insert(i, x);
            true
        }
    }
}

impl State {
    fn join(&mut self, o: &State) -> bool {
        let mut ch = false;
        for x in &o.inited {
            if let Err(i) = self.inited.binary_search(x) {
                self.inited.insert(i, *x);
                ch = true;
            }
        }
        for &x in &o.dead {
            ch |= add_sorted(&mut self.dead, x);
        }
        for &x in &o.escaped {
            ch |= add_sorted(&mut self.escaped, x);
        }
        for m in &o.moved {
            if !self.moved.iter().any(|x| x.place == m.place && x.uninit == m.uninit) {
                self.moved.push(m.clone());
                ch = true;
            }
        }
        for (l, ls) in &o.holds {
            match self.holds.iter_mut().find(|h| h.0 == *l) {
                Some(h) => {
                    for x in ls {
                        if let Err(i) = h.1.binary_search(x) {
                            h.1.insert(i, *x);
                            ch = true;
                        }
                    }
                }
                None => {
                    self.holds.push((*l, ls.clone()));
                    ch = true;
                }
            }
        }
        ch
    }
    /// The local was overwritten: the loans through it are of the old value (Polonius' loan_killed_at)
    fn kill_loans_of(&mut self, l: u32, loans: &[Loan]) {
        for h in &mut self.holds {
            h.1.retain(|&ln| loans[ln as usize].place.local != l);
        }
    }
    fn held(&self, l: u32) -> &[u32] {
        self.holds.iter().find(|h| h.0 == l).map(|h| h.1.as_slice()).unwrap_or(&[])
    }
    fn set_holds(&mut self, l: u32, mut ls: Vec<u32>) {
        ls.sort_unstable();
        ls.dedup();
        match self.holds.iter_mut().find(|h| h.0 == l) {
            Some(h) => h.1 = ls,
            None => self.holds.push((l, ls)),
        }
    }
}

impl Bk {
    pub fn fresh_zst(&mut self) -> u32 {
        self.zst += 1;
        self.zst
    }

    pub fn ev(&mut self, b: Option<Block>, e: Ev) {
        if let Some(b) = b {
            self.evs.push((b, e));
        }
    }

    /// The sources a list of events adds to the open frame
    pub fn src(&mut self, s: Src) {
        if let Some(f) = self.frames.last_mut() {
            if !f.contains(&s) {
                f.push(s);
            }
        }
    }

    fn resolve(&self, srcs: &[Src], st: &State) -> Vec<u32> {
        let mut out = vec![];
        for s in srcs {
            match *s {
                Src::Loan(l) => out.push(l),
                Src::Local(x) => out.extend_from_slice(st.held(x)),
            }
        }
        out
    }

    /// Run the analyses. `nblocks` is the number of blocks of the function (block 0 is the entry)
    pub fn check(&self, nblocks: usize) -> Option<BkError> {
        if self.level == 0 || self.evs.is_empty() {
            return None;
        }
        if std::env::var_os("ZUSTC_BK_DEBUG").is_some() {
            for (i, l) in self.locals.iter().enumerate() {
                eprintln!("local {i}: {} refs={} copy={} mut={:?}", l.name, l.refs, l.copy, l.mutable);
            }
            for (i, l) in self.loans.iter().enumerate() {
                eprintln!("loan {i}: {:?} mut={}", l.place, l.mutable);
            }
            for (b, e) in &self.evs {
                eprintln!("b{}: {e:?}", b.0);
            }
            eprintln!("edges: {:?}", self.edges.iter().map(|e| (e.0 .0, e.1 .0)).collect::<Vec<_>>());
        }
        // ---- location-insensitive pre-pass (as Polonius' LocationInsensitive / Hybrid): if no error is possible when
        // control flow is ignored, the dataflow is not needed. The mutability errors that do not depend on the flow
        // are found here
        let mut errs: Vec<BkError> = vec![];
        let (mut need_moves, mut need_loans, mut need_inited) = (false, false, false);
        let mut by_local: HashMap<u32, Vec<u32>> = HashMap::default();
        if self.level >= 2 {
            for (i, lo) in self.loans.iter().enumerate() {
                by_local.entry(lo.place.local).or_default().push(i as u32);
            }
        }
        for (_, e) in &self.evs {
            match e {
                Ev::Bind(_, false) => need_moves = true,
                Ev::Acc(p, a, sp) => {
                    if *a == Acc::Move {
                        need_moves = true;
                    }
                    let lo = &self.locals[p.local as usize];
                    if *a == Acc::Write && p.proj.is_empty() {
                        need_inited |= lo.mutable == Some(false);
                    } else {
                        self.check_mut(&State::default(), p, *a, *sp, &mut errs);
                    }
                    if !need_loans {
                        if let Some(ls) = by_local.get(&p.local) {
                            let mine = if let Acc::Borrow(l) = a { Some(*l) } else { None };
                            let wants_mut = match a {
                                Acc::Read => false,
                                Acc::Borrow(l) => self.loans[*l as usize].mutable,
                                _ => true,
                            };
                            need_loans = ls.iter().any(|&l| {
                                Some(l) != mine && (wants_mut || self.loans[l as usize].mutable) && overlap(&self.loans[l as usize].place, p)
                            });
                        }
                    }
                }
                Ev::Dead(l, _) => {
                    if by_local.contains_key(l) {
                        need_loans = true;
                    }
                }
                Ev::Ret(..) | Ev::Escape(..) => need_loans |= self.level >= 2,
                _ => {}
            }
        }
        if !(need_moves || need_loans || need_inited) {
            errs.sort_by_key(|e| (e.sp.line, e.sp.col));
            return errs.into_iter().next();
        }
        let n = nblocks.max(1 + self.evs.iter().map(|e| e.0 .0 as usize).max().unwrap_or(0)).max(1 + self.edges.iter().map(|e| e.0 .0.max(e.1 .0) as usize).max().unwrap_or(0));
        let mut evs: Vec<Vec<&Ev>> = vec![vec![]; n];
        for (b, e) in &self.evs {
            evs[b.0 as usize].push(e);
        }
        let mut succ: Vec<Vec<usize>> = vec![vec![]; n];
        let mut pred: Vec<Vec<usize>> = vec![vec![]; n];
        for &(a, b) in &self.edges {
            if !succ[a.0 as usize].contains(&(b.0 as usize)) {
                succ[a.0 as usize].push(b.0 as usize);
                pred[b.0 as usize].push(a.0 as usize);
            }
        }
        let nl = self.locals.len();
        let loans_on = need_loans;
        // ---- drop check: a drop of a local that is certainly moved out runs nothing (NLL's drop-liveness counts only
        // the maybe-initialized). Forward: the locals moved out on every path, joined by intersection
        if evs.iter().flatten().any(|e| matches!(e, Ev::Acc(_, Acc::Drop, _))) {
            let words = nl.div_ceil(64).max(1);
            let step = |m: &mut Vec<u64>, e: &Ev| match e {
                Ev::Acc(p, Acc::Move, _) if p.proj.is_empty() => m[p.local as usize / 64] |= 1 << (p.local % 64),
                Ev::Acc(p, Acc::Write, _) if p.proj.is_empty() => m[p.local as usize / 64] &= !(1 << (p.local % 64)),
                Ev::Bind(l, _) | Ev::Def(l, _) => m[*l as usize / 64] &= !(1 << (l % 64)),
                _ => {}
            };
            let mut min: Vec<Option<Vec<u64>>> = vec![None; n];
            min[0] = Some(vec![0u64; words]);
            let mut work = vec![0usize];
            let mut rounds = 0usize;
            while let Some(b) = work.pop() {
                rounds += 1;
                if rounds > n.saturating_mul(nl + 2).saturating_add(16) {
                    break;
                }
                let mut m = min[b].clone().unwrap();
                for e in &evs[b] {
                    step(&mut m, e);
                }
                for &s2 in &succ[b] {
                    let changed = match &mut min[s2] {
                        Some(x) => {
                            let mut ch = false;
                            for (a, y) in x.iter_mut().zip(&m) {
                                let v = *a & y;
                                ch |= v != *a;
                                *a = v;
                            }
                            ch
                        }
                        slot @ None => {
                            *slot = Some(m.clone());
                            true
                        }
                    };
                    if changed && !work.contains(&s2) {
                        work.push(s2);
                    }
                }
            }
            for b in 0..n {
                let Some(mut m) = min[b].clone() else { continue };
                let mut keep: Vec<&Ev> = Vec::with_capacity(evs[b].len());
                for &e in &evs[b] {
                    if let Ev::Acc(p, Acc::Drop, _) = e {
                        if m[p.local as usize / 64] >> (p.local % 64) & 1 == 1 {
                            continue;
                        }
                    }
                    step(&mut m, e);
                    keep.push(e);
                }
                evs[b] = keep;
            }
        }
        // ---- liveness of locals (backward): live_in[b] as a bit set
        let words = nl.div_ceil(64).max(1);
        let mut live_in = vec![vec![0u64; words]; n];
        let mut live_out = vec![vec![0u64; words]; n];
        let set = |v: &mut [u64], i: u32| v[i as usize / 64] |= 1 << (i % 64);
        let clr = |v: &mut [u64], i: u32| v[i as usize / 64] &= !(1 << (i % 64));
        // (a read of a local that may hold loans keeps it live; only those matter for loans)
        let transfer_back = |live: &mut Vec<u64>, e: &Ev| match e {
            Ev::Acc(p, a, _) => {
                if *a == Acc::Write && p.proj.is_empty() {
                    clr(live, p.local);
                } else {
                    set(live, p.local);
                }
            }
            Ev::Def(l, _) => clr(live, *l),
            Ev::Bind(l, _) | Ev::Dead(l, _) => clr(live, *l),
            _ => {}
        };
        if loans_on {
            let mut changed = true;
            while changed {
                changed = false;
                for b in (0..n).rev() {
                    let mut out = vec![0u64; words];
                    for &s in &succ[b] {
                        for (o, x) in out.iter_mut().zip(&live_in[s]) {
                            *o |= x;
                        }
                    }
                    let mut live = out.clone();
                    for e in evs[b].iter().rev() {
                        transfer_back(&mut live, e);
                    }
                    if live != live_in[b] {
                        live_in[b] = live;
                        changed = true;
                    }
                    live_out[b] = out;
                }
            }
        }
        // ---- moves and loans (forward)
        let mut st_in: Vec<Option<State>> = vec![None; n];
        st_in[0] = Some(State::default());
        let mut work: Vec<usize> = vec![0];
        let mut on_work = vec![false; n];
        on_work[0] = true;
        // (the fixpoint is reached: a block is put on the worklist only when the state at its entry grew, and a state
        // grows at most `height` times. It is the union of: moved paths (from Move accesses and uninitialized
        // Binds, each moved or uninit), inited locals, a holds entry per local and its loans, dead and escaped loans.
        // So there are at most n * (height + 1) + 1 rounds; past that the analysis would be wrong, and a program is
        // not accepted on a wrong analysis)
        let (nl, nloans) = (self.locals.len(), self.loans.len());
        let nmoved = self.evs.iter().filter(|e| matches!(e.1, Ev::Acc(_, Acc::Move, _) | Ev::Bind(..))).count();
        let height = (2 * nmoved).saturating_add(nl).saturating_add(nl.saturating_mul(nloans + 1)).saturating_add(2 * nloans);
        let max_rounds = n.saturating_mul(height.saturating_add(1)).saturating_add(1);
        let mut rounds = 0usize;
        while let Some(b) = work.pop() {
            on_work[b] = false;
            rounds += 1;
            if rounds > max_rounds {
                debug_assert!(false, "borrow check: no fixpoint after {rounds} rounds");
                return Some(BkError { sp: super::SP, msg: "internal: the borrow check did not reach a fixpoint".to_string() });
            }
            let mut st = st_in[b].clone().unwrap();
            self.run_block(&evs[b], &mut st, None, &live_out[b], loans_on, &mut vec![]);
            for &s in &succ[b] {
                let ch = match &mut st_in[s] {
                    Some(x) => x.join(&st),
                    slot @ None => {
                        *slot = Some(st.clone());
                        true
                    }
                };
                if ch && !on_work[s] {
                    on_work[s] = true;
                    work.push(s);
                }
            }
        }
        // ---- report (on the fixpoint)
        for b in 0..n {
            let Some(st) = &st_in[b] else { continue };
            let mut st = st.clone();
            self.run_block(&evs[b], &mut st, Some(&mut errs), &live_out[b], loans_on, &mut vec![]);
            // (the function ends here (return / panic): a loan stored behind a parameter must not be of a local)
            if succ[b].is_empty() {
                for &ln in &st.escaped {
                    let lo = &self.loans[ln as usize];
                    if lo.place.owned_len() == lo.place.proj.len() && !self.params.contains(&lo.place.local) {
                        let name = path_str(&self.locals, &lo.place);
                        errs.push(BkError { sp: lo.sp, msg: format!("`{name}` does not live long enough (stored where it outlives the function)") });
                    }
                }
            }
        }
        errs.sort_by_key(|e| (e.sp.line, e.sp.col));
        errs.into_iter().next()
    }

    /// Apply the events of one block (and report errors when errs is given). `live_out` is the liveness at the end of
    /// the block; the liveness at each event is recomputed backward from it
    fn run_block(&self, evs: &[&Ev], st: &mut State, mut errs: Option<&mut Vec<BkError>>, live_out: &[u64], loans_on: bool, live_at: &mut Vec<Vec<u64>>) {
        // (liveness after each event: only for reporting)
        let loans_on = loans_on && errs.is_some();
        if loans_on {
            live_at.clear();
            live_at.resize(evs.len(), vec![]);
            let mut live = live_out.to_vec();
            for (i, e) in evs.iter().enumerate().rev() {
                live_at[i] = live.clone();
                match e {
                    Ev::Acc(p, a, _) => {
                        let w = &mut live[p.local as usize / 64];
                        if *a == Acc::Write && p.proj.is_empty() {
                            *w &= !(1 << (p.local % 64));
                        } else {
                            *w |= 1 << (p.local % 64);
                        }
                    }
                    Ev::Def(l, _) | Ev::Bind(l, _) | Ev::Dead(l, _) => live[*l as usize / 64] &= !(1 << (l % 64)),
                    _ => {}
                }
            }
        }
        let is_live = |live: &[u64], l: u32| live.get(l as usize / 64).is_some_and(|w| w >> (l % 64) & 1 == 1);
        // (the loans live at a point: held by a local that is live after the point, or stored where they outlive
        // the function)
        let live_loans = |st: &State, live: &[u64]| -> Vec<u32> {
            let mut out = st.escaped.clone();
            for (l, ls) in &st.holds {
                if is_live(live, *l) {
                    out.extend_from_slice(ls);
                }
            }
            out.sort_unstable();
            out.dedup();
            out
        };
        for (i, e) in evs.iter().enumerate() {
            match e {
                Ev::Bind(l, init) => {
                    st.moved.retain(|m| m.place.local != *l);
                    if let Ok(i) = st.inited.binary_search(l) {
                        st.inited.remove(i);
                    }
                    if !*init {
                        st.moved.push(Moved { place: BPlace { local: *l, proj: vec![] }, uninit: true, sp: super::SP });
                    } else if self.locals[*l as usize].mutable == Some(false) {
                        st.inited.push(*l);
                        st.inited.sort_unstable();
                    }
                    st.holds.retain(|h| h.0 != *l);
                    st.kill_loans_of(*l, &self.loans);
                }
                Ev::Dead(l, sp) => {
                    for (ln, lo) in self.loans.iter().enumerate() {
                        if lo.place.local == *l && lo.place.owned_len() == lo.place.proj.len() {
                            add_sorted(&mut st.dead, ln as u32);
                            if let (true, Some(errs)) = (st.escaped.binary_search(&(ln as u32)).is_ok(), errs.as_deref_mut()) {
                                let name = path_str(&self.locals, &lo.place);
                                if name == TEMP {
                                    errs.push(BkError { sp: lo.sp, msg: "temporary value dropped while borrowed (E0716)".to_string() });
                                    continue;
                                }
                                errs.push(BkError { sp: lo.sp, msg: format!("`{name}` does not live long enough (stored where it outlives the function)") });
                            }
                        }
                    }
                    if loans_on {
                        if let Some(errs) = errs.as_deref_mut() {
                            // (a loan of the local, or of what it owns, still live when it goes away)
                            for ln in live_loans(st, &live_at[i]) {
                                let lo = &self.loans[ln as usize];
                                if lo.place.local == *l && lo.place.owned_len() == lo.place.proj.len() {
                                    let name = path_str(&self.locals, &BPlace { local: *l, proj: vec![] });
                                    if name == TEMP {
                                        errs.push(BkError { sp: lo.sp, msg: "temporary value dropped while borrowed: it is freed while still in use (E0716)".to_string() });
                                        continue;
                                    }
                                    errs.push(BkError { sp: lo.sp, msg: if sp.line == 0 { format!("`{name}` does not live long enough (dropped while still borrowed)") } else { format!("`{name}` does not live long enough (borrowed value dropped at {} while still borrowed)", at(self.line_base, *sp)) } });
                                }
                            }
                        }
                    }
                    st.moved.retain(|m| m.place.local != *l);
                    st.holds.retain(|h| h.0 != *l);
                }
                Ev::Acc(_, Acc::Drop, _) => {}
                Ev::Acc(p, a, sp) => {
                    if let Some(errs) = errs.as_deref_mut() {
                        if *a == Acc::Write && p.proj.is_empty() {
                            self.check_mut(st, p, *a, *sp, errs);
                        }
                        self.check_moved(st, p, *a, *sp, errs);
                        if loans_on {
                            let ll = live_loans(st, &live_at[i]);
                            self.check_loans(&ll, p, *a, *sp, errs);
                        }
                    }
                    if let (Acc::Move, Some(errs)) = (a, errs.as_deref_mut()) {
                        if p.owned_len() < p.proj.len() || p.proj.contains(&Proj::Index) {
                            let what = if p.owned_len() < p.proj.len() { "which is behind a reference" } else { "an indexed place" };
                            errs.push(BkError { sp: *sp, msg: format!("cannot move out of `{}`, {what}", path_str(&self.locals, p)) });
                        }
                    }
                    match a {
                        // (a new instance of the loan: the one of the previous iteration of a loop is gone)
                        Acc::Borrow(ln) => {
                            if let Ok(i) = st.dead.binary_search(ln) {
                                st.dead.remove(i);
                            }
                            if self.forever.binary_search(ln).is_ok() {
                                add_sorted(&mut st.escaped, *ln);
                            }
                            if let Ok(k) = self.held_by.binary_search_by_key(ln, |x| x.0) {
                                for &h in &self.held_by[k].1 {
                                    let mut ls = st.held(h).to_vec();
                                    ls.push(*ln);
                                    st.set_holds(h, ls);
                                }
                            }
                        }
                        Acc::Move => {
                            let own = p.owned_len();
                            if own == p.proj.len() && !p.proj.contains(&Proj::Index) {
                                st.moved.push(Moved { place: p.clone(), uninit: false, sp: *sp });
                            }
                        }
                        Acc::Write => {
                            // (reinitializes the path and everything under it)
                            if p.owned_len() == p.proj.len() {
                                st.moved.retain(|m| !(m.place.local == p.local && prefix(&p.proj, &m.place.proj)));
                            }
                            if p.proj.is_empty() {
                                st.holds.retain(|h| h.0 != p.local);
                                st.kill_loans_of(p.local, &self.loans);
                                if self.locals[p.local as usize].mutable == Some(false) {
                                    if let Err(i) = st.inited.binary_search(&p.local) {
                                        st.inited.insert(i, p.local);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ev::Keep(l, srcs) => {
                    // (a loan of a local already gone, kept by a local still used: `let y = { let z = 0; &z };`)
                    if let Some(errs) = errs.as_deref_mut() {
                        if loans_on && is_live(&live_at[i], *l) {
                            let ls = self.resolve(srcs, st);
                            if let Some(&ln) = ls.iter().find(|ln| st.dead.binary_search(ln).is_ok()) {
                                let lo = &self.loans[ln as usize];
                                let name = path_str(&self.locals, &BPlace { local: lo.place.local, proj: vec![] });
                                errs.push(BkError { sp: lo.sp, msg: format!("`{name}` does not live long enough (dropped while still borrowed)") });
                            }
                        }
                    }
                }
                Ev::Def(l, srcs) | Ev::Flow(l, srcs) => {
                    let mut ls = self.resolve(srcs, st);
                    if let Ev::Flow(..) = e {
                        ls.extend_from_slice(st.held(*l));
                    }
                    st.set_holds(*l, ls);
                }
                Ev::Escape(srcs) => {
                    for ln in self.resolve(srcs, st) {
                        add_sorted(&mut st.escaped, ln);
                    }
                }
                Ev::Ret(srcs, sp) => {
                    if loans_on {
                        if let Some(errs) = errs.as_deref_mut() {
                            for ln in self.resolve(srcs, st) {
                                let lo = &self.loans[ln as usize];
                                if lo.place.owned_len() == lo.place.proj.len() {
                                    let what = if self.locals[lo.place.local as usize].name == "self" { "a value owned by the function" } else { "a local variable" };
                                    let name = path_str(&self.locals, &lo.place);
                                    // (a closure that borrows it: at the closure, E0373)
                                    errs.push(if name == TEMP {
                                        BkError { sp: *sp, msg: "cannot return reference to temporary value (E0515)".to_string() }
                                    } else if lo.capture {
                                        BkError { sp: lo.sp, msg: format!("closure may outlive the current function, but it borrows `{name}`, which is owned by the current function (E0373)") }
                                    } else {
                                        BkError { sp: *sp, msg: format!("cannot return a reference to {what}: `{name}` is dropped when the function returns") }
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Mutation of a local not declared `mut`
    fn check_mut(&self, st: &State, p: &BPlace, a: Acc, sp: Sp, errs: &mut Vec<BkError>) {
        let lo = &self.locals[p.local as usize];
        let writes = match a {
            Acc::Borrow(l) => self.loans[l as usize].mutable,
            Acc::Write | Acc::Modify => true,
            _ => false,
        };
        // (through a `&`: `&mut *r` / `r.x = 1` with r: &T)
        if writes && p.proj.contains(&Proj::Shr) {
            let i = p.proj.iter().rposition(|x| *x == Proj::Shr).unwrap();
            let behind = path_str(&self.locals, &BPlace { local: p.local, proj: p.proj[..=i].to_vec() });
            let msg = match a {
                Acc::Borrow(_) => format!("cannot borrow `{}` as mutable, as `{behind}` is behind a `&` reference", path_str(&self.locals, p)),
                _ => format!("cannot assign to `{}`, which is behind a `&` reference", path_str(&self.locals, p)),
            };
            let sp = if let Acc::Borrow(l) = a { self.loans[l as usize].at } else { sp };
            errs.push(BkError { sp, msg });
            return;
        }
        if lo.mutable != Some(false) || p.owned_len() < p.proj.len() {
            return;
        }
        let name = &lo.name;
        let msg = match a {
            Acc::Borrow(l) if self.loans[l as usize].mutable => {
                let msg = format!("cannot borrow `{}` as mutable, as `{name}` is not declared as mutable", path_str(&self.locals, p));
                errs.push(BkError { sp: self.loans[l as usize].at, msg });
                return;
            }
            Acc::Write | Acc::Modify if !p.proj.is_empty() => {
                format!("cannot assign to `{}`, as `{name}` is not declared as mutable", path_str(&self.locals, p))
            }
            Acc::Write if st.inited.binary_search(&p.local).is_ok() => format!("cannot assign twice to immutable variable `{name}`"),
            Acc::Modify => format!("cannot assign twice to immutable variable `{name}`"),
            _ => return,
        };
        errs.push(BkError { sp, msg });
    }

    fn check_moved(&self, st: &State, p: &BPlace, a: Acc, sp: Sp, errs: &mut Vec<BkError>) {
        for m in &st.moved {
            if m.place.local != p.local {
                continue;
            }
            let hit = match a {
                // (assigning a whole path reinitializes it: only a moved prefix above it is an error)
                Acc::Write => m.place.proj.len() < p.proj.len() && prefix(&m.place.proj, &p.proj),
                _ => overlap(&m.place, p),
            };
            if !hit {
                continue;
            }
            let name = path_str(&self.locals, &m.place);
            let msg = if m.uninit {
                format!("used binding `{name}` isn't initialized")
            } else {
                let verb = match a {
                    Acc::Read | Acc::Drop => "use",
                    Acc::Move => "use",
                    Acc::Write | Acc::Modify => "assign to part",
                    Acc::Borrow(_) => "borrow",
                };
                let what = if m.place.proj.len() > p.proj.len() { "partially moved" } else { "moved" };
                format!("{verb} of {what} value: `{}` (moved at {})", path_str(&self.locals, p), at(self.line_base, m.sp))
            };
            errs.push(BkError { sp, msg });
            return;
        }
    }

    fn check_loans(&self, live: &[u32], p: &BPlace, a: Acc, sp: Sp, errs: &mut Vec<BkError>) {
        let mine = if let Acc::Borrow(l) = a { Some(l) } else { None };
        let wants_mut = match a {
            Acc::Read | Acc::Drop => false,
            Acc::Borrow(l) => self.loans[l as usize].mutable,
            Acc::Move | Acc::Write | Acc::Modify => true,
        };
        for &ln in live {
            if Some(ln) == mine {
                continue;
            }
            let lo = &self.loans[ln as usize];
            if lo.place.local != p.local || lo.quiet && lo.sp == sp {
                continue;
            }
            if !lo.mutable && !wants_mut {
                continue;
            }
            let conflict = match a {
                // (overwriting a reference does not touch what it points to)
                Acc::Write => {
                    prefix(&p.proj, &lo.place.proj) && !lo.place.proj[p.proj.len()..].first().is_some_and(|x| matches!(x, Proj::Deref(false) | Proj::Shr))
                        || prefix(&lo.place.proj, &p.proj)
                }
                _ => overlap(&lo.place, p),
            };
            if !conflict {
                continue;
            }
            let name = path_str(&self.locals, p);
            let at = at(self.line_base, lo.sp);
            let lk = if lo.mutable { "mutable" } else { "immutable" };
            let msg = match a {
                Acc::Read | Acc::Drop => format!("cannot use `{name}` because it was mutably borrowed (at {at})"),
                Acc::Move => format!("cannot move out of `{name}` because it is borrowed (at {at})"),
                Acc::Write | Acc::Modify => format!("cannot assign to `{name}` because it is borrowed (at {at})"),
                Acc::Borrow(l) if self.loans[l as usize].mutable && lo.mutable => {
                    format!("cannot borrow `{name}` as mutable more than once at a time (first borrow at {at})")
                }
                Acc::Borrow(l) => {
                    let k = if self.loans[l as usize].mutable { "mutable" } else { "immutable" };
                    format!("cannot borrow `{name}` as {k} because it is also borrowed as {lk} (at {at})")
                }
            };
            errs.push(BkError { sp, msg });
            return;
        }
    }
}

#[cfg(test)]
#[path = "borrowck_props.rs"]
mod props;

// ---------------------------------------------------------------------------
// Recording, from acg
// ---------------------------------------------------------------------------

use super::{err_pos, ident_of, strip_paren, Cx};
use crate::parse::{Expr, Pat};

/// The position of an expression for a message (`&x` has none of its own: use x's)
pub fn pos(e: &Expr) -> Sp {
    let mut e = e;
    loop {
        match e {
            Expr::Ref(_, x) | Expr::Paren(x) => e = x,
            _ => return err_pos(e),
        }
    }
}

/// How a closure uses a captured place
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum CapMode {
    /// Not known from the use (acg's capture analysis decides)
    Use,
    Ref,
    RefMut,
    /// Moved out (bound by value, not Copy)
    Val,
}

/// The pattern looks at the value (a discriminant, a literal, a length), so matching reads the scrutinee
/// p binds by reference somewhere (`ref x`, `ref mut x`)
fn pat_binds_ref(p: &Pat) -> bool {
    pat_any(p, &|q| matches!(q, Pat::Ident(true, ..)))
}

fn pat_binds_ref_mut(p: &Pat) -> bool {
    pat_any(p, &|q| matches!(q, Pat::Ident(true, true, ..)))
}

fn pat_any(p: &Pat, f: &dyn Fn(&Pat) -> bool) -> bool {
    if f(p) {
        return true;
    }
    match p {
        Pat::Ident(_, _, _, Some(s)) => pat_any(s, f),
        Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Or(ps) | Pat::Slice(ps) => ps.iter().any(|q| pat_any(q, f)),
        Pat::Struct(_, fs, _) => fs.iter().any(|(_, q)| pat_any(q, f)),
        Pat::Ref(_, q) => pat_any(q, f),
        _ => false,
    }
}

fn pat_reads(p: &Pat) -> bool {
    match p {
        Pat::Wild | Pat::Rest => false,
        Pat::Ident(_, _, _, sub) => sub.as_deref().is_some_and(pat_reads),
        Pat::Tuple(ps) => ps.iter().any(pat_reads),
        Pat::Ref(_, q) => pat_reads(q),
        _ => true,
    }
}

impl Cx<'_> {
    pub(crate) fn bk_on(&self) -> bool {
        self.bk.on && self.inline_depth == 0
    }

    pub(crate) fn bk_ev(&mut self, e: Ev) {
        let b = self.b.live_block();
        self.bk.ev(b, e);
    }

    /// A local comes into scope at loc
    pub(crate) fn bk_local(&mut self, loc: Loc, name: &str, ty: Ty) {
        let mutable = self.bk.mut_next.take();
        let uninit = std::mem::take(&mut self.bk.uninit_next);
        if !self.bk_on() {
            return;
        }
        let id = self.bk.locals.len() as u32;
        let (copy, refs) = self.bk_ty(ty);
        self.bk.locals.push(Local { name: name.to_string(), ty, copy, refs, mutable });
        self.bk.by_loc.insert(loc, id);
        self.bk_ev(Ev::Bind(id, !uninit));
        if refs {
            if let Some(srcs) = self.bk.bind_srcs.clone().filter(|s| !s.is_empty()) {
                self.bk_ev(Ev::Def(id, srcs));
            }
        }
    }

    /// The scrutinee of `match` / `if let` / `while let` / a destructuring `let`. A place is read only if a pattern
    /// looks at the value (`let (a, _) = t;` and `match x { _ => .. }` do not read t / x); what it holds flows into the
    /// bindings (see bind_srcs)
    pub(crate) fn bk_scrutinee(&mut self, e: &Expr, expected: Option<Ty>, pats: &[&Pat]) -> super::R<Option<super::Place>> {
        // (read first: the scrutinee may hold matches of its own)
        let in_let = std::mem::take(&mut self.bk.let_scrut);
        if !self.bk_on() {
            return self.scrutinee_as(e, expected);
        }
        let quiet = super::is_place_expr(e) && !pats.iter().any(|p| pat_reads(p));
        self.bk_frame();
        if quiet {
            self.bk.place_depth += 1;
        }
        let r = self.scrutinee_as(e, expected);
        if quiet {
            self.bk.place_depth -= 1;
            if let Some((p, _)) = self.bk_place(e) {
                if self.bk.locals[p.local as usize].refs {
                    self.bk.src(Src::Local(p.local));
                }
            }
        }
        // (matching a reference binds by reference (default binding modes): the bindings reborrow `*e`)
        if let Some((mut p, Some(Ty::Ref(m, _)))) = self.bk_place(e) {
            let binds = pats.iter().any(|p| {
                let mut v = vec![];
                crate::ast_closure::pat_idents(p, &mut v);
                !v.is_empty()
            });
            if binds {
                p.proj.push(ref_deref(m));
                let sp = err_pos(e);
                let ln = self.bk.loans.len() as u32;
                self.bk.loans.push(Loan { place: p.clone(), mutable: m, sp, at: sp, quiet: false, capture: false });
                self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
                self.bk.src(Src::Loan(ln));
            }
        }
        // (a temporary bound by `ref` (`match f() { Some(ref m) => .. }`): borrowed, it goes away at the end of the
        // statement)
        if self.bk.level >= 2 && !in_let && !super::is_place_expr(e) && pats.iter().any(|p| pat_binds_ref(p)) {
            let id = self.bk.locals.len() as u32;
            self.bk.locals.push(Local { name: TEMP.to_string(), ty: Ty::Unit, copy: true, refs: false, mutable: Some(true) });
            self.bk_ev(Ev::Bind(id, true));
            let sp = err_pos(e);
            let ln = self.bk.loans.len() as u32;
            let m = pats.iter().any(|p| pat_binds_ref_mut(p));
            self.bk.loans.push(Loan { place: BPlace { local: id, proj: vec![] }, mutable: m, sp, at: sp, quiet: true, capture: false });
            self.bk_ev(Ev::Acc(BPlace { local: id, proj: vec![] }, Acc::Borrow(ln), sp));
            self.bk.src(Src::Loan(ln));
            self.bk.stmt_temps.push(id);
        }
        let srcs = self.bk_frame_end();
        self.bk.scrut = srcs;
        // (a pattern that writes `'static` (`Foo::Bar::<'static> { .. }`): checked from here on)
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && pats.iter().any(|p| pat_any(p, &|q| matches!(q, Pat::TupleStruct(pp, _) | Pat::Struct(pp, _, _) if crate::regions::path_names_lt(pp)))) {
            self.rg_late_on();
        }
        self.rg.scrut = if self.rg_on() { self.rt_expr(e) } else { None };
        self.bk.scrut_place = self.bk_place(e).map(|p| (p.0, err_pos(e)));
        r
    }

    /// A binding by value moves the part of the scrutinee it binds (if it is not Copy)
    pub(crate) fn bk_pat_move(&mut self, t: Ty, name: &str) {
        if !self.bk_on() {
            return;
        }
        let Some(p) = self.bk.pat_path.clone() else { return };
        // (a binding of the whole scrutinee (`match c { c2 => .. }`) is not tracked (rustc accepts assigning to c's
        // fields in the arm); a unit struct pattern `S` binds nothing)
        if self.bk.scrut_place.as_ref().is_some_and(|s| s.0 == p) {
            return;
        }
        if let Ty::Struct(sid) = t {
            if self.prog.structs[self.prog.base_of(sid) as usize].name == name {
                return;
            }
        }
        if !self.bk_ty(t).0 {
            let sp = self.bk.scrut_place.as_ref().map(|x| x.1).unwrap_or(super::SP);
            let a = if self.bk.guard_arm { Acc::Read } else { Acc::Move };
            self.bk_acc_place(p, a, sp);
        }
    }

    /// A `ref` / `ref mut` binding borrows the part of the scrutinee it binds: the loan flows into the binding.
    /// Returns the bind_srcs to restore
    pub(crate) fn bk_pat_ref(&mut self, m: bool, explicit: bool) -> Option<Vec<Src>> {
        let saved = self.bk.bind_srcs.clone();
        if !self.bk_on() || !explicit {
            return saved;
        }
        let Some(p) = self.bk.pat_path.clone() else { return saved };
        let sp = self.bk.scrut_place.as_ref().map(|x| x.1).unwrap_or(super::SP);
        let ln = self.bk.loans.len() as u32;
        self.bk.loans.push(Loan { place: p.clone(), mutable: m, sp, at: sp, quiet: false, capture: false });
        self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
        let mut s = saved.clone().unwrap_or_default();
        s.push(Src::Loan(ln));
        self.bk.bind_srcs = Some(s);
        saved
    }

    /// Descend into a part of the pattern (returns the path to restore)
    pub(crate) fn bk_pat_proj(&mut self, pr: Proj) -> Option<BPlace> {
        let saved = self.bk.pat_path.clone();
        if let Some(p) = &mut self.bk.pat_path {
            p.proj.push(pr);
        }
        saved
    }

    /// A field of a tuple / tuple struct / enum variant pattern (deref: the pattern matched through a reference)
    pub(crate) fn bk_pat_field(&mut self, deref: bool, variant: Option<&str>, f: &str) -> Option<BPlace> {
        if deref {
            return self.bk.pat_path.take();
        }
        let name = match variant {
            Some(v) => format!("{v}::{f}"),
            None => f.to_string(),
        };
        self.bk_pat_proj(Proj::Field(name))
    }

    /// Bind a pattern against the last scrutinee
    pub(crate) fn bk_bind_begin(&mut self) {
        self.rg.pat = self.rg.scrut.clone();
        self.rg.pat_ref = None;
        if self.bk_on() {
            self.bk.bind_srcs = Some(self.bk.scrut.clone());
            self.bk.pat_path = self.bk.scrut_place.as_ref().map(|x| x.0.clone());
        }
    }
    pub(crate) fn bk_bind_end(&mut self) {
        self.rg.pat = None;
        self.bk.bind_srcs = None;
        self.bk.pat_path = None;
    }

    /// (Copy, may hold references), cached
    pub(crate) fn bk_ty(&mut self, t: Ty) -> (bool, bool) {
        if let Some(&r) = self.bk.ty_info.get(&t) {
            return r;
        }
        let r = (is_copy(self.prog, t, 0), has_refs(self.prog, t, 0));
        self.bk.ty_info.insert(t, r);
        r
    }

    pub(crate) fn bk_local_of(&self, name: &str) -> Option<u32> {
        let (loc, _) = self.lookup(name)?;
        self.bk.by_loc.get(&loc).copied()
    }

    /// The locals of the innermost scope go out of scope (in reverse order)
    pub(crate) fn bk_dead_scope(&mut self, sp: Sp) {
        if !self.bk_on() {
            return;
        }
        let Some(s) = self.scopes.last() else { return };
        let ids: Vec<(u32, Ty)> = s.iter().rev().filter_map(|(_, l, t)| self.bk.by_loc.get(l).map(|&id| (id, *t))).collect();
        for (id, t) in ids {
            // (drop check: a value whose `Drop` may see what it borrows is used when it is dropped)
            if self.bk.locals[id as usize].refs && self.bk.level >= 2 && self.dropck(t, 0) {
                self.bk_ev(Ev::Acc(BPlace { local: id, proj: vec![] }, Acc::Drop, sp));
            }
            self.bk_ev(Ev::Dead(id, sp));
        }
        // (the temporaries its `let`s extended)
        let depth = self.scopes.len().saturating_sub(1);
        while self.bk.scope_temps.last().is_some_and(|x| x.0 >= depth) {
            let (_, id) = self.bk.scope_temps.pop().unwrap();
            self.bk_ev(Ev::Dead(id, sp));
        }
    }

    /// The drop of a value of type t runs a `Drop` the program implements, which may use the references the value
    /// holds. The prelude's containers drop their elements only (as std's `#[may_dangle]`)
    pub(crate) fn dropck(&self, t: Ty, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        match t {
            Ty::Struct(sid) => {
                let prog = self.prog;
                let base = prog.base_of(sid);
                let sd = &prog.structs[sid as usize];
                let prelude = prog.struct_item.get(base as usize).is_some_and(|&i| i >= prog.prelude_from);
                if prelude || prog.closure_of_struct.contains_key(&base) {
                    return sd.args.iter().any(|&a| self.dropck(a, depth + 1));
                }
                prog.impls.contains(&(base, "Drop".to_string()))
                    || sd.fields.iter().any(|f| self.dropck(f.1, depth + 1))
                    || sd.variants.iter().any(|v| v.fields.iter().any(|f| self.dropck(f.1, depth + 1)))
            }
            Ty::Box(i) | Ty::Array(i, _) => self.dropck(*i, depth + 1),
            _ => false,
        }
    }

    /// The place e names, rooted at a local (None for temporaries, statics, captures ..)
    pub(crate) fn bk_place(&self, e: &Expr) -> Option<(BPlace, Option<Ty>)> {
        match e {
            Expr::Paren(x) => self.bk_place(x),
            Expr::Path(_, p) => {
                let id = ident_of(p)?;
                let (loc, t) = self.lookup(id)?;
                let l = *self.bk.by_loc.get(&loc)?;
                Some((BPlace { local: l, proj: vec![] }, Some(t)))
            }
            Expr::Field(b, name, _) => {
                let (mut p, t) = self.bk_place(b)?;
                let mut t = t?;
                loop {
                    match t {
                        Ty::Ref(m, i) => {
                            p.proj.push(ref_deref(m));
                            t = *i;
                        }
                        Ty::Box(i) => {
                            p.proj.push(Proj::Deref(true));
                            t = *i;
                        }
                        _ => break,
                    }
                }
                let Ty::Struct(sid) = t else { return None };
                let sd = &self.prog.structs[sid as usize];
                if sd.is_enum {
                    return None;
                }
                let f = sd.fields.iter().find(|f| f.0 == *name)?;
                // (the fields of a union are the same memory: a union place is the whole union)
                if !self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name) {
                    p.proj.push(Proj::Field(name.clone()));
                }
                Some((p, Some(f.1)))
            }
            Expr::Unary(op, x, _) if op == "*" => {
                let (mut p, t) = self.bk_place(x)?;
                match t? {
                    Ty::Ref(m, i) => {
                        p.proj.push(ref_deref(m));
                        Some((p, Some(*i)))
                    }
                    Ty::Box(i) => {
                        p.proj.push(Proj::Deref(true));
                        Some((p, Some(*i)))
                    }
                    _ => None,
                }
            }
            Expr::Index(b, _, _) => {
                let (mut p, t) = self.bk_place(b)?;
                let mut t = t?;
                while let Ty::Ref(m, i) = t {
                    p.proj.push(ref_deref(m));
                    t = *i;
                }
                p.proj.push(Proj::Index);
                let et = match t {
                    Ty::Array(e, _) | Ty::Slice(e) => Some(*e),
                    _ => None,
                };
                Some((p, et))
            }
            _ => None,
        }
    }

    /// Record an access of the place e
    pub(crate) fn bk_acc(&mut self, e: &Expr, a: Acc) {
        if !self.bk_on() {
            return;
        }
        let Some((p, _)) = self.bk_place(e) else { return };
        let l = p.local;
        self.bk_ev(Ev::Acc(p, a, err_pos(e)));
        if matches!(a, Acc::Read | Acc::Move) && self.bk.locals[l as usize].refs {
            self.bk.src(Src::Local(l));
        }
    }

    pub(crate) fn bk_acc_place(&mut self, p: BPlace, a: Acc, sp: Sp) {
        if !self.bk_on() {
            return;
        }
        let l = p.local;
        self.bk_ev(Ev::Acc(p, a, sp));
        if matches!(a, Acc::Read | Acc::Move) && self.bk.locals[l as usize].refs {
            self.bk.src(Src::Local(l));
        }
    }

    /// `&e` / `&mut e`
    pub(crate) fn bk_borrow(&mut self, e: &Expr, m: bool, sp: Sp) {
        if !self.bk_on() {
            return;
        }
        let quiet = std::mem::take(&mut self.bk.no_loan_src);
        let Some((p, _)) = self.bk_place(e) else { return };
        let ln = self.bk.loans.len() as u32;
        self.bk.loan_of.insert(e as *const Expr as usize, ln);
        self.bk.loans.push(Loan { place: p.clone(), mutable: m, sp, at: sp, quiet: false, capture: false });
        let l = p.local;
        self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
        if !quiet {
            self.bk.src(Src::Loan(ln));
        }
        // (reborrowing through a reference also carries the loans the reference holds)
        if self.bk.locals[l as usize].refs {
            self.bk.src(Src::Local(l));
        }
    }

    /// `&e` of a temporary (not promoted to a static): a hidden local that goes out of scope at the end of the
    /// statement (unless the `let` extends it), so a loan of it still live then is an error (E0716)
    pub(crate) fn bk_temp_borrow(&mut self, e: &Expr, m: bool, t: Ty) {
        let key = e as *const Expr as usize;
        let ext = self.bk.extended.contains(&key);
        if !self.bk_on() || self.bk.level < 2 || !(ext || self.bk.direct.contains(&key)) {
            return;
        }
        let quiet = std::mem::take(&mut self.bk.no_loan_src);
        let id = self.bk.locals.len() as u32;
        let (copy, refs) = self.bk_ty(t);
        self.bk.locals.push(Local { name: TEMP.to_string(), ty: t, copy, refs, mutable: Some(true) });
        self.bk_ev(Ev::Bind(id, true));
        let sp = err_pos(e);
        let p = BPlace { local: id, proj: vec![] };
        let ln = self.bk.loans.len() as u32;
        self.bk.loan_of.insert(e as *const Expr as usize, ln);
        self.bk.loans.push(Loan { place: p.clone(), mutable: m, sp, at: sp, quiet: true, capture: false });
        self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
        if !quiet {
            self.bk.src(Src::Loan(ln));
        }
        // (extended by the `let`: it goes away with the block's locals)
        if ext {
            self.bk.scope_temps.push((self.scopes.len().saturating_sub(1), id));
        } else {
            self.bk.stmt_temps.push(id);
        }
    }

    /// The statements from `mark` on are over: their temporaries go out of scope
    pub(crate) fn bk_temps_end(&mut self, mark: usize) {
        if self.bk.stmt_temps.len() <= mark {
            return;
        }
        let ts: Vec<u32> = self.bk.stmt_temps.drain(mark..).collect();
        for t in ts {
            self.bk_ev(Ev::Dead(t, super::SP));
        }
    }

    /// The event for the receiver (a `&mut self` borrow is two-phase: the caller records it after the arguments)
    pub(crate) fn bk_recv_ev(&mut self, recv: &Expr, recv_ty: Ty, flows: bool) -> Option<Ev> {
        let Some((mut p, t)) = self.bk_place(recv) else { return None };
        let sp = err_pos(recv);
        match recv_ty {
            Ty::Ref(m, _) => {
                let mut t = t;
                loop {
                    match t {
                        Some(Ty::Ref(rm, i)) => {
                            p.proj.push(ref_deref(rm));
                            t = Some(*i);
                        }
                        Some(Ty::Box(i)) => {
                            p.proj.push(Proj::Deref(true));
                            t = Some(*i);
                        }
                        _ => break,
                    }
                }
                let ln = self.bk.loans.len() as u32;
                self.bk.loans.push(Loan { place: p.clone(), mutable: m, sp, at: sp, quiet: false, capture: false });
                let l = p.local;
                if flows {
                    self.bk.src(Src::Loan(ln));
                }
                if self.bk.locals[l as usize].refs {
                    self.bk.src(Src::Local(l));
                }
                Some(Ev::Acc(p, Acc::Borrow(ln), sp))
            }
            _ => {
                if t.is_some_and(|t| !matches!(t, Ty::Ref(..) | Ty::Box(_))) && !is_copy(self.prog, recv_ty, 0) {
                    return Some(Ev::Acc(p, Acc::Move, sp));
                }
                None
            }
        }
    }

/// Whether the result of calling id may borrow from its argument i (self is 0)
    pub(crate) fn bk_ret_from(&self, id: u32, i: usize) -> bool {
        let sig = &self.prog.sigs[id as usize];
        // (a method of a prelude view (`chunks.remainder()`): the result borrows what the view borrows, which the view's
        // local passes on, not the view)
        if i == 0 && sig.has_self {
            if let Some(Ty::Ref(_, Ty::Struct(v))) = sig.params.first() {
                if is_view(self.prog, *v) {
                    return false;
                }
            }
        }
        // (`Iterator::next`: an item never borrows the iterator itself)
        if i == 0 && sig.has_self && self.prog.names.get(id as usize).is_some_and(|n| n.ends_with("::next") || n.ends_with("::next_back")) {
            return false;
        }
        // (a prelude view (`v.iter()`, `s.chars()`) borrows the receiver: the prelude writes no lifetimes)
        if i == 0 && sig.has_self && matches!(sig.params.first(), Some(Ty::Ref(..))) {
            let mut r = sig.ret;
            if let Ty::Struct(o) = r {
                // (`Option<Iter<T>>` ..)
                if self.prog.structs[o as usize].args.len() == 1 && !is_view(self.prog, o) {
                    r = self.prog.structs[o as usize].args[0];
                }
            }
            if let Ty::Struct(v) = r {
                if is_view(self.prog, v) {
                    return true;
                }
            }
        }
        let m = sig.ret_from;
        // (not known: say no, so that a borrow is never kept alive by a guess)
        m != 0 && i < 63 && m >> i & 1 == 1
    }

    pub(crate) fn bk_push(&mut self, e: Option<Ev>) {
        if let Some(e) = e {
            self.bk_ev(e);
        }
    }

    /// A closure captures the local n: by value it is moved (or copied), by reference it is borrowed (mutably if the
    /// closure assigns it) and the closure holds the loan
    pub(crate) fn bk_capture(&mut self, c: &Expr, n: &str, loc: Loc, t: Ty, by_move: bool) {
        if !self.bk_on() {
            return;
        }
        let Some(&l) = self.bk.by_loc.get(&loc) else { return };
        let Expr::Closure(_, _, _, body, sp) = c else { return };
        let sp = *sp;
        // (edition 2021 closures capture the places they use (`x.a`), not whole variables (RFC 2229))
        let mut paths: Vec<(BPlace, Option<Ty>, CapMode, Sp)> = vec![];
        self.bk_cap_paths(body, n, &mut paths);
        if paths.is_empty() {
            paths.push((BPlace { local: l, proj: vec![] }, Some(t), CapMode::Use, super::SP));
        }
        let move_kw = matches!(c, Expr::Closure(true, ..));
        // (the places of n the body assigns or borrows mutably: those are captured by `&mut`)
        let mut muts: Vec<BPlace> = vec![];
        self.bk_cap_muts(body, n, &mut muts);
        let assigns = crate::ast_closure::assigns_capture(c, &[n.to_string()]);
        for (p, pt, mode, _) in &mut paths {
            // (stop at an index, and a move closure at a deref: it captures the reference)
            let cut = p.proj.iter().position(|x| *x == Proj::Index || move_kw && matches!(x, Proj::Deref(_) | Proj::Shr));
            if let Some(i) = cut {
                p.proj.truncate(i);
                *pt = None;
            }
            if move_kw {
                *mode = CapMode::Val;
            } else if *mode == CapMode::Use {
                // (a use that is not a pattern: acg's analysis says whether the closure takes the value)
                let m = assigns && muts.iter().any(|q| overlap(q, p));
                *mode = if by_move { CapMode::Val } else if m { CapMode::RefMut } else { CapMode::Ref };
            }
        }
        paths.sort_by_key(|p| p.0.proj.len());
        let mut keep: Vec<(BPlace, Option<Ty>, CapMode, Sp)> = vec![];
        for (p, pt, mode, at) in paths {
            if p.local != l {
                continue;
            }
            match keep.iter_mut().find(|k| prefix(&k.0.proj, &p.proj)) {
                Some(k) => {
                    if mode > k.2 {
                        k.2 = mode;
                        k.3 = at;
                    }
                }
                None => keep.push((p, pt, mode, at)),
            }
        }
        for (p, pt, mode, at) in keep {
            // (the use in the body, for a mutability error; the closure itself otherwise)
            let at = if at.line == 0 { sp } else { at };
            let pt = if p.proj.is_empty() { Some(t) } else { pt };
            if mode == CapMode::Val {
                let copy = pt.is_none_or(|t| self.bk_ty(t).0);
                // (a copy a closure with no `move` holds of what it only reads: rustc's closure borrows it. Not of a
                // reference: one passed where a reference is expected is reborrowed (`f(b)` is `f(&*b)`), and the
                // closure holds a copy of it then (rustc's capture of `*b` by reference, truncated))
                if !(copy && !move_kw && pt.is_some_and(|t| !matches!(t, Ty::Ref(..) | Ty::Ptr(..) | Ty::Fn(_)))) {
                    self.bk_acc_place(p, if copy { Acc::Read } else { Acc::Move }, sp);
                    continue;
                }
            }
            let ln = self.bk.loans.len() as u32;
            self.bk.cap_loans.entry(c as *const Expr as usize).or_default().push(ln);
            self.bk.loans.push(Loan { place: p.clone(), mutable: mode == CapMode::RefMut, sp, at, quiet: false, capture: true });
            self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
            self.bk.src(Src::Loan(ln));
        }
        if self.bk.locals[l as usize].refs {
            self.bk.src(Src::Local(l));
        }
    }

    /// A place rooted at the variable n (`n`, `n.a`, `*n.b`)
    fn bk_rooted_at(&self, e: &Expr, n: &str) -> Option<(BPlace, Option<Ty>)> {
        let mut root = e;
        loop {
            match root {
                Expr::Paren(x) | Expr::Field(x, _, _) => root = x,
                Expr::Unary(op, x, _) if op == "*" => root = x,
                _ => break,
            }
        }
        match root {
            Expr::Path(_, p) if ident_of(p) == Some(n) => self.bk_place(e),
            _ => None,
        }
    }

    /// The parts of the place (base, of type t) that the pattern binds or looks at, and how
    fn bk_pat_cap(&self, pat: &Pat, base: &BPlace, t: Option<Ty>, sp: Sp, out: &mut Vec<(BPlace, Option<Ty>, CapMode, Sp)>) {
        let field = |b: &BPlace, name: String| {
            let mut b = b.clone();
            b.proj.push(Proj::Field(name));
            b
        };
        match pat {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident(by_ref, m, _, None) => {
                let mode = match (by_ref, m) {
                    (true, true) => CapMode::RefMut,
                    (true, false) => CapMode::Ref,
                    // (a Copy value bound by value is read: captured by reference)
                    _ if t.is_some_and(|t| is_copy(self.prog, t, 0)) => CapMode::Ref,
                    _ if t.is_some() => CapMode::Val,
                    _ => CapMode::Use,
                };
                out.push((base.clone(), t, mode, sp));
            }
            Pat::Tuple(ps) if !ps.iter().any(|p| matches!(p, Pat::Rest)) || matches!(ps.last(), Some(Pat::Rest)) => {
                let fields = match t {
                    Some(Ty::Struct(sid)) if self.prog.is_tuple(sid) => Some(self.prog.structs[sid as usize].fields.clone()),
                    _ => None,
                };
                for (i, p) in ps.iter().enumerate() {
                    if matches!(p, Pat::Rest) {
                        break;
                    }
                    let ft = fields.as_ref().and_then(|f| f.get(i)).map(|f| f.1);
                    let name = fields.as_ref().and_then(|f| f.get(i)).map(|f| f.0.clone()).unwrap_or_else(|| i.to_string());
                    self.bk_pat_cap(p, &field(base, name), ft, sp, out);
                }
            }
            Pat::TupleStruct(path, ps) if !ps.iter().any(|p| matches!(p, Pat::Rest)) => {
                let Some(Ty::Struct(sid)) = t else {
                    out.push((base.clone(), t, CapMode::Use, sp));
                    return;
                };
                let sd = &self.prog.structs[sid as usize];
                let vname = path.segs.last().map(|s| s.name.as_str()).unwrap_or("");
                // (a struct, or an enum of one variant: the fields are captured one by one)
                let (fields, prefix) = if !sd.is_enum {
                    (sd.fields.clone(), None)
                } else {
                    match sd.variants.iter().find(|v| v.name == vname) {
                        Some(v) => (v.fields.clone(), Some(v.name.clone())),
                        None => {
                            out.push((base.clone(), t, CapMode::Use, sp));
                            return;
                        }
                    }
                };
                let mut sub = vec![];
                for (p, f) in ps.iter().zip(&fields) {
                    let name = match &prefix {
                        Some(v) => format!("{v}::{}", f.0),
                        None => f.0.clone(),
                    };
                    self.bk_pat_cap(p, &field(base, name), Some(f.1), sp, &mut sub);
                }
                if sd.is_enum && sd.variants.len() > 1 {
                    // (the discriminant is read: the whole place, as strongly as its parts are used)
                    let mode = sub.iter().map(|x| x.2).max().unwrap_or(CapMode::Ref).max(CapMode::Ref);
                    out.push((base.clone(), t, mode, sp));
                } else {
                    out.extend(sub);
                }
            }
            Pat::Slice(ps) if ps.iter().all(|p| matches!(p, Pat::Wild | Pat::Rest)) => {}
            Pat::Slice(ps) => {
                // (the whole array, as strongly as its elements are bound)
                let et = match t {
                    Some(Ty::Array(e, _)) => Some(*e),
                    _ => None,
                };
                let mut sub = vec![];
                for p in ps {
                    self.bk_pat_cap(p, base, et, sp, &mut sub);
                }
                let mode = sub.iter().map(|x| x.2).max().unwrap_or(CapMode::Ref).max(CapMode::Ref);
                out.push((base.clone(), t, mode, sp));
            }
            Pat::Ident(true, m, _, Some(_)) => out.push((base.clone(), t, if *m { CapMode::RefMut } else { CapMode::Ref }, sp)),
            Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) => out.push((base.clone(), t, CapMode::Ref, sp)),
            _ => out.push((base.clone(), t, CapMode::Use, sp)),
        }
    }

    /// The places rooted at n that the closure body assigns (`n.a = ..`, `n += 1`) or borrows mutably (`&mut n.b`)
    fn bk_cap_muts(&self, e: &Expr, n: &str, out: &mut Vec<BPlace>) {
        match e {
            Expr::Assign(l, _) | Expr::AssignOp(_, l, _, _) | Expr::Ref(true, l) => {
                if let Some((p, _)) = self.bk_rooted_at(l, n) {
                    out.push(p);
                }
            }
            Expr::Macro(_, g, _) => {
                for a in crate::ast_closure::macro_args(g).unwrap_or_default() {
                    self.bk_cap_muts(&a, n, out);
                }
                return;
            }
            _ => {}
        }
        crate::ast_closure::each_child_pub(e, &mut |c| self.bk_cap_muts(c, n, out));
    }

    /// The places rooted at the variable n that the closure body uses
    pub(crate) fn bk_cap_paths(&self, e: &Expr, n: &str, out: &mut Vec<(BPlace, Option<Ty>, CapMode, Sp)>) {
        // (destructuring a captured place captures the parts the patterns use: `let (ref mut x, _) = a;`)
        // (a use that captures nothing (`match a { [_, ..] => .. }`) is marked by a place of no local)
        let seen = BPlace { local: u32::MAX, proj: vec![] };
        match e {
            Expr::Match(x, arms) => {
                if let Some((p, t)) = self.bk_rooted_at(x, n) {
                    out.push((seen, None, CapMode::Use, super::SP));
                    for a in arms {
                        self.bk_pat_cap(&a.pat, &p, t, err_pos(x), out);
                        if let Some(g) = &a.guard {
                            self.bk_cap_paths(g, n, out);
                        }
                        self.bk_cap_paths(&a.body, n, out);
                    }
                    return;
                }
            }
            Expr::Let(pat, x) => {
                if let Some((p, t)) = self.bk_rooted_at(x, n) {
                    out.push((seen, None, CapMode::Use, super::SP));
                    self.bk_pat_cap(pat, &p, t, err_pos(x), out);
                    return;
                }
            }
            Expr::Block(_, _, b) => {
                for st in &b.stmts {
                    match st {
                        crate::parse::Stmt::Let(pat, _, Some(init), els, ..) => {
                            match self.bk_rooted_at(init, n) {
                                Some((p, t)) if !matches!(pat, Pat::Ident(..)) => {
                                    out.push((seen.clone(), None, CapMode::Use, super::SP));
                                    self.bk_pat_cap(pat, &p, t, err_pos(init), out)
                                }
                                _ => self.bk_cap_paths(init, n, out),
                            }
                            if let Some(eb) = els {
                                self.bk_cap_paths(&Expr::Block(None, false, eb.clone()), n, out);
                            }
                        }
                        crate::parse::Stmt::Expr(x, _) => self.bk_cap_paths(x, n, out),
                        _ => {}
                    }
                }
                return;
            }
            _ => {}
        }
        let mut root = e;
        loop {
            match root {
                Expr::Paren(x) | Expr::Field(x, _, _) | Expr::Index(x, _, _) => root = x,
                Expr::Unary(op, x, _) if op == "*" => root = x,
                _ => break,
            }
        }
        if let Expr::Path(_, p) = root {
            if ident_of(p) == Some(n) {
                if let Some(x) = self.bk_place(e) {
                    out.push((x.0, x.1, CapMode::Use, err_pos(e)));
                } else if let Some(x) = self.bk_place(root) {
                    out.push((x.0, x.1, CapMode::Use, err_pos(e)));
                }
                // (the index expressions are uses of their own)
                let mut x = e;
                loop {
                    match x {
                        Expr::Index(b, i, _) => {
                            self.bk_cap_paths(i, n, out);
                            x = b;
                        }
                        Expr::Paren(b) | Expr::Field(b, _, _) => x = b,
                        Expr::Unary(_, b, _) => x = b,
                        _ => break,
                    }
                }
                return;
            }
        }
        if let Expr::Macro(_, g, _) = e {
            match crate::ast_closure::macro_args(g) {
                Some(args) => {
                    for a in &args {
                        // (`println!("{n}")`: the whole variable)
                        if let Expr::Lit(t, _) = a {
                            if crate::ast_closure::fmt_inline_names(t).iter().any(|x| x == n) {
                                if let Some(x) = self.bk_local_of(n) {
                                    out.push((BPlace { local: x, proj: vec![] }, None, CapMode::Use, super::SP));
                                }
                            }
                        }
                        self.bk_cap_paths(a, n, out);
                    }
                }
                // (not readable as arguments: assume the whole variable is used)
                None => {
                    if g.contains(n) {
                        if let Some(x) = self.bk_local_of(n) {
                            out.push((BPlace { local: x, proj: vec![] }, None, CapMode::Use, super::SP));
                        }
                    }
                }
            }
            return;
        }
        crate::ast_closure::each_child_pub(e, &mut |c| self.bk_cap_paths(c, n, out));
    }

    /// A hidden local holds the loans of an argument until the call is made (only mutable ones matter: a shared
    /// argument loan conflicts only with a write, and arguments do not write)
    pub(crate) fn bk_call_temp(&mut self, srcs: &[Src], m: bool) {
        if !self.bk_on() || !m {
            return;
        }
        let loans: Vec<Src> = srcs.iter().filter(|s| matches!(s, Src::Loan(_))).copied().collect();
        if loans.is_empty() {
            return;
        }
        let id = self.bk.locals.len() as u32;
        self.bk.locals.push(Local { name: "<argument>".into(), ty: Ty::Unit, copy: true, refs: true, mutable: None });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, loans));
        self.bk.call_temps.push(id);
    }

    /// The call is made: the argument loans end
    pub(crate) fn bk_call_temps_end(&mut self, t0: usize) {
        while self.bk.call_temps.len() > t0 {
            let id = self.bk.call_temps.pop().unwrap();
            if id == u32::MAX || !self.bk_on() {
                continue;
            }
            self.bk_ev(Ev::Acc(BPlace { local: id, proj: vec![] }, Acc::Read, super::SP));
            self.bk_ev(Ev::Dead(id, super::SP));
        }
    }

    /// A loan of the place e, with no access recorded for it
    pub(crate) fn bk_loan_only(&mut self, e: &Expr, m: bool) -> Option<u32> {
        let (p, _) = self.bk_place(e)?;
        let sp = err_pos(e);
        let ln = self.bk.loans.len() as u32;
        self.bk.loans.push(Loan { place: p, mutable: m, sp, at: sp, quiet: true, capture: false });
        Some(ln)
    }

    /// What srcs borrow is stored into the local the place e is rooted at
    pub(crate) fn bk_flow_into(&mut self, e: &Expr, srcs: &[Src]) {
        if !self.bk_on() || srcs.is_empty() {
            return;
        }
        if let Some((p, _)) = self.bk_place(e) {
            let l = p.local;
            if self.bk.locals[l as usize].refs {
                let srcs: Vec<Src> = srcs.iter().filter(|s| **s != Src::Local(l)).copied().collect();
                self.bk_ev(Ev::Flow(l, srcs));
            }
        }
    }

    /// Start collecting what flows into a value
    pub(crate) fn bk_frame(&mut self) {
        if self.bk_on() {
            self.bk.frames.push(vec![]);
        }
    }

    /// Stop collecting, without passing the sources on
    pub(crate) fn bk_frame_end_quiet(&mut self) -> Vec<Src> {
        if !self.bk_on() {
            return vec![];
        }
        self.bk.frames.pop().unwrap_or_default()
    }

    /// Stop collecting (the sources also flow into the enclosing frame: `let a = { let b = &x; b };`)
    pub(crate) fn bk_frame_end(&mut self) -> Vec<Src> {
        if !self.bk_on() {
            return vec![];
        }
        let f = self.bk.frames.pop().unwrap_or_default();
        for s in &f {
            self.bk.src(*s);
        }
        f
    }

    /// Stop collecting for a call whose signature is not known (a closure, a fn pointer, `dyn Fn`): the loans made
    /// for its arguments are not passed on, what the locals read hold is
    pub(crate) fn bk_frame_end_call(&mut self) {
        if !self.bk_on() {
            return;
        }
        let f = self.bk.frames.pop().unwrap_or_default();
        for s in f {
            if let Src::Local(_) = s {
                self.bk.src(s);
            }
        }
    }

    /// The local named n (just bound from init) holds what the frame collected
    pub(crate) fn bk_def_named(&mut self, n: &str, srcs: &[Src], init: &Expr) {
        if !self.bk_on() || srcs.is_empty() {
            return;
        }
        if let Some(l) = self.bk_local_of(n) {
            if self.bk.locals[l as usize].refs {
                self.bk_ev(Ev::Def(l, srcs.to_vec()));
                let mut direct = vec![];
                self.bk_direct(init, &mut direct);
                if !direct.is_empty() {
                    self.bk_ev(Ev::Keep(l, direct));
                }
            }
        }
    }

    /// `left = value` (after the value is computed)
    pub(crate) fn bk_assign(&mut self, left: &Expr, srcs: &[Src]) {
        if !self.bk_on() {
            return;
        }
        let Some((p, _)) = self.bk_place(strip_paren(left)) else { return };
        let l = p.local;
        let whole = p.proj.is_empty();
        // (`*x = ..` / `x.f = ..` through a reference parameter: what is stored outlives the function)
        let through = p.owned_len() < p.proj.len() && self.bk.params.contains(&l);
        self.bk_ev(Ev::Acc(p, Acc::Write, err_pos(left)));
        if through && !srcs.is_empty() {
            self.bk_ev(Ev::Escape(srcs.to_vec()));
        }
        if self.bk.locals[l as usize].refs {
            if whole {
                self.bk_ev(Ev::Def(l, srcs.to_vec()));
            } else if !srcs.is_empty() {
                self.bk_ev(Ev::Flow(l, srcs.to_vec()));
            }
        }
    }

    /// The value returned (e): only what e borrows directly is checked (`&x`, `x.as_str()`, a local, the branches of
    /// an `if` / `match` / block). What flows through other calls is approximated too coarsely for this check
    /// (`v.get(0).cloned()` holds no loan of v)
    pub(crate) fn bk_ret(&mut self, e: &Expr) {
        if !self.bk_on() {
            return;
        }
        let mut srcs = vec![];
        self.bk_direct(e, &mut srcs);
        // (`&f()`: a borrow of a temporary, which is not promoted to a static (as direct_temps))
        let mut x = e;
        while let Expr::Paren(i) = x {
            x = i;
        }
        if let Expr::Ref(m, t) = x {
            let made = match &**t {
                Expr::MethodCall(..) => true,
                Expr::Call(f, _) => matches!(&**f, Expr::Path(_, p) if p.segs.last().is_some_and(|s| s.name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_'))),
                Expr::Macro(p, ..) => p.segs.last().is_some_and(|s| s.name == "vec" || s.name == "format"),
                _ => false,
            };
            // (not of a reference: `&h()` with h returning `&'static str` is deref-coerced to what h returns)
            if made && self.bk.level >= 2 && self.peek(t).is_some_and(|ty| !matches!(ty, Ty::Ref(..) | Ty::Ptr(..))) {
                let id = self.bk.locals.len() as u32;
                self.bk.locals.push(Local { name: TEMP.to_string(), ty: Ty::Unit, copy: true, refs: false, mutable: Some(true) });
                self.bk_ev(Ev::Bind(id, true));
                let ln = self.bk.loans.len() as u32;
                let sp = err_pos(t);
                self.bk.loans.push(Loan { place: BPlace { local: id, proj: vec![] }, mutable: *m, sp, at: sp, quiet: true, capture: false });
                srcs.push(Src::Loan(ln));
            }
        }
        if !srcs.is_empty() {
            self.bk_ev(Ev::Ret(srcs, pos(e)));
        }
    }

    fn bk_direct(&self, e: &Expr, out: &mut Vec<Src>) {
        match e {
            Expr::Paren(x) => self.bk_direct(x, out),
            // (a call of the program's fn: what its result borrows from, by its signature (`fn gimme(x: &T) -> &U`);
            // the prelude writes no lifetimes, so its fns are not followed)
            Expr::Call(f, args) => {
                if let Some(id) = self.fn_item(f) {
                    if self.prog.item_of.get(id as usize).copied().flatten().is_some_and(|i| i < self.prog.prelude_from) {
                        for (i, a) in args.iter().enumerate() {
                            if self.bk_ret_from(id, i) {
                                self.bk_direct(a, out);
                            }
                        }
                    }
                }
            }
            Expr::Ref(_, x) => {
                if let Some(&ln) = self.bk.loan_of.get(&(&**x as *const Expr as usize)) {
                    out.push(Src::Loan(ln));
                }
            }
            Expr::MethodCall(_, name, _, args, _) => {
                if let Some(&ln) = self.bk.loan_of.get(&(e as *const Expr as usize)) {
                    out.push(Src::Loan(ln));
                }
                // (a closure passed to an iterator adapter, which its result holds: `lines.filter_map(|l| .. self.x ..)`
                // returned. Not `find(|a| ..)`, whose result does not)
                let holds = matches!(name.as_str(), "map" | "filter" | "filter_map" | "flat_map" | "take_while" | "skip_while" | "map_while" | "inspect" | "scan");
                for a in args.iter().filter(|_| holds) {
                    if let Some(ls) = self.bk.cap_loans.get(&(super::strip_paren(a) as *const Expr as usize)) {
                        out.extend(ls.iter().map(|&l| Src::Loan(l)));
                    }
                }
            }
            Expr::Path(..) => {
                if let Some((p, _)) = self.bk_place(e) {
                    if p.proj.is_empty() && self.bk.locals[p.local as usize].refs {
                        out.push(Src::Local(p.local));
                    }
                }
            }
            Expr::Block(_, _, b) => {
                if let Some(crate::parse::Stmt::Expr(t, false)) = b.stmts.last() {
                    self.bk_direct(t, out);
                }
            }
            // (a closure: the loans of what it captures by reference)
            Expr::Closure(..) => {
                if let Some(ls) = self.bk.cap_loans.get(&(e as *const Expr as usize)) {
                    out.extend(ls.iter().map(|&l| Src::Loan(l)));
                }
            }
            Expr::If(_, b, els) => {
                if let Some(crate::parse::Stmt::Expr(t, false)) = b.stmts.last() {
                    self.bk_direct(t, out);
                }
                if let Some(x) = els {
                    self.bk_direct(x, out);
                }
            }
            Expr::Match(_, arms) => {
                for a in arms {
                    self.bk_direct(&a.body, out);
                }
            }
            _ => {}
        }
    }

    /// Run the checker on the lowered body
    pub(crate) fn bk_check(&mut self) -> Result<(), String> {
        if !self.bk.on || self.bk.evs.is_empty() {
            return Ok(());
        }
        let mut bk = std::mem::take(&mut self.bk);
        bk.edges = self.b.cfg_edges();
        // (the reborrows the region check found must outlive the function)
        if self.rg.on && !self.rg.rerun && self.rg.closure.is_none() {
            let loan = |p: usize| bk.loan_of.get(&p).or_else(|| bk.recv_loans.get(&p)).copied();
            let mut f: Vec<u32> = self.rg.forever().into_iter().filter_map(loan).collect();
            f.sort_unstable();
            f.dedup();
            bk.forever = f;
            // (and those that must outlive a region of a local's type: held by the local)
            let mut hb: Vec<(u32, Vec<u32>)> = vec![];
            for (p, locs) in self.rg.holders() {
                let Some(ln) = loan(p) else { continue };
                let ls: Vec<u32> = locs.iter().filter_map(|l| bk.by_loc.get(l).copied()).filter(|&l| bk.locals[l as usize].refs).collect();
                if !ls.is_empty() {
                    hb.push((ln, ls));
                }
            }
            hb.sort_by_key(|x| x.0);
            hb.dedup_by_key(|x| x.0);
            bk.held_by = hb;
        }
        let r = bk.check(self.b.n_blocks());
        self.bk = bk;
        match r {
            Some(e) => Err(self.at(e.sp, e.msg)),
            None => Ok(()),
        }
    }
}
