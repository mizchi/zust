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
    pub(crate) fn owned_len_pub(&self) -> usize {
        self.owned_len()
    }
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
    /// The local is read in a returned position (Bk::ret_leaves): the loans it holds that reach a universal region
    /// only by being returned (Bk::ret_only) are live from here to the return (Polonius Alpha: the constraint holds
    /// where the value is returned)
    RetHold(u32),
    /// The local keeps these loans (what its initializer borrows directly): they must not be of a dead local
    Keep(u32, Vec<Src>),
    /// These loans are stored behind the local's `&mut`s, so many derefs deep (`*q = &y` with q = &mut p): the
    /// locals those point to hold them. The flag: what is stored is a reference to something that holds references
    /// (`*q = &mut &y`), whose region is shared with what the `&mut`s point into (a `&mut T` is invariant in T)
    Store(u32, u32, bool, Vec<Src>),
    /// These loans are held by what the local's references point to, shared ones too (a closure called through a
    /// reference to it, `r(&a)` with `r = &g`: the closure's type holds the region its calls' arguments flow into)
    HoldThrough(u32, Vec<Src>),
    /// A match's guard begins (rustc's fake borrows of the match: the places its patterns test, and the deref
    /// prefixes of the place its bindings are bound from, whose flag is false: what they point to is not borrowed) or
    /// ends (None). The events between are the guard's, in the order they are made: a guard's evaluation is one piece
    /// of the source, nested in the match's
    Guard(Option<Vec<(BPlace, bool)>>),
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
    /// The `let` that declares it (line 0: not known)
    pub decl: Sp,
    /// It stands for the place a reference points to (a closure's capture held by reference): its places start with
    /// a deref (bk_place), which is not shown
    pub via_ref: bool,
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
    /// The receivers of a block's tail's method calls that are temporaries (bk_tail_drop_temps): Cx::expr collects
    /// what each one's value holds (watched), as a `let` of it would
    pub watch: crate::fxhash::FxSet<usize>,
    pub watched: HashMap<usize, Vec<Src>>,
    /// The watched receivers that are method calls, by position (a call the lowering copies, as a method of what an
    /// auto-deref reaches (`y.borrow().clone()`), is not at its address): what their values hold
    pub watch_at: Vec<(Sp, Vec<Src>)>,
    /// A closure's body: the borrow checker's locals of its captures held by value. They outlive the body's call, in
    /// the closure's environment: at each exit (its end, `return`, `?`), what they hold escapes it (Cx::bk_cap_escape)
    pub cap_vals: Vec<u32>,
    /// The body of a closure whose kind nothing decides (Insts::kind_free): a mutable loan of what it captures by
    /// reference, returned, escapes the `FnMut` body
    pub fnmut_ret: bool,
    /// The loans of implicit reborrows (`f(x)` with x: `&mut T`: bk_reborrow_arg)
    pub implicit: Vec<u32>,
    /// The loans that are two-phase borrows' reservations (shared loans of a `&mut self` receiver while the arguments
    /// are evaluated)
    pub reserved: Vec<u32>,
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
    /// Its expected type is the type inference's of the `let` (`let b = a;`, no annotation): no coercion site, a
    /// `&mut` there is moved as where nothing is expected
    pub move_mut_hint: bool,
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
    /// A fn pointer call's implicit argument reborrows without the region check, by the argument's index: the result
    /// holds those of the arguments it may borrow from (`let r = f(x); *x = 1; use(r)` is E0506)
    pub fnptr_reborrows: Vec<(usize, u32)>,
    /// The `&mut` loans of a call's implicit argument reborrows, without the region check, and the call temporaries
    /// of their reservations: two-phase borrows (rustc's), taken once the arguments are evaluated (call_fn). `f(x, *x)`
    /// reads `*x` while it is reserved; `f(x, &*x)` conflicts when it is taken (E0502)
    pub arg_acts: Vec<(Ev, u32)>,
    /// The call being lowered takes its `&mut self` once its arguments are evaluated (activate): what the arguments
    /// borrow, shared loans too, is held until then (`s.take(s.get())`: E0502)
    pub hold_args: bool,
    /// The parameters of the call being lowered whose arguments a `&self` receiver stores (shared_recv_stores)
    pub store_args: Vec<usize>,
    /// The `index_mut` call acg makes to read an indexed place (its receiver is seen as shared)
    pub shared_recv: usize,
    /// Binding the pattern of an arm with a guard: a binding by value does not move until the guard holds (the next
    /// arm may bind the same place again)
    pub guard_arm: bool,
    /// The arm whose pattern is bound (its position: a `ref` binding's loan is reported there, as rustc reports it at
    /// the binding; a pattern has no position of its own)
    pub arm_at: Option<Sp>,
    /// Bindings by value of what is not Copy, made so far (where no drop needs the binding too: the check of a move
    /// out of an overloaded deref, deref_move_check, after a pattern that moved)
    pub pat_moves: u32,
    /// Evaluating match guards: the locals each one's arm's pattern bound (ids lo..hi), innermost last, which the
    /// guard sees by shared reference, and the first access of one that this makes an error (E0507 / E0594 / E0596)
    pub guard_binds: Vec<(u32, u32)>,
    /// A closure's captures made in a guard: only a mutable borrow of a binding is checked (bk_ev)
    pub guard_cap: bool,
    pub guard_err: Option<(Sp, String)>,
    /// The place being lowered is written or borrowed mutably (`*x = ..`, `&mut *x`): a `deref_mut` acg makes
    /// for it is a mutable borrow
    pub place_mut: bool,
    /// The tail expression of the function body (its address)
    pub tail: usize,
    /// The block whose value a statement discards (its address; 0: none), and the hidden locals holding such values
    pub discard: usize,
    pub discard_temps: Vec<u32>,
    /// A block's value that holds what one of its locals holds (`{ let y = &x; y }`): a hidden local made at its tail,
    /// while its locals live, which stands for it after (its direct sources, as the block's names are gone then)
    pub block_val: HashMap<usize, u32>,
    /// The block a statement discards (`{ .. };`, its address): its value is not used after it
    pub stmt_block: usize,
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
    /// The result is an `impl Trait` that captures no lifetime (before edition 2024, its bounds name none, not generic):
    /// what it returns must not borrow through a parameter's reference (E0700)
    pub ret_no_capture: bool,
    /// The result is an `impl Trait`: a closure that borrows a local and is returned from a local holding it is
    /// rustc's E0597 at the use in its body (one returned itself, or in a `Box<dyn ..>`, is E0373)
    pub ret_impl: bool,
    /// The parameters whose regions are all 'static (`x: &'static u8`, `'a: 'static`): returning them captures nothing
    pub static_params: Vec<u32>,
    /// The parameters whose type has a region that may not be 'static under its own reference (`s: &Foo<'a>`): a
    /// reference copied out of their fields (`s.0`) holds it
    pub inner_params: Vec<u32>,
    /// The locals bound (once, never assigned again) from what holds such a parameter's reference (`let y = x;`):
    /// they hold it too (check fills it)
    pub param_derived: std::cell::RefCell<Vec<u32>>,
    /// unused_mut is denied: a `let mut` local that nothing needs mutable is an error (check)
    pub lint_mut: bool,
    /// The `mut` locals a closure that holds them by value mutates (`move || x += 1`): needed mutable
    pub mut_used: Vec<u32>,
    /// The `mut` locals declared without a value and assigned whole (`let mut x; x = 1;`), whose second assignment the
    /// dataflow looks for; and those it found (check)
    pub mut_flow: std::cell::RefCell<(Vec<u32>, Vec<u32>)>,
    /// In the report: the E0716 of each temporary's loan still in use when the temporary goes away (the loan, the
    /// error's index, the live locals that hold it then), the loans a returned value holds and the locals it is read
    /// from, with where it is returned. Such an E0716 is E0515 at the return, as rustc reports it (`for ref x in xs {
    /// v.push(x) } v`), also where a loop's next item drops the loan from what the returned local holds
    pub ret_temps: std::cell::RefCell<(Vec<(u32, usize, Vec<u32>)>, Vec<(u32, Sp)>, Vec<(u32, Sp)>)>,
    /// Locals that own a value whose type implements `Drop` (a user's), and the place of that value in the local,
    /// through boxes and fields (`Box<Scribble>`: `*`; `Wrapper(Scribble)`: `.0`): its `drop` may use what a `&mut`
    /// field points to, so a borrow through it must not outlive the local (E0713)
    pub drop_owners: Vec<(u32, Vec<Proj>)>,
    /// The locals whose drop at the end of their scope may see what they borrow (dropck): a loan of itself one holds
    /// there is in use while it is dropped
    pub dropck_locals: Vec<u32>,
    /// A `*x` through an overloaded `Deref` with no `DerefMut` (`Rc`, `Arc`, a `Ref` guard, the program's): (the
    /// local, the place of its `Shr` in the projection) → the type's name, for the errors of writing through it
    /// (bk_place records them: it reads `self` only)
    pub overloaded: std::cell::RefCell<HashMap<(u32, usize), String>>,
    /// Type → (Copy, may hold references)
    pub ty_info: HashMap<Ty, (bool, bool)>,
    /// Hidden locals of temporaries borrowed in the statements being lowered (they go out of scope at the end of
    /// their statement)
    pub stmt_temps: Vec<u32>,
    /// Those of them whose `Drop` may see what they borrow: dropped when they go out of scope (bk_tail_drop_temps)
    pub drop_temps: Vec<u32>,
    /// The temporaries of the `let` initializers being lowered that are followed (see direct_temps): their addresses
    pub direct: Vec<usize>,
    /// Loans the region check found must outlive the function (a reborrow through a parameter flowing into a
    /// universal region): live from where each is made to the end (sorted)
    pub forever: Vec<u32>,
    /// Reborrows that must outlive a universal region only by being returned (Rg::forever with the returned values'
    /// constraints, less forever): live while what holds them is, and from a read in a returned position to the
    /// return (Ev::RetHold), and from where they are made if that is in a returned position (sorted)
    pub ret_only: Vec<u32>,
    /// The loans made in a returned position (sorted)
    pub ret_made: Vec<u32>,
    /// The returned positions of the body (its tail's and its `return`s' leaves: a block's tail, an `if`'s branches,
    /// a `match`'s arms; addresses), and how deep the lowering is in one
    pub ret_leaves: crate::fxhash::FxSet<usize>,
    pub in_ret: u32,
    /// Hidden locals that hold a value copied out of what a local points to (`*r` of `r: &&String`, bk_copy_out_of),
    /// with the copied type: they hold what r holds, with each loan of a place that holds such values replaced by
    /// what that place's local holds (copy_masks, made by bk_check)
    pub copy_out: Vec<(u32, Ty)>,
    /// What the items of each `for` loop being lowered hold (the loan of v for `for x in &v` / `v.iter()`): the
    /// bindings of an item that holds references hold it
    pub for_items: Vec<Vec<Src>>,
    /// The drop of a `match` scrutinee some of whose arms take the whole value (`x => drop(x)`): those arms move it,
    /// so it holds nothing there (bk_scrutinee; the match takes it)
    pub scrut_drop: Option<u32>,
    /// The address of the value a `let` with no type written is initialized from (through parentheses and block
    /// tails, value_tail): no coercion site, so a `&mut` read out of a place there by value is moved, not reborrowed
    /// (`let b = a[0];`, `let y = unsafe { *p };`)
    pub plain_init: usize,
    /// For each of copy_out's locals, its mask in masks: whether each loan is of a place that holds the copied type
    pub copy_masks: HashMap<u32, usize>,
    /// copy_masks' masks, one for each copied type
    pub masks: Vec<Vec<bool>>,
    /// The loans of each closure's captures by reference, by the closure's address
    pub cap_loans: HashMap<usize, Vec<u32>>,
    /// The use in a closure's body of the place a capture loan borrows itself (the first, not a part's: `p` of
    /// `println!("{:?}", p)` where `p.x += 5` makes the loan mutable), by the loan: where rustc reports its E0597
    pub cap_path_at: HashMap<u32, Sp>,
    /// The locals a closure holds by value that hold loans, by the closure's address: the closure holds what they do
    /// (`move || iter.next()` with `iter = vec.iter()`)
    pub cap_held: HashMap<usize, Vec<u32>>,
    /// The closures (by address) whose bodies mutate what they capture: `FnMut`, called through a `&mut` of them
    pub fnmut_exprs: Vec<usize>,
    /// The types of such closures
    pub fnmut: Vec<Ty>,
    /// The closures (by address) that may move what they capture by value out of themselves (bk_may_consume): not
    /// `FnMut` for sure
    pub fnonce_exprs: Vec<usize>,
    /// The closures (by address) whose bodies move a capture that is not `Copy` (`|| drop(v)`): `FnOnce`, a call of
    /// one by name moves it
    pub fnonce_sure: Vec<usize>,
    /// The types of such closures
    pub fnonce: Vec<Ty>,
    /// Where the closures of fnmut / fnonce are written
    pub clo_at: Vec<(Ty, Sp)>,
    /// Loans the region check found must outlive a region of locals' types (sorted by loan): those locals hold them
    pub held_by: Vec<(u32, Vec<u32>)>,
    /// The loan of each method call's receiver, by the call's address (while the region check is on)
    pub recv_loans: HashMap<usize, u32>,
    /// bk_loan_only is taking a method's receiver (`for x in v.iter_mut()`): a reference is auto-dereferenced
    pub recv_deref: bool,
    /// The scrutinee being lowered is a `let`'s initializer (its temporary is extended)
    pub let_scrut: bool,
    /// The `let` being lowered binds a macro's hidden local (`assert_eq!`'s `__exr_l`): its temporaries live as long as
    /// the macro's statement in rustc (a `match`), not the `let`'s
    pub macro_let: bool,
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
pub fn direct_temps(prog: &Program, e: &Expr, out: &mut Vec<usize>) {
    direct_temps_in(prog, e, false, out)
}

/// The temporaries of a `let`'s initializer the statement drops (direct_temps): a `&` of a temporary in a
/// constructor's arguments is the `let`'s to extend (extended_refs), not a statement's
pub fn direct_temps_let(prog: &Program, e: &Expr, out: &mut Vec<usize>) {
    direct_temps_in(prog, e, true, out)
}

fn direct_temps_in(prog: &Program, e: &Expr, in_let: bool, out: &mut Vec<usize>) {
    fn strip(mut e: &Expr) -> &Expr {
        while let Expr::Paren(p) = e {
            e = p;
        }
        e
    }
    let args = match strip(e) {
        // (`let x: &str = { mk().as_str() };`: the block's value is its tail's, and so are its temporaries)
        Expr::Block(_, _, b) => {
            if let Some(crate::parse::Stmt::Expr(t, false)) = b.stmts.last() {
                direct_temps_in(prog, t, in_let, out);
            }
            return;
        }
        // (a tuple / array / struct of such calls: each element's (`{ (String::new().as_str(),) }` of edition 2024))
        Expr::Tuple(xs) | Expr::Array(xs) => {
            for x in xs {
                direct_temps_in(prog, x, in_let, out);
            }
            return;
        }
        Expr::Struct(_, fs, None) => {
            for f in fs {
                direct_temps_in(prog, &f.1, in_let, out);
            }
            return;
        }
        // (a tuple struct's or a variant's constructor (`Some(&[name])`) in a `let`'s initializer: an extending
        // expression, so a `&` of a temporary in its arguments is extended by the `let` (extended_refs), as rustc does
        // since 1.79; what its arguments' calls borrow is not. Elsewhere (an edition 2024 block's tail,
        // `consume({ (Some(&temp()),) })`) its arguments are a call's)
        Expr::Call(f, a) if in_let && is_ctor_path(prog, f) => {
            for x in a {
                direct_temps_in(prog, x, in_let, out);
            }
            return;
        }
        // (a `&` there (a constructor's argument, a tuple's field): what it borrows is extended, not the receiver of a
        // method it is made from, as for the `let`'s own `&` (direct_ref_recv): `let r = Some(&*temp().get());` used
        // after is E0716)
        Expr::Ref(..) if in_let => {
            direct_ref_recv(prog, e, out);
            return;
        }
        Expr::Call(_, a) => a,
        Expr::MethodCall(r, _, _, a, _) => {
            // (the receiver made by a call (`mk(1).get()`): borrowed by a `&self` method, it is a temporary too)
            if made_temp(prog, r) {
                out.push(&**r as *const Expr as usize);
            }
            a
        }
        _ => return,
    };
    for a in args {
        direct_arg(prog, a, out);
    }
}

/// direct_ref of an argument, also of a block's tail: `gimme({ let v = 22; &(v,) })` passes what the tail borrows,
/// which is not extended (it is not the `let`'s own value). (Not for the `let`'s value: `let s = { ..; &(v,) };`
/// extends it to the `let`'s block, not the inner one it is made in)
fn direct_arg(prog: &Program, a: &Expr, out: &mut Vec<usize>) {
    let mut e = a;
    while let Expr::Paren(p) = e {
        e = p;
    }
    if let Expr::Block(_, _, b) = e {
        if let Some(crate::parse::Stmt::Expr(t, false)) = b.stmts.last() {
            direct_arg(prog, t, out);
        }
        return;
    }
    direct_ref(prog, a, out);
}

/// A value a call makes (not a place, not a constant rustc would promote): `f()`, `x.m()`, `vec![..]`
/// The temporaries of a block's tail that live to the end of the enclosing statement (edition 2021): the receivers of
/// its method calls that are not places (`D(&t).end()`), and what a `*` derefs that a call makes (`*a.borrow() + 1`),
/// through its operators. Each as (the expression, it with no parentheses)
fn tail_temps<'e>(e: &'e Expr, out: &mut Vec<(&'e Expr, &'e Expr)>, depth: u32) {
    if depth > 16 {
        return;
    }
    match e {
        Expr::Paren(x) => tail_temps(x, out, depth + 1),
        Expr::MethodCall(recv, ..) => {
            let r = strip_paren(recv);
            if !super::is_place_expr(r) {
                out.push((&**recv, r));
            }
            tail_temps(r, out, depth + 1);
        }
        Expr::Unary(op, x, _) if op == "*" => {
            let r = strip_paren(x);
            if matches!(r, Expr::MethodCall(..) | Expr::Call(..)) {
                out.push((&**x, r));
            }
            tail_temps(r, out, depth + 1);
        }
        Expr::Unary(_, x, _) => tail_temps(x, out, depth + 1),
        Expr::Binary(_, l, r, _) => {
            tail_temps(l, out, depth + 1);
            tail_temps(r, out, depth + 1);
        }
        _ => {}
    }
}

fn made_temp(prog: &Program, x: &Expr) -> bool {
    let mut x = x;
    while let Expr::Paren(p) = x {
        x = p;
    }
    match x {
        Expr::MethodCall(..) => true,
        // (a fn's call; a constructor's (`Some(v)`, `W(v)`) is promoted to a static only when its arguments are
        // constants, as a tuple literal is)
        Expr::Call(f, args) => match &**f {
            Expr::Path(_, p) if p.segs.last().is_some_and(|s| s.name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')) => true,
            // (`AddFlags(1)` of a fn named so, beside a braced `struct AddFlags`: a call, as the lowering takes it)
            Expr::Path(_, p) => args.iter().any(|a| runtime_value(prog, a)) || prog.fn_by_p(&crate::program::PPath::of(p), None).is_some(),
            _ => false,
        },
        Expr::Macro(p, ..) => p.segs.last().is_some_and(|s| s.name == "vec" || s.name == "format"),
        // (a tuple / array / struct literal of a value only known when it runs (`(v,)` of a local v): not promoted to a
        // static as a constant one is (`&(1, 2)`), so a temporary)
        Expr::Tuple(xs) | Expr::Array(xs) => !xs.is_empty() && xs.iter().any(|x| runtime_value(prog, x)),
        Expr::Struct(_, fs, None) => fs.iter().any(|f| runtime_value(prog, &f.1)),
        // (`S { a: 3, ..s }`: the base's other fields are read too)
        Expr::Struct(_, fs, Some(b)) => runtime_value(prog, b) || fs.iter().any(|f| runtime_value(prog, &f.1)),
        // (`&{ let v = 4; (v,) }`: a block's value is a temporary, moved or copied out of what its tail names)
        Expr::Block(None, _, b) => block_tail(b).is_some_and(|t| runtime_value(prog, t)),
        // (a `match` / `if` value (`(match f { true => a, false => b }).loan()`): a value expression, so a temporary)
        Expr::Match(..) | Expr::If(..) => true,
        _ => false,
    }
}

/// The value a block ends with (its tail), if it has one
fn block_tail(b: &crate::parse::Block) -> Option<&Expr> {
    match b.stmts.last() {
        Some(crate::parse::Stmt::Expr(t, false)) => Some(t),
        _ => None,
    }
}

/// e's value is only known when it runs: it reads a local (a lowercase name), calls something or makes a temporary
fn runtime_value(prog: &Program, e: &Expr) -> bool {
    match e {
        Expr::Paren(x) | Expr::Field(x, _, _) | Expr::Cast(x, _) | Expr::Unary(_, x, _) | Expr::Ref(_, x) => runtime_value(prog, x),
        Expr::Path(None, p) => p.segs.len() == 1 && p.segs[0].name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') && !matches!(p.segs[0].name.as_str(), "true" | "false"),
        Expr::Call(..) | Expr::MethodCall(..) | Expr::Index(..) | Expr::Macro(..) => true,
        // (a block, an `if` or a `match` is the value it ends with: `({ 1 },)` and `(if true { 1 } else { 2 },)` are
        // promoted as rustc does; a runtime condition or scrutinee makes it a temporary)
        Expr::Block(_, _, b) => block_tail(b).is_some_and(|t| runtime_value(prog, t)),
        Expr::If(c, t, e) => runtime_value(prog, c) || block_tail(t).is_some_and(|t| runtime_value(prog, t)) || e.as_deref().is_some_and(|e| runtime_value(prog, e)),
        Expr::Match(x, arms) => runtime_value(prog, x) || arms.iter().any(|a| runtime_value(prog, &a.body) || a.guard.as_ref().is_some_and(|g| runtime_value(prog, g))),
        Expr::Binary(_, a, b, _) => runtime_value(prog, a) || runtime_value(prog, b),
        _ => made_temp(prog, e),
    }
}

/// The local a place expression is rooted at (`v` of `v.f[i]`)
pub(crate) fn root_local(e: &Expr) -> Option<&str> {
    match e {
        Expr::Path(None, p) => super::ident_of(p),
        Expr::Field(x, ..) | Expr::Index(x, ..) | Expr::Paren(x) => root_local(x),
        _ => None,
    }
}

/// Whether e names the local n (in a closure's body too; not in a macro's arguments)
pub(crate) fn mentions(e: &Expr, n: &str) -> bool {
    if let Expr::Path(None, p) = e {
        if super::ident_of(p) == Some(n) {
            return true;
        }
    }
    let mut found = false;
    crate::ast_closure::each_child_pub(e, &mut |c| {
        if !found && mentions(c, n) {
            found = true;
        }
    });
    found
}

/// The addresses of marks lists that are in orig, for the same nodes of copy, a clone of orig the lowering made
pub fn copy_marks(orig: &Expr, copy: &Expr, marks: &mut Vec<usize>, depth: u32) {
    if depth > 32 {
        return;
    }
    if marks.contains(&(orig as *const Expr as usize)) {
        marks.push(copy as *const Expr as usize);
    }
    let (mut a, mut b): (Vec<*const Expr>, Vec<*const Expr>) = (vec![], vec![]);
    crate::ast_closure::each_child_pub(orig, &mut |c| a.push(c as *const Expr));
    crate::ast_closure::each_child_pub(copy, &mut |c| b.push(c as *const Expr));
    if a.len() == b.len() {
        for (x, y) in a.into_iter().zip(b) {
            // SAFETY: the children of orig and copy, which outlive this call
            copy_marks(unsafe { &*x }, unsafe { &*y }, marks, depth + 1);
        }
    }
}

/// `let x = &*R.m();` / `&R.m().f` with R a temporary a call makes: the `let` extends what `&` borrows (the call's
/// result), not R, the receiver it is made from, which goes away at the end of the statement (E0716 when the result
/// borrows it: `&mut *RefCell::new(1).borrow_mut()`, `&*String::from("b").as_str()`). Its address, for direct
pub fn direct_ref_recv(prog: &Program, a: &Expr, out: &mut Vec<usize>) {
    fn strip(mut e: &Expr) -> &Expr {
        while let Expr::Paren(p) = e {
            e = p;
        }
        e
    }
    let Expr::Ref(_, x) = strip(a) else { return };
    let mut y = strip(x);
    loop {
        match y {
            Expr::Field(b, ..) | Expr::Index(b, ..) => y = strip(b),
            Expr::Unary(op, b, _) if op == "*" => y = strip(b),
            Expr::MethodCall(r, ..) => {
                if made_temp(prog, r) {
                    out.push(&**r as *const Expr as usize);
                }
                return;
            }
            _ => return,
        }
    }
}

/// A path that names a tuple struct's or a variant's constructor, not a fn (`Some`, `W`, `Self`, `Option::Some::<T>`;
/// not `some` of `let some = Some;`)
fn is_ctor_path(prog: &Program, f: &Expr) -> bool {
    let Expr::Path(_, p) = f else { return false };
    // (a path that names a variant or a tuple struct, not a local fn pointer written in upper case
    // (`let F: fn(&String) -> &String = id; F(&temp())`), whose arguments are a call's)
    if !p.segs.last().is_some_and(|s| s.name.starts_with(|c: char| c.is_ascii_uppercase())) {
        return false;
    }
    let pp = crate::program::PPath::of(p);
    prog.variant_by_p(&pp, None).is_some() || (prog.struct_by_p(&pp, None).is_some() && prog.fn_by_p(&pp, None).is_none())
}

/// The temporaries a `let`'s initializer borrows that the `let` extends: `&e` (direct_ref) as the initializer or
/// within its extending expressions, the arguments of a tuple struct's or a variant's constructor, the fields of a
/// tuple, array or struct literal, a block's tail (`let x = Some((&temp(), W(&temp())));`)
pub fn extended_refs(prog: &Program, e: &Expr, out: &mut Vec<usize>) {
    let mut x = e;
    while let Expr::Paren(p) | Expr::Cast(p, _) = x {
        x = p;
    }
    match x {
        Expr::Ref(..) => direct_ref(prog, x, out),
        Expr::Call(f, a) if is_ctor_path(prog, f) => {
            for y in a {
                extended_refs(prog, y, out);
            }
        }
        Expr::Tuple(xs) | Expr::Array(xs) => {
            for y in xs {
                extended_refs(prog, y, out);
            }
        }
        Expr::Struct(_, fs, None) => {
            for f in fs {
                extended_refs(prog, &f.1, out);
            }
        }
        _ => {}
    }
}

/// `&e` with e a temporary that is never promoted to a static: the result of a call of a fn (not a struct's or
/// variant's constructor), of a method, of `vec!` / `format!` (its addresses, for direct_temps)
pub fn direct_ref(prog: &Program, a: &Expr, out: &mut Vec<usize>) {
    fn strip(mut e: &Expr) -> &Expr {
        while let Expr::Paren(p) = e {
            e = p;
        }
        e
    }
    if let Expr::Ref(_, x) = strip(a) {
        if made_temp(prog, x) {
            out.push(&**x as *const Expr as usize);
            out.push(strip(x) as *const Expr as usize);
        }
        // (`&f().0`, `&x.borrow().1`, `&v.clone()[0]`: a place in such a temporary (temp_root tells, with the types))
        let mut y = strip(x);
        while let Expr::Field(b, ..) | Expr::Index(b, ..) | Expr::Unary(_, b, _) = y {
            if matches!(y, Expr::Unary(op, ..) if op != "*") {
                break;
            }
            if made_temp(prog, b) {
                out.push(&**x as *const Expr as usize);
                break;
            }
            y = strip(b);
        }
        // (`&*Box::new(1)`: a borrow of what a temporary owns)
        if let Expr::Unary(op, y, _) = strip(x) {
            if op == "*" && made_temp(prog, y) {
                out.push(&**x as *const Expr as usize);
            }
        }
    }
}

/// ZUSTC_BORROWCK=n: the level of the check (read once: it is asked for each function)
pub fn level() -> u8 {
    static L: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *L.get_or_init(|| match std::env::var("ZUSTC_BORROWCK") {
        Ok(v) => v.parse().unwrap_or(2),
        Err(_) => 2,
    })
}

fn bk_debug() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *C.get_or_init(|| std::env::var_os("ZUSTC_BK_DEBUG").is_some())
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
        Ty::Array(e, _) => part_copy(prog, *e, depth + 1),
        Ty::Slice(_) | Ty::Str | Ty::Dyn(_) | Ty::Box(_) => false,
        Ty::Struct(sid) => {
            let sd = &prog.structs[sid as usize];
            if sd.symbolic {
                return true;
            }
            // (a type parameter's placeholder: Copy when its bounds say so)
            // (a bound with Copy among its supertraits too, `T: Num` of `trait Num: Copy`)
            // (a placeholder's associated type (`<T as Op>::Output#`): taken for Copy, as what makes it so may be
            // a where clause of its trait (`trait Op where Self::Output: Copy`) or of the fn, which are not read)
            if let Some(bs) = &sd.placeholder {
                return sd.name.starts_with('<') || bs.iter().any(|b| prog.bound_implies(b, &["Copy"], false, 0));
            }
            if prog.closure_of_struct.contains_key(&prog.base_of(sid)) || prog.is_tuple(sid) {
                return sd.fields.iter().all(|f| part_copy(prog, f.1, depth + 1));
            }
            // (a derived `Copy` on `Option<T>` needs `T: Copy`: look at the fields of the instance)
            prog.impls.contains(&(prog.base_of(sid), "Copy".to_string()))
                && sd.fields.iter().all(|f| part_copy(prog, f.1, depth + 1))
                && sd.variants.iter().all(|v| v.fields.iter().all(|f| part_copy(prog, f.1, depth + 1)))
        }
        _ => true,
    }
}

/// is_copy of a part of a value (a field, an element): a `&mut` there is not `Copy`. Nothing reborrows it where the
/// value is passed, so the value is moved (`let t2 = t;` with `t: (&mut i32, i32)`)
fn part_copy(prog: &Program, t: Ty, depth: u32) -> bool {
    !matches!(t, Ty::Ref(true, _)) && is_copy(prog, t, depth)
}

/// How t holds references, for a copy of it: 0 none, 1 only in types whose `Copy` impl is for their 'static
/// instance (`impl Copy for Foo<'static>`, and tuples and arrays of them), 2 others too. A copy of a value whose type
/// is 1 proves its type `Copy`, so what it borrows must be 'static
pub fn static_copy(prog: &Program, t: Ty, depth: u32) -> u8 {
    match t {
        _ if depth > 8 => 2,
        Ty::Struct(sid) if prog.static_copy.contains(&prog.base_of(sid)) => 1,
        Ty::Array(e, _) => static_copy(prog, *e, depth + 1),
        Ty::Struct(sid) if prog.is_tuple(sid) => prog.structs[sid as usize].fields.iter().map(|f| static_copy(prog, f.1, depth + 1)).max().unwrap_or(0),
        _ => 2 * has_refs(prog, t, 0) as u8,
    }
}

/// Whether `let p = v;` with v of type t moves v, or a part of it, out: p binds by value a part whose type is not
/// Copy (`let (n, _) = pair;` with n an integer copies). An enum variant's pattern, or a part whose type is not
/// followed: as binds_by_value
pub fn pat_moves(prog: &Program, p: &Pat, t: Ty, depth: u32) -> bool {
    if depth > 8 {
        return crate::ast_closure::binds_by_value(p);
    }
    let parts = |ps: &[Pat], fs: &[(String, Ty, u32)]| match Cx::tuple_fields(ps.iter(), fs.len(), super::SP) {
        Ok(v) => v.iter().any(|(i, q)| pat_moves(prog, q, fs[*i].1, depth + 1)),
        Err(_) => ps.iter().any(crate::ast_closure::binds_by_value),
    };
    match p {
        Pat::Wild | Pat::Rest => false,
        // (`x` / `x @ ..` binds the whole by value; `ref x` borrows; an uppercase name is a constant or a unit variant)
        Pat::Ident(by_ref, _, n, sub) => {
            if *by_ref {
                sub.as_deref().is_some_and(|q| pat_moves(prog, q, t, depth + 1))
            } else if n.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') && n != "_" {
                // (a `&mut` bound by value is moved: a pattern does not reborrow it, as an argument is)
                !is_copy(prog, t, 0) || matches!(t, Ty::Ref(true, _))
            } else {
                false
            }
        }
        Pat::Tuple(ps) => match t {
            Ty::Struct(sid) if prog.is_tuple(sid) => parts(ps, &prog.structs[sid as usize].fields),
            _ => crate::ast_closure::binds_by_value(p),
        },
        // (`&mut n` / `&n` of a reference: the reference is not moved, its referent is matched)
        Pat::Ref(_, q) => match t {
            Ty::Ref(_, i) => pat_moves(prog, q, *i, depth + 1),
            _ => crate::ast_closure::binds_by_value(p),
        },
        // (a struct's pattern, or an enum variant's: the variant's fields, by its name (`E::V(n, _)`))
        Pat::TupleStruct(path, ps) => match t {
            Ty::Struct(sid) if !prog.is_tuple(sid) => match variant_fields(prog, sid, path) {
                Some(fs) => parts(ps, fs),
                None => crate::ast_closure::binds_by_value(p),
            },
            _ => crate::ast_closure::binds_by_value(p),
        },
        Pat::Struct(path, named, _) => match t {
            Ty::Struct(sid) if !prog.is_tuple(sid) => match variant_fields(prog, sid, path) {
                Some(fs) => named.iter().any(|(n, q)| fs.iter().find(|f| f.0 == *n).map_or(crate::ast_closure::binds_by_value(q), |f| pat_moves(prog, q, f.1, depth + 1))),
                None => crate::ast_closure::binds_by_value(p),
            },
            _ => crate::ast_closure::binds_by_value(p),
        },
        _ => crate::ast_closure::binds_by_value(p),
    }
}

/// The fields a struct's or a variant's pattern (path) matches in a value of the struct sid: a struct's own, or those
/// of the enum's variant the path's last segment names
fn variant_fields<'p>(prog: &'p Program, sid: u32, path: &crate::parse::Path) -> Option<&'p [(String, Ty, u32)]> {
    let sd = &prog.structs[sid as usize];
    if sd.variants.is_empty() && !sd.is_enum {
        return Some(&sd.fields);
    }
    let name = path.segs.last()?.name.as_str();
    sd.variants.iter().find(|v| v.name == name).map(|v| v.fields.as_slice())
}

/// The prelude's iterator and view types: raw pointers into what they were made from, so they hold the loan of it
const VIEWS: &[&str] = &[
    "Windows", "Chunks", "ChunksExact", "Iter", "IterMut", "Chars", "DebugStruct", "DebugTuple", "DebugMap", "MutexGuard", "Ref", "RefMut", "PeekMut", "HmIter",
    "HmIterMut", "BtIter", "BtIterMut", "DqIter", "Split", "SplitStr", "MatchIndices", "RSplitStr", "CharIndices", "Bytes", "SplitFn",
];

pub fn is_view(prog: &Program, sid: u32) -> bool {
    let b = prog.base_of(sid);
    VIEWS.contains(&prog.structs[b as usize].name.as_str()) && prog.struct_item.get(b as usize).is_some_and(|&i| i >= prog.prelude_from)
}

/// Whether a value of type t may hold a reference (and so a loan)
pub fn has_refs(prog: &Program, t: Ty, _depth: u32) -> bool {
    // (the structs already answered: a few, so a list)
    fn go(prog: &Program, t: Ty, seen: &mut Vec<u32>, memo: &mut Vec<(u32, bool)>) -> bool {
        match t {
            Ty::Ref(..) => true,
            Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => go(prog, *i, seen, memo),
            Ty::Dyn(_) | Ty::Param(_) => true,
            Ty::Struct(sid) => {
                // (a recursive type: what it holds is what the rest of it holds)
                if seen.contains(&sid) {
                    return false;
                }
                if let Some(&(_, r)) = memo.iter().find(|m| m.0 == sid) {
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
                memo.push((sid, r));
                r
            }
            _ => false,
        }
    }
    go(prog, t, &mut vec![], &mut vec![])
}

fn at(lb: u32, sp: Sp) -> String {
    format!("{}:{}", lb + sp.line, sp.col + 1)
}

/// Each block's position in the reverse postorder rustc's MIR borrow check visits blocks in, as far as the blocks
/// show it: of a loop's test, the way out comes before the body (the successor that leads back to the test is explored
/// first, so it comes after); of an `if`, the `else` before the `then` (the first successor is explored first).
/// Unreachable blocks: last
fn rustc_rpo<'s>(n: usize, succ: &dyn Fn(usize) -> &'s [usize]) -> Vec<usize> {
    let reaches = |from: usize, to: usize| -> bool {
        let (mut seen, mut q) = (vec![false; n], vec![from]);
        while let Some(b) = q.pop() {
            if b == to {
                return true;
            }
            if b >= n || seen[b] {
                continue;
            }
            seen[b] = true;
            q.extend(succ(b).iter().copied());
        }
        false
    };
    let order = |b: usize| -> Vec<usize> {
        let mut s = succ(b).to_vec();
        if s.len() == 2 && reaches(s[1], b) && !reaches(s[0], b) {
            s.swap(0, 1);
        }
        s
    };
    let mut post = Vec::with_capacity(n);
    let (mut seen, mut stack) = (vec![false; n], vec![]);
    if n > 0 {
        seen[0] = true;
        stack.push((0usize, order(0), 0usize));
    }
    while let Some((b, s, k)) = stack.last_mut() {
        if let Some(&x) = s.get(*k) {
            *k += 1;
            if x < n && !seen[x] {
                seen[x] = true;
                let o = order(x);
                stack.push((x, o, 0));
            }
        } else {
            post.push(*b);
            stack.pop();
        }
    }
    let mut pos = vec![usize::MAX; n];
    for (i, &b) in post.iter().rev().enumerate() {
        pos[b] = i;
    }
    pos
}

/// The natural loops of the blocks (one per header: the header and what reaches an edge back to it without passing it),
/// each as the blocks it holds
fn natural_loops<'s>(n: usize, succ: &dyn Fn(usize) -> &'s [usize]) -> Vec<Vec<bool>> {
    let mut preds: Vec<Vec<usize>> = vec![vec![]; n];
    for b in 0..n {
        for &s in succ(b) {
            if s < n {
                preds[s].push(b);
            }
        }
    }
    // (the edges back to a block on the DFS stack)
    let mut back: Vec<(usize, usize)> = vec![];
    let (mut state, mut stack) = (vec![0u8; n], vec![(0usize, 0usize)]);
    if n == 0 {
        return vec![];
    }
    state[0] = 1;
    while let Some(&mut (b, ref mut k)) = stack.last_mut() {
        match succ(b).get(*k) {
            Some(&s) => {
                *k += 1;
                if s < n {
                    match state[s] {
                        0 => {
                            state[s] = 1;
                            stack.push((s, 0));
                        }
                        1 => back.push((b, s)),
                        _ => {}
                    }
                }
            }
            None => {
                state[b] = 2;
                stack.pop();
            }
        }
    }
    let mut out: Vec<(usize, Vec<bool>)> = vec![];
    for (t, h) in back {
        let i = match out.iter().position(|x| x.0 == h) {
            Some(i) => i,
            None => {
                let mut m = vec![false; n];
                m[h] = true;
                out.push((h, m));
                out.len() - 1
            }
        };
        let body = &mut out[i].1;
        let mut q = vec![t];
        while let Some(x) = q.pop() {
            if body[x] {
                continue;
            }
            body[x] = true;
            q.extend(preds[x].iter().copied());
        }
    }
    out.into_iter().map(|x| x.1).collect()
}

pub(crate) fn path_str(locals: &[Local], p: &BPlace) -> String {
    let mut s = locals[p.local as usize].name.clone();
    // (a closure's capture held by reference is shown as the variable: `this.x`, not `*this.x`)
    let skip = (locals[p.local as usize].via_ref && p.proj.first() == Some(&Proj::Deref(false))) as usize;
    for (i, pr) in p.proj.iter().enumerate().skip(skip) {
        match pr {
            Proj::Field(f) => {
                // (an array pattern's element: `a[2]`; a slice's, counted from its ends: `s[..]`, as rustc writes it)
                if !f.starts_with('[') {
                    s.push('.');
                } else if f.contains(';') || f.contains("..-") {
                    s.push_str("[..]");
                    continue;
                }
                // (an enum variant's field is written without the variant: `y.0` of `Some(..)`, as rustc does)
                let f = f.trim_start_matches(UNION_FIELD);
                s.push_str(f.rsplit("::").next().unwrap_or(f));
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

/// The elements a slice pattern's place is. An array's are where they are: `[2]` (2..3), `[1..19]` (the rest between).
/// A slice's are counted as rustc's MIR does, from the pattern's ends, with the least length the pattern needs: `[1;3]`
/// (the second from the start), `[-1;3]` (the last), `[1..-2]` (the rest, without the first and the last two)
#[derive(Clone, Copy)]
enum Elems {
    Abs(usize, usize),
    Start(usize, usize),
    End(usize, usize),
    Sub(usize, usize),
}

fn elems(p: &Proj) -> Option<Elems> {
    let Proj::Field(f) = p else { return None };
    let inner = f.strip_prefix('[')?.strip_suffix(']')?;
    if let Some((a, b)) = inner.split_once("..") {
        return match b.strip_prefix('-') {
            Some(b) => Some(Elems::Sub(a.parse().ok()?, b.parse().ok()?)),
            None => Some(Elems::Abs(a.parse().ok()?, b.parse().ok()?)),
        };
    }
    if let Some((i, m)) = inner.split_once(';') {
        let m = m.parse().ok()?;
        return match i.strip_prefix('-') {
            Some(e) => Some(Elems::End(e.parse().ok()?, m)),
            None => Some(Elems::Start(i.parse().ok()?, m)),
        };
    }
    inner.parse::<usize>().ok().map(|i| Elems::Abs(i, i + 1))
}

/// The two may be the same elements (rustc's place_projection_conflict). Of a slice: the b-th from the start and the
/// e-th from the end may be one when b + e reaches the least length both patterns need; the rest `[f..-t]` holds the
/// b-th from the start when b >= f, and the e-th from the end when e > t; two rests may overlap.
fn elems_overlap(x: Elems, y: Elems) -> bool {
    use Elems::*;
    match (x, y) {
        (Abs(a0, a1), Abs(b0, b1)) => a0 < b1 && b0 < a1,
        (Start(a, _), Start(b, _)) | (End(a, _), End(b, _)) => a == b,
        (Start(b, m1), End(e, m2)) | (End(e, m2), Start(b, m1)) => b + e >= m1.max(m2),
        (Start(b, _), Sub(f, _)) | (Sub(f, _), Start(b, _)) => b >= f,
        (End(e, _), Sub(_, t)) | (Sub(_, t), End(e, _)) => e > t,
        _ => true,
    }
}

/// a is a prefix of b (an index `[..]` is any element; a slice pattern's elements are where they overlap)
fn prefix(a: &[Proj], b: &[Proj]) -> bool {
    a.len() <= b.len() && a.iter().zip(b).all(|(x, y)| same_proj(x, y))
}

/// The two projections may be the same part (an index `[..]` is any element)
fn same_proj(x: &Proj, y: &Proj) -> bool {
    match (elems(x), elems(y)) {
        (Some(a), Some(b)) => elems_overlap(a, b),
        (Some(_), None) => *y == Proj::Index,
        (None, Some(_)) => *x == Proj::Index,
        (None, None) => x == y || (*x == Proj::Index && *y == Proj::Index),
    }
}

/// The two paths part at two fields of one union (`u.a` and `u.b.x`): the same memory, whatever follows. Neither is
/// a prefix of the other, so this is not prefix's (whose callers rely on the length)
fn union_overlap(a: &[Proj], b: &[Proj]) -> bool {
    for (x, y) in a.iter().zip(b) {
        if let (Proj::Field(f), Proj::Field(g)) = (x, y) {
            if f != g && f.starts_with(UNION_FIELD) && g.starts_with(UNION_FIELD) {
                return true;
            }
        }
        if !same_proj(x, y) {
            return false;
        }
    }
    false
}

/// The mark of a union's field in a place (Proj::Field)
pub(crate) const UNION_FIELD: char = '\u{1}';

/// The two places overlap (one contains the other)
fn overlap(a: &BPlace, b: &BPlace) -> bool {
    a.local == b.local && (prefix(&a.proj, &b.proj) || prefix(&b.proj, &a.proj) || union_overlap(&a.proj, &b.proj))
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
    /// A part of the local the local owns (`list.0`, `*b` of a Box) was overwritten: the loans through the references
    /// that part held are of the old value (Polonius' loan_killed_at for an assignment to a projection: `*list = n`
    /// after `&mut list.value`), also those that escaped (they are of what the old reference points to)
    fn kill_loans_through(&mut self, p: &BPlace, loans: &[Loan]) {
        let gone = |ln: u32| {
            let lp = &loans[ln as usize].place;
            lp.local == p.local && prefix(&p.proj, &lp.proj) && lp.owned_len() >= p.proj.len() && lp.owned_len() < lp.proj.len()
        };
        for h in &mut self.holds {
            h.1.retain(|&ln| !gone(ln));
        }
        self.escaped.retain(|&ln| !gone(ln));
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
        if bk_debug() {
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
        // (unused_mut: the `mut` locals something needs mutable: a `&mut` of what they own, a write of a part, `+=`,
        // a second assignment of all of it, a closure holding them by value that mutates them)
        let mut used: Vec<u32> = if self.lint_mut { self.mut_used.clone() } else { vec![] };
        let mut whole: Vec<u32> = vec![];
        let mut bound: Vec<u32> = vec![];
        if self.lint_mut {
            for (_, e) in &self.evs {
                match e {
                    Ev::Acc(p, a, _) if self.locals[p.local as usize].mutable == Some(true) && p.owned_len() == p.proj.len() => match a {
                        Acc::Borrow(l) if self.loans[*l as usize].mutable => used.push(p.local),
                        Acc::Modify => used.push(p.local),
                        Acc::Write if !p.proj.is_empty() => used.push(p.local),
                        Acc::Write => whole.push(p.local),
                        _ => {}
                    },
                    Ev::Bind(l, true) => bound.push(*l),
                    _ => {}
                }
            }
            used.sort_unstable();
            used.dedup();
            // (assigned whole after a `let` that gave it a value: a second assignment; declared without one: the
            // dataflow decides)
            let mut flow: Vec<u32> = vec![];
            for l in whole {
                if used.binary_search(&l).is_ok() {
                    continue;
                }
                if bound.contains(&l) {
                    if let Err(i) = used.binary_search(&l) {
                        used.insert(i, l);
                    }
                } else if !flow.contains(&l) {
                    flow.push(l);
                }
            }
            flow.sort_unstable();
            need_inited |= !flow.is_empty();
            *self.mut_flow.borrow_mut() = (flow, vec![]);
        }
        let mut by_local: HashMap<u32, Vec<u32>> = HashMap::default();
        if self.level >= 2 {
            for (i, lo) in self.loans.iter().enumerate() {
                by_local.entry(lo.place.local).or_default().push(i as u32);
            }
        }
        // (the places of the matches whose guards are running: rustc's fake borrows, which a guard must not change)
        let mut guards: Vec<&[(BPlace, bool)]> = vec![];
        for (_, e) in &self.evs {
            match e {
                Ev::Guard(Some(g)) => guards.push(g),
                Ev::Guard(None) => {
                    guards.pop();
                }
                Ev::Acc(p, a, sp) if !guards.is_empty() && self.guard_conflict(&guards, p, *a).is_some() => {
                    let name = path_str(&self.locals, p);
                    let what = if matches!(a, Acc::Borrow(_)) { "mutably borrow" } else { "assign" };
                    errs.push(BkError { sp: *sp, msg: format!("cannot {what} `{name}` in match guard (E0510)") });
                }
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
            self.unused_mut(&used, &mut errs);
            // (the borrows of a local not `mut` reported as one error at its declaration: rustc reports them after the rest)
        errs.sort_by_key(|e| (e.msg.contains("is not declared as mutable") && self.locals.iter().any(|l| l.decl.line != 0 && l.decl == e.sp), e.sp.line, e.sp.col));
            return errs.into_iter().next();
        }
        let n = nblocks.max(1 + self.evs.iter().map(|e| e.0 .0 as usize).max().unwrap_or(0)).max(1 + self.edges.iter().map(|e| e.0 .0.max(e.1 .0) as usize).max().unwrap_or(0));
        // (each block's events, in order: counted first, so each block's list is allocated once)
        let mut counts = vec![0u32; n];
        for (b, _) in &self.evs {
            counts[b.0 as usize] += 1;
        }
        let mut evs: Vec<Vec<&Ev>> = counts.iter().map(|&c| Vec::with_capacity(c as usize)).collect();
        for (b, e) in &self.evs {
            evs[b.0 as usize].push(e);
        }
        // (each block's distinct successors, in the order of their edges, in one array: block b's are
        // flat[so[b]..so[b + 1]])
        let mut es: Vec<(u32, u32)> = self.edges.iter().map(|e| (e.0 .0, e.1 .0)).collect();
        es.sort_by_key(|e| e.0);
        let mut flat: Vec<usize> = Vec::with_capacity(es.len());
        let mut so: Vec<usize> = vec![0; n + 1];
        let mut ei = 0;
        for (b, o) in so.iter_mut().enumerate().take(n) {
            *o = flat.len();
            let start = flat.len();
            while ei < es.len() && es[ei].0 as usize == b {
                let t = es[ei].1 as usize;
                if !flat[start..].contains(&t) {
                    flat.push(t);
                }
                ei += 1;
            }
        }
        so[n] = flat.len();
        let succ = |b: usize| &flat[so[b]..so[b + 1]];
        let nl = self.locals.len();
        let loans_on = need_loans;
        // (an `impl Trait` that captures no lifetime: the locals that hold a parameter's reference, as bound from it)
        if self.ret_no_capture {
            let all: Vec<&Ev> = self.evs.iter().map(|x| &x.1).collect();
            let again: Vec<u32> = all.iter().filter_map(|e| match e {
                Ev::Flow(l, _) | Ev::Store(l, ..) => Some(*l),
                _ => None,
            }).collect();
            let mut d: Vec<u32> = vec![];
            loop {
                let n0 = d.len();
                for e in &all {
                    if let Ev::Def(l, srcs) = e {
                        let from = |s: &Src| matches!(s, Src::Local(p) if (self.params.contains(p) && !self.static_params.contains(p)) || d.contains(p));
                        if !d.contains(l) && !again.contains(l) && !self.params.contains(l) && srcs.iter().any(from) {
                            d.push(*l);
                        }
                    }
                }
                if d.len() == n0 || d.len() > 256 {
                    break;
                }
            }
            *self.param_derived.borrow_mut() = d;
        }
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
                for &s2 in succ(b) {
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
        // (one array each: block b's words are row(b))
        let mut live_in = vec![0u64; words * n];
        let mut live_out = vec![0u64; words * n];
        let row = move |b: usize| b * words..(b + 1) * words;
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
            let (mut out, mut live) = (vec![0u64; words], vec![0u64; words]);
            while changed {
                changed = false;
                for b in (0..n).rev() {
                    out.fill(0);
                    for &s in succ(b) {
                        for (o, x) in out.iter_mut().zip(&live_in[row(s)]) {
                            *o |= x;
                        }
                    }
                    live.copy_from_slice(&out);
                    for e in evs[b].iter().rev() {
                        transfer_back(&mut live, e);
                    }
                    if live[..] != live_in[row(b)] {
                        live_in[row(b)].copy_from_slice(&live);
                        changed = true;
                    }
                    live_out[row(b)].copy_from_slice(&out);
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
            self.run_block(&evs[b], &mut st, None, &live_out[row(b)], loans_on, &mut vec![]);
            let ns = succ(b).len();
            for (i, &s) in succ(b).iter().enumerate() {
                let ch = match &mut st_in[s] {
                    Some(x) => x.join(&st),
                    slot @ None => {
                        // (the last successor takes the state: it is not used after)
                        *slot = Some(if i + 1 == ns { std::mem::take(&mut st) } else { st.clone() });
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
        let mut live_at = vec![];
        *self.ret_temps.borrow_mut() = (vec![], vec![], vec![]);
        // (the block each error is found in, for the order rustc reports them in: the ones before, none)
        let mut err_block: Vec<usize> = vec![usize::MAX; errs.len()];
        for b in 0..n {
            let Some(mut st) = st_in[b].take() else { continue };
            self.run_block(&evs[b], &mut st, Some(&mut errs), &live_out[row(b)], loans_on, &mut live_at);
            err_block.resize(errs.len(), b);
            // (the function ends here (return / panic): a loan stored behind a parameter must not be of a local)
            if succ(b).is_empty() {
                for &ln in &st.escaped {
                    let lo = &self.loans[ln as usize];
                    if lo.place.owned_len() == lo.place.proj.len() && !self.params.contains(&lo.place.local) {
                        let name = path_str(&self.locals, &lo.place);
                        errs.push(BkError { sp: lo.sp, msg: format!("`{name}` does not live long enough (stored where it outlives the function)") });
                    }
                    // (through a `&mut` field of a parameter whose type has a `Drop` impl, which is dropped when the
                    // function returns: E0713)
                    if self.params.contains(&lo.place.local) && self.drop_field_borrow(lo, &st) {
                        errs.push(BkError { sp: lo.sp, msg: "borrow may still be in use when destructor runs (E0713)".to_string() });
                    }
                }
            }
        }
        // (a temporary's loan that a returned value holds: E0515 where it is returned, not E0716 where it goes away)
        {
            let rt = self.ret_temps.borrow();
            for (ln, ei, hs) in &rt.0 {
                let ret = rt.1.iter().find(|r| r.0 == *ln).or_else(|| rt.2.iter().find(|r| hs.contains(&r.0)));
                if let (Some(&(_, sp)), Some(e)) = (ret, errs.get_mut(*ei)) {
                    *e = BkError { sp, msg: "cannot return value referencing temporary value (E0515)".to_string() };
                }
            }
        }
        err_block.resize(errs.len(), usize::MAX);
        // (rustc reports one use of a value a move moved, the first in its reverse postorder: of a use after a loop
        // and one in the loop's next iteration, the one after it (moves/nested-loop-moved-value-wrong-continue.rs))
        let moved_key = |m: &str| -> Option<(String, String)> {
            let i = m.find(" moved value: `").or_else(|| m.find(" partially moved value: `"))?;
            let rest = &m[i..];
            let path = rest.split('`').nth(1)?.to_string();
            let at = rest.split("(moved at ").nth(1)?.to_string();
            Some((path, at))
        };
        let keys: Vec<Option<(String, String)>> = errs.iter().map(|e| moved_key(&e.msg)).collect();
        let dup = keys.iter().enumerate().any(|(i, k)| k.is_some() && keys[..i].contains(k));
        if dup {
            // (the loops a block is in: a use in fewer of the loops the move is in comes first, as rustc's MIR has the
            // way out of a loop before its body, where zustc's loops over a range test at their end)
            let pos = rustc_rpo(n, &succ);
            let loops = natural_loops(n, &succ);
            let lb = self.line_base;
            let move_block = |at: &str| (0..n).find(|&b| evs[b].iter().any(|e| matches!(e, Ev::Acc(_, Acc::Move, sp) if self::at(lb, *sp) == at)));
            let mblock: Vec<Option<usize>> = keys.iter().map(|k| k.as_ref().and_then(|k| move_block(k.1.trim_end_matches(')')))).collect();
            let rank = |i: usize| {
                let b = err_block.get(i).copied().filter(|&b| b < n);
                let depth = match (b, mblock[i]) {
                    (Some(b), Some(m)) => loops.iter().filter(|l| l[b] && l[m]).count(),
                    _ => 0,
                };
                (depth, b.and_then(|b| pos.get(b).copied()).unwrap_or(0), i)
            };
            let mut keep = vec![true; errs.len()];
            for i in 0..errs.len() {
                let Some(k) = &keys[i] else { continue };
                keep[i] = !(0..errs.len()).any(|j| j != i && keys[j].as_ref() == Some(k) && rank(j) < rank(i));
            }
            let mut it = keep.iter();
            errs.retain(|_| *it.next().unwrap_or(&true));
        }
        if self.lint_mut {
            used.extend(self.mut_flow.borrow().1.iter().copied());
            used.sort_unstable();
            self.unused_mut(&used, &mut errs);
        }
        // (the borrows of a local not `mut` reported as one error at its declaration: rustc reports them after the rest)
        errs.sort_by_key(|e| (e.msg.contains("is not declared as mutable") && self.locals.iter().any(|l| l.decl.line != 0 && l.decl == e.sp), e.sp.line, e.sp.col));
        errs.into_iter().next()
    }

    /// unused_mut: the `let mut` locals of the program's (declared where it is known, not a closure's capture, not a
    /// name the lowering made) that nothing needs mutable (used, sorted)
    fn unused_mut(&self, used: &[u32], errs: &mut Vec<BkError>) {
        if !self.lint_mut {
            return;
        }
        for (i, lo) in self.locals.iter().enumerate() {
            if lo.mutable == Some(true) && lo.decl.line != 0 && !lo.via_ref && !lo.name.starts_with("__") && !lo.name.starts_with('#') && lo.name != TEMP && !self.cap_vals.contains(&(i as u32)) && used.binary_search(&(i as u32)).is_err() {
                errs.push(BkError { sp: lo.decl, msg: format!("variable does not need to be mutable: `{}` (unused_mut)", lo.name) });
            }
        }
    }

    /// Apply the events of one block (and report errors when errs is given). `live_out` is the liveness at the end of
    /// the block; the liveness at each event is recomputed backward from it
    fn run_block(&self, evs: &[&Ev], st: &mut State, mut errs: Option<&mut Vec<BkError>>, live_out: &[u64], loans_on: bool, live_at: &mut Vec<u64>) {
        // (liveness after each event: only for reporting. Event i's words are live_at[i * w..][..w])
        let loans_on = loans_on && errs.is_some();
        let w = live_out.len();
        if loans_on {
            live_at.clear();
            live_at.resize(evs.len() * w, 0);
            let mut live = live_out.to_vec();
            for (i, e) in evs.iter().enumerate().rev() {
                live_at[i * w..(i + 1) * w].copy_from_slice(&live);
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
                    } else if self.locals[*l as usize].mutable == Some(false) || self.mut_flow.borrow().0.binary_search(l).is_ok() {
                        st.inited.push(*l);
                        st.inited.sort_unstable();
                    }
                    // (what it holds through the regions of its type stays: NLL's liveness is of the variable)
                    let kept = self.region_held(st, *l);
                    st.holds.retain(|h| h.0 != *l);
                    if !kept.is_empty() {
                        st.set_holds(*l, kept);
                    }
                    st.kill_loans_of(*l, &self.loans);
                }
                Ev::Dead(l, sp) => {
                    for (ln, lo) in self.loans.iter().enumerate() {
                        // (a borrow through a `&mut` field of the `Drop` value going away: gone with it, for a local
                        // that takes it after (`let r = { let s = S(&mut x); &mut *s.0 };`, E0713 at the Keep))
                        if lo.place.local == *l && lo.place.owned_len() < lo.place.proj.len() && self.drop_field_borrow(lo, st) {
                            add_sorted(&mut st.dead, ln as u32);
                            continue;
                        }
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
                            // (a borrow through a `&mut` field of a `Drop` value the local owns, still in use when
                            // the local is dropped here (`members.push(child.raw)` in a loop): its drop may use the
                            // field, E0713)
                            if self.drop_owners.iter().any(|d| d.0 == *l) {
                                let ll = live_loans(st, &live_at[i * w..(i + 1) * w]);
                                if let Some(&ln) = ll.iter().find(|&&ln| self.loans[ln as usize].place.local == *l && self.drop_field_borrow(&self.loans[ln as usize], st)) {
                                    errs.push(BkError { sp: self.loans[ln as usize].sp, msg: "borrow may still be in use when destructor runs (E0713)".to_string() });
                                }
                            }
                            // (a loan of the local, or of what it owns, still live when it goes away)
                            for ln in live_loans(st, &live_at[i * w..(i + 1) * w]) {
                                let lo = &self.loans[ln as usize];
                                if lo.place.local == *l && lo.place.owned_len() == lo.place.proj.len() {
                                    let name = path_str(&self.locals, &BPlace { local: *l, proj: vec![] });
                                    if name == TEMP {
                                        let live = &live_at[i * w..(i + 1) * w];
                                        let hs = st.holds.iter().filter(|h| h.0 != *l && is_live(live, h.0) && h.1.binary_search(&ln).is_ok()).map(|h| h.0).collect();
                                        self.ret_temps.borrow_mut().0.push((ln, errs.len(), hs));
                                        errs.push(BkError { sp: lo.sp, msg: "temporary value dropped while borrowed: it is freed while still in use (E0716)".to_string() });
                                        continue;
                                    }
                                    errs.push(BkError { sp: lo.sp, msg: if sp.line == 0 { format!("`{name}` does not live long enough (dropped while still borrowed)") } else { format!("`{name}` does not live long enough (borrowed value dropped at {} while still borrowed)", at(self.line_base, *sp)) } });
                                }
                            }
                        }
                    }
                    st.moved.retain(|m| m.place.local != *l);
                    // (what it holds stays: a value read from it before it went away may flow into a local after
                    // (`let a = match y { (ref mut a, _) => a };`). A local not live holds no live loan)
                }
                Ev::Acc(p, Acc::Drop, _) => {
                    // (a value whose drop may see what it borrows, holding a loan of itself (`d.0.set(Some(&d))` with
                    // D: Drop): the loan is in use while d is dropped, E0597 at the borrow)
                    // (only a local dropped at its scope's end with such a drop: not `let _ = d;`, a fake read. A move
                    // of it on another path does not take the loan away: one that held a loan of itself cannot be
                    // moved, rustc's E0505)
                    if let (Some(errs), true, true) = (errs.as_deref_mut(), loans_on, p.proj.is_empty() && self.dropck_locals.contains(&p.local)) {
                        let own = st.held(p.local).iter().copied().find(|&ln| {
                            let lo = &self.loans[ln as usize];
                            lo.place.local == p.local && lo.place.owned_len() == lo.place.proj.len()
                        });
                        if let Some(ln) = own {
                            let name = path_str(&self.locals, &BPlace { local: p.local, proj: vec![] });
                            if name != TEMP {
                                errs.push(BkError { sp: self.loans[ln as usize].sp, msg: format!("`{name}` does not live long enough (dropped while still borrowed)") });
                            }
                        }
                    }
                }
                Ev::Acc(p, a, sp) => {
                    if let Some(errs) = errs.as_deref_mut() {
                        if *a == Acc::Write && p.proj.is_empty() {
                            self.check_mut(st, p, *a, *sp, errs);
                            // (unused_mut: a `let mut x;` assigned when it may have a value already needs the `mut`)
                            if st.inited.binary_search(&p.local).is_ok() && self.mut_flow.borrow().0.binary_search(&p.local).is_ok() {
                                self.mut_flow.borrow_mut().1.push(p.local);
                            }
                        }
                        self.check_moved(st, p, *a, *sp, errs);
                        if loans_on {
                            let ll = live_loans(st, &live_at[i * w..(i + 1) * w]);
                            self.check_loans(&ll, p, *a, *sp, errs);
                        }
                    }
                    // (not out of a closure's capture held by reference: which captures are held by value is
                    // decided from the syntax, the body may move one the summary took for a borrow)
                    if let (Acc::Move, Some(errs), false) = (a, errs.as_deref_mut(), self.locals[p.local as usize].via_ref) {
                        if p.owned_len() < p.proj.len() || p.proj.contains(&Proj::Index) {
                            let what = if p.owned_len() < p.proj.len() { "which is behind a reference" } else { "an indexed place" };
                            // (of a temporary's part (a match on `(a, b)`), or of a local the lowering made (a `for` over an
                            // Option's `__exr_ox`): of the reference, as rustc says it)
                            let lname = &self.locals[p.local as usize].name;
                            let msg = if (lname == TEMP || lname.starts_with("__exr")) && p.owned_len() < p.proj.len() {
                                // (a `&mut`'s deref is `Deref(false)`, a `&`'s `Shr`, ref_deref)
                                let m = matches!(p.proj.get(p.owned_len()), Some(Proj::Deref(false)));
                                format!("cannot move out of a {} reference", if m { "mutable" } else { "shared" })
                            } else {
                                format!("cannot move out of `{}`, {what}", path_str(&self.locals, p))
                            };
                            errs.push(BkError { sp: *sp, msg });
                        }
                    }
                    match a {
                        // (a new instance of the loan: the one of the previous iteration of a loop is gone)
                        Acc::Borrow(ln) => {
                            if let Ok(i) = st.dead.binary_search(ln) {
                                st.dead.remove(i);
                            }
                            if self.forever.binary_search(ln).is_ok() || (self.ret_only.binary_search(ln).is_ok() && self.ret_made.binary_search(ln).is_ok()) {
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
                            // (reinitializes the path and everything under it; not through an index, which is some element,
                            // not a known one: `a[1] = x` after `let [_, _] = a;` leaves a[0] moved)
                            if p.owned_len() == p.proj.len() && !p.proj.contains(&Proj::Index) {
                                // (a union's field written: the whole union is, its other fields too)
                                st.moved.retain(|m| !(m.place.local == p.local && (prefix(&p.proj, &m.place.proj) || union_overlap(&p.proj, &m.place.proj))));
                            }
                            if !p.proj.is_empty() && p.owned_len() == p.proj.len() && !p.proj.contains(&Proj::Index) && !super::no_polonius() {
                                st.kill_loans_through(p, &self.loans);
                            }
                            if p.proj.is_empty() {
                                st.holds.retain(|h| h.0 != p.local);
                                st.kill_loans_of(p.local, &self.loans);
                                if !super::no_polonius() {
                                    st.kill_loans_through(p, &self.loans);
                                }
                                if self.locals[p.local as usize].mutable == Some(false) || self.mut_flow.borrow().0.binary_search(&p.local).is_ok() {
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
                        if loans_on && is_live(&live_at[i * w..(i + 1) * w], *l) {
                            let ls = self.resolve(srcs, st);
                            if let Some(&ln) = ls.iter().find(|ln| st.dead.binary_search(ln).is_ok()) {
                                let lo = &self.loans[ln as usize];
                                let name = path_str(&self.locals, &BPlace { local: lo.place.local, proj: vec![] });
                                if lo.place.owned_len() < lo.place.proj.len() {
                                    errs.push(BkError { sp: lo.sp, msg: "borrow may still be in use when destructor runs (E0713)".to_string() });
                                } else if name == TEMP {
                                    self.ret_temps.borrow_mut().0.push((ln, errs.len(), vec![*l]));
                                    errs.push(BkError { sp: lo.sp, msg: "temporary value dropped while borrowed: it is freed while still in use (E0716)".to_string() });
                                } else {
                                    errs.push(BkError { sp: lo.sp, msg: format!("`{name}` does not live long enough (dropped while still borrowed)") });
                                }
                            }
                        }
                    }
                }
                Ev::Def(l, srcs) | Ev::Flow(l, srcs) => {
                    let mut ls = self.resolve(srcs, st);
                    // (a value copied out of what a reference points to: not the loan of the place it was read from,
                    // what that place holds)
                    if let Some(mask) = self.copy_masks.get(l).map(|&i| &self.masks[i]) {
                        let mut out = Vec::with_capacity(ls.len());
                        for ln in ls {
                            if mask.get(ln as usize).copied().unwrap_or(false) {
                                out.extend_from_slice(st.held(self.loans[ln as usize].place.local));
                            } else {
                                out.push(ln);
                            }
                        }
                        ls = out;
                    }
                    if let Ev::Flow(..) = e {
                        ls.extend_from_slice(st.held(*l));
                    } else {
                        let kept = self.region_held(st, *l);
                        // (a loan of a local already gone that it holds through the regions of its type, and it is
                        // used: `let d = { let p = ..; f.m(&p) };`. Not what the value computed flows from: an
                        // over-approximation)
                        if let (Some(errs), true) = (errs.as_deref_mut(), loans_on && is_live(&live_at[i * w..(i + 1) * w], *l)) {
                            if let Some(&ln) = kept.iter().find(|ln| st.dead.binary_search(ln).is_ok() && self.loans[**ln as usize].place.owned_len() == self.loans[**ln as usize].place.proj.len()) {
                                let lo = &self.loans[ln as usize];
                                let name = path_str(&self.locals, &BPlace { local: lo.place.local, proj: vec![] });
                                errs.push(BkError { sp: lo.sp, msg: if name == TEMP { "temporary value dropped while borrowed (E0716)".to_string() } else { format!("`{name}` does not live long enough (dropped while still borrowed)") } });
                            }
                        }
                        ls.extend(kept);
                    }
                    st.set_holds(*l, ls);
                }
                Ev::Escape(srcs) => {
                    for ln in self.resolve(srcs, st) {
                        add_sorted(&mut st.escaped, ln);
                    }
                }
                Ev::RetHold(l) => {
                    let held: Vec<u32> = st.held(*l).to_vec();
                    let ls: Vec<u32> = held.iter().copied().filter(|ln| self.ret_only.binary_search(ln).is_ok()).collect();
                    for ln in ls {
                        // (and the loans made after it that the same local holds (`let t = (&mut *x, &mut *x); t.0`):
                        // it was live when they were made. Not a two-phase borrow's reservation, which its arguments
                        // may read (`let r = v.get_mut(v.len() - 1); r`))
                        if let (true, Some(errs)) = (loans_on, errs.as_deref_mut()) {
                            if !self.reserved.contains(&ln) {
                                for &later in held.iter().filter(|&&o| o > ln) {
                                    let lo = &self.loans[later as usize];
                                    let n0 = errs.len();
                                    self.check_loans(&[ln], &lo.place, Acc::Borrow(later), lo.sp, errs);
                                    if errs.len() > n0 {
                                        break;
                                    }
                                }
                            }
                        }
                        // (two reborrows returned for a universal region are both live at the return: the later one
                        // conflicts with the earlier, also when one expression made both (`let t = (&mut *x, &mut *x);
                        // t`), whose conflicts inside it the loans of locals do not see)
                        if let (true, Some(errs)) = (loans_on, errs.as_deref_mut()) {
                            for &other in st.escaped.iter().filter(|&&o| o != ln && self.ret_only.binary_search(&o).is_ok()) {
                                let (l1, l2) = if other < ln { (other, ln) } else { (ln, other) };
                                let lo = &self.loans[l2 as usize];
                                let n0 = errs.len();
                                self.check_loans(&[l1], &lo.place, Acc::Borrow(l2), lo.sp, errs);
                                if errs.len() > n0 {
                                    break;
                                }
                            }
                        }
                        add_sorted(&mut st.escaped, ln);
                    }
                }
                // (checked in the pre-pass, in the order the events are made)
                Ev::Guard(_) => {}
                Ev::HoldThrough(l, srcs) => {
                    let ls = self.resolve(srcs, st);
                    let targets: Vec<u32> = st.held(*l).iter().map(|&ln| &self.loans[ln as usize].place).filter(|p| p.owned_len() == p.proj.len()).map(|p| p.local).collect();
                    for t in targets {
                        if t != *l && self.locals[t as usize].refs {
                            let mut h = st.held(t).to_vec();
                            h.extend(ls.iter().copied());
                            st.set_holds(t, h);
                        }
                    }
                }
                Ev::Store(l, derefs, deep, srcs) => {
                    let ls = self.resolve(srcs, st);
                    // (the locals the `&mut`s point to, deref by deref; one behind a parameter: it outlives the
                    // function)
                    let mut at = vec![*l];
                    let mut escapes = false;
                    for _ in 0..*derefs {
                        // (each local the `&mut`s point to, and whether through a parameter's pointee)
                        let mut next: Vec<(u32, bool)> = vec![];
                        let push = |next: &mut Vec<(u32, bool)>, local: u32, behind: bool| match next.iter_mut().find(|n| n.0 == local) {
                            Some(n) => n.1 |= behind,
                            None => next.push((local, behind)),
                        };
                        for &x in &at {
                            for &ln in st.held(x) {
                                let lo = &self.loans[ln as usize];
                                if !lo.mutable {
                                    continue;
                                }
                                let through_ref = lo.place.owned_len() < lo.place.proj.len();
                                let behind = through_ref && self.params.contains(&lo.place.local);
                                if through_ref && !behind {
                                    // (a reborrow through a local's `&mut` (`q = &mut *p`): it points to what p points to)
                                    for &h in st.held(lo.place.local) {
                                        let hl = &self.loans[h as usize];
                                        if hl.mutable {
                                            push(&mut next, hl.place.local, hl.place.owned_len() < hl.place.proj.len() && self.params.contains(&hl.place.local));
                                        }
                                    }
                                } else {
                                    push(&mut next, lo.place.local, behind);
                                }
                            }
                        }
                        // (a `&mut p` holds what p holds too (`p = &mut z`): z is reached through p, one deref
                        // deeper, not by this one. `*q = &mut y` with q = &mut p changes p, not z, nor what z points
                        // to when z is a parameter's. Unless what is stored holds references itself: `*q = &mut &y`
                        // makes p's type's inner region, z's too, y's)
                        if !*deep {
                            let through: Vec<u32> = next.iter().map(|n| n.0).filter(|&c| next.iter().any(|&(o, _)| o != c && st.held(o).iter().any(|&h| self.loans[h as usize].place.local == c))).collect();
                            next.retain(|n| !through.contains(&n.0));
                        }
                        escapes |= next.iter().any(|n| n.1);
                        at = next.iter().filter(|n| !n.1).map(|n| n.0).collect();
                    }
                    if escapes {
                        for &ln in &ls {
                            add_sorted(&mut st.escaped, ln);
                        }
                    }
                    for t in at {
                        if t != *l && self.locals[t as usize].refs {
                            let mut h = st.held(t).to_vec();
                            h.extend_from_slice(&ls);
                            st.set_holds(t, h);
                        }
                    }
                }
                Ev::Ret(srcs, sp) => {
                    if loans_on {
                        if let Some(errs) = errs.as_deref_mut() {
                            let ls = self.resolve(srcs, st);
                            let mut rt = self.ret_temps.borrow_mut();
                            rt.1.extend(ls.iter().map(|&ln| (ln, *sp)));
                            rt.2.extend(srcs.iter().filter_map(|s| if let Src::Local(x) = s { Some((*x, *sp)) } else { None }));
                            drop(rt);
                            // (a borrow through a `&mut` field of a local whose type implements `Drop`: the local's
                            // drop, when the function returns, may use the field, so the borrow is still in use then
                            // (`&mut *s.0` with s: Scribble). Not through a `&` field: a shared one is copied out)
                            // (also what is reborrowed through a reference a local holds: `&r.0` with `r = &*v.0`
                            // returns a borrow of what r points to, the loans r holds)
                            let mut through = ls.clone();
                            let mut k = 0;
                            while k < through.len() && through.len() < 64 {
                                let lp = &self.loans[through[k] as usize].place;
                                if lp.owned_len() < lp.proj.len() {
                                    for &h in st.held(lp.local) {
                                        if !through.contains(&h) {
                                            through.push(h);
                                        }
                                    }
                                }
                                k += 1;
                            }
                            // (an `impl Trait` that captures no lifetime: a borrow through a parameter's reference, or the
                            // reference itself, is in its hidden type, which may not hold it)
                            if self.ret_no_capture {
                                let derived = self.param_derived.borrow();
                                let non_static = |l: u32| (self.params.contains(&l) && !self.static_params.contains(&l)) || derived.contains(&l);
                                let through_param = ls.iter().any(|&ln| {
                                    let lp = &self.loans[ln as usize].place;
                                    non_static(lp.local) && lp.owned_len() < lp.proj.len()
                                });
                                let param_ref = srcs.iter().any(|s| matches!(s, Src::Local(l) if non_static(*l) && self.locals.get(*l as usize).is_some_and(|x| x.refs)));
                                if through_param || param_ref {
                                    errs.push(BkError { sp: *sp, msg: "hidden type for `impl Trait` captures lifetime that does not appear in bounds (E0700)".to_string() });
                                }
                            }
                            if let Some(&ln) = through.iter().find(|&&ln| self.drop_field_borrow(&self.loans[ln as usize], st)) {
                                errs.push(BkError { sp: self.loans[ln as usize].sp, msg: "borrow may still be in use when destructor runs (E0713)".to_string() });
                            }
                            // (a closure whose kind nothing decides that returns a mutable borrow of what it captures
                            // by reference (a closure that mutates it, `|| { || x.push(()) }`): the borrow is through
                            // the `&mut self` of a call, which it cannot outlive)
                            if self.fnmut_ret && ls.iter().any(|&ln| {
                                let lo = &self.loans[ln as usize];
                                lo.mutable && self.locals.get(lo.place.local as usize).is_some_and(|l| l.via_ref)
                            }) {
                                errs.push(BkError { sp: *sp, msg: "captured variable cannot escape `FnMut` closure body (what is returned holds a mutable borrow of a capture)".to_string() });
                            }
                            for &ln in &ls {
                                let lo = &self.loans[ln as usize];
                                if lo.place.owned_len() == lo.place.proj.len() {
                                    let what = if self.locals[lo.place.local as usize].name == "self" { "a value owned by the function" } else { "a local variable" };
                                    let name = path_str(&self.locals, &lo.place);
                                    // (a closure in a local returned as an `impl Trait`: rustc's E0597 at the use in its
                                    // body, for each capture it borrows (the first on the earliest line is reported))
                                    if name != TEMP && lo.capture && self.ret_impl && !srcs.contains(&Src::Loan(ln)) {
                                        for &cl in &ls {
                                            let co = &self.loans[cl as usize];
                                            if co.capture && co.place.owned_len() == co.place.proj.len() {
                                                let at = self.cap_path_at.get(&cl).copied().unwrap_or(co.at);
                                                errs.push(BkError { sp: at, msg: format!("`{}` does not live long enough (E0597)", path_str(&self.locals, &co.place)) });
                                            }
                                        }
                                        break;
                                    }
                                    // (a closure that borrows it: at the closure, E0373)
                                    errs.push(if name == TEMP {
                                        BkError { sp: *sp, msg: "cannot return reference to temporary value (E0515)".to_string() }
                                    } else if lo.capture {
                                        BkError { sp: lo.sp, msg: format!("closure may outlive the current function, but it borrows `{name}`, which is owned by the current function (E0373)") }
                                    } else {
                                        if synthetic_param(&name) {
                                            BkError { sp: *sp, msg: "cannot return value referencing function parameter (E0515)".to_string() }
                                        } else {
                                            BkError { sp: *sp, msg: format!("cannot return a reference to {what}: `{name}` is dropped when the function returns (E0515)") }
                                        }
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

    /// The loans local l holds through the regions of its type (held_by): kept when it is bound or assigned again
    fn region_held(&self, st: &State, l: u32) -> Vec<u32> {
        if self.held_by.is_empty() {
            return vec![];
        }
        st.held(l).iter().copied().filter(|ln| self.held_by.binary_search_by_key(ln, |x| x.0).is_ok_and(|k| self.held_by[k].1.contains(&l))).collect()
    }

    /// Mutation of a local not declared `mut`
    fn check_mut(&self, st: &State, p: &BPlace, a: Acc, sp: Sp, errs: &mut Vec<BkError>) {
        let lo = &self.locals[p.local as usize];
        let writes = match a {
            Acc::Borrow(l) => self.loans[l as usize].mutable,
            Acc::Write | Acc::Modify => true,
            _ => false,
        };
        // (through a `&`: `&mut *r` / `r.x = 1` with r: &T. Not in a closure's body through a capture held by
        // reference: the function that makes the closure reports it, at the capture)
        if writes && p.proj.contains(&Proj::Shr) && !self.locals[p.local as usize].via_ref {
            let i = p.proj.iter().rposition(|x| *x == Proj::Shr).unwrap();
            let behind = path_str(&self.locals, &BPlace { local: p.local, proj: p.proj[..=i].to_vec() });
            // (through an overloaded `Deref`: "data in an `Rc`", as rustc says)
            let over = self.overloaded.borrow().get(&(p.local, i)).map(|n| if matches!(n.as_str(), "Rc" | "Arc") { format!("data in an `{n}`") } else { format!("data in dereference of `{n}`") });
            let msg = match (a, over) {
                (Acc::Borrow(_), Some(d)) => format!("cannot borrow {d} as mutable (E0596)"),
                (_, Some(d)) => format!("cannot assign to {d} (E0594)"),
                (a, None) => match a {
                    Acc::Borrow(_) => format!("cannot borrow `{}` as mutable, as `{behind}` is behind a `&` reference", path_str(&self.locals, p)),
                    _ => format!("cannot assign to `{}`, which is behind a `&` reference", path_str(&self.locals, p)),
                },
            };
            // (a closure's capture through the `&`: at the closure, as rustc reports it, not at the use in its body)
            let sp = match a {
                Acc::Borrow(l) if self.loans[l as usize].capture => self.loans[l as usize].sp,
                Acc::Borrow(l) => self.loans[l as usize].at,
                _ => sp,
            };
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
                // (two or more such borrows of it: rustc reports them as one error, at the declaration)
                // (not with a closure's capture among them: rustc reports those as the closure's own errors)
                let of = |x: &&Loan| x.mutable && x.place.local == p.local && x.place.owned_len() == x.place.proj.len();
                let n = if self.loans.iter().filter(of).any(|x| x.capture) { 0 } else { self.loans.iter().filter(of).count() };
                let sp = if n >= 2 && lo.decl.line != 0 { lo.decl } else { self.loans[l as usize].at };
                errs.push(BkError { sp, msg });
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

    /// lo borrows through a `&mut` field of a value whose type implements `Drop` that its local owns (drop_owners),
    /// not moved out: the value's drop may use the field, so the borrow must end before the drop
    fn drop_field_borrow(&self, lo: &Loan, st: &State) -> bool {
        let (pr, ol) = (&lo.place.proj, lo.place.owned_len());
        // (not past a `&` deref after it: `&**s.data` with `data: &mut &'static u32` borrows what the shared
        // reference points to, which the drop does not reach)
        if ol >= pr.len() || pr[ol] != Proj::Deref(false) || pr.iter().skip(ol + 1).any(|x| *x == Proj::Shr) {
            return false;
        }
        self.drop_owners.iter().filter(|d| d.0 == lo.place.local).any(|(_, op)| {
            let k = op.len();
            // (not when the owner, or what holds it, was moved out: then it is not dropped here)
            pr.len() > k && prefix(op, &pr[..k]) && matches!(pr[k], Proj::Field(_)) && !st.moved.iter().any(|m| m.place.local == lo.place.local && prefix(&m.place.proj, op))
        })
    }

    /// The place of a running guard's match that a write or a `&mut` of p changes (E0510): p is the place or one it
    /// is part of, or, for a place a pattern tests, what it points to (`*x = None` in a guard of
    /// `match x { &mut None => .. }`). Not a part of it (`s.b = 2` of `match s`): rustc's fake borrows are shallow
    fn guard_conflict<'g>(&self, guards: &[&'g [(BPlace, bool)]], p: &BPlace, a: Acc) -> Option<&'g BPlace> {
        let changes = match a {
            Acc::Write | Acc::Modify => true,
            Acc::Borrow(l) => self.loans[l as usize].mutable,
            _ => false,
        };
        if !changes {
            return None;
        }
        guards.iter().flat_map(|gs| gs.iter()).find(|(g, deep)| {
            // (a write to another field of a union the pattern tests is to the tested memory, union_overlap)
            g.local == p.local
                && (prefix(&p.proj, &g.proj) || union_overlap(&p.proj, &g.proj) || *deep && prefix(&g.proj, &p.proj) && p.proj[g.proj.len()..].iter().all(|x| matches!(x, Proj::Deref(_))))
        }).map(|g| &g.0)
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
                        || union_overlap(&p.proj, &lo.place.proj)
                }
                _ => overlap(&lo.place, p),
            };
            if !conflict {
                continue;
            }
            let name = path_str(&self.locals, p);
            let at = at(self.line_base, lo.sp);
            // (a two-phase borrow's reservation conflicts as a shared loan, and is reported as the `&mut` it is)
            let lo_mut = lo.mutable || self.reserved.contains(&ln);
            let lk = if lo_mut { "mutable" } else { "immutable" };
            let msg = match a {
                // (the reference read for an implicit reborrow of the previous round of a loop: `f(x)` again)
                Acc::Read if lo.sp == sp && self.implicit.contains(&ln) => {
                    format!("cannot borrow `*{name}` as mutable more than once at a time (first borrow at {at}, in the previous iteration of the loop)")
                }
                Acc::Read | Acc::Drop => format!("cannot use `{name}` because it was mutably borrowed (at {at})"),
                Acc::Move => format!("cannot move out of `{name}` because it is borrowed (at {at})"),
                Acc::Write | Acc::Modify => format!("cannot assign to `{name}` because it is borrowed (at {at})"),
                Acc::Borrow(l) if self.loans[l as usize].mutable && lo_mut => {
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
    /// Moved out by a pattern's binding by value of a `&mut` (`let S { p } = s;` with p: `&mut T`): not reborrowed,
    /// as a `&mut` passed by value is
    ValMove,
}

/// The expression whose value e is: through parentheses and the tails of blocks (`unsafe { *p }`)
pub(crate) fn value_tail(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(x) => value_tail(x),
        Expr::Block(None, _, b) => match b.stmts.last() {
            Some(crate::parse::Stmt::Expr(t, false)) => value_tail(t),
            _ => e,
        },
        e => e,
    }
}

/// The parameters of a closure whose value its result is made of, as the body is written (`|a| Some(a)`, `|a| &a.x`,
/// `|a| if c { a } else { b }`, `|s| s.as_str()`): the result of a call holds what those arguments borrow, as rustc
/// infers the closure's one signature. `names`: the parameters bound by name, with their indices. Not what a call or
/// a copy out of the parameter gives (`|k| map.get(k)` borrows the map)
pub(crate) fn closure_ret_params(e: &Expr, names: &[(String, usize)], out: &mut Vec<usize>, depth: u32) {
    if depth > 16 || names.is_empty() {
        return;
    }
    // (the parameter a place is rooted at: `a`, `a.x`, `a[0]`, `*a`)
    fn root<'e>(e: &'e Expr) -> Option<&'e str> {
        match e {
            Expr::Paren(x) | Expr::Field(x, ..) | Expr::Index(x, ..) => root(x),
            Expr::Unary(op, x, _) if op == "*" => root(x),
            Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => Some(p.segs[0].name.as_str()),
            _ => None,
        }
    }
    fn block(b: &crate::parse::Block, names: &[(String, usize)], out: &mut Vec<usize>, depth: u32) {
        // (a parameter a `let` of the block shadows is not the tail's)
        let Some(crate::parse::Stmt::Expr(t, false)) = b.stmts.last() else { return };
        let lets = |n: &str| b.stmts.iter().any(|s| matches!(s, crate::parse::Stmt::Let(q, ..) if pat_any(q, &|q| matches!(q, Pat::Ident(_, _, m, _) if m == n))));
        let names: Vec<(String, usize)> = names.iter().filter(|x| !lets(&x.0)).cloned().collect();
        closure_ret_params(t, &names, out, depth + 1);
    }
    let add = |n: &str, out: &mut Vec<usize>| {
        if let Some(&(_, i)) = names.iter().find(|x| x.0 == n) {
            if !out.contains(&i) {
                out.push(i);
            }
        }
    };
    match e {
        Expr::Paren(x) | Expr::Cast(x, _) => closure_ret_params(x, names, out, depth + 1),
        Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => add(&p.segs[0].name, out),
        Expr::Ref(_, x) => {
            if let Some(n) = root(x) {
                add(n, out);
            }
        }
        // (a constructor's arguments: `Some(a)`, `Wrap(a, 1)`)
        Expr::Call(f, args) if matches!(&**f, Expr::Path(None, p) if p.segs.last().is_some_and(|s| s.name.starts_with(|c: char| c.is_ascii_uppercase()))) => {
            for a in args {
                closure_ret_params(a, names, out, depth + 1);
            }
        }
        Expr::Tuple(xs) | Expr::Array(xs) => {
            for x in xs {
                closure_ret_params(x, names, out, depth + 1);
            }
        }
        Expr::Struct(_, fs, _) => {
            for (_, x) in fs {
                closure_ret_params(x, names, out, depth + 1);
            }
        }
        // (a view the receiver's referent gives: `s.as_str()`, `v.first()`)
        Expr::MethodCall(recv, m, _, args, _) if args.is_empty() && ["as_str", "as_slice", "as_ref", "as_mut", "as_deref", "as_deref_mut", "as_bytes", "as_mut_slice", "trim", "trim_start", "trim_end", "first", "last", "first_mut", "last_mut", "iter", "iter_mut", "chars", "bytes", "lines", "unwrap"].contains(&m.as_str()) => {
            closure_ret_params(recv, names, out, depth + 1)
        }
        Expr::Block(None, false, b) => block(b, names, out, depth),
        Expr::If(_, t, els) => {
            block(t, names, out, depth);
            if let Some(x) = els {
                closure_ret_params(x, names, out, depth + 1);
            }
        }
        Expr::Match(_, arms) => {
            for a in arms {
                // (an arm's pattern may shadow a parameter)
                let names: Vec<(String, usize)> = names.iter().filter(|x| !pat_any(&a.pat, &|q| matches!(q, Pat::Ident(_, _, m, _) if *m == x.0))).cloned().collect();
                closure_ret_params(&a.body, &names, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// A pattern that binds the whole value by value (`x`, `x @ ..`): it moves the scrutinee
pub(crate) fn pat_takes_whole(q: &Pat) -> bool {
    match q {
        Pat::Ident(false, _, n, _) => n.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') && n != "_",
        // (`x @ Some(_) | x @ None`: each alternative takes it)
        Pat::Or(ps, _) => !ps.is_empty() && ps.iter().all(pat_takes_whole),
        _ => false,
    }
}

/// p binds by reference somewhere (`ref x`, `ref mut x`)
pub(crate) fn pat_binds_ref(p: &Pat) -> bool {
    pat_any(p, &|q| matches!(q, Pat::Ident(true, ..)))
}

/// p binds by `ref` somewhere not under a `&` pattern: a loan of the scrutinee itself, not of what a reference in it
/// points to (`(&Some(ref c), _)` borrows `*s.0`)
fn pat_binds_ref_outside_ref(p: &Pat) -> bool {
    match p {
        Pat::Ident(r, _, _, sub) => *r || sub.as_deref().is_some_and(pat_binds_ref_outside_ref),
        Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Or(ps, _) | Pat::Slice(ps) => ps.iter().any(pat_binds_ref_outside_ref),
        Pat::Struct(_, fs, _) => fs.iter().any(|(_, q)| pat_binds_ref_outside_ref(q)),
        _ => false,
    }
}

pub(crate) fn pat_binds_ref_mut(p: &Pat) -> bool {
    pat_any(p, &|q| matches!(q, Pat::Ident(true, true, ..)))
}

/// p binds the name n somewhere
pub(crate) fn pat_binds(p: &Pat, n: &str) -> bool {
    pat_any(p, &|q| matches!(q, Pat::Ident(_, _, m, _) if m == n))
}

fn pat_any(p: &Pat, f: &dyn Fn(&Pat) -> bool) -> bool {
    if f(p) {
        return true;
    }
    match p {
        Pat::Ident(_, _, _, Some(s)) => pat_any(s, f),
        Pat::Tuple(ps) | Pat::TupleStruct(_, ps) | Pat::Or(ps, _) | Pat::Slice(ps) => ps.iter().any(|q| pat_any(q, f)),
        Pat::Struct(_, fs, _) => fs.iter().any(|(_, q)| pat_any(q, f)),
        Pat::Ref(_, q) => pat_any(q, f),
        _ => false,
    }
}

/// The pattern looks at the value (a discriminant, a literal, a length), so matching reads the scrutinee
fn pat_reads(p: &Pat) -> bool {
    match p {
        Pat::Wild | Pat::Rest => false,
        Pat::Ident(_, _, _, sub) => sub.as_deref().is_some_and(pat_reads),
        // (an array pattern tests nothing of the array itself (`let [_, x] = a;` after a[0] moved); a slice's length
        // is not followed)
        Pat::Tuple(ps) | Pat::Slice(ps) => ps.iter().any(pat_reads),
        Pat::Ref(_, q) => pat_reads(q),
        _ => true,
    }
}

/// pat_reads for a scrutinee of type t: a pattern of a struct that is not an enum tests nothing of the struct itself
/// (`match y { Y(_, ref mut b) => b }` after `y.0` is borrowed mutably), only what its parts' patterns test
fn pat_reads_ty(prog: &Program, p: &Pat, t: Option<Ty>) -> bool {
    let plain = |sid: u32| !prog.structs[sid as usize].is_enum;
    match (p, t) {
        (Pat::TupleStruct(_, ps), Some(Ty::Struct(sid))) if plain(sid) => ps.iter().any(pat_reads),
        (Pat::Struct(_, fps, _), Some(Ty::Struct(sid))) if plain(sid) => fps.iter().any(|(_, q)| pat_reads(q)),
        _ => pat_reads(p),
    }
}

impl Cx<'_> {
    pub(crate) fn bk_on(&self) -> bool {
        self.bk.on && self.inline_depth == 0
    }

    pub(crate) fn bk_ev(&mut self, e: Ev) {
        let b = self.b.live_block();
        // (in a match guard, the arm's bindings are seen by shared reference: moving one out, assigning to it or
        // borrowing it mutably is rustc's E0507 / E0594 / E0596 (`A(v) if { drop(v); false }`). Assigning and
        // borrowing mutably through a `&mut` too (`Some(ref mut y) if let Some(ref mut z) = *y`); not through a `&`,
        // which the borrow check reports)
        // (each enclosing guard's: an outer arm's bindings stay so in a guard nested in its guard)
        // (a closure's captures (guard_cap): only a mutable borrow, `|| { y += 1; }`; guard_closure_move and
        // guard_mut_use see the rest, by what the body does with them (`let &mut n = foo;` moves nothing))
        if let (false, Ev::Acc(p, a, sp), None) = (self.bk.guard_binds.is_empty(), &e, &self.bk.guard_err) {
            if self.bk.guard_binds.iter().any(|&(lo, hi)| (lo..hi).contains(&p.local)) && !p.proj.contains(&Proj::Shr) {
                let path = path_str(&self.bk.locals, p);
                let cap = self.bk.guard_cap;
                let msg = match a {
                    Acc::Move if !cap && p.owned_len() == p.proj.len() => Some(format!("cannot move out of `{path}` in pattern guard (E0507)")),
                    Acc::Write | Acc::Modify if !cap => Some(format!("cannot assign to `{path}`, as it is immutable for the pattern guard (E0594)")),
                    Acc::Borrow(ln) if self.bk.loans[*ln as usize].mutable => {
                        Some(format!("cannot borrow `{path}` as mutable, as it is immutable for the pattern guard (E0596)"))
                    }
                    _ => None,
                };
                if let Some(m) = msg {
                    self.bk.guard_err = Some((*sp, m));
                }
            }
        }
        // (in a returned position: the loans made here, and the locals read here (Ev::RetHold))
        if self.bk.in_ret > 0 {
            if let Ev::Acc(p, a, _) = &e {
                match a {
                    Acc::Borrow(ln) => {
                        add_sorted(&mut self.bk.ret_made, *ln);
                    }
                    Acc::Read | Acc::Move => {
                        let l = p.local;
                        self.bk.ev(b, e);
                        self.bk.ev(b, Ev::RetHold(l));
                        return;
                    }
                    _ => {}
                }
            }
        }
        self.bk.ev(b, e);
    }

    /// The returned positions of e (as rg_ret_at follows them): what is returned when e is
    pub(crate) fn bk_ret_leaves(&mut self, e: &Expr) {
        if !self.bk_on() {
            return;
        }
        match e {
            Expr::Paren(x) => self.bk_ret_leaves(x),
            Expr::Block(None, _, b) => {
                if let Some(crate::parse::Stmt::Expr(t, false)) = b.stmts.last() {
                    self.bk_ret_leaves(t);
                }
            }
            Expr::If(_, b, els) => {
                if let Some(crate::parse::Stmt::Expr(t, false)) = b.stmts.last() {
                    self.bk_ret_leaves(t);
                }
                if let Some(x) = els {
                    self.bk_ret_leaves(x);
                }
            }
            Expr::Match(_, arms) => {
                for a in arms {
                    self.bk_ret_leaves(&a.body);
                }
            }
            // (a loop's value comes from its `break`s, not its body, and the region check does not follow those)
            Expr::Loop(..) => {}
            // (a jump returns nothing here)
            Expr::While(..) | Expr::For(..) | Expr::Return(..) | Expr::Break(..) | Expr::Continue(..) => {}
            _ => {
                self.bk.ret_leaves.insert(e as *const Expr as usize);
            }
        }
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
        // (a closure whose parameters' regions are one for all its calls holds what its calls' arguments borrow)
        let held = !refs && matches!(ty, Ty::Struct(s) if { let b = self.prog.base_of(s); self.hr_lost.iter().any(|h| h.0 == b) });
        self.bk.locals.push(Local { name: name.to_string(), ty, copy, refs: refs || held, mutable, decl: self.rg.let_sp, via_ref: false });
        self.bk.by_loc.insert(loc, id);
        if refs && self.bk.level >= 2 && matches!(ty, Ty::Struct(_) | Ty::Box(_)) {
            let mut paths = vec![];
            self.drop_paths(ty, &mut vec![], &mut paths, 0);
            for p in paths {
                self.bk.drop_owners.push((id, p));
            }
        }
        self.bk_ev(Ev::Bind(id, !uninit));
        if refs {
            if let Some(srcs) = self.bk.bind_srcs.clone().filter(|s| !s.is_empty()) {
                self.bk_ev(Ev::Def(id, srcs));
            }
        }
    }

    /// q binds the whole scrutinee by value (pat_takes_whole), not a constant it is compared with (a lowercase
    /// `const` name, which a pattern of a program with `lower_consts` takes)
    pub(crate) fn takes_whole(&self, q: &Pat) -> bool {
        match q {
            Pat::Or(ps, _) => !ps.is_empty() && ps.iter().all(|p| self.takes_whole(p)),
            // (not a constant's name, nor a unit variant's in scope (`none` of `use G::*;`): the pattern compares with
            // it. A static's is a binding (no pattern names a static))
            Pat::Ident(_, _, n, sub) => {
                pat_takes_whole(q)
                    && !(sub.is_none()
                        && (self.prog.const_index.get(n.as_str()).is_some_and(|&ci| self.prog.consts[ci as usize].stat.is_none())
                            || super::unit_variant_ident(self.prog, q).is_some()))
            }
            _ => false,
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
        let st = if super::is_place_expr(e) { self.bk_place(e).and_then(|x| x.1) } else { None };
        let quiet = super::is_place_expr(e) && !pats.iter().any(|p| pat_reads_ty(self.prog, p, st));
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
        let mut own_loan = None;
        // (not a constant (`match Some(1) { Some(ref x) => .. }`), promoted to a static as `&Some(1)` is, unless
        // bound `ref mut`. A value that holds references is the temporary's (Def after the frame): a binding's `*x`
        // copies them out (`Some(ref s)` of an `Option<&String>`, `out.push(*s)`, bk_copy_out_of))
        let mut temp_holds = None;
        let scrut_t = match &r {
            Ok(Some(p)) => Some(p.ty()),
            _ => None,
        };
        if self.bk.level >= 2 && !in_let && !super::is_place_expr(e) && pats.iter().any(|p| pat_binds_ref(p)) && (pats.iter().any(|p| pat_binds_ref_mut(p)) || made_temp(self.prog, e)) {
            let id = self.bk.locals.len() as u32;
            let (ty, refs) = match scrut_t {
                Some(t) => (t, self.bk_ty(t).1),
                None => (Ty::Unit, false),
            };
            self.bk.locals.push(Local { name: TEMP.to_string(), ty, copy: true, refs, mutable: Some(true), decl: super::SP, via_ref: false });
            if refs {
                temp_holds = Some(id);
            }
            self.bk_ev(Ev::Bind(id, true));
            let sp = err_pos(e);
            let ln = self.bk.loans.len() as u32;
            let m = pats.iter().any(|p| pat_binds_ref_mut(p));
            self.bk.loans.push(Loan { place: BPlace { local: id, proj: vec![] }, mutable: m, sp, at: sp, quiet: true, capture: false });
            self.bk_ev(Ev::Acc(BPlace { local: id, proj: vec![] }, Acc::Borrow(ln), sp));
            self.bk.src(Src::Loan(ln));
            self.bk.stmt_temps.push(id);
            own_loan = Some(ln);
        }
        // (`let ref mut x = 5;` / `let ref x = f();`: a temporary the `let` extends, which goes away with the block's
        // locals (returning x is E0515). Not a constant one, as `let x = &5;` (`let ref x = 5;`))
        if self.bk.level >= 2 && in_let && !super::is_place_expr(e) && pats.iter().any(|p| pat_binds_ref(p)) {
            let m = pats.iter().any(|p| pat_binds_ref_mut(p));
            if m || made_temp(self.prog, e) {
                let id = self.bk.locals.len() as u32;
                self.bk.locals.push(Local { name: TEMP.to_string(), ty: Ty::Unit, copy: true, refs: false, mutable: Some(true), decl: super::SP, via_ref: false });
                self.bk_ev(Ev::Bind(id, true));
                let sp = err_pos(e);
                let ln = self.bk.loans.len() as u32;
                self.bk.loans.push(Loan { place: BPlace { local: id, proj: vec![] }, mutable: m, sp, at: sp, quiet: true, capture: false });
                self.bk_ev(Ev::Acc(BPlace { local: id, proj: vec![] }, Acc::Borrow(ln), sp));
                self.bk.src(Src::Loan(ln));
                self.bk.scope_temps.push((self.scopes.len().saturating_sub(1), id));
            }
        }
        let srcs = self.bk_frame_end();
        // (the temporary its `ref` bindings borrow holds what the value holds, but its own loan)
        if let Some(id) = temp_holds {
            let held: Vec<Src> = srcs.iter().copied().filter(|s| !matches!((s, own_loan), (Src::Loan(l), Some(o)) if *l == o)).collect();
            if !held.is_empty() {
                self.bk_ev(Ev::Def(id, held));
            }
        }
        // (a scrutinee value whose drop may use what it borrows (`match heap.peek_mut() { Some(_) => .. }`, a guard
        // of the prelude in it): a temporary of the statement, dropped at its end, so its loans live through the
        // arms; also when an arm moves a part out (`Some(t)`), as the drop is then of what may be left. Not when a
        // pattern takes the whole value (`x`, `x @ ..`))
        // (when only some of the arms take the whole value (`x if flag => drop(x), _ => ..`), those arms move it: the
        // match ends its drop there (Bk::scrut_drop), the others keep it)
        self.bk.scrut_drop = None;
        if self.bk.level >= 2 && !in_let && self.user_code && !super::is_place_expr(e) && !srcs.is_empty() && !pats.iter().all(|q| self.takes_whole(q)) {
            if let Ok(Some(p)) = &r {
                let t = p.ty();
                if self.dropck(t, 0) {
                    let id = self.bk.locals.len() as u32;
                    self.bk.locals.push(Local { name: TEMP.to_string(), ty: t, copy: false, refs: true, mutable: Some(true), decl: super::SP, via_ref: false });
                    self.bk_ev(Ev::Bind(id, true));
                    // (not the loan of the scrutinee's own storage its `ref` bindings take: its drop does not use
                    // that (`(Struct(1), 1)` matched by `(l, ref s) if g`))
                    let held: Vec<Src> = srcs.iter().copied().filter(|s| !matches!((s, own_loan), (Src::Loan(l), Some(o)) if *l == o)).collect();
                    self.bk_ev(Ev::Def(id, held));
                    self.bk.stmt_temps.push(id);
                    self.bk.drop_temps.push(id);
                    if pats.iter().any(|q| self.takes_whole(q)) {
                        self.bk.scrut_drop = Some(id);
                    }
                }
            }
        }
        self.bk.scrut = srcs;
        // (a pattern that writes `'static` (`Foo::Bar::<'static> { .. }`): checked from here on)
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && pats.iter().any(|p| pat_any(p, &|q| matches!(q, Pat::TupleStruct(pp, _) | Pat::Struct(pp, _, _) if crate::regions::path_names_lt(pp)))) {
            self.rg_late_on();
        }
        self.rg.scrut = if self.rg_on() { self.rt_expr(e) } else { None };
        self.bk.scrut_place = self.bk_place(e).map(|p| (p.0, err_pos(e)));
        // (a temporary matched by a `&` pattern that binds by value (`match (l1, l2) { (_, &[hd, ..]) => .. }`): a
        // place of its own, so that what the binding moves out through the reference is an error at the scrutinee,
        // as it is of a local (E0507 / E0508). Not with `ref` bindings of the temporary itself, whose loans are made
        // above; one under a `&` pattern borrows what a reference in it points to)
        let by_value_deref = |p: &Pat| pat_any(p, &|q| matches!(q, Pat::Ref(_, inner) if pat_any(inner, &|r| matches!(r, Pat::Ident(false, ..)))));
        if self.bk.scrut_place.is_none() && self.bk.level >= 1 && pats.iter().any(|p| by_value_deref(p)) && !pats.iter().any(|p| pat_binds_ref_outside_ref(p)) {
            if let Ok(Some(pl)) = &r {
                let id = self.bk.locals.len() as u32;
                self.bk.locals.push(Local { name: TEMP.to_string(), ty: pl.ty(), copy: false, refs: false, mutable: Some(true), decl: super::SP, via_ref: false });
                self.bk_ev(Ev::Bind(id, true));
                self.bk.stmt_temps.push(id);
                self.bk.scrut_place = Some((BPlace { local: id, proj: vec![] }, err_pos(e)));
            }
        }
        if let (Ok(Some(p)), true) = (&r, self.bk.level >= 1) {
            let t = p.ty();
            // (`match d.f { Some(x) => .. }` with d's type implementing `Drop`: what the pattern moves is d's)
            let owner = self.drop_owner(e);
            for pat in pats {
                self.drop_pat_check(pat, t, err_pos(e), owner.as_deref())?;
            }
        }
        r
    }

    /// A binding by value moves the part of the scrutinee it binds (if it is not Copy)
    pub(crate) fn bk_pat_move(&mut self, t: Ty, name: &str) {
        if !self.bk_on() {
            return;
        }
        // (a unit struct pattern `S` binds nothing: no move (`let Unit = *w;` through a Deref))
        if let Ty::Struct(sid) = t {
            if self.prog.structs[self.prog.base_of(sid) as usize].name == name {
                return;
            }
        }
        if !self.bk_ty(t).0 || matches!(t, Ty::Ref(true, _)) {
            self.bk.pat_moves += 1;
        }
        let Some(p) = self.bk.pat_path.clone() else { return };
        // (a binding of the whole scrutinee (`match c { c2 => .. }`) is not tracked (rustc accepts assigning to c's
        // fields in the arm))
        if self.bk.scrut_place.as_ref().is_some_and(|s| s.0 == p) {
            return;
        }
        // (a `&mut` bound by value is moved out too: a pattern does not reborrow it (`if let Some(mut x) = opt` in a
        // loop, with opt: `Option<&mut T>`, is E0382 the second time))
        if !self.bk_ty(t).0 || matches!(t, Ty::Ref(true, _)) {
            // (a move out of what the scrutinee owns is at the arm, where its binding is, as rustc reports a use after
            // it: `[.., _y] => {}` after `[_, _, _x]` moved `a[2]`. One through a reference, which cannot be moved
            // out of, and one outside a match arm are at the scrutinee, as rustc reports E0507 / E0508)
            let at_arm = self.bk.arm_at.filter(|_| p.owned_len() == p.proj.len());
            let sp = at_arm.or(self.bk.scrut_place.as_ref().map(|x| x.1)).unwrap_or(super::SP);
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
        let sp = self.bk.arm_at.or(self.bk.scrut_place.as_ref().map(|x| x.1)).unwrap_or(super::SP);
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
        // (a number, a bool, a char: Copy, no reference, not worth a lookup)
        if t.is_int() || matches!(t, Ty::Bool | Ty::Char | Ty::F32 | Ty::F64 | Ty::Unit) {
            return (true, false);
        }
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
                if !self.bk.dropck_locals.contains(&id) {
                    self.bk.dropck_locals.push(id);
                }
            }
            self.bk_ev(Ev::Dead(id, sp));
        }
        // (the temporaries its `let`s extended: all of this scope's, also below a holder of an enclosing scope's
        // pushed later (a block's value, bk_block_value: `let r = { let ref x = make(); x };`))
        let depth = self.scopes.len().saturating_sub(1);
        if self.bk.scope_temps.iter().any(|x| x.0 >= depth) {
            let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.bk.scope_temps).into_iter().partition(|x| x.0 >= depth);
            self.bk.scope_temps = kept;
            for (_, id) in gone.into_iter().rev() {
                self.bk_ev(Ev::Dead(id, sp));
            }
        }
    }

    /// A jump out of the scopes from `depth` on (`break` / `continue` out of a loop's body): their locals go out of
    /// scope there, innermost first, as at their ends (bk_dead_scope). The scopes stay: the code after the jump is
    /// not reached
    pub(crate) fn bk_dead_exit(&mut self, depth: usize, sp: Sp) {
        if !self.bk_on() || depth >= self.scopes.len() {
            return;
        }
        let ids: Vec<(u32, Ty)> = self.scopes[depth..].iter().rev().flat_map(|s| s.iter().rev()).filter_map(|(_, l, t)| self.bk.by_loc.get(l).map(|&id| (id, *t))).collect();
        for (id, t) in ids {
            if self.bk.locals[id as usize].refs && self.bk.level >= 2 && self.dropck(t, 0) {
                self.bk_ev(Ev::Acc(BPlace { local: id, proj: vec![] }, Acc::Drop, sp));
                if !self.bk.dropck_locals.contains(&id) {
                    self.bk.dropck_locals.push(id);
                }
            }
            self.bk_ev(Ev::Dead(id, sp));
        }
        let temps: Vec<u32> = self.bk.scope_temps.iter().rev().filter(|x| x.0 >= depth).map(|x| x.1).collect();
        for id in temps {
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
                // (a guard of the prelude (`c.borrow()`'s `Ref`, a `MutexGuard`): its `Drop` releases what it borrows, so
                // that must still be there, as std's guards (no `#[may_dangle]`))
                if prelude && matches!(prog.structs[base as usize].name.as_str(), "Ref" | "RefMut" | "PeekMut" | "MutexGuard" | "RwLockReadGuard" | "RwLockWriteGuard") {
                    return true;
                }
                if prelude || prog.closure_of_struct.contains_key(&base) {
                    return sd.args.iter().any(|&a| self.dropck(a, depth + 1));
                }
                // (a `Drop` of a type that holds no reference sees no loan: `Token` in `(Cell<Option<&i32>>, Token)`,
                // whose drop needs nothing the cell borrows alive, as rustc's drop check requires only its parameters)
                (prog.impls.contains(&(base, "Drop".to_string())) && has_refs(prog, t, 0))
                    || sd.fields.iter().any(|f| self.dropck(f.1, depth + 1))
                    || sd.variants.iter().any(|v| v.fields.iter().any(|f| self.dropck(f.1, depth + 1)))
            }
            Ty::Box(i) | Ty::Array(i, _) => self.dropck(*i, depth + 1),
            // (a trait object's drop is not known: it may use all it borrows (`Box<dyn FnMut()>` of a closure that
            // borrows v mutably, used when it is dropped))
            Ty::Dyn(_) => true,
            _ => false,
        }
    }

    /// `*x` through an overloaded `Deref` with no `DerefMut` (`*rc`): what it reaches is shared, as through a `&`
    /// (writing it or borrowing it mutably is E0594 / E0596). The deref's target type; None for another struct
    fn bk_overloaded_shr(&self, p: &mut BPlace, sid: u32) -> Option<Ty> {
        let name = self.shared_deref(sid)?;
        let did = self.prog.method(sid, "deref")?;
        let Ty::Ref(_, i) = self.prog.sigs[did as usize].ret else { return None };
        if i.is_fat() || matches!(i, Ty::Str | Ty::Slice(_) | Ty::Dyn(_)) {
            return None;
        }
        let args: Vec<Option<Ty>> = self.prog.structs[sid as usize].args.iter().map(|&a| Some(a)).collect();
        let inner = self.prog.partial_subst(*i, &args);
        self.bk.overloaded.borrow_mut().insert((p.local, p.proj.len()), name);
        p.proj.push(Proj::Shr);
        Some(inner)
    }

    /// The name of a type whose `Deref` has no `DerefMut` (`Rc`, `Arc`, a `Ref` guard, the program's): what it reaches
    /// is shared. Not a Vec / String (their own), nor the prelude's `MutexGuard`, which writes through its `deref`
    pub(crate) fn shared_deref(&self, sid: u32) -> Option<String> {
        if self.vec_fields(sid).is_some() || self.prog.method(sid, "deref_mut").is_some() || self.prog.method(sid, "deref").is_none() {
            return None;
        }
        let base = self.prog.base_of(sid);
        let n = &self.prog.structs[base as usize].name;
        let prelude = self.prog.struct_item.get(base as usize).is_none_or(|&i| i >= self.prog.prelude_from);
        if n == "MutexGuard" && prelude {
            return None;
        }
        Some(n.clone())
    }

    /// The place e names, rooted at a local (None for temporaries, statics, captures ..)
    pub(crate) fn bk_place(&self, e: &Expr) -> Option<(BPlace, Option<Ty>)> {
        match e {
            Expr::Paren(x) => self.bk_place(x),
            Expr::Path(_, p) => {
                let id = ident_of(p)?;
                let (loc, t) = self.lookup(id)?;
                let l = *self.bk.by_loc.get(&loc)?;
                let proj = if self.bk.locals[l as usize].via_ref { vec![Proj::Deref(false)] } else { vec![] };
                Some((BPlace { local: l, proj }, Some(t)))
            }
            Expr::Field(b, name, _) => {
                let (mut p, t) = self.bk_place(b)?;
                let mut t = t?;
                // (overloaded derefs followed: a `Deref` whose target is the type itself (`Target = S`) never reaches
                // a field, and is not followed past a few (lowering reports the recursion))
                let mut n = 0;
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
                        // (a field of what an overloaded `Deref` reaches: `rc.y` with `rc: Rc<Point>`)
                        Ty::Struct(sid) if !self.prog.structs[sid as usize].is_enum && self.prog.field(sid, name).is_none() => {
                            // (as far as rustc's recursion limit, past which it rejects the program (E0055): a chain
                            // that long is followed, and a cycle (`type Target = Self`) ends)
                            n += 1;
                            if n > 128 {
                                return None;
                            }
                            t = self.bk_overloaded_shr(&mut p, sid)?;
                            if self.prog.has_param(t) {
                                return None;
                            }
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
                // (the fields of a union are the same memory: one is marked, and two different ones overlap whatever
                // follows them (`u.s.a` and `u.z.c`, see prefix))
                if self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name) {
                    p.proj.push(Proj::Field(format!("{UNION_FIELD}{name}")));
                } else {
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
                    // (an overloaded `Deref` with no `DerefMut` (`*rc`): what it reaches is shared, as through a `&`.
                    // Writing or borrowing it mutably is E0594 / E0596)
                    Ty::Struct(sid) => {
                        let inner = self.bk_overloaded_shr(&mut p, sid)?;
                        Some((p, Some(inner).filter(|&t| !self.prog.has_param(t))))
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
                    // (the prelude's Vec, whose `Index` gives an element: `c.v[0].cell` is a place in c)
                    Ty::Struct(sid) if self.prelude_vec(sid) => self.prog.structs[sid as usize].args.first().copied(),
                    _ => None,
                };
                Some((p, et))
            }
            _ => None,
        }
    }

    /// sid is the prelude's Vec
    fn prelude_vec(&self, sid: u32) -> bool {
        let b = self.prog.base_of(sid);
        self.prog.structs[b as usize].name == "Vec" && self.prog.struct_item.get(b as usize).is_some_and(|&i| i >= self.prog.prelude_from)
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

    /// A copy of e, of type t: what it borrows is 'static when t's `Copy` impl is for its 'static instance (see
    /// static_copy). Only for a place the local owns, of a local whose references are all in such types
    pub(crate) fn bk_copy(&mut self, e: &Expr, t: Ty) {
        let Some((p, _)) = self.bk_place(e) else { return };
        let lo = &self.bk.locals[p.local as usize];
        if p.owned_len() < p.proj.len() || !lo.refs || static_copy(self.prog, t, 0) != 1 || static_copy(self.prog, lo.ty, 0) != 1 {
            return;
        }
        self.bk_ev(Ev::Escape(vec![Src::Local(p.local)]));
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
        // (`&**s` with `s: &&T`: a reborrow through the shared reference s points to, which may be copied out first:
        // what that holds, not the loan s is (bk_copy_out_of))
        let through_shared = match (self.bk.locals[l as usize].ty, p.proj.first(), p.proj.get(1)) {
            (Ty::Ref(_, u @ Ty::Ref(false, _)), Some(Proj::Deref(_) | Proj::Shr), Some(Proj::Deref(_) | Proj::Shr)) if self.bk.level >= 2 => Some(*u),
            _ => None,
        };
        self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
        if !quiet {
            self.bk.src(Src::Loan(ln));
        }
        // (reborrowing through a reference also carries the loans the reference holds)
        if self.bk.locals[l as usize].refs {
            match through_shared {
                Some(u) => {
                    let c = self.bk_copy_out_of(l, u);
                    self.bk.src(Src::Local(c));
                }
                None => self.bk.src(Src::Local(l)),
            }
        }
    }

    /// A `&mut` local passed where a `&mut` is expected is reborrowed (`f(x)` is `f(&mut *x)`; where a `&` is, m false,
    /// `f(&*x)`): the loan of `*x`, which lives as long as the region check finds it must (Rg::forever, Rg::holders), by
    /// the argument's address
    pub(crate) fn bk_reborrow_arg(&mut self, a: &Expr, m: bool, two_phase: bool) -> Option<u32> {
        // (a field of a local too (`v.push(c.raw)`): the reborrow is of `*c.raw`, which a `Drop` c must not outlive
        // (E0713))
        let mut root = strip_paren(a);
        while let Expr::Field(x, _, _) = root {
            root = strip_paren(x);
        }
        if !self.bk_on() || !matches!(root, Expr::Path(..)) {
            return None;
        }
        let Some((mut p, Some(Ty::Ref(true, _)))) = self.bk_place(a) else { return None };
        // (through a field of a value whose type has a `Drop` impl: only the region check knows how long the loan
        // lives, so it is on from here (and the function is checked again with it from the start))
        if !self.rg.on && !p.proj.is_empty() && self.inline_depth == 0 && self.bk.drop_owners.iter().any(|d| d.0 == p.local) {
            self.rg_late_on();
        }
        p.proj.push(ref_deref(true));
        let sp = err_pos(a);
        let ln = self.bk.loans.len() as u32;
        // (without the region check: a borrow of `*x` at the call that nothing holds after it. A loan of `*x` still
        // live there conflicts (`let v = &*x; take(x); use(v)` is E0502))
        // (a call's (two_phase): held until the call is made, as an argument `&x` is. A `&mut` one is reserved
        // until then, a shared loan, and taken at the call (Bk::arg_acts))
        if !self.rg.on {
            if two_phase && m {
                self.bk.loans.push(Loan { place: p.clone(), mutable: false, sp, at: sp, quiet: false, capture: false });
                self.bk.reserved.push(ln);
                self.bk_ev(Ev::Acc(p.clone(), Acc::Borrow(ln), sp));
                self.bk_call_temp(&[Src::Loan(ln)], true);
                let tid = *self.bk.call_temps.last()?;
                let real = self.bk.loans.len() as u32;
                self.bk.loans.push(Loan { place: p.clone(), mutable: true, sp, at: sp, quiet: false, capture: false });
                self.bk.arg_acts.push((Ev::Acc(p, Acc::Borrow(real), sp), tid));
                return Some(real);
            }
            self.bk.loans.push(Loan { place: p.clone(), mutable: m, sp, at: sp, quiet: false, capture: false });
            self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
            if two_phase {
                self.bk_call_temp(&[Src::Loan(ln)], true);
            }
            return Some(ln);
        }
        self.bk.loan_of.insert(a as *const Expr as usize, ln);
        self.bk.loans.push(Loan { place: p.clone(), mutable: m, sp, at: sp, quiet: false, capture: false });
        if m {
            self.bk.implicit.push(ln);
        }
        self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
        Some(ln)
    }

    /// A hidden local that holds a `U` copied out of what the local x points to (`*r` with `r: &U`): what x holds, with
    /// each loan of a place that holds U values replaced by what that place's local holds (copy_masks). `let r = &a;
    /// let t = *r;` holds what a holds, not a's loan
    pub(crate) fn bk_copy_out_of(&mut self, x: u32, u: Ty) -> u32 {
        let id = self.bk.locals.len() as u32;
        self.bk.locals.push(Local { name: "<copy>".into(), ty: u, copy: true, refs: true, mutable: None, decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, vec![Src::Local(x)]));
        self.bk.copy_out.push((id, u));
        id
    }

    /// `*x` of a value x: `&U` with U a `Copy` type holding references, copied out (srcs: what x holds). A loan of a
    /// place whose type holds U values (a container of the U x points to, or the U itself) is what x is, not what U
    /// holds: what that place holds stands for it. A loan of anything else (`&s` in `*id(&&s)`) is what U holds
    pub(crate) fn bk_copy_out(&mut self, srcs: &[Src], u: Ty) {
        for &s in srcs {
            let s = match s {
                Src::Loan(ln) => {
                    let p = &self.bk.loans[ln as usize].place;
                    let pt = self.bk.locals.get(p.local as usize).map(|l| l.ty);
                    match pt.and_then(|t| self.place_ty(t, &p.proj)) {
                        Some(t) if self.ty_holds(t, u, 0) => Src::Local(p.local),
                        _ => s,
                    }
                }
                s => s,
            };
            self.bk.src(s);
        }
    }

    /// The type of the place of local type t through proj (None where it is not followed: an index of a slice's type
    /// is its element's)
    fn place_ty(&self, t: Ty, proj: &[Proj]) -> Option<Ty> {
        let mut t = t;
        for x in proj {
            t = match (x, t) {
                (Proj::Deref(_) | Proj::Shr, Ty::Ref(_, i) | Ty::Box(i) | Ty::Ptr(_, i)) => *i,
                (Proj::Index, Ty::Array(i, _) | Ty::Slice(i)) => *i,
                // (an element of the prelude's `Vec` / `VecDeque`, which index to their element)
                (Proj::Index, Ty::Struct(sid)) if matches!(self.prog.structs[self.prog.base_of(sid) as usize].name.as_str(), "Vec" | "VecDeque") => *self.prog.structs[sid as usize].args.first()?,
                (Proj::Field(f), Ty::Struct(sid)) => self.prog.field(sid, f)?.0,
                _ => return None,
            };
        }
        Some(t)
    }

    /// t is u or holds u values (a struct's type arguments, an array's or a slice's elements, what a `Box` holds)
    fn ty_holds(&self, t: Ty, u: Ty, depth: u32) -> bool {
        if t == u {
            return true;
        }
        if depth > 6 {
            return false;
        }
        match t {
            Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => self.ty_holds(*i, u, depth + 1),
            Ty::Struct(sid) => self.prog.structs[sid as usize].args.iter().any(|&a| self.ty_holds(a, u, depth + 1)),
            _ => false,
        }
    }

    /// `f().0`, `x.borrow().1`, `v.clone()[0]`: a place in a temporary a call makes, through fields, indexes and derefs
    /// of what the temporary owns (a `Box`, a guard's or an `Rc`'s `Deref`, which borrow it), not through a reference
    /// it holds (`v.iter().next().unwrap().0` is a place of `v`'s): the temporary's expression
    pub(crate) fn temp_root<'e>(&self, e: &'e Expr) -> Option<&'e Expr> {
        let mut x = strip_paren(e);
        loop {
            let b = match x {
                Expr::Field(b, ..) | Expr::Index(b, ..) => &**b,
                Expr::Unary(op, b, _) if op == "*" => &**b,
                _ => return None,
            };
            if matches!(self.peek(b), Some(Ty::Ref(..) | Ty::Ptr(..)) | None) {
                return None;
            }
            let b = strip_paren(b);
            if made_temp(self.prog, b) {
                return Some(b);
            }
            x = b;
        }
    }

    /// The type of a field `b.f` as declared, through the `Box`es and `Deref`s of what b owns (not of a reference): the
    /// type before a coercion (peek gives what the field is coerced to). A prelude type's own fields are not seen from
    /// the program (`x.borrow().0` is of what the `Ref` derefs to)
    pub(crate) fn owned_field_ty(&self, e: &Expr) -> Option<Ty> {
        let Expr::Field(b, name, _) = strip_paren(e) else { return None };
        let mut t = self.peek(b)?;
        for _ in 0..4 {
            match t {
                Ty::Box(i) => t = *i,
                Ty::Struct(sid) => {
                    let base = self.prog.base_of(sid);
                    let prelude = self.prog.struct_item.get(base as usize).is_none_or(|&i| i >= self.prog.prelude_from);
                    let deref = self.prog.method(sid, "deref");
                    if !(prelude && deref.is_some()) {
                        if let Some((ft, _)) = self.prog.field(sid, name) {
                            return Some(ft);
                        }
                    }
                    // (the impl's generic `Target`: of the struct's arguments)
                    match deref.map(|d| self.prog.sigs[d as usize].ret) {
                        Some(Ty::Ref(_, i)) => t = self.prog.subst(*i, &self.prog.structs[sid as usize].args),
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
        None
    }

    /// `let b: &mut T = a;` (want `&T` too) with a place a of a `&mut`: the loan of `*a` b holds
    pub(crate) fn bk_reborrow_let(&mut self, a: &Expr, want: Ty) {
        let Ty::Ref(m, want_in) = want else { return };
        if !self.bk_on() {
            return;
        }
        let Some((mut p, Some(Ty::Ref(true, inner)))) = self.bk_place(a) else { return };
        p.proj.push(ref_deref(true));
        // (a deref coercion through what a points to (`let y: &i32 = x;` with `x: &mut &i32`, or a `Box`): the loan is
        // of what that points to (`**x`), which overwriting `*x` does not touch, as in rustc)
        // (not toward a trait object: `&mut &i32` as a `&dyn Debug` unsizes the `&i32` itself, which is borrowed)
        let mut t = *inner;
        for _ in 0..4 {
            if t == *want_in || matches!(want_in, Ty::Dyn(_)) {
                break;
            }
            match t {
                Ty::Ref(rm, i) => {
                    p.proj.push(ref_deref(rm));
                    t = *i;
                }
                Ty::Box(i) => {
                    p.proj.push(Proj::Deref(true));
                    t = *i;
                }
                _ => break,
            }
        }
        let sp = err_pos(a);
        let ln = self.bk.loans.len() as u32;
        self.bk.loans.push(Loan { place: p.clone(), mutable: m, sp, at: sp, quiet: false, capture: false });
        self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
        self.bk.src(Src::Loan(ln));
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
        self.bk.locals.push(Local { name: TEMP.to_string(), ty: t, copy, refs, mutable: Some(true), decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        // (a block, which has no position: its first statement's, usually on the line of its `{`)
        let sp = match err_pos(e) {
            s if s.line != 0 => s,
            s => match strip_paren(e) {
                Expr::Block(_, _, b) => match b.stmts.first() {
                    Some(crate::parse::Stmt::Let(.., p)) => *p,
                    Some(crate::parse::Stmt::Expr(x, _)) => err_pos(x),
                    _ => s,
                },
                _ => s,
            },
        };
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

    /// `let _ = x;`: not a read of x (it may be moved out or borrowed), but rustc's fake read keeps x live there, so
    /// what x borrows must still be there (`let z = gimme(&{ .. }); let _ = z;` is E0716)
    pub(crate) fn bk_fake_read(&mut self, e: &Expr) {
        if !self.bk_on() {
            return;
        }
        let Some((p, _)) = self.bk_place(e) else { return };
        if self.bk.locals[p.local as usize].refs {
            self.bk_ev(Ev::Acc(BPlace { local: p.local, proj: vec![] }, Acc::Drop, err_pos(e)));
        }
    }

    /// The statements from `mark` on are over: their temporaries go out of scope
    pub(crate) fn bk_direct_pub(&self, e: &Expr, out: &mut Vec<Src>) {
        self.bk_direct(e, out)
    }

    /// A value that holds srcs is used here (a hidden local holds it and is read): a loan of one of them gone away
    /// by now is an error
    pub(crate) fn bk_hold_after(&mut self, srcs: Vec<Src>, sp: Sp) {
        let id = self.bk.locals.len() as u32;
        self.bk.locals.push(Local { name: TEMP.to_string(), ty: Ty::Unit, copy: true, refs: true, mutable: None, decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, srcs.clone()));
        self.bk_ev(Ev::Keep(id, srcs));
        self.bk_ev(Ev::Acc(BPlace { local: id, proj: vec![] }, Acc::Read, sp));
        self.bk_ev(Ev::Dead(id, super::SP));
    }

    /// A block's value of type t that a statement discards, which holds what srcs borrow: a hidden local, dropped at
    /// the statement's end (bk_drop_held)
    pub(crate) fn bk_hold_value(&mut self, srcs: Vec<Src>, t: Ty) {
        let id = self.bk.locals.len() as u32;
        self.bk.locals.push(Local { name: TEMP.to_string(), ty: t, copy: false, refs: true, mutable: Some(true), decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, srcs));
        self.bk.discard_temps.push(id);
    }

    /// The tail e of the block blk, whose value (of type t) may hold references: what it holds directly through the
    /// block's locals is kept in a hidden local, which lives with the enclosing block's locals and stands for the
    /// block's value after it (bk_direct): `let r = { let x = 1; let y = &x; y };` used after is E0597
    pub(crate) fn bk_block_value(&mut self, blk: &crate::parse::Block, e: &Expr, t: Ty) {
        if !self.bk_on() || self.bk.level < 2 || !self.bk_ty(t).1 {
            return;
        }
        let mut d = vec![];
        self.bk_direct(e, &mut d);
        if d.is_empty() {
            return;
        }
        let id = self.bk.locals.len() as u32;
        let (copy, refs) = self.bk_ty(t);
        self.bk.locals.push(Local { name: TEMP.to_string(), ty: t, copy, refs, mutable: Some(true), decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, d));
        self.bk.block_val.insert(blk as *const crate::parse::Block as usize, id);
        self.bk.scope_temps.push((self.scopes.len().saturating_sub(2), id));
    }

    /// A temporary of type t that holds what srcs borrow and whose drop may use it: a hidden local of the statement,
    /// dropped at its end (bk_temps_end), so the loans live until then. Returns the local (a call that takes the
    /// temporary by value ends what it holds there)
    pub(crate) fn bk_drop_temp(&mut self, t: Ty, srcs: Vec<Src>) -> u32 {
        let id = self.bk.locals.len() as u32;
        self.bk.locals.push(Local { name: TEMP.to_string(), ty: t, copy: false, refs: true, mutable: Some(true), decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, srcs));
        self.bk.stmt_temps.push(id);
        self.bk.drop_temps.push(id);
        id
    }

    pub(crate) fn bk_drop_held(&mut self, ts: &[u32], sp: Sp) {
        for &id in ts {
            self.bk_ev(Ev::Acc(BPlace { local: id, proj: vec![] }, Acc::Drop, sp));
            self.bk_ev(Ev::Dead(id, super::SP));
        }
    }

    pub(crate) fn bk_temps_end(&mut self, mark: usize) {
        if self.bk.stmt_temps.len() <= mark {
            return;
        }
        let ts: Vec<u32> = self.bk.stmt_temps.drain(mark..).collect();
        for t in ts {
            if self.bk.drop_temps.contains(&t) {
                self.bk_ev(Ev::Acc(BPlace { local: t, proj: vec![] }, Acc::Drop, super::SP));
            }
            self.bk_ev(Ev::Dead(t, super::SP));
        }
    }

    /// Before a `for` loop's pattern is bound to an item: when it binds by `ref` (`for ref x in 0..3`), the item is a
    /// temporary of the iteration its `ref` bindings borrow (E0716 when such a borrow is used after the iteration).
    /// Returns what to restore (bk_for_item_end)
    pub(crate) fn bk_for_item_begin(&mut self, f: &Expr, t: Ty) -> Option<(Option<u32>, Option<Vec<Src>>, Option<BPlace>, Option<(BPlace, Sp)>)> {
        let Expr::For(_, fpat, it, _) = f else { return None };
        if !self.bk_on() || self.bk.level < 2 || !pat_binds_ref(fpat) {
            return None;
        }
        // (an item that holds references holds what the loop's items do (bk_for_item_srcs): a binding's `*x` copies
        // that out (bk_copy_out_of))
        let refs = self.bk_ty(t).1;
        let id = self.bk.locals.len() as u32;
        self.bk.locals.push(Local { name: TEMP.to_string(), ty: t, copy: true, refs, mutable: Some(true), decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        if let (true, Some(srcs)) = (refs, self.bk.bind_srcs.clone().filter(|s| !s.is_empty())) {
            self.bk_ev(Ev::Def(id, srcs));
        }
        let place = BPlace { local: id, proj: vec![] };
        // (an array literal has no position of its own: its first element's)
        let at = |e: &Expr| Some(err_pos(e)).filter(|s| s.line != 0).unwrap_or_else(|| super::expr_start(e));
        let sp = match (at(it), strip_paren(it)) {
            (s, Expr::Array(xs)) if s.line == 0 && !xs.is_empty() => at(&xs[0]),
            (s, _) => s,
        };
        let saved = (Some(id), self.bk.bind_srcs.clone(), self.bk.pat_path.replace(place.clone()), self.bk.scrut_place.replace((place, sp)));
        Some(saved)
    }

    /// Before a `for` loop's pattern is bound to an item of type t: when t holds references, its bindings hold what the
    /// loop's items do (Bk::for_items). Returns the bind_srcs to restore
    pub(crate) fn bk_for_item_srcs(&mut self, t: Ty) -> Option<Vec<Src>> {
        let saved = self.bk.bind_srcs.clone();
        if self.bk_on() && self.bk_ty(t).1 {
            if let Some(it) = self.bk.for_items.last().filter(|it| !it.is_empty()).cloned() {
                self.bk.bind_srcs = Some(it);
            }
        }
        saved
    }

    /// After the pattern is bound (bk_for_item_begin): the item's temporary, to end with the iteration
    pub(crate) fn bk_for_item_end(&mut self, saved: Option<(Option<u32>, Option<Vec<Src>>, Option<BPlace>, Option<(BPlace, Sp)>)>) -> Option<u32> {
        let (id, srcs, path, scrut) = saved?;
        self.bk.bind_srcs = srcs;
        self.bk.pat_path = path;
        self.bk.scrut_place = scrut;
        // (gone with the body's scope: at the end of the iteration, and on a `break` / `continue` out of it
        // (bk_dead_scope, bk_dead_exit))
        if let Some(id) = id {
            self.bk.scope_temps.push((self.scopes.len().saturating_sub(1), id));
        }
        id
    }

    /// The end of a loop's iteration: the temporaries of the statement made since mark (a `while let` scrutinee's that
    /// its `ref` bindings borrow, the body's tail's) go away, as a loop's body is a terminating scope (E0716 for
    /// `while let Some(ref x) = it.next() { v.push(x); }` when v is used after). They are left out of what ends at
    /// the enclosing statement
    pub(crate) fn bk_iteration_end(&mut self, mark: usize) {
        self.bk_temps_end(mark);
    }

    /// The temporaries of a block's tail that a method is called on (`D(&_t1).end()`) live until the end of the
    /// enclosing statement, after the block's locals (edition 2021). One whose `Drop` may see what it borrows uses
    /// its loans when it is dropped there: a loan of one of the block's locals is then E0597 at the local
    /// Before a block's tail is lowered: its method calls' receivers that are temporaries, whose values Cx::expr
    /// collects (Bk::watch). Returns them, for bk_unwatch
    pub(crate) fn bk_watch_tail(&mut self, e: &Expr) -> (Vec<usize>, usize) {
        let mut added = vec![];
        // (the position watches of the enclosing tails stay: a block in an argument of a watched call has its own)
        let mark = self.bk.watch_at.len();
        if !self.bk_on() || self.bk.level < 2 || !self.user_code {
            return (added, mark);
        }
        let mut temps = vec![];
        tail_temps(e, &mut temps, 0);
        for (recv, r) in temps {
            for a in [recv as *const Expr as usize, r as *const Expr as usize] {
                if self.bk.watch.insert(a) {
                    added.push(a);
                }
            }
            if matches!(r, Expr::MethodCall(..)) && err_pos(r).line != 0 {
                self.bk.watch_at.push((err_pos(r), vec![]));
            }
        }
        (added, mark)
    }

    pub(crate) fn bk_unwatch(&mut self, (added, mark): &(Vec<usize>, usize)) {
        for a in added {
            self.bk.watch.remove(a);
            self.bk.watched.remove(a);
        }
        self.bk.watch_at.truncate(*mark);
    }

    pub(crate) fn bk_tail_drop_temps(&mut self, e: &Expr) {
        if !self.bk_on() || self.bk.level < 2 || !self.user_code {
            return;
        }
        let mut temps = vec![];
        tail_temps(e, &mut temps, 0);
        for (recv, r) in temps {
            let Some(t) = self.peek(r) else { continue };
            if !self.dropck(t, 0) {
                continue;
            }
            // (what its value holds, as lowering it collected (`id(rows)`, `rows.wrap()`: what rows held), else as the
            // syntax tells)
            let mut srcs = vec![];
            self.bk_direct(r, &mut srcs);
            for a in [recv as *const Expr as usize, r as *const Expr as usize] {
                if let Some(s) = self.bk.watched.get(&a) {
                    for x in s {
                        if !srcs.contains(x) {
                            srcs.push(*x);
                        }
                    }
                }
            }
            let at = err_pos(r);
            if let Some((_, s)) = self.bk.watch_at.iter().find(|w| w.0 == at && at.line != 0) {
                for x in s {
                    if !srcs.contains(x) {
                        srcs.push(*x);
                    }
                }
            }
            if !srcs.is_empty() {
                let id = self.bk.locals.len() as u32;
                self.bk.locals.push(Local { name: TEMP.to_string(), ty: t, copy: false, refs: true, mutable: Some(true), decl: super::SP, via_ref: false });
                self.bk_ev(Ev::Bind(id, true));
                self.bk_ev(Ev::Def(id, srcs));
                self.bk.drop_temps.push(id);
                self.bk.stmt_temps.push(id);
            }
        }
    }

    /// The places in a value of type t of the values whose type implements `Drop` (a user's), through boxes and the
    /// fields of the program's structs and tuples (not into such a value: its own drop covers its fields)
    fn drop_paths(&self, t: Ty, at: &mut Vec<Proj>, out: &mut Vec<Vec<Proj>>, depth: u32) {
        if depth > 4 || out.len() >= 8 {
            return;
        }
        match t {
            Ty::Box(i) => {
                at.push(Proj::Deref(true));
                self.drop_paths(*i, at, out, depth + 1);
                at.pop();
            }
            Ty::Struct(sid) if self.user_drop_name(sid).is_some() => out.push(at.clone()),
            Ty::Struct(sid) => {
                let base = self.prog.base_of(sid);
                let user = self.prog.struct_item.get(base as usize).is_some_and(|&i| i < self.prog.prelude_from);
                let sd = &self.prog.structs[sid as usize];
                if !(user || self.prog.is_tuple(sid)) || !sd.variants.is_empty() || self.prog.closure_of_struct.contains_key(&base) {
                    return;
                }
                for (n, ft, _) in &sd.fields {
                    at.push(Proj::Field(n.clone()));
                    self.drop_paths(*ft, at, out, depth + 1);
                    at.pop();
                }
            }
            _ => {}
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
    /// init is a call of a fn whose result `impl Trait` may borrow from some argument (SigSyn::ret_from with OPAQUE_FROM, by the
    /// edition)
    pub(crate) fn bk_opaque_call(&self, init: &Expr) -> bool {
        let Expr::Call(f, _) = strip_paren(init) else { return false };
        let Some((_, sig)) = self.callee(f) else { return false };
        self.bk_opaque_mask(sig.ret_from).is_some_and(|m| m & !(1 << 63) != 0)
    }

    /// A result `impl Trait`'s ret_from (SigSyn::ret_from, OPAQUE_FROM) by the edition's rules, as a known ret_from
    fn bk_opaque_mask(&self, rf: u64) -> Option<u64> {
        if rf >> 63 & 1 == 1 || rf & crate::types::OPAQUE_FROM == 0 {
            return None;
        }
        let m = if self.prog.edition >= 2024 { rf & ((1 << 31) - 1) } else { rf >> 31 & ((1 << 31) - 1) };
        Some(1 << 63 | m)
    }

    pub(crate) fn bk_ret_from(&self, id: u32, i: usize) -> bool {
        let sig = &self.prog.sigs[id as usize];
        // (a method of a prelude view (`chunks.remainder()`): the result borrows what the view borrows, which the view's
        // local passes on, not the view. Not a RefCell guard's or a `PeekMut`'s `deref` / `deref_mut`: `&mut *g` and
        // `&g[0]` borrow the guard g, which must outlive them (a `PeekMut` dropped while what `deref_mut` gave is
        // written, the heap left out of order, E0505). A MutexGuard is left out: here `if let Some(&r) = guard.get(&k) { return r }`
        // (tests/probe/modules_intern.rs) would count r, a `&'static` copied out, as a borrow of the guard, as the
        // prelude has no lifetimes to tell; and `&g[0]` through it borrows the guard already
        // (tests/security/borrow_mutex_guard_deref_outlives.rs))
        if i == 0 && sig.has_self {
            if let Some(Ty::Ref(_, Ty::Struct(v))) = sig.params.first() {
                let guard = matches!(self.prog.structs[self.prog.base_of(*v) as usize].name.as_str(), "Ref" | "RefMut" | "PeekMut");
                if is_view(self.prog, *v) && !guard {
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
                // (`Option<Iter<T>>`, `Result<MutexGuard<T>, E>` (`m.lock()`) ..)
                if !self.prog.structs[o as usize].args.is_empty() && !is_view(self.prog, o) {
                    r = self.prog.structs[o as usize].args[0];
                }
            }
            if let Ty::Struct(v) = r {
                if is_view(self.prog, v) {
                    return true;
                }
            }
        }
        // (an argument of a type parameter that outlives a lifetime the result names: the result may hold it
        // (`fn scoped<'a, F: Fn() + 'a>(f: F) -> Guard<'a>`))
        if let (Some(Ty::Param(k)), Some(rs)) = (sig.params.get(i), sig.regions.as_deref()) {
            if let (Some((name, _)), Some(ret)) = (sig.generics.get(*k as usize), rs.slots.last()) {
                if rs.ty_outlives.iter().any(|(n, c)| n == name && *c < crate::types::R_STATIC && ret.contains(c)) {
                    return true;
                }
            }
        }
        // (a result `impl Trait`: what it captures, by the edition's rules (`fn f(x: &Vec<T>) -> impl Display` holds
        //  the borrow of x from 2024))
        let m = self.bk_opaque_mask(sig.ret_from).unwrap_or(sig.ret_from);
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
    ///
    /// Returns whether the closure holds a copy of the variable of which only the `Copy` parts it uses count: the
    /// code then empties the copy's parts that need dropping, and the variable stays where it was
    pub(crate) fn bk_capture(&mut self, c: &Expr, n: &str, loc: Loc, t: Ty, by_move: bool) -> bool {
        if !self.bk_on() {
            return false;
        }
        let Some(&l) = self.bk.by_loc.get(&loc) else { return false };
        let Expr::Closure(_, _, _, body, sp) = c else { return false };
        let sp = *sp;
        // (edition 2021 closures capture the places they use (`x.a`), not whole variables (RFC 2229))
        let mut paths: Vec<(BPlace, Option<Ty>, CapMode, Sp)> = vec![];
        self.bk_cap_paths(body, n, &mut paths);
        if paths.is_empty() {
            paths.push((BPlace { local: l, proj: vec![] }, Some(t), CapMode::Use, super::SP));
        }
        let move_kw = matches!(c, Expr::Closure(true, ..));
        if (by_move || move_kw) && self.bk.locals[l as usize].refs {
            self.bk.cap_held.entry(c as *const Expr as usize).or_default().push(l);
        }
        let let_moved = matches!(t, Ty::Ref(true, _)) && crate::ast_closure::let_moves_capture(c, &[n.to_string()]);
        // (the places of n the body assigns or borrows mutably: those are captured by `&mut`)
        let mut muts: Vec<BPlace> = vec![];
        let mut calls: Vec<BPlace> = vec![];
        self.bk_cap_muts(body, n, &mut muts, &mut calls);
        let assigns = crate::ast_closure::assigns_capture(c, &[n.to_string()]);
        // (unused_mut: a closure that mutates the variable itself, not what a reference in it points to, needs it `mut`)
        if self.bk.lint_mut && muts.iter().chain(&calls).any(|q| q.local == l && q.owned_len() == q.proj.len()) {
            self.bk.mut_used.push(l);
        }
        // (a closure that mutates what it captures, by value or by `&mut`: `FnMut`; one that holds a value that is not
        // `Copy` may move it out of itself (`FnOnce`, called by value): not known here, so not `FnMut` either)
        let ca = c as *const Expr as usize;
        // (a unique borrow through a `&mut` it captures (`|| set(&mut *x)`, `|| foo(x)`) mutates too)
        let unique = |q: &BPlace| q.proj.len() == q.owned_len() + 1 && q.proj.last() == Some(&Proj::Deref(false));
        if (assigns && !muts.is_empty()) || muts.iter().any(|q| q.owned_len() == q.proj.len() || unique(q)) || !calls.is_empty() {
            if !self.bk.fnmut_exprs.contains(&ca) {
                self.bk.fnmut_exprs.push(ca);
            }
        }
        // (a `let` that destructures the capture moves only the parts that are not Copy, both for the closure that
        // may be `FnOnce` and for the one that surely is)
        let prog = self.prog;
        if (by_move || move_kw) && !self.bk_ty(t).0 && !self.bk.fnonce_exprs.contains(&ca) && (crate::ast_closure::moves_capture_by(c, &[n.to_string()], &|p, _| pat_moves(prog, p, t, 0)) || self.bk_may_consume(body, n)) {
            self.bk.fnonce_exprs.push(ca);
        }
        if (by_move || move_kw) && !self.bk_ty(t).0 && !self.bk.fnonce_sure.contains(&ca) && crate::ast_closure::moves_capture_by(c, &[n.to_string()], &|p, _| pat_moves(prog, p, t, 0)) {
            self.bk.fnonce_sure.push(ca);
        }
        for (p, pt, mode, _) in &mut paths {
            // (stop at an index, and a move closure at a deref: it captures the reference. So does a shared reference held by
            // value (edition 2021's capture of `*h` used through a deref, ClosureSyn::deref): a copy of the reference)
            let shared_copy = by_move && matches!(t, Ty::Ref(false, _));
            let cut = p.proj.iter().position(|x| *x == Proj::Index || (move_kw || shared_copy) && matches!(x, Proj::Deref(_) | Proj::Shr));
            if let Some(i) = cut {
                p.proj.truncate(i);
                *pt = None;
            }
            // (the body borrows uniquely only through the `&mut` p is (`s.put(x)`, `foo(x)` with `x: &mut u32`): the
            // capture is of `*x`, which borrowing mutably needs no `mut x`)
            if !move_kw && *mode == CapMode::Use && !by_move {
                if let Some(q) = muts.iter().find(|q| unique(q) && q.local == p.local && q.proj.len() == p.proj.len() + 1 && q.proj.starts_with(&p.proj)) {
                    if !muts.iter().any(|r| r.local == p.local && r.proj.len() <= p.proj.len() && overlap(r, p)) && !calls.iter().any(|r| overlap(r, p)) {
                        *pt = match *pt {
                            Some(Ty::Ref(_, i)) => Some(*i),
                            _ => None,
                        };
                        *p = q.clone();
                    }
                }
            }
            if move_kw {
                *mode = (*mode).max(CapMode::Val);
            } else if *mode == CapMode::Use {
                // (a use that is not a pattern: acg's analysis says whether the closure takes the value)
                // (`&mut x` of what the closure owns no reference to (`|| { &mut my_var; }`) is a `&mut` capture too)
                // (`&mut *x` through a reference the closure captures (`|| set(&mut *x)`): a unique borrow of `*x`, so
                // a `&mut` capture of it too (E0596 through a `&`; two such closures at once conflict))
                let m = muts.iter().any(|q| overlap(q, p) && (assigns || q.owned_len() == q.proj.len() || q.proj.len() == q.owned_len() + 1)) || calls.iter().any(|q| overlap(q, p));
                *mode = if by_move { CapMode::Val } else if m { CapMode::RefMut } else { CapMode::Ref };
            }
        }
        // (before edition 2021 a closure captures whole variables, not the places it uses: `|| &self.number`
        // borrows `self`, which conflicts with `self.thing.bar(..)`'s `&mut` (nll/issue-51268.rs). A unique borrow
        // through a `&mut` variable (`|| self.n += 1`) stays of `*self`: it needs no `mut self`. In a closure's body,
        // a variable the closure holds by reference is `*x` of its reference (nll/issue-40510-4.rs))
        if self.prog.edition < 2021 {
            let base = self.bk.locals[l as usize].via_ref as usize;
            for (p, pt, mode, _) in &mut paths {
                if p.local != l || p.proj.len() < base || (base == 1 && p.proj[0] != Proj::Deref(false)) {
                    continue;
                }
                let uniq = (*mode == CapMode::RefMut && p.proj.get(base) == Some(&Proj::Deref(false))) as usize;
                if p.proj.len() > base + uniq {
                    p.proj.truncate(base + uniq);
                    *pt = match (uniq, t) {
                        (0, _) => Some(t),
                        (_, Ty::Ref(_, i)) => Some(*i),
                        _ => None,
                    };
                }
            }
        }
        // (a closure that holds the capture by value holds all of the variable: the code copies it whole into the
        // closure. Of a variable that needs dropping, the parts the body uses (rustc's captures) are not all that
        // is held: a part used after the closure would be dropped twice, by the closure and where it is used.
        // When the body uses only `Copy` parts and the rest can be emptied (no `Drop` impl of the program's in it),
        // the closure's copy is emptied of the rest and the variable stays where it was, as in rustc. Otherwise
        // the variable is moved, all of it. A type with a `Drop` impl of the program's is left to the parts:
        // rustc does not let them be moved out of it)
        let mut copy_cap = false;
        if (by_move || move_kw) && !self.bk_ty(t).0 && self.needs_drop(t) {
            let used: Vec<&(BPlace, Option<Ty>, CapMode, Sp)> = paths.iter().filter(|x| x.0.local == l).collect();
            let parts = !used.is_empty() && used.iter().all(|x| !x.0.proj.is_empty() && x.0.proj.iter().all(|p| matches!(p, Proj::Field(_))) && x.1.is_some_and(|pt| self.bk_ty(pt).0));
            if parts && self.zeroable(t) {
                copy_cap = true;
            } else if !parts {
                let at = paths.iter().map(|x| x.3).find(|a| a.line != 0).unwrap_or(super::SP);
                paths = vec![(BPlace { local: l, proj: vec![] }, Some(t), CapMode::Val, at)];
            }
        }
        paths.sort_by_key(|p| p.0.proj.len());
        // (and the first use of the kept place itself, which a part's stronger use does not replace)
        let mut keep: Vec<(BPlace, Option<Ty>, CapMode, Sp, Sp)> = vec![];
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
                None => keep.push((p, pt, mode, at, at)),
            }
        }
        for (p, pt, mode, at, path_at) in keep {
            // (the use in the body, for a mutability error; the closure itself otherwise)
            let at = if at.line == 0 { sp } else { at };
            let pt = if p.proj.is_empty() { Some(t) } else { pt };
            if mode >= CapMode::Val {
                // (a `&mut` held by value is moved into the closure: the borrow check counts it as Copy for the
                // reborrows of passing it, which a capture is not (`move || *r = 1`, `|| { let bar = r; }`, a
                // pattern's binding of it, ValMove). Passed as an argument, it may be reborrowed
                // (`drop::<&mut U>(r)`): not followed)
                let moved = move_kw || let_moved || mode == CapMode::ValMove;
                let copy = pt.is_none_or(|t| self.bk_ty(t).0 && !(matches!(t, Ty::Ref(true, _)) && moved));
                // (a copy a closure with no `move` holds of what it only reads: rustc's closure borrows it. Not of a
                // reference: one passed where a reference is expected is reborrowed (`f(b)` is `f(&*b)`), and the
                // closure holds a copy of it then (rustc's capture of `*b` by reference, truncated))
                // (a `&mut` passed as an argument is reborrowed (`look(x)` is `look(&*x)`, `foo(x)` is
                // `foo(&mut *x)`): the closure borrows `*x`, uniquely when the body passes it where a `&mut` goes.
                // A borrow of `*x` that conflicts is E0500 / E0502)
                if copy && !move_kw && matches!(pt, Some(Ty::Ref(true, _))) {
                    self.bk_acc_place(p.clone(), Acc::Read, sp);
                    let mut q = p;
                    q.proj.push(Proj::Deref(false));
                    let uniq = muts.iter().any(|m| *m == q);
                    let ln = self.bk.loans.len() as u32;
                    if path_at.line != 0 {
                        self.bk.cap_path_at.insert(ln, path_at);
                    }
                    self.bk.cap_loans.entry(c as *const Expr as usize).or_default().push(ln);
                    self.bk.loans.push(Loan { place: q.clone(), mutable: uniq, sp, at, quiet: false, capture: true });
                    self.bk_ev(Ev::Acc(q, Acc::Borrow(ln), sp));
                    self.bk.src(Src::Loan(ln));
                    continue;
                }
                if !(copy && !move_kw && pt.is_some_and(|t| !matches!(t, Ty::Ref(..) | Ty::Ptr(..) | Ty::Fn(_)))) {
                    self.bk_acc_place(p, if copy { Acc::Read } else { Acc::Move }, sp);
                    continue;
                }
            }
            let ln = self.bk.loans.len() as u32;
            if path_at.line != 0 {
                self.bk.cap_path_at.insert(ln, path_at);
            }
            self.bk.cap_loans.entry(c as *const Expr as usize).or_default().push(ln);
            self.bk.loans.push(Loan { place: p.clone(), mutable: mode == CapMode::RefMut, sp, at, quiet: false, capture: true });
            self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
            self.bk.src(Src::Loan(ln));
        }
        if self.bk.locals[l as usize].refs {
            self.bk.src(Src::Local(l));
        }
        copy_cap
    }

    pub(crate) fn bk_rooted_at_pub(&self, e: &Expr, n: &str) -> Option<(BPlace, Option<Ty>)> {
        self.bk_rooted_at(e, n)
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
        // (a unit struct (`Unit`, a pattern that names it): nothing to test, nothing captured)
        let unit = |n: &str| matches!(t, Some(Ty::Struct(sid)) if { let sd = &self.prog.structs[sid as usize]; !sd.is_enum && sd.fields.is_empty() && sd.name == n });
        match pat {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident(false, false, n, None) if unit(n) => {}
            Pat::Path(p) if p.segs.last().is_some_and(|s| unit(&s.name)) => {}
            Pat::Ident(by_ref, m, _, None) => {
                let mode = match (by_ref, m) {
                    (true, true) => CapMode::RefMut,
                    (true, false) => CapMode::Ref,
                    // (a `&mut` bound by value is moved out, which is_copy does not tell)
                    _ if matches!(t, Some(Ty::Ref(true, _))) => CapMode::ValMove,
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
            // (a struct's fields, or those of an enum's variant, by name: `SStruct { b, .. }` captures `x.b` only)
            Pat::Struct(path, fps, _) => {
                let Some(Ty::Struct(sid)) = t else {
                    out.push((base.clone(), t, CapMode::Use, sp));
                    return;
                };
                let sd = &self.prog.structs[sid as usize];
                let vname = path.segs.last().map(|s| s.name.as_str()).unwrap_or("");
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
                for (fname, p) in fps {
                    let Some(f) = fields.iter().find(|f| f.0 == *fname) else {
                        out.push((base.clone(), t, CapMode::Use, sp));
                        return;
                    };
                    let name = match &prefix {
                        Some(v) => format!("{v}::{}", f.0),
                        None => f.0.clone(),
                    };
                    self.bk_pat_cap(p, &field(base, name), Some(f.1), sp, &mut sub);
                }
                // (a union's capture is of the union, not of a field (the fields overlap: `u.b` is `u.a`); none where
                // the pattern captures nothing (`U { a: _ }`))
                let union = self.prog.unions.contains(&self.prog.structs[self.prog.base_of(sid) as usize].name);
                if sd.is_enum && sd.variants.len() > 1 || union && !sub.is_empty() {
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
            // (the unit variant of an enum of one variant: nothing to test, so nothing read (edition 2021 captures
            // nothing for `match sv { SingleVariant::A => .. }`). Not a constant, which compares the value: of the
            // enum (`E::ZERO`), of another type with the variant's name (`Other::V`), or in scope by itself)
            Pat::Path(p)
                if matches!(t, Some(Ty::Struct(sid)) if {
                    let sd = &self.prog.structs[sid as usize];
                    let ename = &self.prog.structs[self.prog.base_of(sid) as usize].name;
                    let ename = ename.rsplit("::").next().unwrap_or(ename);
                    let n = p.segs.len();
                    let qual = match n {
                        0 => false,
                        1 => self.prog.const_index.get(p.segs[0].name.as_str()).is_none(),
                        // (the enum, `Self`, or a type alias of the enum (`type A = E;`, `A::V`))
                        _ => {
                            let q = p.segs[n - 2].name.as_str();
                            q == ename
                                || q == "Self"
                                || self.prog.aliases.get(q).is_some_and(|(_, ty)| {
                                    matches!(ty, crate::types::TySyn::Named(a) | crate::types::TySyn::App(a, _) if a.rsplit("::").next() == Some(ename))
                                })
                        }
                    };
                    sd.is_enum && sd.variants.len() == 1 && sd.variants[0].fields.is_empty() && qual && p.segs.last().is_some_and(|s| s.name == sd.variants[0].name)
                }) => {}
            Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) => out.push((base.clone(), t, CapMode::Ref, sp)),
            // (a pattern not followed into: borrowed mutably if it binds a part by `ref mut`, else as the body uses it)
            _ if crate::ast_closure::binds_ref_mut(pat) => out.push((base.clone(), t, CapMode::RefMut, sp)),
            _ => out.push((base.clone(), t, CapMode::Use, sp)),
        }
    }

    /// The places rooted at n that the closure body assigns (`n.a = ..`, `n += 1`) or borrows mutably (`&mut n.b`)
    fn bk_cap_muts(&self, e: &Expr, n: &str, out: &mut Vec<BPlace>, calls: &mut Vec<BPlace>) {
        // (a `&mut` of the capture passed where a `&mut` is expected (`foo(x)` with `x: &mut u32`, `fn foo(_: &mut
        // u32)`): reborrowed, a unique borrow of `*x`. Not where a `&` is: a shared reborrow)
        let params: Option<(&[Expr], &[Ty])> = match e {
            Expr::Call(f, args) => match strip_paren(f) {
                Expr::Path(None, fp) if ident_of(fp).is_none_or(|id| self.lookup(id).is_none()) => {
                    self.prog.fn_by_p(&crate::program::PPath::of(fp), self.self_sid).map(|id| (&args[..], &self.prog.sigs[id as usize].params[..]))
                }
                _ => None,
            },
            Expr::MethodCall(r, m, _, args, _) => match self.bk_rooted_at(r, n).and_then(|x| x.1).or_else(|| self.peek(r)).map(|t| self.strip_refs(t)) {
                Some(Ty::Struct(sid)) => self.prog.self_method_of(sid, m).or_else(|| self.prog.self_default_method(sid, m)).map(|id| (&args[..], self.prog.sigs[id as usize].params.get(1..).unwrap_or(&[]))),
                _ => None,
            },
            _ => None,
        };
        if let Some((args, ps)) = params {
            for (a, pt) in args.iter().zip(ps) {
                if !matches!(pt, Ty::Ref(true, _)) {
                    continue;
                }
                if let Some((mut p, Some(Ty::Ref(true, _)))) = self.bk_rooted_at(strip_paren(a), n) {
                    p.proj.push(Proj::Deref(false));
                    out.push(p);
                }
            }
        }
        match e {
            Expr::Assign(l, _) | Expr::AssignOp(_, l, _, _) | Expr::Ref(true, l) => {
                // (`v[1] = 4`: v itself, as the capture stops at an index)
                let mut l = &**l;
                while let Expr::Index(b, _, _) = strip_paren(l) {
                    l = b;
                }
                if let Some((p, _)) = self.bk_rooted_at(l, n) {
                    out.push(p);
                }
            }
            // (a call of a captured closure that is `FnMut` (`tick1()`): it is borrowed by `&mut` for the call, so
            // captured by `&mut`)
            Expr::Call(f, _) => {
                if let Some((p, Some(Ty::Struct(sid)))) = self.bk_rooted_at(strip_paren(f), n) {
                    if p.proj.is_empty() && self.bk.fnmut.contains(&Ty::Struct(sid)) {
                        calls.push(p);
                    }
                }
            }
            // (a `&mut self` method of the place (`v.push(4)`): captured by `&mut`)
            Expr::MethodCall(r, m, ..) => {
                if let Some((p, Some(Ty::Struct(sid)))) = self.bk_rooted_at(r, n) {
                    let mut_self = self.prog.self_method_of(sid, m).or_else(|| self.prog.self_default_method(sid, m)).is_some_and(|id| {
                        let sg = &self.prog.sigs[id as usize];
                        sg.has_self && matches!(sg.params.first(), Some(Ty::Ref(true, _)))
                    });
                    if mut_self {
                        calls.push(p);
                    }
                }
            }
            Expr::Macro(_, g, _) => {
                for a in crate::ast_closure::macro_args(g).unwrap_or_default() {
                    self.bk_cap_muts(&a, n, out, calls);
                }
                return;
            }
            // (a pattern that binds a part of the place by `ref mut` (`let (ref mut x, _) = p;`): a mutable borrow
            // of it)
            Expr::Match(x, arms) if arms.iter().any(|a| crate::ast_closure::binds_ref_mut(&a.pat)) => {
                if let Some((p, t)) = self.bk_rooted_at(x, n) {
                    for a in arms.iter().filter(|a| crate::ast_closure::binds_ref_mut(&a.pat)) {
                        self.bk_ref_mut_parts(&a.pat, &p, t, out);
                    }
                }
            }
            Expr::Let(pat, x) if crate::ast_closure::binds_ref_mut(pat) => {
                if let Some((p, t)) = self.bk_rooted_at(x, n) {
                    self.bk_ref_mut_parts(pat, &p, t, out);
                }
            }
            Expr::Block(_, _, b) => {
                for st in &b.stmts {
                    if let crate::parse::Stmt::Let(pat, _, Some(x), ..) = st {
                        if crate::ast_closure::binds_ref_mut(pat) {
                            if let Some((p, t)) = self.bk_rooted_at(x, n) {
                                self.bk_ref_mut_parts(pat, &p, t, out);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        crate::ast_closure::each_child_pub(e, &mut |c| self.bk_cap_muts(c, n, out, calls));
    }

    /// The parts of the place p (of type t) that the pattern binds by `ref mut` (`p.0` of `let (ref mut x, _) = p;`):
    /// what the closure borrows mutably. All of p when the pattern's parts are not followed
    fn bk_ref_mut_parts(&self, pat: &Pat, p: &BPlace, t: Option<Ty>, out: &mut Vec<BPlace>) {
        let mut v = vec![];
        self.bk_pat_cap(pat, p, t, super::SP, &mut v);
        let n0 = out.len();
        // (a part the patterns are not followed into that binds by `ref mut` (`S { value: ref mut y }`) is RefMut
        // too (bk_pat_cap); one that does not (`S { value: ref y }`) keeps its own mode)
        out.extend(v.into_iter().filter(|x| x.2 == CapMode::RefMut && x.0.local == p.local).map(|x| x.0));
        if out.len() == n0 {
            out.push(p.clone());
        }
    }

    /// Whether a closure's body may take its capture n by value, beyond what moves_capture sees: a method of it that
    /// takes `self` (or one not known), a pattern that binds a part of it by value (`match n { Some(x) => .. }`), in a
    /// macro's arguments too
    fn bk_may_consume(&self, e: &Expr, n: &str) -> bool {
        match e {
            Expr::MethodCall(r, m, ..) => {
                if let Some((_, rt)) = self.bk_rooted_at(r, n) {
                    let by_value = match rt {
                        Some(Ty::Struct(sid)) => self.prog.self_method_of(sid, m).or_else(|| self.prog.self_default_method(sid, m)).is_none_or(|id| {
                            let sg = &self.prog.sigs[id as usize];
                            !(sg.has_self && matches!(sg.params.first(), Some(Ty::Ref(..))))
                        }),
                        Some(Ty::Ref(..)) => false,
                        _ => true,
                    };
                    if by_value {
                        return true;
                    }
                }
            }
            Expr::Match(x, arms) => {
                if self.bk_rooted_at(x, n).is_some() && arms.iter().any(|a| crate::ast_closure::binds_by_value(&a.pat)) {
                    return true;
                }
            }
            Expr::Let(p, x) => {
                if self.bk_rooted_at(x, n).is_some() && crate::ast_closure::binds_by_value(p) {
                    return true;
                }
            }
            Expr::Macro(_, g, _) => {
                return crate::ast_closure::macro_args(g).unwrap_or_default().iter().any(|a| self.bk_may_consume(a, n));
            }
            _ => {}
        }
        let mut found = false;
        crate::ast_closure::each_child_pub(e, &mut |c| {
            if !found && self.bk_may_consume(c, n) {
                found = true;
            }
        });
        found
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
        // (the arguments read at their positions in the source: a use's position is reported (E0597 at
        // `println!("{:?}", p)`'s `p`))
        if let Expr::Macro(_, g, msp) = e {
            match crate::ast_closure::macro_args_at(g, msp.at) {
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
        self.bk.locals.push(Local { name: "<argument>".into(), ty: Ty::Unit, copy: true, refs: true, mutable: None, decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, loans));
        self.bk.call_temps.push(id);
    }

    /// A hidden local holds what an argument holds (its loans, shared ones too, and what the locals it reads hold) until
    /// the call is made: for a call whose `&mut self` is taken then (Bk::hold_args)
    pub(crate) fn bk_call_temp_all(&mut self, srcs: &[Src]) {
        if !self.bk_on() || srcs.is_empty() {
            return;
        }
        let id = self.bk.locals.len() as u32;
        self.bk.locals.push(Local { name: "<argument>".into(), ty: Ty::Unit, copy: true, refs: true, mutable: None, decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, srcs.to_vec()));
        self.bk.call_temps.push(id);
    }

    /// What the callee of a call holds (the locals read for it, the loans made for it), held until the call ends
    /// (bk_call_temps_end), after the arguments
    pub(crate) fn bk_callee_hold(&mut self, srcs: &[Src]) {
        if !self.bk_on() || srcs.is_empty() {
            return;
        }
        let id = self.bk.locals.len() as u32;
        self.bk.locals.push(Local { name: "<callee>".into(), ty: Ty::Unit, copy: true, refs: true, mutable: None, decl: super::SP, via_ref: false });
        self.bk_ev(Ev::Bind(id, true));
        self.bk_ev(Ev::Def(id, srcs.to_vec()));
        self.bk.call_temps.push(id);
    }

    /// `IndexMut::index_mut(&mut b, ..)` of a place written: the `&mut b` loan, held until the index is evaluated
    /// (bk_call_temps_end)
    pub(crate) fn bk_index_mut(&mut self, b: &Expr) -> bool {
        let Some((p, _)) = self.bk_place(b) else { return false };
        let sp = err_pos(b);
        let ln = self.bk.loans.len() as u32;
        self.bk.loans.push(Loan { place: p.clone(), mutable: true, sp, at: sp, quiet: false, capture: false });
        self.bk_ev(Ev::Acc(p, Acc::Borrow(ln), sp));
        self.bk_call_temp(&[Src::Loan(ln)], true);
        true
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
    /// unused_mut: e (a place) is changed in a way no access records (an array's slice method that mutates,
    /// `arr.sort()`): what it is rooted at needs `mut` when e is a part it owns
    pub(crate) fn bk_mut_use(&mut self, e: &Expr) {
        if !self.bk.lint_mut {
            return;
        }
        if let Some((p, _)) = self.bk_place(e) {
            if p.owned_len() == p.proj.len() {
                self.bk.mut_used.push(p.local);
            }
        }
    }

    pub(crate) fn bk_loan_only(&mut self, e: &Expr, m: bool) -> Option<u32> {
        let (mut p, _) = self.bk_place(e)?;
        // (a method's receiver that is a reference (`v.iter_mut()` with `v: &mut [T]`): it is auto-dereferenced, so
        // the loan is of `*v`, a reborrow, not of the local v)
        if let Some(Ty::Ref(rm, _)) = self.peek(e) {
            if self.bk.recv_deref {
                p.proj.push(ref_deref(rm));
            }
        }
        let sp = err_pos(e);
        // (unused_mut: `for x in v.iter_mut()` / `&mut v` takes a `&mut` of v, which no access records)
        if m && self.bk.lint_mut && p.owned_len() == p.proj.len() {
            self.bk.mut_used.push(p.local);
        }
        let ln = self.bk.loans.len() as u32;
        self.bk.loans.push(Loan { place: p, mutable: m, sp, at: sp, quiet: true, capture: false });
        Some(ln)
    }

    /// A call's `&mut place` argument whose referent shares a lifetime or a type parameter of the signature with
    /// another argument: what that argument borrows may be stored in the place (`push(&x, &mut v)` of
    /// `fn push<'a>(r: &'a i32, v: &mut Vec<&'a i32>)` stores x's loan in v: `x = 2;` while v is used is E0506)
    pub(crate) fn bk_arg_flows(&mut self, id: u32, args: &[&Expr]) {
        self.bk_arg_flows_at(id, args, false);
    }

    /// bk_arg_flows before the arguments are made, when one of them assigns (`put(r, { r = &x; }, &mut v)`): what a
    /// local passed holds then flows too, as rustc's region of the local holds both loans (only the locals' sources
    /// are known before)
    pub(crate) fn bk_arg_flows_before(&mut self, id: u32, args: &[&Expr]) {
        fn assigns(e: &Expr, depth: u32) -> bool {
            if depth > 32 {
                return false;
            }
            if matches!(e, Expr::Assign(..) | Expr::AssignOp(..)) {
                return true;
            }
            if matches!(e, Expr::Closure(..)) {
                return false;
            }
            let mut found = false;
            crate::ast_closure::each_child_pub(e, &mut |c| found = found || assigns(c, depth + 1));
            found
        }
        if self.bk_on() && args.iter().any(|a| assigns(a, 0)) {
            self.bk_arg_flows_at(id, args, true);
        }
    }

    fn bk_arg_flows_at(&mut self, id: u32, args: &[&Expr], before: bool) {
        if !self.bk_on() || self.bk.level < 2 || args.len() < 2 {
            return;
        }
        let prog = self.prog;
        let sig = &prog.sigs[id as usize];
        if args.len() != sig.params.len() || !sig.params.iter().any(|t| matches!(t, Ty::Ref(true, _))) {
            return;
        }
        fn params(prog: &Program, t: Ty, out: &mut Vec<u8>, depth: u32) {
            match t {
                Ty::Param(i) => out.push(i),
                Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => params(prog, *i, out, depth + 1),
                Ty::Struct(s) if depth < 8 && prog.structs[s as usize].symbolic => {
                    for &a in &prog.structs[s as usize].args {
                        params(prog, a, out, depth + 1);
                    }
                }
                _ => {}
            }
        }
        let rs = sig.regions.as_deref();
        for (i, a) in args.iter().enumerate() {
            let Ty::Ref(true, inner) = sig.params[i] else { continue };
            let Expr::Ref(true, place) = strip_paren(a) else { continue };
            // (only into a referent that may hold references (not `swap(&mut v, &mut w)` of two `Vec<u8>`))
            if !self.peek(place).is_some_and(|t| self.bk_ty(t).1) {
                continue;
            }
            // (what the referent may hold: the lifetimes it names (after the reference's own), its type parameters)
            let lts: Vec<u8> = rs.and_then(|r| r.slots.get(i)).map(|s| s.iter().skip(1).copied().filter(|&c| c < crate::types::R_STATIC).collect()).unwrap_or_default();
            let mut tps = vec![];
            params(prog, *inner, &mut tps, 0);
            if lts.is_empty() && tps.is_empty() {
                continue;
            }
            let mut srcs = vec![];
            for (j, b) in args.iter().enumerate() {
                if j == i {
                    continue;
                }
                let sj = rs.and_then(|r| r.slots.get(j));
                let by_lt = !lts.is_empty() && sj.is_some_and(|s| s.iter().any(|c| lts.contains(c)));
                let by_tp = !tps.is_empty() && {
                    let mut v = vec![];
                    params(prog, sig.params[j], &mut v, 0);
                    v.iter().any(|p| tps.contains(p))
                };
                if !(by_lt || by_tp) {
                    continue;
                }
                // (the argument itself when the shared lifetime is its reference's own (`r: &'a i32`) or the type
                // parameter is its type (`a: T`); behind its reference (`b: &mut T`, `r: &&'a i32`), what the referent
                // holds)
                let own = match sig.params[j] {
                    Ty::Ref(..) => by_lt && sj.and_then(|s| s.first()).is_some_and(|c| lts.contains(c)),
                    _ => true,
                };
                if own {
                    self.bk_direct(b, &mut srcs);
                } else if let Expr::Ref(_, bp) = strip_paren(b) {
                    match self.bk_place(bp) {
                        Some((p, _)) if self.bk.locals[p.local as usize].refs => srcs.push(Src::Local(p.local)),
                        Some(_) => {}
                        None => self.bk_direct(bp, &mut srcs),
                    }
                } else {
                    self.bk_direct(b, &mut srcs);
                }
            }
            if before {
                srcs.retain(|s| matches!(s, Src::Local(_)));
            }
            self.bk_flow_into(place, &srcs);
        }
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
    /// bk_frame_end_call, and the result holds what the arguments `from` borrow directly too
    pub(crate) fn bk_frame_end_call_from<E: std::ops::Deref<Target = Expr>>(&mut self, args: &[E], from: &[usize]) {
        if !self.bk_on() {
            return;
        }
        self.bk_frame_end_call();
        for &i in from {
            if let Some(a) = args.get(i) {
                let mut v = vec![];
                self.bk_direct(a, &mut v);
                for s in v {
                    self.bk.src(s);
                }
            }
        }
    }

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
            // (a call's result `impl Trait` that captures its arguments' borrows (`let a = display_len(&x);`, edition
            //  2024) holds them whatever its hidden type is (`usize`))
            if !self.bk.locals[l as usize].refs && self.bk_opaque_call(init) {
                self.bk.locals[l as usize].refs = true;
            }
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
        let Some((p, pt)) = self.bk_place(strip_paren(left)) else { return };
        let l = p.local;
        let whole = p.proj.is_empty();
        // (a reference to what holds references (`&mut &u8`): see Ev::Store)
        let deep = matches!(pt, Some(Ty::Ref(_, i)) if self.bk_ty(*i).1);
        // (`*x = ..` / `x.f = ..` through a reference parameter: what is stored outlives the function)
        let through = p.owned_len() < p.proj.len() && self.bk.params.contains(&l);
        let derefs = p.proj.iter().filter(|x| matches!(x, Proj::Deref(false))).count() as u32;
        // (the local's loans are not known by field: through `*q.0` of `q = (&mut p, &mut z)` it is not known which
        // it points to, so only a store through derefs of the whole local (`*q`, `**q`, `(*q).f`) is followed)
        let whole_derefs = p.proj.iter().take_while(|x| matches!(x, Proj::Deref(false))).count() as u32 == derefs;
        // (the old value is dropped by the assignment: a `Drop` that may see what it borrows uses its loans there
        // (`base = false;` before `wrapper = W(&mut base);` in a loop, E0506), as at its scope's end)
        if let (true, Some(t)) = (whole, pt) {
            if self.bk.locals[l as usize].refs && self.bk.level >= 2 && self.dropck(t, 0) {
                self.bk_ev(Ev::Acc(p.clone(), Acc::Drop, err_pos(left)));
            }
        }
        // (a value that holds what the local holds (`r = &mut *r;`, `r = r;`): what it holds now, taken before the
        // write clears it (a hidden local holds it), so the new value keeps the old loans of what r points to)
        let snap;
        let srcs = if whole && self.bk.locals[l as usize].refs && srcs.contains(&Src::Local(l)) {
            let id = self.bk.locals.len() as u32;
            let (ty, copy) = (self.bk.locals[l as usize].ty, self.bk.locals[l as usize].copy);
            self.bk.locals.push(Local { name: "<old>".into(), ty, copy, refs: true, mutable: None, decl: super::SP, via_ref: false });
            self.bk_ev(Ev::Bind(id, true));
            self.bk_ev(Ev::Def(id, vec![Src::Local(l)]));
            snap = srcs.iter().map(|&s| if s == Src::Local(l) { Src::Local(id) } else { s }).collect::<Vec<Src>>();
            &snap[..]
        } else {
            srcs
        };
        self.bk_ev(Ev::Acc(p, Acc::Write, err_pos(left)));
        if through && !srcs.is_empty() {
            self.bk_ev(Ev::Escape(srcs.to_vec()));
        }
        // (through a `&mut` the local holds: what is stored is held by what it points to)
        if !through && derefs > 0 && whole_derefs && !srcs.is_empty() && self.bk.locals[l as usize].refs {
            self.bk_ev(Ev::Store(l, derefs, deep, srcs.to_vec()));
        }
        if self.bk.locals[l as usize].refs {
            if whole {
                // (a reborrow through the local itself (`y = &mut *y;`, `y = y;`): overwriting y kills the loans of
                // what its old value points to (Polonius' loan_killed_at), the new value's too. What the old value
                // borrowed stays with the reborrow)
                // (only those through a reference: a loan of an owned part (`s = S { r: &s.n, .. }`) stays, and writing
                // `s.n` while it lives conflicts)
                let srcs: Vec<Src> = srcs.iter().copied().filter(|s| !matches!(s, Src::Loan(ln) if self.bk.loans[*ln as usize].place.local == l && self.bk.loans[*ln as usize].place.owned_len() < self.bk.loans[*ln as usize].place.proj.len())).collect();
                self.bk_ev(Ev::Def(l, srcs));
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
        // (a local `c: &&U` returned as a `&U`, or passed to a constructor whose field is a `&U` (`Some(c)` returned as
        // an `Option<&U>`): the deref coercion copies `*c` out, so the value holds what that holds, not the loan c is
        // (as at a coercion site of a `let` or an argument, bk_copy_out_of))
        if self.bk.level >= 2 && srcs.iter().any(|s| matches!(s, Src::Local(_))) {
            let local_of = |me: &Self, a: &Expr| -> Option<(u32, Ty)> {
                let Expr::Path(None, p) = strip_paren(a) else { return None };
                let l = me.bk_local_of(super::ident_of(p)?)?;
                match me.bk.locals[l as usize].ty {
                    Ty::Ref(_, u @ Ty::Ref(false, _)) => Some((l, *u)),
                    _ => None,
                }
            };
            let mut coerced: Vec<(u32, Ty)> = vec![];
            match strip_paren(e) {
                x @ Expr::Path(..) => {
                    if let (Some((l, u)), Ty::Ref(_, r)) = (local_of(self, x), self.ret) {
                        if Ty::Ref(false, r) == u || matches!(u, Ty::Ref(false, i) if *i == *r) {
                            coerced.push((l, u));
                        }
                    }
                }
                Expr::Call(f, args) if matches!(&**f, Expr::Path(_, p) if p.segs.last().is_some_and(|s| s.name.starts_with(|c: char| c.is_ascii_uppercase()))) => {
                    if let Some(Ty::Struct(sid)) = self.peek(e) {
                        let targs = &self.prog.structs[sid as usize].args;
                        for a in args {
                            if let Some((l, u)) = local_of(self, a) {
                                if targs.contains(&u) && !targs.contains(&self.bk.locals[l as usize].ty) {
                                    coerced.push((l, u));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            for (l, u) in coerced {
                let c = self.bk_copy_out_of(l, u);
                for s in srcs.iter_mut() {
                    if *s == Src::Local(l) {
                        *s = Src::Local(c);
                    }
                }
            }
        }
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
                self.bk.locals.push(Local { name: TEMP.to_string(), ty: Ty::Unit, copy: true, refs: false, mutable: Some(true), decl: super::SP, via_ref: false });
                self.bk_ev(Ev::Bind(id, true));
                let ln = self.bk.loans.len() as u32;
                let sp = err_pos(t);
                self.bk.loans.push(Loan { place: BPlace { local: id, proj: vec![] }, mutable: *m, sp, at: sp, quiet: true, capture: false });
                srcs.push(Src::Loan(ln));
            }
        }
        if !srcs.is_empty() {
            // (a chain of method calls is reported where it starts (rustc's span of the returned value), not at its
            // last method's name: `raw\n.iter()\n.collect()`)
            let mut x = e;
            while let Expr::Paren(i) | Expr::Ref(_, i) = x {
                x = i;
            }
            let sp = if matches!(x, Expr::MethodCall(..)) { super::expr_start(x) } else { pos(e) };
            self.bk_ev(Ev::Ret(srcs, if sp.line == 0 { pos(e) } else { sp }));
        }
    }

    /// What the value of e holds under the borrows it is (a chain's root local's loans: `parts.first()` holds what parts
    /// holds)
    /// e is an iterator made by `copied` / `cloned` of an iterator, through adapters that hold it (`v.iter().copied()`,
    /// `v.iter().cloned().filter(..)`): its items are copies
    fn bk_copies_items(&self, e: &Expr) -> bool {
        let mut e = e;
        for _ in 0..16 {
            while let Expr::Paren(x) = e {
                e = x;
            }
            let Expr::MethodCall(r, name, ..) = e else { return false };
            let mut rt = self.peek(r);
            while let Some(Ty::Ref(_, i)) = rt {
                rt = Some(*i);
            }
            if !self.peek(e).zip(rt).is_some_and(|(t, r)| self.ty_holds(t, r, 0)) {
                return false;
            }
            if matches!(name.as_str(), "copied" | "cloned") {
                return true;
            }
            e = r;
        }
        false
    }

    fn bk_held(&self, e: &Expr, out: &mut Vec<Src>) {
        match e {
            Expr::Paren(x) | Expr::MethodCall(x, ..) => self.bk_held(x, out),
            // (a reference in a parameter's field (`s.0.iter().copied()` with `s: &Foo<'a>`) holds a region of the
            // parameter's type: for an `impl Trait` that captures none (E0700), the only check that reads it)
            Expr::Field(..) if self.bk.ret_no_capture => {
                if let Some((p, _)) = self.bk_place(e) {
                    if self.bk.inner_params.contains(&p.local) && self.peek(e).is_some_and(|t| has_refs(self.prog, t, 0)) {
                        out.push(Src::Local(p.local));
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
            _ => {}
        }
    }

    fn bk_direct(&self, e: &Expr, out: &mut Vec<Src>) {
        match e {
            // (a cast keeps what the value borrows: `Box::new(B(&*v)) as Box<dyn X>`)
            Expr::Paren(x) | Expr::Cast(x, _) => self.bk_direct(x, out),
            // (`Box::new(x)`: the box holds what x does (the prelude writes no lifetimes, so its fns are not
            // followed otherwise))
            Expr::Call(f, args) if args.len() == 1 && matches!(&**f, Expr::Path(None, p) if p.segs.len() == 2 && p.segs[0].name == "Box" && p.segs[1].name == "new") => {
                self.bk_direct(&args[0], out)
            }
            // (a call of the program's fn: what its result borrows from, by its signature (`fn gimme(x: &T) -> &U`);
            // the prelude writes no lifetimes, so its fns are not followed)
            // (a call's value made a temporary a method borrows (`String::new().as_str()`): that temporary's loan)
            Expr::Call(f, args) => {
                if let Some(&ln) = self.bk.loan_of.get(&(e as *const Expr as usize)) {
                    out.push(Src::Loan(ln));
                }
                if let Some(id) = self.fn_item(f) {
                    if self.prog.item_of.get(id as usize).copied().flatten().is_some_and(|i| i < self.prog.prelude_from) {
                        for (i, a) in args.iter().enumerate() {
                            if self.bk_ret_from(id, i) {
                                self.bk_direct(a, out);
                            }
                        }
                    }
                } else if matches!(&**f, Expr::Path(_, p) if p.segs.last().is_some_and(|s| s.name.starts_with(|c: char| c.is_ascii_uppercase()))) {
                    // (a tuple struct's or a variant's constructor (`Some(r)`): it holds what its arguments do)
                    for a in args {
                        self.bk_direct(a, out);
                    }
                }
            }
            Expr::Ref(_, x) => {
                if let Some(&ln) = self.bk.loan_of.get(&(&**x as *const Expr as usize)) {
                    out.push(Src::Loan(ln));
                }
            }
            Expr::MethodCall(recv, name, _, args, _) => {
                let n0 = out.len();
                if let Some(&ln) = self.bk.loan_of.get(&(e as *const Expr as usize)) {
                    out.push(Src::Loan(ln));
                }
                // (`r.clone()` of a reference whose referent is not `Clone` (`(&x).clone()` with x: T): the reference's
                // copy, which holds what r holds (E0515 when it is returned))
                // (not a `clone` of the referent's own: `fn clone(&self) -> &'static S` is what it says)
                let own_clone = matches!(self.peek(recv), Some(Ty::Ref(_, Ty::Struct(sid))) if self.prog.method(*sid, "clone").is_some());
                if out.len() == n0 && name == "clone" && args.is_empty() && !own_clone && matches!(self.peek(e), Some(Ty::Ref(..))) && self.peek(recv) == self.peek(e) {
                    self.bk_direct(recv, out);
                }
                // (a method of the prelude's types whose result holds references: what its receiver holds (`it.next()`,
                // `raw.iter().map(..).collect()`). The prelude writes no lifetimes)
                if out.len() == n0 && self.peek(e).is_some_and(|t| has_refs(self.prog, t, 0)) {
                    let prelude = |t: Ty| match t {
                        Ty::Struct(sid) => self.prog.struct_item.get(self.prog.base_of(sid) as usize).is_some_and(|&i| i >= self.prog.prelude_from),
                        _ => false,
                    };
                    let mut rt = self.peek(recv);
                    while let Some(Ty::Ref(_, i)) = rt {
                        rt = Some(*i);
                    }
                    // (an adapter whose closure makes the items (`map`, `filter_map` ..): they hold what the closure's
                    // result does, not the receiver's items, when the closure does not use its parameter
                    // (`raw.iter().map(|_| "ok")`))
                    let made = matches!(name.as_str(), "map" | "filter_map" | "flat_map" | "map_while")
                        && matches!(args.first().map(|a| super::strip_paren(a)), Some(Expr::Closure(_, ps, _, body, _)) if ps.iter().all(|(p, _)| {
                            let mut v = vec![];
                            crate::ast_closure::pat_idents(p, &mut v);
                            v.iter().all(|n| !mentions(body, n))
                        }));
                    if rt.is_some_and(prelude) && !made {
                        // (`copied` / `cloned` of an `Option<&T>` take a reference off: what is under it, not the
                        // borrow itself (`parts.first().copied()` holds what parts holds, not a loan of parts). Of an
                        // iterator, the adapter holds the iterator, and so its borrow (`s.0.iter().copied()`); what
                        // is taken out of it (`.next()`, `.collect()`, through other adapters) is the copies)
                        let adapter = self.peek(e).zip(rt).is_some_and(|(t, r)| self.ty_holds(t, r, 0));
                        let copies = matches!(name.as_str(), "copied" | "cloned") || self.bk_copies_items(recv);
                        if adapter || !copies {
                            self.bk_direct(recv, out);
                        }
                        if copies {
                            self.bk_held(recv, out);
                        }
                    } else if matches!(rt, Some(Ty::Str | Ty::Slice(_))) && self.peek(e).is_some_and(|t| matches!(t, Ty::Struct(s) if is_view(self.prog, s))) {
                        // (a view of a str / slice (`s.chars()`, `s.split(',')`): it holds what the receiver does)
                        self.bk_direct(recv, out);
                    }
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
                if let Some(&t) = self.bk.block_val.get(&(b as *const crate::parse::Block as usize)) {
                    out.push(Src::Local(t));
                } else if let Some(crate::parse::Stmt::Expr(t, false)) = b.stmts.last() {
                    self.bk_direct(t, out);
                }
            }
            // (a closure: the loans of what it captures by reference, and what it holds by value holds)
            Expr::Closure(..) => {
                if let Some(ls) = self.bk.cap_loans.get(&(e as *const Expr as usize)) {
                    out.extend(ls.iter().map(|&l| Src::Loan(l)));
                }
                if let Some(ls) = self.bk.cap_held.get(&(e as *const Expr as usize)) {
                    out.extend(ls.iter().map(|&l| Src::Local(l)));
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
            // (a tuple, an array, a struct's literal: they hold what their parts do (`(x, &x)` returned, E0515 also
            // where no region check runs))
            Expr::Tuple(es) | Expr::Array(es) => {
                let et = |i: usize| match self.peek(e) {
                    Some(Ty::Array(t, _)) => Some(*t),
                    Some(Ty::Struct(sid)) if self.prog.is_tuple(sid) => self.prog.structs[sid as usize].fields.get(i).map(|f| f.1),
                    _ => None,
                };
                for (i, x) in es.iter().enumerate() {
                    self.bk_direct_part(x, et(i), out);
                }
            }
            // (an element coerced as the Array arm's: `[&x; 1]` of x a reference)
            Expr::Repeat(x, _) => {
                let want = match self.peek(e) {
                    Some(Ty::Array(t, _)) => Some(*t),
                    _ => None,
                };
                self.bk_direct_part(x, want, out);
            }
            Expr::Struct(_, fs, base) => {
                let sid = match self.peek(e) {
                    Some(Ty::Struct(sid)) => Some(sid),
                    _ => None,
                };
                for (f, x) in fs {
                    self.bk_direct_part(x, sid.and_then(|s| self.prog.field(s, f)).map(|f| f.0), out);
                }
                // (the base gives the fields the literal does not write: what it holds is kept where one of them may
                // hold a reference (`W { r: x, ..s }` takes `n: usize` of s, not the loan in `s.r`). Where a written
                // field may hold one too, the base's loan may be in that field's region only: followed where the
                // struct has one lifetime, no type parameter, and a field the base gives has that lifetime
                // (`V { r: x, ..s }` of `V<'a>`, not of `V<'a, 'b>`: rustc relates s's regions to the result's by
                // the fields it gives))
                if let Some(b) = base {
                    let kept = sid.is_none_or(|s| {
                        let sd = &self.prog.structs[s as usize];
                        let (mut given, mut written) = (vec![], false);
                        for (i, f) in sd.fields.iter().enumerate() {
                            if has_refs(self.prog, f.1, 0) {
                                if fs.iter().any(|(n, _)| *n == f.0) {
                                    written = true;
                                } else {
                                    given.push(i);
                                }
                            }
                        }
                        !given.is_empty()
                            && (!written
                                || sd.args.is_empty()
                                    && self.prog.adt_regions.get(&self.prog.base_of(s)).is_some_and(|r| r.lts.len() == 1 && given.iter().any(|&i| r.fields.get(i).is_some_and(|v| v.contains(&0)))))
                    });
                    if kept {
                        self.bk_direct(b, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// A part of an aggregate whose type is want (bk_direct): `&x` of x a reference, where want is not a reference to
    /// x's type, is a deref coercion (`W { slice: &self }` with self: `&[T]`), which copies `*x`'s reference: it holds
    /// what x does, not a loan of x
    fn bk_direct_part(&self, x: &Expr, want: Option<Ty>, out: &mut Vec<Src>) {
        if let (Expr::Ref(_, inner), Some(w)) = (strip_paren(x), want) {
            if let Some(it @ Ty::Ref(..)) = self.peek(inner) {
                if !matches!(w, Ty::Ref(_, i) if *i == it) {
                    self.bk_direct(inner, out);
                    return;
                }
            }
        }
        self.bk_direct(x, out);
    }

    /// Run the checker on the lowered body
    pub(crate) fn bk_check(&mut self) -> Result<(), String> {
        if !self.bk.on || self.bk.evs.is_empty() {
            return Ok(());
        }
        let mut bk = std::mem::take(&mut self.bk);
        bk.edges = self.b.cfg_edges();
        // (one mask for each copied type: it depends on the type and the loans only)
        let mut by_ty: Vec<Ty> = Vec::new();
        for &(c, u) in &bk.copy_out {
            let i = match by_ty.iter().position(|&t| t == u) {
                Some(i) => i,
                None => {
                    let m: Vec<bool> = bk.loans.iter().map(|lo| {
                        let t = bk.locals.get(lo.place.local as usize).map(|l| l.ty);
                        t.and_then(|t| self.place_ty(t, &lo.place.proj)).is_some_and(|t| self.ty_holds(t, u, 0))
                    }).collect();
                    by_ty.push(u);
                    bk.masks.push(m);
                    bk.masks.len() - 1
                }
            };
            bk.copy_masks.insert(c, i);
        }
        // (the reborrows the region check found must outlive the function)
        if self.rg.on && !self.rg.rerun && self.rg.closure.is_none() {
            let loan = |p: usize| bk.loan_of.get(&p).or_else(|| bk.recv_loans.get(&p)).copied();
            let mut f: Vec<u32> = self.rg.forever().into_iter().filter_map(loan).collect();
            f.sort_unstable();
            f.dedup();
            let mut r: Vec<u32> = self.rg.forever_returned().into_iter().filter_map(loan).filter(|l| f.binary_search(l).is_err()).collect();
            r.sort_unstable();
            r.dedup();
            bk.forever = f;
            bk.ret_only = r;
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

/// A closure's parameter that a pattern binds from (`__p0` for `|(a, ref b)| ..`, closure_synth), or a place in it
/// (`__p0.0`)
pub(crate) fn synthetic_param(name: &str) -> bool {
    name.strip_prefix("__p").is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()) && r.trim_start_matches(|c: char| c.is_ascii_digit()).chars().next().is_none_or(|c| c == '.'))
}
