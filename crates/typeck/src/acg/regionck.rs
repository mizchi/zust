//! Region checking against signatures (issue #1): NLL's check of universal regions.
//!
//! `Ty` carries no regions. Here, in a function that has regions in its signature, each value of interest gets a
//! *region tree*: one region variable per region slot of its type (see `types::RSigSyn`; a reference's region, then
//! its pointee's; a struct's lifetime parameters, then its type arguments'). Lowering relates the trees where values
//! flow (a `let`, an assignment, a call's arguments and result, the returned value) with outlives constraints
//! `a: b`, following variance. The regions of the signature are universal; the function may only assume the
//! relations among them that its declaration and the types of its parameters imply. When the constraints require
//! `'a: 'b` of two universal regions without that, it is "lifetime may not live long enough".
//!
//! NLL's constraints between regions hold at every point (only liveness is per point), and a universal region holds
//! the whole body, so this check needs no control flow: it is reachability in the constraint graph from each
//! universal region (as rustc's `check_universal_regions`).
//!
//! What is not followed (a coercion that changes the shape of a type, a pattern that binds by parts, a closure, a
//! generic function's instance, a type the summaries cannot map) has no tree, and constraints touching it are left
//! out: an error is reported only when the constraints that are known require it.

use super::{Cx, Loc, Sp};
use crate::fxhash::FxMap as HashMap;
use crate::parse::{Expr, GArg, Stmt, Ty as PTy};
use crate::program::Program;
use crate::types::{RSigSyn, Ty, R_BOUND, R_DYN_ARGS, R_DYN_DEFAULT, R_ELIDED, R_ELIDED_ADT, R_FN, R_STATIC, R_UNKNOWN};

/// Region variables of a value, aligned with the region slots of its type
pub type RT = Vec<u32>;
/// `'static`
pub const STATIC: u32 = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Var {
    Co,
    Contra,
    Inv,
    Bi,
}

impl Var {
    /// The variance of a position inside a position of variance self
    fn then(self, inner: Var) -> Var {
        match (self, inner) {
            (Var::Bi, _) | (_, Var::Bi) => Var::Bi,
            (Var::Inv, _) | (_, Var::Inv) => Var::Inv,
            (Var::Contra, Var::Contra) => Var::Co,
            (Var::Contra, _) | (_, Var::Contra) => Var::Contra,
            _ => Var::Co,
        }
    }
    /// Two uses of one parameter
    fn join(self, o: Var) -> Var {
        match (self, o) {
            (Var::Bi, x) | (x, Var::Bi) => x,
            (a, b) if a == b => a,
            _ => Var::Inv,
        }
    }
}

/// The region check of one function. What it tracks is in `st`, made when the check first writes it: a function the
/// check leaves out (most of them: the gate, generic instances, the prelude) makes and drops none of it
#[derive(Default)]
pub struct Rg {
    pub on: bool,
    /// The function may be checked (its own body, not a closure's), though the gate may have left it out
    pub eligible: bool,
    /// The `let` being lowered: where an error is reported for what has no position of its own (a block)
    pub let_sp: Sp,
    /// The function's own definition (of an instance: the generic one) and an instance's type arguments
    pub base: u32,
    pub inst_args: Vec<Ty>,
    /// A function the gate left out: how its receiver elides (`&self`), and whether another receiver leaves the
    /// result's regions unknown, for a check begun later (rg_late_on)
    pub late: Option<(bool, bool)>,
    /// A closure's body: its region signature (codes CLO_P + i: a region the bound binds, a placeholder; CLO_E + i:
    /// one from outside the closure)
    pub closure: Option<Box<crate::program::CloRg>>,
    /// The check is on from the function's start whatever the gate says (compile's second run); the check began late
    /// (rg_late_on); a local bound before it with no tree was met (the function is compiled again)
    pub force: bool,
    /// The result is `impl Trait + 'static`: its hidden type's regions are 'static whatever the parameters are
    pub static_opaque: bool,
    pub began_late: bool,
    pub rerun: bool,
    /// Nesting of call_fn (this and what follows: where lowering is, written whether the check is on or not)
    pub depth: u32,
    /// The lifetimes a path call writes on its type (`A::<'a>::new(..)`, `<A<'a> as Tr>::f(..)`): (the call's depth
    /// in call_fn, the callee, the lifetimes in the type's order). rg_call gives them to the impl's lifetimes
    pub call_self: Option<(u32, u32, Vec<String>)>,
    /// The receiver of the method call being lowered, as passed, and its position
    pub recv: Option<(RT, Sp)>,
    /// The last scrutinee's tree; the part of it the pattern being bound matches, and the region of the reference
    /// matched through last (bindings in it are by reference)
    pub scrut: Option<(Ty, RT)>,
    pub pat: Option<(Ty, RT)>,
    pub pat_ref: Option<u32>,
    /// The method call being made (call_fn is given no span for it)
    pub call_sp: Sp,
    /// The elements' tree of the array or slice a slice pattern matches
    pub pat_elem: Option<(Ty, RT)>,
    /// The call being made is of a closure made where nothing was expected (by its definition): the regions of its
    /// signature are one for all its calls (clo_shared)
    pub shared_clo: Option<(u32, Option<usize>)>,
    /// The state of the check (RgSt), read through Deref: a function that never writes it reads an empty one
    pub st: Option<Box<RgSt>>,
}

impl std::ops::Deref for Rg {
    type Target = RgSt;
    #[inline]
    fn deref(&self) -> &RgSt {
        match &self.st {
            Some(b) => b,
            None => empty_st(),
        }
    }
}

impl std::ops::DerefMut for Rg {
    #[inline]
    fn deref_mut(&mut self) -> &mut RgSt {
        self.st.get_or_insert_with(Default::default)
    }
}

impl Rg {
    /// Whether the state was made (written)
    pub fn has_st(&self) -> bool {
        self.st.is_some()
    }
}

/// The state of a function whose check wrote nothing (never written)
fn empty_st() -> &'static RgSt {
    static E: std::sync::LazyLock<RgSt> = std::sync::LazyLock::new(RgSt::default);
    &E
}

/// What the region check of a function tracks (Rg::st)
#[derive(Default)]
pub struct RgSt {
    /// A closure's body (Rg::closure is set)
    pub in_closure: bool,
    /// Region variables made (0 is `'static`)
    pub n: u32,
    /// The universal regions (not `'static`), with their names
    pub univ: Vec<(u32, String)>,
    /// The placeholders of the instance's type parameters (zustc check of a generic fn nothing calls), with their
    /// universal regions: a placeholder's region is the same wherever its type is
    pub ph: Vec<(u32, u32)>,
    /// A placeholder's associated type's universal region, and the regions it is of (its placeholder's, its bound's
    /// lifetimes'): it outlives what they all outlive
    pub proj_comps: Vec<(u32, Vec<u32>)>,
    /// Those regions by their placeholder's region and the associated type's name: a signature's `V::Output` is of
    /// the placeholder its type arguments' trees give V (the instance's own `T`, or a callee's `V` that a caller's `U`
    /// fills: `take::<U>(x)` with `fn take<V: Tr>(_: &V::Out)`)
    pub assoc_regs: Vec<(u32, String, u32)>,
    /// A parameter's or the result's type is not followed: what the signature implies is not all known
    pub implied_partial: bool,
    /// The function's where clauses on a type that is no type parameter (`&'max G: Get`): the type, its tree as
    /// written, and the traits it is bounded by
    pub where_trees: Vec<(Ty, RT, Vec<String>)>,
    /// Outlives relations among universal regions the function may assume (a: b)
    pub known: Vec<(u32, u32)>,
    /// Constraints a: b, with where they come from
    pub edges: Vec<(u32, u32, Sp)>,
    pub locals: HashMap<Loc, RT>,
    /// The trees of call results (by the address of the call expression)
    pub calls: HashMap<usize, (Ty, RT)>,
    /// The result of the last call lowered (at the depth of call_fn it was made at)
    pub last_call: Option<(u32, Ty, RT)>,
    /// The tree of the return type
    pub ret: Option<RT>,
    /// `&a` expressions (their addresses) lowered as a deref coercion, with the derefs past a's: the pointer `a: &T`
    /// holds (`*a: &T` of `a: &&T` ..), where a `&T` is expected. Their tree is a's, less those derefs' regions
    pub coerced: Vec<(usize, usize)>,
    /// The function's own lifetime parameters (in `RSigSyn::lts` order)
    pub lts: Vec<u32>,
    pub lt_names: Vec<String>,
    /// Variance of a struct / enum definition: its lifetime parameters, then its type parameters
    pub variance: HashMap<u32, std::sync::Arc<[Var]>>,
    /// The definitions whose variance is being computed; whether a provisional one was read; the ones computed, in
    /// order (see variance)
    pub var_open: Vec<u32>,
    pub var_cycle: bool,
    pub var_log: Vec<u32>,
    /// Borrows of what the function owns (a local, not through a reference): (region, the place's text, where)
    pub loans: Vec<(u32, String, Sp)>,
    /// Instantiating a type written in a body (`let` annotation, cast): a trait object's default lifetime is inferred
    pub dyn_infer: bool,
    /// ZUSTC_RG_DEBUG: the kinds of values not followed
    pub missed: Vec<&'static str>,
    /// A match lowered as another (its or-pattern arms with guards split): the one lowered
    pub alias: HashMap<usize, &'static Expr>,
    /// Values of no region borrowed as temporaries no constant promotion makes static (`&(x + 1)`, `&(5.f())`)
    pub temps: HashMap<usize, Ty>,
    /// Where values are returned (a borrow that escapes there is reported there: E0515)
    pub ret_sps: Vec<Sp>,
    /// The constraints the returned values make (ranges of edges, rg_ret): they hold where the value is returned,
    /// not before (Polonius Alpha's location-sensitive constraints, issue #1). A reborrow that must outlive a universal
    /// region only through them is live while what holds it is, not to the function's end (Rg::forever)
    pub ret_edges: Vec<(usize, usize)>,
    /// A closure's body: the locals that are its captures, the region its environment is borrowed for, the regions
    /// from outside
    pub cap_locs: Vec<Loc>,
    pub env_r: u32,
    pub externals: Vec<u32>,
    /// A closure's body: the regions its codes name (CLO_U: of the function it is in, related as there)
    pub clo_regs: Vec<(u8, u32)>,
    pub outer_univ: Vec<u32>,
    /// CLO_S regions: only borrows of what the closure owns must not reach them
    pub soft: Vec<u32>,
    /// While a callee's fn pointer parameter is instantiated for a fn item passed to it: the region its type binds
    /// (`for<'cx> fn(&'cx S) -> &'static S`) is a placeholder, the same for each it binds (inst)
    pub bound_univ: Option<u32>,
    /// While a fn pointer type's parameters are instantiated: the regions it binds there; while its result is: the
    /// one of them its elided regions are (`fn(&u32) -> &u32` is `for<'x> fn(&'x u32) -> &'x u32`), when there is one
    pub fn_bound: Option<Vec<u32>>,
    pub fn_bound_ret: Option<u32>,
    /// The regions a fn pointer's or fn item's type binds (its own lifetimes, those elided in it, those `for<..>`
    /// binds): a call through a local of that type takes them afresh (rg_fnptr_call)
    pub late_bound: Vec<u32>,
    /// The placeholders of trait obligations' binders (rg_obligation): the variable, the last variable made before
    /// the obligation (those up to it are of the outer universe: none may be the placeholder), where, and the trait
    pub hr: Vec<(u32, u32, Sp, String)>,
    /// Trait obligations whose impl is not general enough as their types show (a fn pointer type's binder against
    /// a region of the impl's that is not bound there): where, and the message
    pub hr_errs: Vec<(Sp, String)>,
    /// Rg::wf: imp's relations are required, at this span (not those of the fields: wf_declared)
    pub wf_sp: Option<Sp>,
    pub wf_no_fields: bool,
    /// An impl's method: the tree of its self type (what `Self { .. }` builds)
    pub self_tree: Option<(Ty, RT)>,
    /// The region of each reborrow through a reference (`&mut *x`, `&v[0]` with v a reference), by the address of
    /// its place: one that must outlive a universal region lives to the function's end (Rg::forever)
    pub reborrows: Vec<(usize, u32)>,
    /// Borrows of places the function owns, as reborrows (Rg::holders only: the region check reports those that
    /// outlive the function)
    pub owned_borrows: Vec<(usize, u32)>,
    /// A closure's body: the regions of its captures of the function's locals (from outside)
    pub cap_regions: Vec<u32>,
    /// What imp finds of a struct's type below its own regions (its type arguments' and fields' relations), as
    /// positions in its tree (u32::MAX: 'static), by (struct, depth, wf_no_fields); None: not by positions alone
    pub imp_cache: HashMap<(u32, u32, bool), Option<Vec<(u32, u32)>>>,
    /// The relations imp_rel makes, while imp computes entries of imp_cache
    pub imp_rec: Vec<Vec<(u32, u32)>>,
    /// imp_cache was taken from Insts (rg_begin), to be put back
    pub imp_taken: bool,
    pub clo_shared: Vec<(u32, Vec<u32>)>,
    /// The trees of the type arguments of such a closure's calls (its parameters' and result's types)
    pub clo_targs: Vec<(u32, Vec<Option<RT>>)>,
}

/// Closure region codes (Rg::closure): a placeholder (a region the expected signature binds), one from outside
pub const CLO_P: u8 = 0;
pub const CLO_E: u8 = 0x40;
/// A region of the function the closure is in (a universal one there): related as that function knows
pub const CLO_U: u8 = 0x80;
/// A region of the result of a closure whose signature may come from an expected type it is coerced to: nothing the
/// closure owns may be borrowed for it, what else it is related to is not known
pub const CLO_S: u8 = 0xC0;

/// How a closure's signature names its regions, while its codes are made
struct CloCodes<'a> {
    /// The lifetimes and type parameters of the callee (from outside the closure)
    outer_lts: &'a [String],
    tparams: &'a [String],
    named: Vec<(String, u8)>,
    np: u8,
    ne: u8,
    /// The code an elided region of the result takes (the one region of the inputs), else fresh
    elided: Option<u8>,
}

impl CloCodes<'_> {
    fn p(&mut self) -> u8 {
        self.np = (self.np + 1).min(0x3F);
        CLO_P + self.np - 1
    }
    fn e(&mut self) -> u8 {
        self.ne = (self.ne + 1).min(0x3F);
        CLO_E + self.ne - 1
    }
    fn named(&mut self, l: &str) -> u8 {
        match l {
            "'static" => R_STATIC,
            "'_" => self.p(),
            l => {
                if let Some(&(_, c)) = self.named.iter().find(|x| x.0 == l) {
                    return c;
                }
                let c = if self.outer_lts.iter().any(|x| x == l) { self.e() } else { self.p() };
                self.named.push((l.to_string(), c));
                c
            }
        }
    }
    fn elided(&mut self) -> u8 {
        match self.elided {
            Some(c) => c,
            None => self.p(),
        }
    }

    /// The codes of a value of type t written pt; false: not followed
    fn of(&mut self, prog: &Program, pt: &PTy, t: Ty, out: &mut Vec<u8>, depth: u32) -> bool {
        if depth > 12 {
            return false;
        }
        let fresh = |out: &mut Vec<u8>| match count(prog, t, 0) {
            Some(n) => {
                out.extend(std::iter::repeat_n(R_ELIDED, n));
                true
            }
            None => false,
        };
        match (pt, t) {
            (PTy::Ref(l, _, pi), Ty::Ref(_, ti)) => {
                let c = match l {
                    None => self.elided(),
                    Some(l) => self.named(l),
                };
                out.push(c);
                self.of(prog, pi, *ti, out, depth + 1)
            }
            (PTy::Ptr(_, pi), Ty::Ptr(_, ti)) | (PTy::Slice(pi), Ty::Slice(ti)) | (PTy::Array(pi, _), Ty::Array(ti, _)) => self.of(prog, pi, *ti, out, depth + 1),
            (PTy::Path(None, p), Ty::Box(ti)) if p.segs.last().is_some_and(|s| s.name == "Box") => match p.segs.last().and_then(|s| s.args.iter().find_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None })) {
                Some(pi) => self.of(prog, pi, *ti, out, depth + 1),
                None => fresh(out),
            },
            // (the callee's type parameter: what it is instantiated with comes from outside)
            (PTy::Path(None, p), _) if p.segs.len() == 1 && p.segs[0].args.is_empty() && self.tparams.iter().any(|x| *x == p.segs[0].name) => match count(prog, t, 0) {
                Some(n) => {
                    for _ in 0..n {
                        let c = self.e();
                        out.push(c);
                    }
                    true
                }
                None => false,
            },
            (PTy::Tuple(ps), Ty::Struct(sid)) if prog.is_tuple(sid) && prog.structs[sid as usize].args.len() == ps.len() => {
                let args = prog.structs[sid as usize].args.clone();
                ps.iter().zip(args).all(|(p, a)| self.of(prog, p, a, out, depth + 1))
            }
            (PTy::Path(None, p), Ty::Struct(sid)) if !is_closure(prog, sid) && p.segs.last().is_some_and(|s| s.name == prog.structs[prog.base_of(sid) as usize].name) => {
                let Some(last) = p.segs.last() else { return false };
                let n = lts_of(prog, prog.base_of(sid));
                let written: Vec<&String> = last.args.iter().filter_map(|a| if let GArg::Lifetime(l) = a { Some(l) } else { None }).collect();
                if written.len() == n {
                    for l in written {
                        let c = self.named(l);
                        out.push(c);
                    }
                } else if written.is_empty() {
                    for _ in 0..n {
                        let c = self.elided();
                        out.push(c);
                    }
                } else {
                    return false;
                }
                let tys: Vec<&PTy> = last.args.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None }).collect();
                let args = prog.structs[sid as usize].args.clone();
                if tys.len() != args.len() {
                    return args.into_iter().all(|a| match count(prog, a, 0) {
                        Some(k) => {
                            out.extend(std::iter::repeat_n(R_ELIDED, k));
                            true
                        }
                        None => false,
                    });
                }
                tys.into_iter().zip(args).all(|(p, a)| self.of(prog, p, a, out, depth + 1))
            }
            _ => fresh(out),
        }
    }
}

/// t holds a trait object (whose lifetime a signature may leave to the default)
pub fn has_dyn(prog: &Program, t: Ty, depth: u32) -> bool {
    match t {
        Ty::Dyn(_) => true,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => has_dyn(prog, *i, depth + 1),
        Ty::Struct(sid) if depth < 4 => prog.structs[sid as usize].args.iter().any(|&a| has_dyn(prog, a, depth + 1)),
        _ => false,
    }
}

fn kind(e: &Expr) -> &'static str {
    match e {
        Expr::Paren(x) => kind(x),
        Expr::Lit(..) => "lit",
        Expr::Path(..) => "path",
        Expr::Unary(..) => "unary",
        Expr::Binary(..) => "binary",
        Expr::Call(..) => "call",
        Expr::MethodCall(..) => "method",
        Expr::Field(..) => "field",
        Expr::Index(..) => "index",
        Expr::Ref(..) => "ref",
        Expr::Cast(..) => "cast",
        Expr::Block(..) => "block",
        Expr::If(..) => "if",
        Expr::Match(..) => "match",
        Expr::Closure(..) => "closure",
        Expr::Tuple(..) => "tuple",
        Expr::Array(..) => "array",
        Expr::Struct(..) => "struct",
        Expr::Macro(..) => "macro",
        Expr::Try(..) => "try",
        _ => "other",
    }
}

/// t is a closure that captures what holds regions, or holds one (in a struct's type arguments, a box, a reference)
fn holds_closure(prog: &Program, t: Ty, depth: u32) -> bool {
    depth < 8
        && match t {
            Ty::Struct(sid) if is_closure(prog, sid) => captures(prog, sid).is_none_or(|(ts, moved)| ts.iter().enumerate().any(|(i, &c)| !moved.get(i).copied().unwrap_or(false) || count(prog, c, 0) != Some(0))),
            Ty::Struct(sid) => prog.structs[sid as usize].args.iter().any(|&a| holds_closure(prog, a, depth + 1)),
            Ty::Ref(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Ptr(_, i) => holds_closure(prog, *i, depth + 1),
            _ => false,
        }
}

pub fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZUSTC_RG_DEBUG").is_some())
}

/// ZUSTC_NO_REGIONCK=1 turns the check off (read once: it is asked for each function)
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZUSTC_NO_REGIONCK").is_none())
}

/// t ends in a trait object (`dyn Tr`, `S<dyn Tr>` with the object its last type argument): the last region of its
/// tree is the object's
fn dyn_tail(prog: &Program, t: Ty, depth: u32) -> bool {
    match t {
        Ty::Dyn(_) => true,
        Ty::Struct(sid) if depth < 8 && !is_closure(prog, sid) => prog.structs[sid as usize].args.last().is_some_and(|&a| dyn_tail(prog, a, depth + 1)),
        _ => false,
    }
}

fn lts_of(prog: &Program, base: u32) -> usize {
    match prog.adt_regions.get(&base) {
        Some(r) => r.lts.len(),
        // (a type parameter's placeholder: one region, the parameter's (what its values may borrow))
        None => prog.is_placeholder(base) as usize,
    }
}

fn is_closure(prog: &Program, sid: u32) -> bool {
    prog.closure_of_struct.contains_key(&prog.base_of(sid))
}

/// A struct's definition, its base (an instance's generic one) and whether it is a tuple, with one lock of the
/// table (an instance of a tuple is named `(A, B)`, the generic one `#tupleN`)
#[inline]
fn adt(prog: &Program, sid: u32) -> (&crate::program::StructDef, u32, bool) {
    let sd = &prog.structs[sid as usize];
    let tuple = sd.name.starts_with('(') || sd.name.starts_with("#tuple");
    (sd, sd.base.unwrap_or(sid), tuple)
}

/// rg_closure_ann_bound: of each parameter the closure annotates with one of the function's lifetimes or 'static,
/// each region the bound (or the fn pointer type) binds (codes below CLO_E) is a placeholder that lifetime would have
/// to be; a region the type names (tgts, where known: `fn(&'a u32)`) must outlive the annotation's, as the closure
/// is to take what the type gives (`|_: &'static u32|` as `fn(&'a u32)` needs 'a: 'static)
fn closure_ann_bound(prog: &Program, rg: &mut RgSt, c: &Expr, codes: &[Vec<u8>], tgts: &[Vec<Option<u32>>], params: &[Ty]) {
    let Expr::Closure(_, inputs, _, _, pos) = c else { return };
    for (i, (_, ann)) in inputs.iter().enumerate() {
        let (Some(ann), Some(bc), Some(&t)) = (ann, codes.get(i), params.get(i)) else { continue };
        if !crate::regions::names_lt(ann) {
            continue;
        }
        let outer = rg.lt_names.clone();
        let mut ac = CloCodes { outer_lts: &outer, tparams: &[], named: vec![], np: 0, ne: 0, elided: None };
        let mut av = vec![];
        if !ac.of(prog, ann, t, &mut av, 0) || av.len() != bc.len() {
            continue;
        }
        let tg = tgts.get(i).filter(|g| g.len() == bc.len());
        for (j, (&b, &a)) in bc.iter().zip(&av).enumerate() {
            // (the annotation's a function's lifetime or 'static)
            let lt = match a {
                R_STATIC => STATIC,
                a if (CLO_E..CLO_U).contains(&a) => match ac.named.iter().find(|x| x.1 == a).and_then(|x| outer.iter().position(|n| *n == x.0)) {
                    Some(k) => rg.lts[k],
                    None => continue,
                },
                _ => continue,
            };
            if b >= CLO_E {
                if let Some(Some(r)) = tg.and_then(|g| g.get(j)) {
                    rg.edge(*r, lt, *pos);
                }
                continue;
            }
            // (the bound's own region, a placeholder)
            let u = rg.var();
            rg.univ.push((u, "'_".to_string()));
            rg.edge(u, lt, *pos);
            rg.edge(lt, u, *pos);
        }
    }
}

/// The parts of a tree (of type t) where pat (an impl's self type, its parameters Params) has its lifetimes (their
/// codes ss, in the order of the tree) and its type parameters: into lts and targs. None where the two do not match
fn proj_split(prog: &Program, pat: Ty, t: Ty, tree: &[u32], at: &mut usize, ss: &[u8], si: &mut usize, lts: &mut [Option<u32>], targs: &mut [Option<RT>], depth: u32) -> Option<()> {
    if depth > 24 {
        return None;
    }
    let mut region = |at: &mut usize, si: &mut usize, lts: &mut [Option<u32>]| -> Option<()> {
        let (&c, &v) = (ss.get(*si)?, tree.get(*at)?);
        if (c as usize) < lts.len() {
            lts[c as usize] = Some(v);
        }
        *si += 1;
        *at += 1;
        Some(())
    };
    match (pat, t) {
        (Ty::Param(k), _) => {
            let n = count(prog, t, depth + 1)?;
            let part = tree.get(*at..*at + n)?.to_vec();
            if let Some(slot) = targs.get_mut(k as usize) {
                *slot = Some(part);
            }
            *at += n;
        }
        (Ty::Ref(m1, p), Ty::Ref(m2, i)) if m1 == m2 => {
            region(at, si, lts)?;
            proj_split(prog, *p, *i, tree, at, ss, si, lts, targs, depth + 1)?;
        }
        (Ty::Ptr(m1, p), Ty::Ptr(m2, i)) if m1 == m2 => proj_split(prog, *p, *i, tree, at, ss, si, lts, targs, depth + 1)?,
        (Ty::Box(p), Ty::Box(i)) | (Ty::Slice(p), Ty::Slice(i)) => proj_split(prog, *p, *i, tree, at, ss, si, lts, targs, depth + 1)?,
        (Ty::Array(p, n1), Ty::Array(i, n2)) if n1 == n2 => proj_split(prog, *p, *i, tree, at, ss, si, lts, targs, depth + 1)?,
        (Ty::Struct(ps), Ty::Struct(ts)) => {
            let ((pd, pb, _), (td, tb, _)) = (adt(prog, ps), adt(prog, ts));
            if pb != tb || prog.closure_of_struct.contains_key(&pb) || pd.args.len() != td.args.len() {
                return None;
            }
            for _ in 0..lts_of(prog, pb) {
                region(at, si, lts)?;
            }
            for (&pa, &ta) in pd.args.iter().zip(&td.args) {
                proj_split(prog, pa, ta, tree, at, ss, si, lts, targs, depth + 1)?;
            }
        }
        (Ty::Dyn(a), Ty::Dyn(b)) if a == b => region(at, si, lts)?,
        // (a fn pointer: its parameters' regions, then its result's (`impl<'a> Foo for fn(&'a ())`))
        (Ty::Fn(pf), Ty::Fn(tf)) if pf.params.len() == tf.params.len() => {
            for (&pp, &tp) in pf.params.iter().zip(&tf.params) {
                proj_split(prog, pp, tp, tree, at, ss, si, lts, targs, depth + 1)?;
            }
            proj_split(prog, pf.ret, tf.ret, tree, at, ss, si, lts, targs, depth + 1)?;
        }
        _ if pat == t && count(prog, t, depth + 1) == Some(0) => {}
        _ => return None,
    }
    Some(())
}

/// The lifetimes a `for<..>` in a bound's text binds (`Get<for<'b> fn(&'b u32)>`: `'b`): those of a type within the
/// bound, not the bound's own. `for` is the keyword only where no identifier goes on before it (`Bufferfor<'a>` is a
/// type's name)
pub fn nested_binder_lts(b: &str) -> Vec<&str> {
    let mut nested = vec![];
    let mut from = 0;
    while let Some(i) = b[from..].find("for").map(|i| from + i) {
        from = i + 3;
        if b[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let after = b[from..].trim_start();
        if let (true, Some(j)) = (after.starts_with('<'), after.find('>')) {
            nested.extend(after[1..j].split(',').map(|l| l.trim()).filter(|l| l.starts_with('\'')));
        }
    }
    nested
}

/// The type arguments the bound of placeholder sid on trait tr writes (`FnLike<&'a isize, u8>`), and the lifetimes
/// they name
fn bound_targ_ptys(prog: &Program, sid: u32, tr: &str) -> Option<(Vec<PTy>, Vec<String>)> {
    let bs = prog.structs[sid as usize].placeholder.as_ref()?;
    let short = tr.split('<').next().unwrap_or(tr).trim();
    let short = short.rsplit("::").next().unwrap_or(short);
    // (one bound of the trait: two (`F: Take<&'a u32> + Take<&'b u64>`) leave which one a call takes to its types)
    let mut of = bs.iter().filter(|b| {
        let n = b.split('<').next().unwrap_or(b).trim();
        n == short || n.rsplit("::").next() == Some(short)
    });
    let b = of.next()?;
    if of.next().is_some() || !b.contains('\'') {
        return None;
    }
    let f = crate::parse::parse_file(&format!("type __B = dyn {b};")).ok()?;
    let Some(crate::parse::ItemKind::Type(_, _, _, Some(PTy::Dyn(dbs)))) = f.items.first().map(|i| &i.kind) else { return None };
    let Some(crate::parse::Bound::Trait(_, p)) = dbs.first() else { return None };
    let args: Vec<PTy> = p.segs.last()?.args.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t.clone()) } else { None }).collect();
    // (the bound's own `for<'a>` is not in its text; a `for<'b>` there is a type's within it (`Get<for<'b> fn(&'b u32)>`),
    // whose lifetimes are that type's own: not the bound's)
    let nested = nested_binder_lts(b);
    let mut lts: Vec<String> = vec![];
    let cs: Vec<char> = b.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '\'' {
            let mut j = i + 1;
            while j < cs.len() && (cs[j].is_alphanumeric() || cs[j] == '_') {
                j += 1;
            }
            let l: String = cs[i..j].iter().collect();
            if l.len() > 1 && l != "'static" && l != "'_" && !lts.contains(&l) && !nested.contains(&l.as_str()) {
                lts.push(l);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    Some((args, lts))
}

/// The type parameters t has within a fn pointer type (`fn(A)`: A); `inside`: within one
fn params_in_fn(prog: &Program, t: Ty, inside: bool, out: &mut [bool], depth: u32) {
    if depth > 24 {
        return;
    }
    match t {
        Ty::Param(i) if inside => {
            if let Some(x) = out.get_mut(i as usize) {
                *x = true;
            }
        }
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => params_in_fn(prog, *i, inside, out, depth + 1),
        Ty::Struct(s) => {
            for &a in &prog.structs[s as usize].args {
                params_in_fn(prog, a, inside, out, depth + 1);
            }
        }
        Ty::Fn(ft) => {
            for &p in ft.params.iter().chain(std::iter::once(&ft.ret)) {
                params_in_fn(prog, p, true, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// A bound's trait, by its last name (`a::Foo<&'x u8>`: `Foo`)
pub(crate) fn bound_trait(b: &str) -> &str {
    let n = b.split('<').next().unwrap_or(b).split('(').next().unwrap_or(b).trim();
    n.rsplit("::").next().unwrap_or(n)
}

/// The lifetimes a bound names, in order, but `'static`, `'_` and those a type within it binds (`for<'b> fn(&'b u32)`)
fn bound_names(b: &str) -> Vec<String> {
    let nested = nested_binder_lts(b);
    let mut lts: Vec<String> = vec![];
    let cs: Vec<char> = b.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '\'' {
            let mut j = i + 1;
            while j < cs.len() && (cs[j].is_alphanumeric() || cs[j] == '_') {
                j += 1;
            }
            let l: String = cs[i..j].iter().collect();
            if l.len() > 1 && l != "'static" && l != "'_" && !lts.contains(&l) && !nested.contains(&l.as_str()) {
                lts.push(l);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    lts
}

/// The type arguments a bound's trait path writes, parsed (`Foo<&'a isize>`: `&'a isize`)
fn bound_type_args(b: &str) -> Option<Vec<PTy>> {
    let f = crate::parse::parse_file(&format!("type __B = dyn {b};")).ok()?;
    let Some(crate::parse::ItemKind::Type(_, _, _, Some(PTy::Dyn(dbs)))) = f.items.first().map(|i| &i.kind) else { return None };
    let Some(crate::parse::Bound::Trait(_, p)) = dbs.first() else { return None };
    Some(p.segs.last()?.args.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t.clone()) } else { None }).collect())
}

/// The mark of a universal region that is a placeholder of a trait obligation's binder, before the trait's name
pub const HR_MARK: char = '\u{1}';

/// A struct or an enum in t (under references, boxes, slices, arrays, type arguments) whose definition bounds a type
/// parameter by 'static (`struct Static<T: 'static>(T)`)
/// t has a reference to a type that holds a placeholder (`&'x T#`, `Vec<&'x <T# as Tr>::Out#>`); `under` is whether
/// a reference is around t
fn refs_placeholder(prog: &Program, t: Ty, under: bool, depth: u32) -> bool {
    if depth > 16 {
        return false;
    }
    match t {
        Ty::Ref(_, i) => refs_placeholder(prog, *i, true, depth + 1),
        Ty::Ptr(_, i) | Ty::Box(i) | Ty::Slice(i) | Ty::Array(i, _) => refs_placeholder(prog, *i, under, depth + 1),
        Ty::Struct(s) if prog.is_placeholder(s) => under,
        Ty::Struct(s) => prog.structs[s as usize].args.iter().any(|&a| refs_placeholder(prog, a, under, depth + 1)),
        _ => false,
    }
}

fn has_static_bound_adt(prog: &Program, t: Ty, depth: u32) -> bool {
    if depth > 16 {
        return false;
    }
    match t {
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Slice(i) | Ty::Array(i, _) => has_static_bound_adt(prog, *i, depth + 1),
        Ty::Struct(s) => {
            let b = prog.base_of(s);
            prog.adt_regions.get(&b).is_some_and(|r| r.ty_outlives.iter().any(|&(_, c)| c == R_STATIC)) || prog.structs[s as usize].args.iter().any(|&a| has_static_bound_adt(prog, a, depth + 1))
        }
        _ => false,
    }
}

/// A placeholder instance (zustc check of a generic fn nothing calls) in a program one of whose own structs or enums
/// bounds a type parameter by 'static (`struct Static<T: 'static>(T)`): its types' well-formedness may need T: 'static
/// (`None::<Static<T>>`, E0310), so its region check is on with the placeholder's region alone
pub(crate) fn placeholder_static_bounds(prog: &Program, inst_args: &[Ty]) -> bool {
    inst_args.iter().any(|&t| matches!(t, Ty::Struct(s) if prog.is_placeholder(s)))
        && prog.adt_regions.iter().any(|(&b, r)| r.ty_outlives.iter().any(|&(_, c)| c == R_STATIC) && prog.struct_item.get(b as usize).is_some_and(|&i| i < prog.prelude_from))
}

/// A closure's captures: their types (among its type arguments: a capture by reference is a pointer to it) and
/// whether each is held by value
fn captures(prog: &Program, sid: u32) -> Option<(&[Ty], &[bool])> {
    let cd = prog.closures.get(prog.closure_of_struct.get(&prog.base_of(sid))?)?;
    let args = prog.structs[sid as usize].args.get(cd.n_parent..cd.n_parent + cd.captures.len())?;
    Some((args, &cd.moved))
}

/// The type parameters of a generic type g that a type c of the same shape instantiates
fn bind_params(prog: &Program, g: Ty, c: Ty, sol: &mut [Option<Ty>], depth: u32) {
    if depth > 12 {
        return;
    }
    match (g, c) {
        (Ty::Param(k), c) => {
            if let Some(s @ None) = sol.get_mut(k as usize) {
                *s = Some(c);
            }
        }
        (Ty::Ref(_, a), Ty::Ref(_, b)) | (Ty::Ptr(_, a), Ty::Ptr(_, b)) | (Ty::Box(a), Ty::Box(b)) | (Ty::Slice(a), Ty::Slice(b)) | (Ty::Array(a, _), Ty::Array(b, _)) => bind_params(prog, *a, *b, sol, depth + 1),
        (Ty::Struct(x), Ty::Struct(y)) if x != y && prog.base_of(x) == prog.base_of(y) => {
            let (xa, ya) = (prog.structs[x as usize].args.clone(), prog.structs[y as usize].args.clone());
            for (a, b) in xa.into_iter().zip(ya) {
                bind_params(prog, a, b, sol, depth + 1);
            }
        }
        _ => {}
    }
}

/// The lifetimes a bound written as a string passes its trait (`GetRef<'a, isize>`: `'a`), in order
fn bound_lts(b: &str) -> Vec<String> {
    let (Some(i), Some(j)) = (b.find('<'), b.rfind('>')) else { return vec![] };
    let Some(inner) = b.get(i + 1..j) else { return vec![] };
    let (mut out, mut depth, mut from) = (vec![], 0i32, 0);
    for (k, ch) in inner.char_indices().chain(std::iter::once((inner.len(), ','))) {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                let a = inner[from..k].trim();
                if a.starts_with('\'') {
                    out.push(a.to_string());
                }
                from = k + 1;
            }
            _ => {}
        }
    }
    out
}

/// The arguments a bound written as a string passes its trait (`Get<&'a i32, T>`: `&'a i32`, `T`), in order
fn bound_args(b: &str) -> Vec<String> {
    let (Some(i), Some(j)) = (b.find('<'), b.rfind('>')) else { return vec![] };
    let Some(inner) = b.get(i + 1..j) else { return vec![] };
    let (mut out, mut depth, mut from) = (vec![], 0i32, 0);
    for (k, ch) in inner.char_indices().chain(std::iter::once((inner.len(), ','))) {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                out.push(inner[from..k].trim().to_string());
                from = k + 1;
            }
            _ => {}
        }
    }
    out
}

/// The lifetimes a type written as a string names, in the order of its region slots, when it writes all of them:
/// references and generic arguments only (`&'a Foo<'b>`: `'a`, `'b`); None for an elided one (`&u8`, `'_`), a trait
/// object, an `impl` type or a fn pointer, whose regions are not in written order
fn written_lts(t: &str) -> Option<Vec<String>> {
    let b = t.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'&' => {
                let rest = t[i + 1..].trim_start();
                if !rest.starts_with('\'') {
                    return None;
                }
            }
            b'\'' => {
                let mut j = i + 1;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                let l = &t[i..j];
                if l == "'_" {
                    return None;
                }
                out.push(l.to_string());
                i = j;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if ["dyn", "impl", "fn", "Fn"].iter().any(|k| t.split(|c: char| !c.is_alphanumeric() && c != '_').any(|w| w == *k || w.starts_with("Fn"))) {
        return None;
    }
    Some(out)
}

/// The number of region slots of t (None: not followed)
pub fn count(prog: &Program, t: Ty, depth: u32) -> Option<usize> {
    if depth > 24 {
        return None;
    }
    Some(match t {
        Ty::Ref(_, i) => 1 + count(prog, *i, depth + 1)?,
        Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => count(prog, *i, depth + 1)?,
        Ty::Struct(sid) => {
            let (sd, base, _) = adt(prog, sid);
            // (a closure: the regions of what it captures, a capture by reference the borrow's region first)
            if prog.closure_of_struct.contains_key(&base) {
                let (ts, moved) = captures(prog, sid)?;
                let mut n = 0;
                for (j, &a) in ts.iter().enumerate() {
                    n += match (moved.get(j).copied().unwrap_or(false), a) {
                        (false, Ty::Ptr(_, i)) => 1 + count(prog, *i, depth + 1)?,
                        _ => count(prog, a, depth + 1)?,
                    };
                }
                return Some(n);
            }
            let mut n = lts_of(prog, base);
            for &a in &sd.args {
                n += count(prog, a, depth + 1)?;
            }
            n
        }
        Ty::Dyn(_) => 1,
        // (a fn pointer: its parameters' regions, then its result's)
        Ty::Fn(ft) => {
            let mut n = count(prog, ft.ret, depth + 1)?;
            for &p in &ft.params {
                n += count(prog, p, depth + 1)?;
            }
            n
        }
        Ty::Assoc(_) => return None,
        _ => 0,
    })
}

/// A deref of a reference of region r on the way to a place: a reborrow of the place lives no longer than r. Through
/// a shared reference, no longer than it alone (NLL's supporting prefixes stop there: `&**p` with p: &mut &'y T lives
/// as long as 'y)
fn deref(ds: &mut Vec<u32>, r: u32, mutable: bool) {
    if !mutable {
        ds.clear();
    }
    ds.push(r);
}

/// Where e is written: borrowck's position, else that of the path it calls or of its receiver
fn epos(e: &Expr) -> Sp {
    let p = super::borrowck::pos(e);
    if p.line != 0 {
        return p;
    }
    match strip(e) {
        Expr::Call(f, _) => epos(f),
        Expr::MethodCall(r, ..) => epos(r),
        Expr::Path(_, p) | Expr::Struct(p, ..) => p.pos,
        Expr::Block(_, _, b) => match b.stmts.last() {
            Some(Stmt::Expr(t, _)) => epos(t),
            _ => p,
        },
        _ => p,
    }
}

/// A closure body's value (its tail, the branches of an `if` / `match` there, what a `return` returns) that is a
/// mutable borrow of a place rooted at one of caps: its position
pub fn fnmut_escape(body: &Expr, caps: &[&str]) -> Option<Sp> {
    if caps.is_empty() {
        return None;
    }
    fn root(e: &Expr) -> Option<&str> {
        match strip(e) {
            Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => Some(p.segs[0].name.as_str()),
            Expr::Field(x, _, _) | Expr::Index(x, _, _) => root(x),
            Expr::Unary(op, x, _) if op == "*" => root(x),
            _ => None,
        }
    }
    fn value(e: &Expr, caps: &[&str], depth: u32) -> Option<Sp> {
        if depth > 16 {
            return None;
        }
        match strip(e) {
            Expr::Ref(true, x) if root(x).is_some_and(|r| caps.contains(&r)) => Some(super::borrowck::pos(e)).filter(|s| s.line != 0).or_else(|| Some(epos(x))),
            Expr::Block(_, _, b) => {
                for st in &b.stmts {
                    if let Some(sp) = returns(st, caps, depth + 1) {
                        return Some(sp);
                    }
                }
                match b.stmts.last() {
                    Some(Stmt::Expr(t, false)) => value(t, caps, depth + 1),
                    _ => None,
                }
            }
            Expr::If(_, b, els) => {
                let t = match b.stmts.last() {
                    Some(Stmt::Expr(t, false)) => value(t, caps, depth + 1),
                    _ => None,
                };
                t.or_else(|| els.as_deref().and_then(|e| value(e, caps, depth + 1)))
            }
            Expr::Match(_, arms) => arms.iter().find_map(|a| value(&a.body, caps, depth + 1)),
            _ => None,
        }
    }
    // (a `return` among a block's statements: only as a statement of its own, not inside nested closures)
    fn returns(st: &Stmt, caps: &[&str], depth: u32) -> Option<Sp> {
        match st {
            Stmt::Expr(e, _) => match strip(e) {
                Expr::Return(Some(v), _) => value(v, caps, depth + 1),
                _ => None,
            },
            _ => None,
        }
    }
    value(body, caps, 0)
}

/// e has no value: it panics, returns, breaks or continues (as written)
fn diverges(e: &Expr) -> bool {
    match strip(e) {
        Expr::Return(..) | Expr::Break(..) | Expr::Continue(..) => true,
        Expr::Macro(p, _, _) => p.segs.last().is_some_and(|s| matches!(s.name.as_str(), "panic" | "unreachable" | "todo" | "unimplemented")),
        Expr::Block(_, _, b) => match b.stmts.last() {
            Some(Stmt::Expr(t, _)) => diverges(t),
            _ => false,
        },
        _ => false,
    }
}

fn recv_sp(r: &Option<Sp>, or: Sp) -> Sp {
    r.unwrap_or(or)
}

/// A requirement a closure's body writes on one of its parameters (closure_reqs)
pub(crate) struct ClosureReq<'e> {
    /// the parameter's type argument (the closure's)
    pub param: usize,
    /// a `let`'s annotation, the parameter its initializer (`let y: &'static u32 = x;`)
    pub ann: Option<&'e PTy>,
    /// or a call whose turbofish writes a lifetime (`W::<&'static u32>(x)`), the parameter its argument `arg`
    pub call: Option<&'e Expr>,
    pub arg: usize,
    pub sp: Sp,
}

/// The requirements a closure's body (its statements and its value, not nested blocks) writes on its parameters
/// (params: by name, their type arguments), as rustc propagates a closure's region requirements to its calls
pub(crate) fn closure_reqs<'e>(body: &'e Expr, params: &[(String, usize)], out: &mut Vec<ClosureReq<'e>>) {
    if params.is_empty() {
        return;
    }
    // (the parameters still in scope: a `let` that binds a name shadows it for the statements after it)
    fn param(live: &[(String, usize)], e: &Expr) -> Option<usize> {
        match strip(e) {
            Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => live.iter().find(|x| x.0 == p.segs[0].name).map(|x| x.1),
            _ => None,
        }
    }
    fn call<'e>(live: &[(String, usize)], e: &'e Expr, out: &mut Vec<ClosureReq<'e>>) {
        // (a fn's or a constructor's turbofish, or a method's: `S.take::<&'static u32>(x)`)
        let (gs, a): (&[GArg], &[Expr]) = match strip(e) {
            Expr::Call(f, a) => match strip(f) {
                Expr::Path(None, p) => match p.segs.last() {
                    Some(sg) => (&sg.args, a),
                    None => return,
                },
                _ => return,
            },
            Expr::MethodCall(_, _, gs, a, _) => (gs, a),
            _ => return,
        };
        let writes = gs.iter().any(|g| matches!(g, GArg::Ty(t) if crate::regions::names_lt(t)));
        if !writes {
            return;
        }
        for (j, x) in a.iter().enumerate() {
            if let Some(g) = param(live, x) {
                out.push(ClosureReq { param: g, ann: None, call: Some(e), arg: j, sp: super::borrowck::pos(x) });
            }
        }
    }
    let mut live: Vec<(String, usize)> = params.to_vec();
    let Expr::Block(_, _, b) = strip(body) else {
        call(&live, body, out);
        return;
    };
    for st in &b.stmts {
        match st {
            Stmt::Let(q, t, init, _, _) => {
                if let Some(init) = init {
                    match (t, param(&live, init)) {
                        (Some(t), Some(g)) if crate::regions::names_lt(t) => out.push(ClosureReq { param: g, ann: Some(t), call: None, arg: 0, sp: super::borrowck::pos(init) }),
                        _ => call(&live, init, out),
                    }
                }
                // (`let x = &C;`: the x after it is that local, not the parameter)
                live.retain(|x| !super::borrowck::pat_binds(q, &x.0));
            }
            Stmt::Expr(e, _) => call(&live, e, out),
            _ => {}
        }
    }
}

fn strip(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(x) => strip(x),
        e => e,
    }
}

/// `x`, `x.f`: the text of a place for messages
fn place_text(e: &Expr) -> Option<String> {
    match strip(e) {
        Expr::Path(None, p) if p.segs.len() == 1 => Some(p.segs[0].name.clone()),
        Expr::Field(x, f, _) => Some(format!("{}.{f}", place_text(x)?)),
        Expr::Index(x, _, _) => place_text(x),
        // (`*b` of a Box the function owns)
        Expr::Unary(op, x, _) if op == "*" => Some(format!("*{}", place_text(x)?)),
        _ => None,
    }
}

/// Reads region codes along a type (see `instantiate`)
struct Codes<'a> {
    codes: Option<&'a [u8]>,
    pos: usize,
    ok: bool,
    /// The region of the reference directly around the type being read (a trait object's default lifetime)
    parent: Option<u32>,
}

impl Codes<'_> {
    fn next(&mut self) -> u8 {
        match self.codes {
            None => R_ELIDED,
            Some(c) => match c.get(self.pos) {
                Some(&x) => {
                    self.pos += 1;
                    x
                }
                None => {
                    self.ok = false;
                    R_UNKNOWN
                }
            },
        }
    }
    /// After a trait object's lifetime: the lifetimes its trait's arguments name
    fn dyn_args(&mut self) -> &[u8] {
        let Some(c) = self.codes else { return &[] };
        if c.get(self.pos) != Some(&R_DYN_ARGS) {
            return &[];
        }
        let n = c.get(self.pos + 1).copied().unwrap_or(0) as usize;
        let from = (self.pos + 2).min(c.len());
        let to = (from + n).min(c.len());
        self.pos = to;
        &c[from..to]
    }
    /// A path type's marker for lifetimes not written (`Vec<T>`)
    fn elided_adt(&mut self) -> bool {
        match self.codes {
            None => true,
            Some(c) if c.get(self.pos) == Some(&R_ELIDED_ADT) => {
                self.pos += 1;
                true
            }
            Some(_) => false,
        }
    }
}

impl RgSt {
    pub fn var(&mut self) -> u32 {
        self.n += 1;
        self.n
    }

    /// a: b
    /// The region a closure's code names (Rg::closure): a placeholder, one from outside, or one of the function it is
    /// in (all universal, made once each)
    pub fn clo_reg(&mut self, c: u8) -> u32 {
        if c == R_STATIC {
            return STATIC;
        }
        if c == R_ELIDED {
            return self.var();
        }
        if let Some(&(_, v)) = self.clo_regs.iter().find(|x| x.0 == c) {
            return v;
        }
        let v = self.var();
        self.univ.push((v, "'_".to_string()));
        if (CLO_S..R_STATIC).contains(&c) {
            self.soft.push(v);
        } else if c >= CLO_U {
            self.outer_univ.push(v);
        } else if c >= CLO_E {
            self.externals.push(v);
        }
        self.clo_regs.push((c, v));
        v
    }

    pub fn edge(&mut self, a: u32, b: u32, sp: Sp) {
        if a != b && a != STATIC {
            self.edges.push((a, b, sp));
        }
    }

    fn constrain(&mut self, a: u32, b: u32, v: Var, sp: Sp) {
        match v {
            Var::Co => self.edge(a, b, sp),
            Var::Contra => self.edge(b, a, sp),
            Var::Inv => {
                self.edge(a, b, sp);
                self.edge(b, a, sp);
            }
            Var::Bi => {}
        }
    }

    /// A tree of fresh variables for t
    pub fn fresh(&mut self, prog: &Program, t: Ty) -> Option<RT> {
        if !self.ph.is_empty() {
            let mut out = vec![];
            return self.fresh_ph(prog, t, &mut out, 0).map(|_| out);
        }
        let n = count(prog, t, 0)?;
        Some((0..n).map(|_| self.var()).collect())
    }

    /// fresh, with the placeholders' regions where their types are (in count's order)
    fn fresh_ph(&mut self, prog: &Program, t: Ty, out: &mut RT, depth: u32) -> Option<()> {
        if depth > 24 {
            return None;
        }
        match t {
            Ty::Ref(_, i) => {
                let v = self.var();
                out.push(v);
                self.fresh_ph(prog, *i, out, depth + 1)?;
            }
            Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.fresh_ph(prog, *i, out, depth + 1)?,
            Ty::Struct(sid) => {
                if let Some(&(_, v)) = self.ph.iter().find(|x| x.0 == sid) {
                    out.push(v);
                    return Some(());
                }
                let (sd, base, _) = adt(prog, sid);
                if prog.closure_of_struct.contains_key(&base) {
                    for _ in 0..count(prog, t, depth)? {
                        let v = self.var();
                        out.push(v);
                    }
                    return Some(());
                }
                for _ in 0..lts_of(prog, base) {
                    let v = self.var();
                    out.push(v);
                }
                for &a in &sd.args {
                    self.fresh_ph(prog, a, out, depth + 1)?;
                }
            }
            Ty::Dyn(_) => {
                let v = self.var();
                out.push(v);
            }
            Ty::Fn(ft) => {
                for &p in &ft.params {
                    self.fresh_ph(prog, p, out, depth + 1)?;
                }
                self.fresh_ph(prog, ft.ret, out, depth + 1)?;
            }
            Ty::Assoc(_) => return None,
            _ => {}
        }
        Some(())
    }

    /// The variance of the lifetime and type parameters of a definition (computed once)
    pub fn variance(&mut self, prog: &Program, base: u32) -> std::sync::Arc<[Var]> {
        if let Some(v) = self.variance.get(&base) {
            // (a type whose variance is being computed: what is known of it so far)
            if self.var_open.contains(&base) {
                self.var_cycle = true;
            }
            return v.clone();
        }
        let sd = &prog.structs[base as usize];
        let nl = lts_of(prog, base);
        let ng = sd.generics.len();
        // (a placeholder: covariant in its one region)
        if sd.placeholder.is_some() {
            let v: std::sync::Arc<[Var]> = vec![Var::Co; nl + ng].into();
            self.variance.insert(base, v.clone());
            return v;
        }
        let prelude = prog.struct_item.get(base as usize).is_some_and(|&i| i >= prog.prelude_from);
        // (the prelude's types stand for std's: raw pointers in them are as `NonNull` (covariant); its cells are
        // invariant)
        if prelude {
            let inv = matches!(sd.name.as_str(), "Cell" | "RefCell" | "UnsafeCell" | "Mutex" | "RwLock" | "OnceCell" | "OnceLock" | "AtomicPtr" | "MutexGuard");
            let v: std::sync::Arc<[Var]> = vec![if inv { Var::Inv } else { Var::Co }; nl + ng].into();
            self.variance.insert(base, v.clone());
            return v;
        }
        // (a recursive type: computed again from what the last round found, until it no longer changes. The
        // variances of the types computed meanwhile from a provisional one are computed again too. Each round can
        // only move a variance up (bivariant, co/contra, invariant), so this ends)
        let ar = prog.adt_regions.get(&base).cloned();
        let fields: Vec<Ty> = sd.fields.iter().map(|f| f.1).chain(sd.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1))).collect();
        let mut cur = vec![Var::Bi; nl + ng];
        self.var_open.push(base);
        let outer_cycle = std::mem::take(&mut self.var_cycle);
        for _ in 0..16 {
            self.variance.insert(base, cur.clone().into());
            let log0 = self.var_log.len();
            self.var_cycle = false;
            let mut acc = vec![Var::Bi; nl + ng];
            for (i, &ft) in fields.iter().enumerate() {
                let codes = ar.as_ref().map(|r| r.fields.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
                let mut c = Codes { codes, pos: 0, ok: true, parent: None };
                self.var_walk(prog, ft, &mut c, Var::Co, nl, &mut acc, 0);
            }
            let again = self.var_cycle && acc != cur;
            cur = acc;
            if !again {
                break;
            }
            for b in self.var_log.drain(log0..) {
                self.variance.remove(&b);
            }
        }
        self.var_open.pop();
        // (a cycle through a type still open further out: that one is computed again, and this one with it)
        self.var_cycle |= outer_cycle;
        // (a parameter not used is an error of its own (E0392); as rustc, bivariant)
        let cur: std::sync::Arc<[Var]> = cur.into();
        self.variance.insert(base, cur.clone());
        self.var_log.push(base);
        cur
    }

    /// The variance of each of a signature's lifetimes (the codes below n) in t, one of its types whose region slots
    /// are codes (ng: its type parameters). None when t has what the walk does not follow (a closure's type)
    pub(crate) fn code_variances(&mut self, prog: &Program, t: Ty, codes: &[u8], n: usize, ng: usize) -> Option<Vec<Var>> {
        let mut acc = vec![Var::Bi; n + ng];
        let mut c = Codes { codes: Some(codes), pos: 0, ok: true, parent: None };
        self.var_walk(prog, t, &mut c, Var::Co, n, &mut acc, 0);
        acc.truncate(n);
        c.ok.then_some(acc)
    }

    #[allow(clippy::too_many_arguments)]
    fn var_walk(&mut self, prog: &Program, t: Ty, c: &mut Codes, v: Var, nl: usize, acc: &mut Vec<Var>, depth: u32) {
        if depth > 24 {
            return;
        }
        let lt = |acc: &mut Vec<Var>, code: u8, v: Var| {
            if (code as usize) < nl {
                acc[code as usize] = acc[code as usize].join(v);
            }
        };
        match t {
            Ty::Ref(m, i) => {
                let code = c.next();
                lt(acc, code, v);
                self.var_walk(prog, *i, c, if m { v.then(Var::Inv) } else { v }, nl, acc, depth + 1);
            }
            Ty::Ptr(m, i) => self.var_walk(prog, *i, c, if m { v.then(Var::Inv) } else { v }, nl, acc, depth + 1),
            Ty::Box(i) => {
                c.elided_adt();
                self.var_walk(prog, *i, c, v, nl, acc, depth + 1)
            }
            Ty::Array(i, _) | Ty::Slice(i) => self.var_walk(prog, *i, c, v, nl, acc, depth + 1),
            Ty::Struct(sid) => {
                let (sd, base, tuple) = adt(prog, sid);
                if prog.closure_of_struct.contains_key(&base) {
                    c.ok = false;
                    return;
                }
                let n2 = lts_of(prog, base);
                let nv = self.variance(prog, base);
                if !tuple {
                    // (the marker of lifetimes not written is read whether or not the type has any)
                    let elided = c.elided_adt();
                    if n2 == 0 || elided {
                    } else {
                        for k in 0..n2 {
                            let code = c.next();
                            lt(acc, code, v.then(nv[k]));
                        }
                    }
                }
                for (j, &a) in sd.args.iter().enumerate() {
                    let w = if tuple { Var::Co } else { nv.get(n2 + j).copied().unwrap_or(Var::Inv) };
                    self.var_walk(prog, a, c, v.then(w), nl, acc, depth + 1);
                }
            }
            Ty::Param(g) => {
                let k = nl + g as usize;
                if k < acc.len() {
                    acc[k] = acc[k].join(v);
                }
            }
            // (contravariant in its parameters' regions and types, covariant in its result's)
            Ty::Fn(ft) => {
                if c.next() == R_FN {
                    for &p in &ft.params {
                        self.var_walk(prog, p, c, v.then(Var::Contra), nl, acc, depth + 1);
                    }
                    self.var_walk(prog, ft.ret, c, v, nl, acc, depth + 1);
                } else {
                    // (a type alias: the regions in it are not followed (no codes: each reads as elided), its type
                    // parameters are, after the definition's lifetimes)
                    let mut none = Codes { codes: None, pos: 0, ok: true, parent: None };
                    for &p in &ft.params {
                        self.var_walk(prog, p, &mut none, v.then(Var::Contra), nl, acc, depth + 1);
                    }
                    self.var_walk(prog, ft.ret, &mut none, v, nl, acc, depth + 1);
                }
            }
            // (invariant in the lifetimes its trait's arguments name)
            Ty::Dyn(_) => {
                let code = c.next();
                lt(acc, code, v);
                for &code in c.dyn_args() {
                    lt(acc, code, Var::Inv);
                }
            }
            _ => {}
        }
    }

    /// The tree of a type written with region codes: a signature's parameter (generic: its type parameters' trees are
    /// targs) or a field. `lts`: the variables of the lifetimes the codes name; `elided`: makes the variable of an
    /// elided region
    #[allow(clippy::too_many_arguments)]
    fn inst(&mut self, prog: &Program, t: Ty, c: &mut Codes, lts: &[u32], targs: &[Option<RT>], elided: &mut dyn FnMut(&mut RgSt) -> u32, out: &mut RT, depth: u32) {
        if depth > 24 || !c.ok {
            c.ok = false;
            return;
        }
        let parent = c.parent.take();
        // (a type alias written without its lifetimes (`b: A` with `type A<'a> = &'a isize`): one marker for what it
        // stands for, whose regions are all elided)
        if matches!(t, Ty::Ref(..) | Ty::Dyn(_) | Ty::Fn(_)) && c.codes.is_some_and(|x| x.get(c.pos) == Some(&R_ELIDED_ADT)) {
            // (an alias whose definition fixes a lifetime (`type S = &'static u8;`): which alias this is, and so
            // which of its regions are fixed, is not known here. Not followed)
            if prog.alias_fixed_lt {
                c.ok = false;
                return;
            }
            c.pos += 1;
            let mut all = Codes { codes: None, pos: 0, ok: true, parent };
            self.inst(prog, t, &mut all, lts, targs, elided, out, depth + 1);
            c.ok &= all.ok;
            return;
        }
        let region = |me: &mut RgSt, c: &mut Codes, elided: &mut dyn FnMut(&mut RgSt) -> u32| -> u32 {
            match c.next() {
                R_STATIC => STATIC,
                R_ELIDED => elided(me),
                R_BOUND => match (me.bound_univ, me.fn_bound_ret) {
                    (Some(u), _) => u,
                    (None, Some(v)) => v,
                    (None, None) => {
                        let v = me.var();
                        if let Some(b) = &mut me.fn_bound {
                            b.push(v);
                        }
                        me.late_bound.push(v);
                        v
                    }
                },
                x if (x as usize) < lts.len() => lts[x as usize],
                _ => {
                    c.ok = false;
                    0
                }
            }
        };
        match t {
            Ty::Ref(_, i) => {
                let r = region(self, c, elided);
                out.push(r);
                c.parent = Some(r);
                self.inst(prog, *i, c, lts, targs, elided, out, depth + 1);
            }
            Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) => self.inst(prog, *i, c, lts, targs, elided, out, depth + 1),
            Ty::Box(i) => {
                c.elided_adt();
                self.inst(prog, *i, c, lts, targs, elided, out, depth + 1)
            }
            Ty::Struct(sid) => {
                let (sd, base, tuple) = adt(prog, sid);
                if prog.closure_of_struct.contains_key(&base) {
                    c.ok = false;
                    return;
                }
                let n = lts_of(prog, base);
                let own0 = out.len();
                if !tuple {
                    if n == 0 {
                        c.elided_adt();
                    } else if c.elided_adt() {
                        for _ in 0..n {
                            let r = elided(self);
                            out.push(r);
                        }
                    } else {
                        for _ in 0..n {
                            let r = region(self, c, elided);
                            out.push(r);
                        }
                    }
                }
                // (a placeholder: its region is the same wherever its type is)
                if n > 0 && !self.ph.is_empty() {
                    if let Some(&(_, v)) = self.ph.iter().find(|x| x.0 == sid) {
                        out.truncate(own0);
                        out.push(v);
                    }
                }
                let ar = prog.adt_regions.get(&base);
                for (j, &a) in sd.args.iter().enumerate() {
                    // (a trait object as a type argument: its default lifetime is the parameter's one lifetime bound
                    // (`struct Ref<'a, T: 'a>` makes `Ref<'a, dyn Tr>` `dyn Tr + 'a`); with two, a variable of its own)
                    if matches!(a, Ty::Dyn(_)) {
                        if let (Some(ar), Some(gn)) = (ar, prog.structs[base as usize].generics.get(j)) {
                            let bs: Vec<u8> = ar.ty_outlives.iter().filter(|x| x.0 == *gn).map(|x| x.1).collect();
                            match bs.as_slice() {
                                [] => {}
                                [R_STATIC] => c.parent = Some(STATIC),
                                [k] if (*k as usize) < n => c.parent = out.get(own0 + *k as usize).copied(),
                                _ => c.parent = Some(self.var()),
                            }
                        }
                    }
                    self.inst(prog, a, c, lts, targs, elided, out, depth + 1);
                }
            }
            Ty::Param(g) => match targs.get(g as usize) {
                Some(Some(t)) => out.extend_from_slice(t),
                Some(None) => c.ok = false,
                None => {}
            },
            Ty::Dyn(_) => {
                // (its lifetime: written, or the default: the reference's around it, else 'static in a signature and
                // inferred in a body)
                let v = match c.next() {
                    R_DYN_DEFAULT => match parent {
                        Some(r) => r,
                        None if self.dyn_infer => self.var(),
                        None => STATIC,
                    },
                    // (`dyn Tr + '_`: an elided lifetime, as any other `'_` (a fresh one of a call's parameter); not the
                    // default, which in `Box<dyn Tr>` is 'static)
                    R_ELIDED if c.codes.is_some() => elided(self),
                    R_ELIDED => match parent {
                        Some(r) => r,
                        None if self.dyn_infer => self.var(),
                        None => STATIC,
                    },
                    R_STATIC => STATIC,
                    x if (x as usize) < lts.len() => lts[x as usize],
                    _ => {
                        c.ok = false;
                        0
                    }
                };
                // (the lifetimes its trait's arguments name: the object has no slots for them)
                c.dyn_args();
                out.push(v);
            }
            // (a fn pointer: the regions it binds are its own, as are those elided in it)
            // (written as a type alias (no R_FN): its regions are its own; the alias's one code is read)
            Ty::Fn(ft) => {
                let mut own = |me: &mut RgSt| me.var();
                if c.next() == R_FN {
                    // (the regions its parameters bind; its result's are the one of them, as elision makes them, or
                    // fresh. A fn pointer within is a binder of its own)
                    let saved = (self.fn_bound.replace(vec![]), self.fn_bound_ret.take());
                    for &p in &ft.params {
                        self.inst(prog, p, c, lts, targs, &mut own, out, depth + 1);
                    }
                    let mut ins = self.fn_bound.take().unwrap_or_default();
                    ins.sort_unstable();
                    ins.dedup();
                    self.fn_bound_ret = if ins.len() == 1 { Some(ins[0]) } else { None };
                    self.inst(prog, ft.ret, c, lts, targs, &mut own, out, depth + 1);
                    self.fn_bound = saved.0;
                    self.fn_bound_ret = saved.1;
                } else {
                    let ok = c.ok;
                    let mut none = Codes { codes: None, pos: 0, ok: true, parent: None };
                    for &p in &ft.params {
                        self.inst(prog, p, &mut none, lts, targs, &mut own, out, depth + 1);
                    }
                    self.inst(prog, ft.ret, &mut none, lts, targs, &mut own, out, depth + 1);
                    c.ok = ok && none.ok;
                }
            }
            // (a placeholder instance's associated type of a type parameter: its placeholder's region, the same
            // wherever it is (rg_begin). The mark a path `T::Foo` is written with, R_ELIDED_ADT, is read)
            Ty::Assoc(a) => {
                let v = match a.base {
                    Ty::Param(g) => match targs.get(g as usize) {
                        Some(Some(t)) if t.len() == 1 => self.assoc_regs.iter().find(|x| x.0 == t[0] && x.1 == a.name).map(|x| x.2),
                        _ => None,
                    },
                    _ => None,
                };
                match v {
                    Some(v) => {
                        if c.codes.is_some_and(|cs| cs.get(c.pos) == Some(&R_ELIDED_ADT)) {
                            c.pos += 1;
                        }
                        out.push(v);
                    }
                    None => c.ok = false,
                }
            }
            _ => {}
        }
    }

    /// Instantiate t with codes (all of them must be read)
    #[allow(clippy::too_many_arguments)]
    pub fn instantiate(&mut self, prog: &Program, t: Ty, codes: Option<&[u8]>, lts: &[u32], targs: &[Option<RT>], elided: &mut dyn FnMut(&mut RgSt) -> u32) -> Option<RT> {
        // (a parameter whose type is not followed)
        if codes.is_some_and(|c| c.contains(&R_UNKNOWN)) {
            return None;
        }
        let u0 = self.univ.len();
        let mut c = Codes { codes, pos: 0, ok: true, parent: None };
        let mut out = vec![];
        self.inst(prog, t, &mut c, lts, targs, elided, &mut out, 0);
        if c.ok && codes.is_none_or(|x| c.pos == x.len()) {
            return Some(out);
        }
        // (a type alias written without its lifetimes that stands for a Box or a struct (`Box<B>` with
        // `type B<'a> = Box<&'a u8>`): its marker is taken for the Box's own. Codes that name no region elide each
        // region, wherever it is, so the type alone gives them; not an alias that fixes a lifetime (see inst))
        if codes.is_some_and(|x| x.iter().all(|&c| c == R_ELIDED || c == R_ELIDED_ADT)) && !prog.alias_fixed_lt {
            self.univ.truncate(u0);
            let mut c = Codes { codes: None, pos: 0, ok: true, parent: None };
            let mut out = vec![];
            self.inst(prog, t, &mut c, lts, targs, elided, &mut out, 0);
            return c.ok.then_some(out);
        }
        None
    }

    /// sub <: sup at type t (the value of sub flows where sup is expected)
    pub fn relate(&mut self, prog: &Program, t: Ty, sub: &[u32], sup: &[u32], sp: Sp) {
        if sub.len() != sup.len() || count(prog, t, 0) != Some(sub.len()) {
            return;
        }
        let mut k = 0;
        self.rel(prog, t, sub, sup, &mut k, Var::Co, sp, 0);
    }

    /// A value of type from (tree sub) flows into a place of type to (tree sup), through the coercions that change
    /// its type: to a trait object (every region of the value outlives the object's), an array to a slice, a
    /// reference to one of what it derefs to (only the outer regions are related there)
    pub fn flow(&mut self, prog: &Program, from: Ty, to: Ty, sub: &[u32], sup: &[u32], sp: Sp) {
        // (no regions on either side: nothing to relate)
        if sub.is_empty() || sup.is_empty() {
            return;
        }
        // (`&mut (dyn Tr + 'a)` to `&mut (dyn Tr + 'b)` with 'a: 'b, `Box` too: an unsizing coercion. The object's
        // lifetime right under the pointer shortens, though `&mut` is invariant)
        if let (Ty::Ref(_, Ty::Dyn(_)), Ty::Ref(_, Ty::Dyn(_))) | (Ty::Box(Ty::Dyn(_)), Ty::Box(Ty::Dyn(_))) = (from, to) {
            if from == to && sub.len() == sup.len() {
                for (&x, &y) in sub.iter().zip(sup) {
                    self.constrain(x, y, Var::Co, sp);
                }
                return;
            }
        }
        if from == to {
            return self.relate(prog, to, sub, sup, sp);
        }
        let (mut i, mut j) = (0, 0);
        self.co(prog, from, to, sub, sup, (&mut i, &mut j), Var::Co, sp, 0);
    }

    #[allow(clippy::too_many_arguments)]
    fn co(&mut self, prog: &Program, from: Ty, to: Ty, sub: &[u32], sup: &[u32], ij: (&mut usize, &mut usize), v: Var, sp: Sp, depth: u32) -> bool {
        let (i, j) = ij;
        if depth > 24 {
            return false;
        }
        if from == to {
            let Some(c) = count(prog, to, 0) else { return false };
            let (Some(a), Some(b)) = (sub.get(*i..*i + c), sup.get(*j..*j + c)) else { return false };
            let mut k = 0;
            self.rel(prog, to, a, b, &mut k, v, sp, depth);
            *i += c;
            *j += c;
            return true;
        }
        let refs = |mut t: Ty| {
            let mut n = 0;
            while let Ty::Ref(_, i) = t {
                n += 1;
                t = *i;
            }
            n
        };
        let base = |mut t: Ty| {
            while let Ty::Ref(_, i) = t {
                t = *i;
            }
            t
        };
        match (from, to) {
            // (a deref coercion (`c: &&'a str` where a `&str` is expected): the reborrow `&**c` it is lives no longer
            // than the references it goes through from the inside out, and a shared one stops that (NLL's reborrow
            // constraints): of `&'1 &'a str` only 'a)
            // (not an unsizing to a trait object (`x: &'a &'static u32` as `&dyn Tr`, with `&u32: Tr`): that keeps the
            // outer reference, whose region is the object's)
            (Ty::Ref(..), Ty::Ref(m, b)) if refs(from) > refs(to) && !(matches!(base(to), Ty::Dyn(_)) && !matches!(base(from), Ty::Dyn(_))) => {
                let mut f = from;
                let mut crossed: Vec<(u32, bool)> = vec![];
                while refs(f) >= refs(to) {
                    let (Ty::Ref(fm, fi), Some(&x)) = (f, sub.get(*i)) else { return false };
                    crossed.push((x, fm));
                    *i += 1;
                    if refs(f) == refs(to) {
                        f = *fi;
                        break;
                    }
                    f = *fi;
                }
                let Some(&y) = sup.get(*j) else { return false };
                for &(x, fm) in crossed.iter().rev() {
                    self.constrain(x, y, v, sp);
                    if !fm {
                        break;
                    }
                }
                *j += 1;
                self.co(prog, f, *b, sub, sup, (i, j), if m { v.then(Var::Inv) } else { v }, sp, depth + 1)
            }
            (Ty::Ref(_, a), Ty::Ref(m, b)) => {
                let (Some(&x), Some(&y)) = (sub.get(*i), sup.get(*j)) else { return false };
                self.constrain(x, y, v, sp);
                *i += 1;
                *j += 1;
                self.co(prog, *a, *b, sub, sup, (i, j), if m { v.then(Var::Inv) } else { v }, sp, depth + 1)
            }
            (Ty::Ptr(_, a), Ty::Ptr(m, b)) => self.co(prog, *a, *b, sub, sup, (i, j), if m { v.then(Var::Inv) } else { v }, sp, depth + 1),
            (Ty::Box(a), Ty::Box(b)) | (Ty::Array(a, _), Ty::Slice(b)) => self.co(prog, *a, *b, sub, sup, (i, j), v, sp, depth + 1),
            (_, Ty::Dyn(_)) => {
                let Some(c) = count(prog, from, 0) else { return false };
                let (Some(xs), Some(&y)) = (sub.get(*i..*i + c), sup.get(*j)) else { return false };
                for &x in xs {
                    self.constrain(x, y, Var::Co, sp);
                }
                *i += c;
                *j += 1;
                true
            }
            _ => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn rel(&mut self, prog: &Program, t: Ty, sub: &[u32], sup: &[u32], k: &mut usize, v: Var, sp: Sp, depth: u32) {
        if depth > 24 {
            return;
        }
        match t {
            Ty::Ref(m, i) => {
                self.constrain(sub[*k], sup[*k], v, sp);
                *k += 1;
                self.rel(prog, *i, sub, sup, k, if m { v.then(Var::Inv) } else { v }, sp, depth + 1);
            }
            Ty::Ptr(m, i) => self.rel(prog, *i, sub, sup, k, if m { v.then(Var::Inv) } else { v }, sp, depth + 1),
            Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.rel(prog, *i, sub, sup, k, v, sp, depth + 1),
            // (a closure: covariant in what it captures)
            Ty::Struct(sid) if is_closure(prog, sid) => {
                let Some((ts, moved)) = captures(prog, sid) else { return };
                for (j, &a) in ts.iter().enumerate() {
                    match (moved.get(j).copied().unwrap_or(false), a) {
                        (false, Ty::Ptr(_, i)) => {
                            self.constrain(sub[*k], sup[*k], v, sp);
                            *k += 1;
                            self.rel(prog, *i, sub, sup, k, v, sp, depth + 1);
                        }
                        _ => self.rel(prog, a, sub, sup, k, v, sp, depth + 1),
                    }
                }
            }
            Ty::Struct(sid) => {
                let (sd, base, tuple) = adt(prog, sid);
                let n = lts_of(prog, base);
                let nv = self.variance(prog, base);
                for j in 0..n {
                    self.constrain(sub[*k], sup[*k], v.then(nv[j]), sp);
                    *k += 1;
                }
                for (j, &a) in sd.args.iter().enumerate() {
                    let w = if tuple { Var::Co } else { nv.get(n + j).copied().unwrap_or(Var::Inv) };
                    self.rel(prog, a, sub, sup, k, v.then(w), sp, depth + 1);
                }
            }
            Ty::Dyn(_) => {
                self.constrain(sub[*k], sup[*k], v, sp);
                *k += 1;
            }
            // (contravariant in its parameters, covariant in its result)
            Ty::Fn(ft) => {
                for &p in &ft.params {
                    self.rel(prog, p, sub, sup, k, v.then(Var::Contra), sp, depth + 1);
                }
                self.rel(prog, ft.ret, sub, sup, k, v, sp, depth + 1);
            }
            _ => {}
        }
    }

    /// The trees of a struct's lifetime parameters and type arguments, from the tree of an instance
    pub fn split_adt(&self, prog: &Program, sid: u32, t: &[u32]) -> Option<(Vec<u32>, Vec<Option<RT>>)> {
        let n = lts_of(prog, prog.base_of(sid));
        let own = t.get(..n)?.to_vec();
        let mut k = n;
        let mut args = vec![];
        for &a in &prog.structs[sid as usize].args {
            let c = count(prog, a, 0)?;
            args.push(Some(t.get(k..k + c)?.to_vec()));
            k += c;
        }
        Some((own, args))
    }

    /// The tree of field i (over all variants) of an instance sid whose tree is t
    pub fn field(&mut self, prog: &Program, sid: u32, i: usize, t: &[u32]) -> Option<RT> {
        let base = prog.base_of(sid);
        let bd = &prog.structs[base as usize];
        let ft = bd.fields.iter().map(|f| f.1).chain(bd.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1))).nth(i)?;
        let (own, args) = self.split_adt(prog, sid, t)?;
        let ar = prog.adt_regions.get(&base).cloned();
        let codes = ar.as_ref().map(|r| r.fields.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        self.instantiate(prog, ft, codes, &own, &args, &mut |me| me.var())
    }

    /// What the types of the signature imply among its regions: `&'a T` means every region in T outlives 'a; a
    /// struct's fields likewise (rustc's implied bounds, with the outlives requirements inferred from fields)
    pub fn implied(&mut self, prog: &Program, t: Ty, tree: &[u32], depth: u32) {
        if depth > 4 {
            return;
        }
        let mut k = 0;
        self.imp(prog, t, tree, &mut k, &mut vec![], depth);
    }

    /// What the types of a value written in the body require among its regions (its well-formedness): as imp's
    /// implied bounds, but as constraints (`let z: Option<&'b &'a u8>` needs 'a: 'b)
    pub fn wf(&mut self, prog: &Program, t: Ty, tree: &[u32], sp: Sp) {
        self.wf_sp = Some(sp);
        let mut k = 0;
        self.imp(prog, t, tree, &mut k, &mut vec![], 0);
        self.wf_sp = None;
    }

    /// wf of a struct's value its fields' values flowed into: only what the definition declares (what its fields
    /// imply already holds of them)
    pub fn wf_declared(&mut self, prog: &Program, t: Ty, tree: &[u32], sp: Sp) {
        self.wf_no_fields = true;
        self.wf(prog, t, tree, sp);
        self.wf_no_fields = false;
    }

    /// The places (addresses) of the reborrows whose region must outlive a universal region ('static too): the loan
    /// is live from where it is made to the end of the function. Not through the constraints of a returned value
    /// (ret_edges): those hold at the return, after which nothing runs, so such a reborrow is live where what holds
    /// it is live, as Polonius Alpha has it (`let b = &mut *a; if c { b } else { a }`)
    pub fn forever(&self) -> Vec<usize> {
        self.forever_by(self.kept_edges())
    }

    /// forever through every constraint, the returned values' too: what must outlive a universal region by being
    /// returned as well (Bk::ret_only is this less forever)
    pub fn forever_returned(&self) -> Vec<usize> {
        if self.ret_edges.is_empty() || super::no_polonius() {
            return vec![];
        }
        self.forever_by(vec![true; self.edges.len()])
    }

    fn forever_by(&self, keep: Vec<bool>) -> Vec<usize> {
        if self.reborrows.is_empty() {
            return vec![];
        }
        let n = self.n as usize + 1;
        // (what outlives each variable, as one array (CSR): no allocation per variable)
        let mut start = vec![0u32; n + 1];
        for (i, &(a, b, _)) in self.edges.iter().enumerate() {
            if (a as usize) < n && (b as usize) < n && keep[i] {
                start[b as usize + 1] += 1;
            }
        }
        for k in 0..n {
            start[k + 1] += start[k];
        }
        let mut fill = start.clone();
        let mut rev = vec![0u32; start[n] as usize];
        for (i, &(a, b, _)) in self.edges.iter().enumerate() {
            if (a as usize) < n && (b as usize) < n && keep[i] {
                rev[fill[b as usize] as usize] = a;
                fill[b as usize] += 1;
            }
        }
        let mut seen = vec![false; n];
        let mut st: Vec<u32> = std::iter::once(STATIC).chain(self.univ.iter().map(|u| u.0)).filter(|&v| (v as usize) < n).collect();
        for &v in &st {
            seen[v as usize] = true;
        }
        while let Some(x) = st.pop() {
            for &y in &rev[start[x as usize] as usize..start[x as usize + 1] as usize] {
                if !seen[y as usize] {
                    seen[y as usize] = true;
                    st.push(y);
                }
            }
        }
        self.reborrows.iter().filter(|&&(_, r)| seen.get(r as usize).copied().unwrap_or(false)).map(|&(p, _)| p).collect()
    }

    /// The locals whose types' regions the reborrows (and borrows of receivers) must outlive, by the reborrow's
    /// address: as NLL, such a borrow is live while the local is (`foo.insert()` with `fn insert(&'a mut self)` in
    /// `impl<'a> Foo<'a>` borrows foo for as long as foo is used)
    /// The edges forever and holders follow: all but the returned values' (ret_edges), unless ZUSTC_NO_POLONIUS
    fn kept_edges(&self) -> Vec<bool> {
        let mut keep = vec![true; self.edges.len()];
        if !super::no_polonius() {
            for &(a, b) in &self.ret_edges {
                keep[a..b].iter_mut().for_each(|k| *k = false);
            }
        }
        keep
    }

    pub fn holders(&self) -> Vec<(usize, Vec<Loc>)> {
        if (self.reborrows.is_empty() && self.owned_borrows.is_empty()) || self.locals.is_empty() {
            return vec![];
        }
        let n = self.n as usize + 1;
        // (the locals (the first 64) each variable is a region of the type of)
        let locs: Vec<(Loc, &RT)> = self.locals.iter().map(|(&l, t)| (l, t)).take(64).collect();
        let mut own = vec![0u64; n];
        for (i, (_, tr)) in locs.iter().enumerate() {
            for &v in tr.iter() {
                if v != STATIC && (v as usize) < n {
                    own[v as usize] |= 1 << i;
                }
            }
        }
        // (not through a returned value's constraints, which hold at the return (Rg::forever): a reborrow returned is
        // not held by a parameter whose type names the region it is returned for)
        let keep = self.kept_edges();
        // (what each variable outlives, as one array (CSR))
        let mut start = vec![0u32; n + 1];
        for (i, &(a, b, _)) in self.edges.iter().enumerate() {
            if (a as usize) < n && (b as usize) < n && keep[i] {
                start[a as usize + 1] += 1;
            }
        }
        for k in 0..n {
            start[k + 1] += start[k];
        }
        let mut fill = start.clone();
        let mut adj = vec![0u32; start[n] as usize];
        for (i, &(a, b, _)) in self.edges.iter().enumerate() {
            if (a as usize) < n && (b as usize) < n && keep[i] {
                adj[fill[a as usize] as usize] = b;
                fill[a as usize] += 1;
            }
        }
        let mut out = vec![];
        let mut seen = vec![0u32; n];
        let mut st = vec![];
        for (k, &(p, r)) in self.reborrows.iter().chain(&self.owned_borrows).enumerate() {
            if r as usize >= n {
                continue;
            }
            let stamp = k as u32 + 1;
            let mut m = 0u64;
            st.clear();
            st.push(r);
            seen[r as usize] = stamp;
            while let Some(x) = st.pop() {
                for &y in &adj[start[x as usize] as usize..start[x as usize + 1] as usize] {
                    if seen[y as usize] != stamp {
                        seen[y as usize] = stamp;
                        m |= own[y as usize];
                        st.push(y);
                    }
                }
            }
            if m != 0 {
                out.push((p, (0..locs.len()).filter(|&i| m >> i & 1 == 1).map(|i| locs[i].0).collect()));
            }
        }
        out
    }

    /// x outlives y: known (implied bounds), or required (wf)
    fn imp_rel(&mut self, x: u32, y: u32) {
        for r in self.imp_rec.iter_mut() {
            r.push((x, y));
        }
        match self.wf_sp {
            Some(sp) => self.edge(x, y, sp),
            None => self.known.push((x, y)),
        }
    }

    fn imp(&mut self, prog: &Program, t: Ty, tree: &[u32], k: &mut usize, outer: &mut Vec<u32>, depth: u32) {
        if *k > tree.len() || depth > 24 {
            return;
        }
        match t {
            Ty::Ref(_, i) => {
                let Some(&r) = tree.get(*k) else { return };
                for &o in outer.iter() {
                    self.imp_rel(r, o);
                }
                *k += 1;
                outer.push(r);
                self.imp(prog, *i, tree, k, outer, depth);
                outer.pop();
            }
            Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.imp(prog, *i, tree, k, outer, depth),
            Ty::Struct(sid) => {
                let Some(c) = count(prog, t, 0) else { return };
                let Some(me) = tree.get(*k..*k + c) else { return };
                let me = me.to_vec();
                for &r in &me {
                    for &o in outer.iter() {
                        self.imp_rel(r, o);
                    }
                }
                *k += c;
                // (what follows depends on the struct's type and the depth alone: computed once, by positions)
                let key = (sid, depth, self.wf_no_fields);
                if let Some(Some(pairs)) = self.imp_cache.get(&key) {
                    let pairs = pairs.clone();
                    for (a, b) in pairs {
                        let m = |i: u32| if i == u32::MAX { STATIC } else { me[i as usize] };
                        self.imp_rel(m(a), m(b));
                    }
                    return;
                }
                let record = !self.imp_cache.contains_key(&key);
                if record {
                    self.imp_rec.push(vec![]);
                }
                // (its type arguments are well formed: `Option<&'s &'l ()>` implies 'l: 's)
                if depth < 4 && !is_closure(prog, sid) {
                    if let Some((_, args)) = self.split_adt(prog, sid, &me) {
                        for (&a, at) in prog.structs[sid as usize].args.iter().zip(args) {
                            if let Some(at) = at {
                                self.implied(prog, a, &at, depth + 1);
                            }
                        }
                    }
                }
                // (the fields: what the definition's types require of its parameters)
                let base = prog.base_of(sid);
                if prog.adt_regions.contains_key(&base) && depth < 3 {
                    let bd = &prog.structs[base as usize];
                    let nf = if self.wf_no_fields { 0 } else { bd.fields.len() + bd.variants.iter().map(|v| v.fields.len()).sum::<usize>() };
                    let fts: Vec<Ty> = prog.structs[sid as usize].fields.iter().map(|f| f.1).chain(prog.structs[sid as usize].variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1))).collect();
                    for i in 0..nf {
                        let n0 = self.n;
                        if let (Some(ft), Some(fr)) = (fts.get(i).copied(), self.field(prog, sid, i, &me)) {
                            // (only the field's regions that are the struct's: fresh ones were made for nothing)
                            if fr.iter().all(|&x| x <= n0) {
                                self.implied(prog, ft, &fr, depth + 1);
                            }
                        }
                    }
                    if let Some(ar) = prog.adt_regions.get(&base).cloned() {
                        let (own, args) = self.split_adt(prog, sid, &me).unwrap_or_default();
                        for &(a, b) in &ar.outlives {
                            if let (Some(&x), Some(&y)) = (own.get(a as usize), own.get(b as usize)) {
                                self.imp_rel(x, y);
                            }
                        }
                        for (tp, b) in &ar.ty_outlives {
                            // (`T: 'static` too: `struct Static<T: 'static>(T)` makes `Static<T>` need T: 'static)
                            let y = if *b == R_STATIC { Some(&STATIC) } else { own.get(*b as usize) };
                            let (Some(g), Some(&y)) = (bd.generics.iter().position(|x| x == tp), y) else { continue };
                            if let Some(Some(at)) = args.get(g) {
                                for &x in at {
                                    self.imp_rel(x, y);
                                }
                            }
                        }
                    }
                }
                if record {
                    let rec = self.imp_rec.pop().unwrap_or_default();
                    let pos = |v: u32| if v == STATIC { Some(u32::MAX) } else { me.iter().position(|&x| x == v).map(|p| p as u32) };
                    let pairs: Option<Vec<(u32, u32)>> = rec.iter().map(|&(x, y)| Some((pos(x)?, pos(y)?))).collect();
                    self.imp_cache.insert(key, pairs);
                }
            }
            Ty::Dyn(_) => *k += 1,
            // (a fn pointer's types imply nothing)
            Ty::Fn(_) => *k += count(prog, t, 0).unwrap_or(0),
            _ => {}
        }
    }

    /// The first universal region that must outlive another the function does not know it outlives
    pub fn check(&self) -> Option<(Sp, String)> {
        if self.edges.is_empty() && self.hr_errs.is_empty() {
            return None;
        }
        // (the universal regions and 'static, as indices; what each is known to outlive, closed)
        let mut ids: Vec<u32> = vec![STATIC];
        ids.extend(self.univ.iter().map(|u| u.0));
        let ix = |v: u32| ids.iter().position(|&x| x == v);
        let nu = ids.len();
        let mut k = vec![vec![false; nu]; nu];
        for i in 0..nu {
            k[i][i] = true;
            k[0][i] = true;
        }
        for &(a, b) in &self.known {
            if let (Some(i), Some(j)) = (ix(a), ix(b)) {
                k[i][j] = true;
            }
        }
        let close = |k: &mut Vec<Vec<bool>>| {
            for m in 0..nu {
                for i in 0..nu {
                    if k[i][m] {
                        for j in 0..nu {
                            if k[m][j] {
                                k[i][j] = true;
                            }
                        }
                    }
                }
            }
        };
        close(&mut k);
        // (a placeholder's associated type outlives what its placeholder and its bound's lifetimes all outlive)
        if !self.proj_comps.is_empty() {
            let mut changed = false;
            for (a, comps) in &self.proj_comps {
                let (Some(ai), Some(cs)) = (ix(*a), comps.iter().map(|&c| ix(c)).collect::<Option<Vec<usize>>>()) else { continue };
                for j in 0..nu {
                    if !k[ai][j] && cs.iter().all(|&c| k[c][j]) {
                        k[ai][j] = true;
                        changed = true;
                    }
                }
            }
            if changed {
                close(&mut k);
            }
        }
        let n = self.n as usize + 1;
        let mut adj: Vec<Vec<(u32, Sp)>> = vec![vec![]; n];
        for &(a, b, sp) in &self.edges {
            if (a as usize) < n && (b as usize) < n {
                adj[a as usize].push((b, sp));
            }
        }
        let mut errs: Vec<(Sp, String)> = vec![];
        // (a closure's signature with regions from outside it, its environment's and its captures' aside: the
        // callee's lifetimes)
        let sig_ext = self.in_closure && self.externals.iter().any(|&e| e != self.env_r && !self.cap_regions.contains(&e));
        for (ui, &(u, ref uname)) in self.univ.iter().enumerate() {
            let i = ui + 1;
            let mut seen = vec![false; n];
            seen[u as usize] = true;
            let mut st = vec![u];
            while let Some(x) = st.pop() {
                for &(y, sp) in &adj[x as usize] {
                    if seen[y as usize] {
                        continue;
                    }
                    seen[y as usize] = true;
                    if let Some(j) = ix(y) {
                        // (a closure's region from outside required to be 'static, or related to one of the function
                        // it is in: the caller's to know)
                        // (not the closure's environment required to be 'static: the closure has it for no longer
                        // than a call (a capture reborrowed into a `&'static mut`))
                        let outside = |a: u32, b: u32| self.externals.contains(&a) && (b == STATIC || self.outer_univ.contains(&b)) && !(self.in_closure && a == self.env_r && b == STATIC);
                        // (nor one of the function's that must outlive a placeholder, when the signature has regions
                        // from outside: the callee's lifetimes in the bound may be it, and what they imply is of them
                        // (`impl FnOnce(&mut &mut &'a i32)` with the function's 'a). With none, it must outlive every
                        // region the bound binds: `take(|_z| x)` with `F: FnMut(&isize) -> &isize`)
                        // (and only a placeholder one of them is known to outlive, as the signature's types imply:
                        // `&'b u32` next to `&'a u32` implies nothing of 'b, so `take(x, |_, _| x)` with
                        // `F: for<'b> FnOnce(&'b u32, &'a u32) -> &'b u32` returns x for every 'b)
                        let placeholder = |b: u32| {
                            sig_ext && !self.externals.contains(&b) && !self.outer_univ.contains(&b) && b != STATIC && ix(b).is_some_and(|bj| self.externals.iter().any(|&e| ix(e).is_some_and(|ei| k[ei][bj])))
                        };
                        if !k[i][j] && !outside(u, y) && !outside(y, u) && !(self.outer_univ.contains(&u) && placeholder(y)) && !self.soft.contains(&u) && !self.soft.contains(&y) {
                            let other = if j == 0 { "'static".to_string() } else { self.univ[j - 1].1.clone() };
                            // (a region the closure's signature binds flowing into one from outside)
                            let escapes = self.in_closure && !self.externals.contains(&u) && !self.outer_univ.contains(&u) && self.cap_regions.contains(&y);
                            // (a placeholder of a trait obligation's binder that must outlive, or be outlived by,
                            // another region: the impl selected holds for some of the binder's lifetimes only)
                            let hr = uname.split_once(HR_MARK).or_else(|| other.split_once(HR_MARK)).map(|x| x.1.to_string());
                            let msg = if let Some(tr) = hr {
                                format!("implementation of `{tr}` is not general enough")
                            } else if escapes {
                                "borrowed data escapes outside of closure (a region of its signature flows into one from outside)".to_string()
                            } else if let Some(tp) = uname.strip_suffix('#') {
                                // (a type parameter's placeholder: what its values may borrow must outlive `other`;
                                // E0311 when `other` has no name)
                                let code = match (j, other.as_str()) {
                                    (0, _) => 310,
                                    (_, "'_") => 311,
                                    _ => 309,
                                };
                                let what = if tp.starts_with('<') { "associated type" } else { "parameter type" };
                                format!("the {what} `{tp}` may not live long enough (it must outlive `{other}`) (E0{code})")
                            } else {
                                format!("lifetime may not live long enough (`{uname}` must outlive `{other}`)")
                            };
                            errs.push((sp, msg));
                        }
                    }
                    st.push(y);
                }
            }
        }
        // (a borrow of what the function owns that must outlive a universal region (or be 'static): it does not
        // live long enough)
        for (r, name, lsp) in &self.loans {
            let mut seen = vec![false; n];
            seen[*r as usize] = true;
            let mut st = vec![*r];
            let mut bad = false;
            // (the borrow escapes as the value returned: reported where it is returned)
            let mut ret_at = None;
            while let Some(x) = st.pop() {
                for &(y, esp) in &adj[x as usize] {
                    if seen[y as usize] {
                        continue;
                    }
                    seen[y as usize] = true;
                    if ix(y).is_some() {
                        bad = true;
                        if ret_at.is_none() && self.ret_sps.contains(&esp) {
                            ret_at = Some(esp);
                        }
                    }
                    st.push(y);
                }
            }
            match ret_at {
                Some(sp) if bad => errs.push((sp, if name.is_empty() {
                    "cannot return value referencing temporary value (E0515)".to_string()
                } else if super::borrowck::synthetic_param(name) {
                    "cannot return value referencing function parameter (E0515)".to_string()
                } else {
                    format!("cannot return value referencing local variable `{name}` (E0515)")
                })),
                _ if bad => errs.push((*lsp, if name.is_empty() { "temporary value dropped while borrowed (it must outlive the function)".to_string() } else { format!("`{name}` does not live long enough (borrowed value must outlive the function)") })),
                _ => {}
            }
        }
        // (a trait obligation's placeholder: it is none of the regions of the outer universe, made before the
        // obligation (rustc's universes: `want_hrtb::<&u32>()` with `impl<'a> Foo<&'a isize> for &'a u32`), and it
        // outlives no region but those it is (it contains no point of the body: rustc's check_bound_universal_region,
        // `impl<'a, 'b: 'a> Foo<'b> for FooS<'a>` for `F: for<'b> Foo<'b>`). 'static and the universal regions are
        // the check above's)
        if !self.hr.is_empty() {
            let mut radj: Vec<Vec<u32>> = vec![vec![]; n];
            for (a, outs) in adj.iter().enumerate() {
                for &(b, _) in outs {
                    radj[b as usize].push(a as u32);
                }
            }
            let reach = |from: u32, g: &dyn Fn(u32) -> Vec<u32>| {
                let mut seen = vec![false; n];
                let mut st = vec![from];
                if (from as usize) < n {
                    seen[from as usize] = true;
                }
                while let Some(x) = st.pop() {
                    for y in g(x) {
                        if (y as usize) < n && !seen[y as usize] {
                            seen[y as usize] = true;
                            st.push(y);
                        }
                    }
                }
                seen
            };
            let univ = |v: u32| v == STATIC || self.univ.iter().any(|u| u.0 == v);
            for (p, v0, sp, tr) in &self.hr {
                if (*p as usize) >= n {
                    continue;
                }
                let fwd = reach(*p, &|x| adj[x as usize].iter().map(|e| e.0).collect());
                let bwd = reach(*p, &|x| radj[x as usize].clone());
                let bad = (1..n as u32).any(|y| y != *p && !univ(y) && fwd[y as usize] && (!bwd[y as usize] || y <= *v0));
                if bad {
                    errs.push((*sp, format!("implementation of `{tr}` is not general enough")));
                }
            }
        }
        errs.extend(self.hr_errs.iter().cloned());
        errs.sort_by_key(|e| (e.0.line, e.0.col));
        if debug() {
            for e in &errs {
                eprintln!("regionck: error candidate at {}:{}: {}", e.0.line, e.0.col, e.1);
            }
        }
        errs.into_iter().next()
    }
}

impl Cx<'_> {
    pub(crate) fn rg_on(&self) -> bool {
        self.rg.on && self.inline_depth == 0
    }

    /// At the start of the body: the universal regions of the signature, the trees of the parameters and of the
    /// return type. Only a function with at least two regions in its signature (or a named one) is checked
    pub(crate) fn rg_begin(&mut self, regions: Option<&RSigSyn>, params: &[(Loc, Ty)], self_elides: bool, force: bool) {
        let prog = self.prog;
        if !self.rg.imp_taken {
            self.rg.imp_cache = std::mem::take(&mut self.insts.lock().unwrap().imp_cache);
            self.rg.imp_taken = true;
        }
        let rs = regions;
        let codes = |i: usize| rs.map(|r| r.slots.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        let mut rg = std::mem::take(&mut self.rg);
        for l in rs.map(|r| r.lts.as_slice()).unwrap_or(&[]) {
            let v = rg.var();
            // (`'_0`: a `'_` of the impl's header)
            rg.univ.push((v, if l.starts_with("'_") { "'_".to_string() } else { l.clone() }));
            rg.lts.push(v);
            rg.lt_names.push(l.clone());
        }
        let lts = rg.lts.clone();
        // (an instance: the generic signature's types, with fresh trees for the type parameters)
        let gsig = &prog.sigs[rg.base as usize];
        let generic = !rg.inst_args.is_empty() && gsig.params.len() == params.len();
        let targs: Vec<Option<RT>> = if generic { rg.inst_args.clone().into_iter().map(|t| rg.fresh(prog, t)).collect() } else { vec![] };
        // (a placeholder's associated types its bounds' traits declare (`<T# as MyTrait<'a>>::Output`): a region each,
        // the same wherever the type is (a parameter's `&'x T::Foo` too), universal (below). It outlives what the
        // trait's declaration bounds it by (`type Output: 'a`), what the function's where clauses do (`T::Output:
        // 'b`), and what its placeholder and the bound's lifetimes all outlive (nll/ty-outlives/
        // projection-where-clause-*.rs). Not of a bound with type arguments, or with a lifetime the function does not
        // name (the bound's own `for<'x>`): what it is of is not known)
        let mut assoc_ph: Vec<(usize, u32, u32, Vec<u32>, Vec<u32>)> = vec![];
        // (nor where a where clause bounds a type not read by a lifetime (`StrategyFor<A>: 'static` of a type alias,
        // the mark `#?`): it may be one of them)
        let unread = rs.is_some_and(|r| r.ty_outlives.iter().any(|x| x.0 == "#?"));
        if generic && !unread && !prog.assoc_bounds.is_empty() {
            for (k, &t) in rg.inst_args.clone().iter().enumerate() {
                let Ty::Struct(sid) = t else { continue };
                if !prog.is_placeholder(sid) {
                    continue;
                }
                let pname = gsig.generics.get(k).map(|g| g.0.clone()).unwrap_or_default();
                let bs = prog.structs[sid as usize].placeholder.clone().unwrap_or_default();
                for b in &bs {
                    let tr = b.split('<').next().unwrap_or(b).trim();
                    let short = tr.rsplit("::").next().unwrap_or(tr).to_string();
                    let blts = bound_lts(b);
                    let n_args = b.find('<').map_or(0, |_| b.matches(',').count() + 1);
                    if n_args != blts.len() {
                        continue;
                    }
                    let Some(regs) = blts.iter().map(|l| if l == "'static" { Some(STATIC) } else { rg.lt_names.iter().position(|x| x == l).and_then(|p| lts.get(p).copied()) }).collect::<Option<Vec<u32>>>() else { continue };
                    let mut names: Vec<(&String, &Vec<String>)> = prog.assoc_bounds.iter().filter(|((t, _), _)| *t == short).map(|((_, n), d)| (n, d)).collect();
                    names.sort();
                    for (name, decl) in names {
                        // (the associated type as typeck finds it: not where an impl gives it (`impl<T> Trait for T {
                        // type Assoc = T; }`), as the body's types are then of that)
                        let Some(Ty::Struct(asid)) = prog.assoc_of(Ty::Struct(sid), name) else { continue };
                        if prog.assoc_placeholder_of(asid).is_none() {
                            continue;
                        }
                        if rg.ph.iter().any(|x| x.0 == asid) {
                            continue;
                        }
                        let va = rg.var();
                        rg.ph.push((asid, va));
                        // (by the placeholder's region: its tree's one region, which becomes the universal one)
                        if let Some(Some([vt])) = targs.get(k).map(|t| t.as_deref()) {
                            rg.assoc_regs.push((*vt, name.clone(), va));
                        }
                        let mut known = vec![];
                        for d in decl.iter().filter(|d| d.starts_with('\'')) {
                            let r = if d == "'static" { Some(STATIC) } else { d[1..].parse::<usize>().ok().and_then(|i| regs.get(i).copied()) };
                            known.extend(r);
                        }
                        let key = format!("{pname}::{name}");
                        for (tp, c) in rs.map(|r| r.ty_outlives.as_slice()).unwrap_or(&[]) {
                            if *tp == key {
                                known.extend(if *c == R_STATIC { Some(STATIC) } else { lts.get(*c as usize).copied() });
                            }
                        }
                        assoc_ph.push((k, asid, va, regs.clone(), known));
                    }
                }
            }
        }
        let mut trees: Vec<Option<RT>> = vec![];
        let mut inputs: Vec<u32> = vec![];
        for (i, &(_, t)) in params.iter().enumerate() {
            let t = if generic { gsig.params[i] } else { t };
            let mut el = |me: &mut RgSt| {
                let v = me.var();
                me.univ.push((v, "'_".to_string()));
                v
            };
            let tr = rg.instantiate(prog, t, codes(i), &lts, &targs, &mut el);
            if let Some(tr) = &tr {
                inputs.extend(tr.iter().copied().filter(|&v| v != STATIC && rg.univ.iter().any(|u| u.0 == v)));
            }
            trees.push(tr);
        }
        // (the return type's elided regions: of `&self` (or a receiver type naming `&Self`: `self: Box<&Self>`), else
        // of the one region among the parameters)
        let self_r = if self_elides { trees.first().and_then(|t| t.as_ref()).and_then(|t| t.first().copied()) } else { None };
        inputs.dedup();
        let one = if inputs.len() == 1 { Some(inputs[0]) } else { None };
        let mut bad = false;
        let ret_t = self.ret;
        let mut el = |_: &mut RgSt| match self_r.or(one) {
            Some(v) => v,
            None => {
                bad = true;
                STATIC
            }
        };
        let gret = if generic { gsig.ret } else { ret_t };
        let ret = rg.instantiate(prog, gret, codes(params.len()), &lts, &targs, &mut el);
        rg.implied_partial = trees.iter().any(|t| t.is_none()) || ret.is_none();
        rg.ret = ret.filter(|_| !bad);
        // (an `impl Trait` result with no region among the parameters: its hidden type's regions are 'static)
        // (or one written `+ 'static`: `fn f<T>(x: T) -> impl Any + 'static`, its hidden type T must be 'static (E0310))
        // (not a closure, nor what holds one: its tree is of the variables it captures, not of the places it uses (RFC
        // 2229: `|| drop(&m.a.0)` through a `&'static` field borrows for 'static), which would be required for no reason)
        let static_opaque = rg.static_opaque && !holds_closure(prog, ret_t, 0);
        let opaque = (inputs.is_empty() && !generic && rs.is_none() || static_opaque) && prog.opaque.contains(&rg.base);
        if opaque {
            rg.ret = count(prog, ret_t, 0).filter(|&c| c > 0).map(|c| vec![STATIC; c]);
        }
        // (a placeholder for a type parameter (zustc check of a generic fn nothing calls): its region is a universal
        // one, named as the placeholder (`T#`), that outlives what the parameter's `T: 'a` bounds name)
        if generic {
            for (k, &t) in rg.inst_args.clone().iter().enumerate() {
                let (Ty::Struct(sid), Some(Some(tr))) = (t, targs.get(k)) else { continue };
                let (Some(&v), true) = (tr.first(), prog.is_placeholder(sid)) else { continue };
                // (an `impl Trait` argument's parameter shown as written: `impl Debug`, not `__impl0`)
                let sd = &prog.structs[sid as usize];
                let shown = match (sd.name.strip_suffix('#'), &sd.placeholder) {
                    (Some(b), Some(bs)) if super::p2_call::hidden_param(b) => {
                        let bs: Vec<String> = bs.iter().map(|b| b.replace(" <", "<").replace("< ", "<").replace(" >", ">").replace(" ,", ",")).collect();
                        format!("impl {}#", bs.join(" + "))
                    }
                    _ => sd.name.clone(),
                };
                rg.univ.push((v, shown));
                rg.ph.push((sid, v));
                let pname = gsig.generics.get(k).map(|g| g.0.as_str()).unwrap_or("");
                // (and what its bounds' traits require of their `Self` (`trait Tr<'a>: 'a`, `T: Tr<'x>`: T: 'x),
                // through supertraits)
                if !prog.trait_self_outlives.is_empty() {
                    let mut todo: Vec<(String, Vec<String>)> = gsig.generics.get(k).map(|g| g.1.iter().map(|b| (b.split('<').next().unwrap_or(b).trim().to_string(), bound_lts(b))).collect()).unwrap_or_default();
                    let mut seen = 0;
                    while let Some((tr, args)) = todo.pop() {
                        seen += 1;
                        if seen > 16 {
                            break;
                        }
                        let short = tr.rsplit("::").next().unwrap_or(&tr).to_string();
                        let find = |m: &HashMap<String, Vec<String>>| m.get(&tr).or_else(|| m.iter().find(|(n, _)| n.rsplit("::").next() == Some(&short)).map(|(_, x)| x)).cloned();
                        for c in find(&prog.trait_self_outlives).unwrap_or_default() {
                            let b = if c == "static" { Some(STATIC) } else { c.parse::<usize>().ok().and_then(|i| args.get(i)).and_then(|l| if l == "'static" { Some(STATIC) } else { rg.lt_names.iter().position(|x| x == l).and_then(|p| lts.get(p).copied()) }) };
                            if let Some(b) = b {
                                rg.known.push((v, b));
                            }
                        }
                        // (a supertrait's lifetimes: as written in the trait's header, not the bound's; only 'static
                        // and its unnamed ones are followed)
                        for sup in find(&prog.trait_supers).unwrap_or_default() {
                            todo.push((sup.split('<').next().unwrap_or(&sup).trim().to_string(), vec![]));
                        }
                    }
                }
                for (tp, c) in rs.map(|r| r.ty_outlives.as_slice()).unwrap_or(&[]) {
                    if tp == pname {
                        let b = if *c == R_STATIC { Some(STATIC) } else { lts.get(*c as usize).copied() };
                        if let Some(b) = b {
                            rg.known.push((v, b));
                        }
                    }
                }
                // (its associated types' regions, made before the parameters' trees: universal, and what each outlives)
                for (k2, asid, va, comps, known) in std::mem::take(&mut assoc_ph) {
                    if k2 != k {
                        assoc_ph.push((k2, asid, va, comps, known));
                        continue;
                    }
                    rg.univ.push((va, prog.structs[asid as usize].name.clone()));
                    rg.known.extend(known.into_iter().map(|r| (va, r)));
                    let mut cs = vec![v];
                    cs.extend(comps);
                    rg.proj_comps.push((va, cs));
                }
            }
        }
        let nuniv = rg.univ.len();
        // (or a signature that writes 'static: `fn f(x: isize) -> &'static [isize]`)
        rg.on = force || nuniv >= 2 || (nuniv >= 1 && (rs.is_some() || has_dyn(prog, ret_t, 0) || placeholder_static_bounds(prog, &rg.inst_args))) || rs.is_some_and(Self::needs_static) || (opaque && rg.ret.is_some());
        if rg.on {
            for (i, &(loc, t)) in params.iter().enumerate() {
                if let Some(tr) = trees[i].clone() {
                    rg.implied(prog, t, &tr, 0);
                    rg.locals.insert(loc, tr);
                }
            }
            // (an impl's method: its self type is well formed, as its callers make sure)
            if let (Some(r), Some(isid)) = (rs, gsig.impl_of) {
                if !r.self_slots.is_empty() {
                    if let Some(st) = rg.instantiate(prog, Ty::Struct(isid), Some(&r.self_slots), &lts, &targs, &mut |me| me.var()) {
                        rg.implied(prog, Ty::Struct(isid), &st, 0);
                        rg.self_tree = Some((Ty::Struct(isid), st));
                    }
                }
            }
            if let Some(tr) = rg.ret.clone() {
                rg.implied(prog, ret_t, &tr, 0);
            }
            if let Some(r) = rs {
                for &(a, b) in &r.outlives {
                    let m = |c: u8| if c == R_STATIC { Some(STATIC) } else { lts.get(c as usize).copied() };
                    if let (Some(x), Some(y)) = (m(a), m(b)) {
                        rg.known.push((x, y));
                    }
                }
            }
        }
        self.rg = rg;
    }

    /// The tree of a value (None: not followed)
    pub(crate) fn rt_expr(&mut self, e: &Expr) -> Option<(Ty, RT)> {
        let r = self.rt_expr_inner(e);
        if r.is_none() && debug() {
            self.rg.missed.push(kind(e));
            eprintln!("regionck: {} not followed at line {}", kind(e), self.line_base + super::borrowck::pos(e).line);
            if let Expr::Path(_, p) = strip(e) {
                let n = p.segs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("::");
                let lt = p.segs.last().and_then(|s| self.lookup(&s.name)).map(|(_, t)| self.prog.tn(t));
                eprintln!("regionck: path {n} not followed (local: {lt:?})");
            }
        }
        r
    }

    fn rt_expr_inner(&mut self, e: &Expr) -> Option<(Ty, RT)> {
        if let Some(x) = self.rg.calls.get(&(e as *const Expr as usize)) {
            return Some(x.clone());
        }
        match e {
            Expr::Paren(x) => self.rt_expr(x),
            // (`(&x, 0)`: the elements' trees in order; one of another type (a coercion) flows into fresh regions)
            Expr::Tuple(es) if !es.is_empty() => {
                let prog = self.prog;
                let t = self.peek(e)?;
                let Ty::Struct(sid) = t else { return None };
                if !prog.is_tuple(sid) || prog.structs[sid as usize].args.len() != es.len() {
                    return None;
                }
                let args = prog.structs[sid as usize].args.clone();
                let mut out = vec![];
                for (x, a) in es.iter().zip(args) {
                    let c = count(prog, a, 0)?;
                    if c == 0 {
                        continue;
                    }
                    let (xt, xtr) = self.rt_expr(x)?;
                    if xt == a && xtr.len() == c {
                        out.extend(xtr);
                    } else {
                        let f = self.rg.fresh(prog, a)?;
                        self.rg.flow(prog, xt, a, &xtr, &f, super::borrowck::pos(x));
                        out.extend(f);
                    }
                }
                Some((t, out))
            }
            Expr::Lit(s, _) if s.starts_with('"') || s.starts_with("r\"") || s.starts_with("r#") => Some((Ty::Ref(false, crate::types::intern(Ty::Str)), vec![STATIC])),
            // (`&f(..)` / `&mut 4` / `&{ let y = ..; y }`: a temporary the function owns (a `&` of a literal is
            // promoted to a static, and so may be a block of a constant alone))
            Expr::Ref(m, x)
                if matches!(strip(x), Expr::MethodCall(..))
                    || (matches!(strip(x), Expr::Call(..)) && self.not_const(x))
                    || self.rg.temps.contains_key(&(strip(x) as *const Expr as usize))
                    || (*m && matches!(strip(x), Expr::Lit(..)))
                    || matches!(strip(x), Expr::Block(None, false, b) if b.stmts.len() > 1)
                    || (matches!(strip(x), Expr::Ref(..) | Expr::Struct(..) | Expr::Tuple(..) | Expr::Array(..)) && (self.mentions_local(x) || self.not_const(x))) =>
            {
                let (t, tr) = match strip(x) {
                    x if self.rg.temps.contains_key(&(x as *const Expr as usize)) => (self.rg.temps[&(x as *const Expr as usize)], vec![]),
                    Expr::Lit(..) => (Ty::U8, vec![]),
                    Expr::Block(_, _, b) => match b.stmts.last() {
                        Some(Stmt::Expr(tail, false)) => self.rt_expr(tail)?,
                        _ => return None,
                    },
                    x @ (Expr::Ref(..) | Expr::Struct(..) | Expr::Tuple(..) | Expr::Array(..)) => self.rt_expr(x)?,
                    x => self.rg.calls.get(&(x as *const Expr as usize)).cloned()?,
                };
                let r = self.rg.var();
                let sp = Some(super::borrowck::pos(e)).filter(|s| s.line != 0).unwrap_or(self.rg.let_sp);
                self.rg.loans.push((r, String::new(), sp));
                let mut out = vec![r];
                out.extend(tr);
                Some((Ty::Ref(*m, crate::types::intern(t)), out))
            }
            // (`&f::<&'a i32>`: a fn item is a constant, its borrow promoted)
            Expr::Ref(false, x) if matches!(strip(x), Expr::Path(..)) && self.fn_item(x).is_some() => {
                let (t, tr) = self.rt_fn_item(x)?;
                let mut out = vec![STATIC];
                out.extend(tr);
                Some((Ty::Ref(false, crate::types::intern(t)), out))
            }
            Expr::Ref(_, x) if self.rg.coerced.iter().any(|c| c.0 == e as *const Expr as usize) => {
                let n = self.rg.coerced.iter().find(|c| c.0 == e as *const Expr as usize).map_or(0, |c| c.1);
                // (the reborrow `&**x` it is lives no longer than the references it goes through, as an explicit one:
                // a `&mut` link's region bounds it, a shared link stops there (deref)); those that reach the place x
                // first (`&s.r` of `s: &'short mut S<'long>` as `&'long i32` needs `'short: 'long`)
                let (mut t, mut tr, mut ds, place) = match self.rt_place(x) {
                    Some((t, tr, ds)) => (t, tr, ds, true),
                    None => {
                        let (t, tr) = self.rt_expr(x)?;
                        (t, tr, vec![], false)
                    }
                };
                let (ds0, owned_place) = (ds.len(), place && ds.is_empty());
                for _ in 0..n {
                    let Ty::Ref(m, i) = t else { return None };
                    if tr.is_empty() {
                        return None;
                    }
                    deref(&mut ds, tr.remove(0), m);
                    t = *i;
                }
                // (a region of its own, as an explicit reborrow's, which the borrow check's loan of it follows: kept
                // to the function's end when it must outlive a lifetime of the signature (`let r: &'a i32 = &x;`))
                // (through `&mut` links only, the reborrow goes through the borrow of the local the coercion takes
                // (`&**(&x)`): it lives no longer than x, E0515 when returned (`&x` of `x: &'a mut i32` as `&'a i32`).
                // A shared link stops that there)
                if let (Ty::Ref(m, _), Some(&last)) = (t, tr.first()) {
                    deref(&mut ds, last, m);
                    let all_mut = ds.len() == ds0 + n + 1 && m;
                    let r = self.rg.var();
                    let sp = super::borrowck::pos(e);
                    for d in ds {
                        self.rg.edge(d, r, sp);
                    }
                    tr[0] = r;
                    if self.rg.closure.is_none() {
                        self.rg.reborrows.push((&**x as *const Expr as usize, r));
                        // (only of a place the function owns: one reached through a reference (`&s.f` of `s: &mut S`,
                        // `&*x`) is a reborrow through it, as the explicit path's)
                        let owned = all_mut && owned_place && !matches!(strip(x), Expr::Call(..) | Expr::MethodCall(..));
                        if owned {
                            if let Some(name) = place_text(x) {
                                self.rg.loans.push((r, name, sp));
                            }
                        }
                    }
                }
                Some((t, tr))
            }
            Expr::Ref(m, x) => {
                let (t, tr, derefs) = self.rt_place(x)?;
                let r = self.rg.var();
                let sp = super::borrowck::pos(e);
                // (of a place the function owns: the borrow ends with the function at the latest)
                if derefs.is_empty() && !matches!(strip(x), Expr::Call(..) | Expr::MethodCall(..)) {
                    if let Some(n) = place_text(x) {
                        self.rg.loans.push((r, n, sp));
                    }
                }
                // (a reborrow through references: it lives no longer than they do)
                if self.rg.closure.is_none() {
                    if derefs.is_empty() {
                        self.rg.owned_borrows.push((&**x as *const Expr as usize, r));
                    } else {
                        self.rg.reborrows.push((&**x as *const Expr as usize, r));
                    }
                }
                for d in derefs {
                    self.rg.edge(d, r, sp);
                }
                let mut out = vec![r];
                out.extend(tr);
                Some((Ty::Ref(*m, crate::types::intern(t)), out))
            }
            Expr::Call(..) | Expr::MethodCall(..) | Expr::Struct(..) | Expr::Tuple(..) | Expr::Array(..) | Expr::Closure(..) => self.rg.calls.get(&(e as *const Expr as usize)).cloned(),
            // (`e as &'static T`: the cast's type, with the regions written in it; e flows into it)
            Expr::Cast(x, ty) => {
                let (it, itr) = self.rt_expr(x)?;
                let to = self.parse_ty(ty).ok()?;
                let prog = self.prog;
                // (`p as _` with p a pointer to a trait object: as typeck, the same type when nothing else is expected;
                // its lifetime inferred)
                let infer = matches!(to, Ty::Param(h) if h == crate::program::HOLE) && matches!(it, Ty::Ptr(_, i) if dyn_tail(prog, *i, 0));
                let to = if infer { it } else { to };
                // (an unsizing cast (`Box::new(v) as Box<dyn Tr + 'static>`): a coercion, the value flows into the
                // type written)
                // (a raw pointer to a value with no trait object in it, cast to one to a trait object (`p as *const (dyn
                // Tr + 'static)`), unsizes too; one between pointers to trait objects is not a coercion)
                let unsize = to != it && has_dyn(prog, to, 0) && (!matches!(to, Ty::Ptr(..)) || !has_dyn(prog, it, 0));
                if to != it && !unsize {
                    return None;
                }
                if infer {
                    // (fresh regions; the object's outlived by the source's)
                    let tree = self.rg.fresh(prog, to)?;
                    if let (Some(&a), Some(&b)) = (itr.last(), tree.last()) {
                        self.rg.edge(a, b, super::borrowck::pos(x));
                    }
                    return Some((to, tree));
                }
                let names = self.rg.lt_names.clone();
                let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                let mut codes = vec![];
                s.slots(ty, false, &mut codes);
                let lts = self.rg.lts.clone();
                self.rg.dyn_infer = true;
                let tree = self.rg.instantiate(prog, to, Some(&codes), &lts, &[], &mut |me| me.var());
                self.rg.dyn_infer = false;
                let tree = tree?;
                if unsize {
                    self.rg.flow(prog, it, to, &itr, &tree, super::borrowck::pos(x));
                    return Some((to, tree));
                }
                // (a cast of a raw pointer may change the regions of what it points to; not a trait object's
                // lifetime: `*mut (dyn Tr + 'a) as *mut (dyn Tr + 'static)` is an error)
                // (`*mut S<dyn Tr + 'a>` too: the object at the end of what it points to)
                match to {
                    Ty::Ptr(_, i) if dyn_tail(prog, *i, 0) => {
                        if let (Some(&a), Some(&b)) = (itr.last(), tree.last()) {
                            self.rg.edge(a, b, super::borrowck::pos(x));
                        }
                    }
                    Ty::Ptr(..) => {}
                    _ => self.rg.relate(prog, to, &itr, &tree, super::borrowck::pos(x)),
                }
                Some((to, tree))
            }
            // (an `if` / `match`'s value: each branch's that does not diverge flows into it)
            Expr::If(_, b, els) => {
                let mut vs: Vec<&Expr> = vec![];
                match b.stmts.last() {
                    Some(Stmt::Expr(t, false)) => vs.push(t),
                    _ => return None,
                }
                vs.push(els.as_deref()?);
                self.rt_branches(&vs)
            }
            Expr::Match(_, arms) => {
                if let Some(&m2) = self.rg.alias.get(&(e as *const Expr as usize)) {
                    return self.rt_expr(m2);
                }
                let vs: Vec<&Expr> = arms.iter().map(|a| &a.body).collect();
                self.rt_branches(&vs)
            }
            // (a block's value (an `unsafe` one's too): its tail's, kept while the block's locals were in scope)
            Expr::Block(None, _, b) => match b.stmts.last() {
                Some(Stmt::Expr(t, false)) => self.rt_expr(t),
                _ => None,
            },
            Expr::Path(..) | Expr::Field(..) | Expr::Unary(..) => match self.rt_place(e) {
                Some((t, tr, _)) => Some((t, tr)),
                // (a fn item as a value (`foo::<&'a ()>`); a unit variant: `None`)
                None if matches!(e, Expr::Path(..)) && self.fn_item(e).is_some() => self.rt_fn_item(e),
                None => self.rg.calls.get(&(e as *const Expr as usize)).cloned(),
            },
            _ => None,
        }
    }

    /// `&e` borrows a temporary of e's value of type t (not a place, not promoted): one the region check follows when
    /// e is no constant (a call of a function, an operation on a local) and t holds no region
    pub(crate) fn rg_temp(&mut self, e: &Expr, t: Ty) {
        if !self.rg_on() {
            return;
        }
        let x = strip(e);
        let k = x as *const Expr as usize;
        // (a constant whose type needs dropping (`&Test` with `impl Drop for Test`) is not promoted either)
        let dropped = || matches!(x, Expr::Path(None, p) if p.segs.len() != 1 || self.lookup(&p.segs[0].name).is_none()) && self.needs_drop(t);
        if self.rg.calls.contains_key(&k) || !(self.not_const(x) || dropped()) || self.bk_ty(t).1 {
            return;
        }
        self.rg.temps.insert(k, t);
    }

    /// e is not a constant expression a `&` would promote to a static: it reads a local, calls a function (not the
    /// few promoted ones: `ptr::null()`, `mem::size_of()`), or compares, reads or casts to integers raw pointers
    fn not_const(&self, e: &Expr) -> bool {
        let ptr_cast = |x: &Expr| matches!(strip(x), Expr::Cast(_, t) if matches!(**t, crate::parse::Ty::Ptr(..) | crate::parse::Ty::Fn(..)));
        match strip(e) {
            // (a static: a constant expression does not borrow one)
            Expr::Path(None, p) if p.segs.len() == 1 => {
                self.lookup(&p.segs[0].name).is_some() || self.prog.const_index.get(&p.segs[0].name).is_some_and(|&ci| self.prog.consts[ci as usize].stat.is_some())
            }
            Expr::Path(None, _) => self.static_of(e).is_some(),
            Expr::Ref(_, x) => self.not_const(x),
            Expr::MethodCall(..) => true,
            Expr::Call(f, args) => match strip(f) {
                Expr::Path(_, p)
                    if self.prog.variant_by_p(&crate::program::PPath::of(p), self.self_sid).is_some()
                        || (self.prog.struct_by_p(&crate::program::PPath::of(p), self.self_sid).is_some()
                            && self.prog.fn_by_p(&crate::program::PPath::of(p), self.self_sid).is_none()) =>
                {
                    args.iter().any(|a| self.not_const(a))
                }
                Expr::Path(_, p) => p.segs.last().is_none_or(|s| !matches!(s.name.as_str(), "null" | "null_mut" | "size_of" | "align_of" | "max_value" | "min_value" | "new" | "from_secs" | "from_millis" | "from_micros" | "from_nanos")),
                _ => true,
            },
            Expr::Binary(op, a, b, _) => self.not_const(a) || self.not_const(b) || (matches!(op.as_str(), "==" | "!=" | "<" | "<=" | ">" | ">=") && (ptr_cast(a) || ptr_cast(b))),
            Expr::Unary(op, x, _) => op == "*" || self.not_const(x),
            Expr::Cast(x, to) => self.not_const(x) || (matches!(**to, crate::parse::Ty::Path(..)) && ptr_cast(x)),
            Expr::Field(x, _, _) => self.not_const(x),
            Expr::Block(None, _, b) if b.stmts.len() == 1 => matches!(&b.stmts[0], Stmt::Expr(t, false) if self.not_const(t)),
            Expr::Tuple(xs) | Expr::Array(xs) => xs.iter().any(|x| self.not_const(x)),
            Expr::Struct(_, fs, _) => fs.iter().any(|f| self.not_const(&f.1)),
            _ => false,
        }
    }

    /// The value of branches (an `if`'s, a `match`'s arms): a fresh tree of the first one's type each flows into;
    /// a branch that diverges (`panic!()`, `return`) has none
    fn rt_branches(&mut self, vs: &[&Expr]) -> Option<(Ty, RT)> {
        let mut got: Vec<(Ty, RT, Sp)> = vec![];
        for v in vs {
            if diverges(v) {
                continue;
            }
            let (t, tr) = self.rt_expr(v)?;
            got.push((t, tr, epos(v)));
        }
        let t = got.first()?.0;
        let prog = self.prog;
        let tree = self.rg.fresh(prog, t)?;
        for (vt, vtr, sp) in got {
            self.rg.flow(prog, vt, t, &vtr, &tree, sp);
        }
        Some((t, tree))
    }

    /// e reads a local (so it is not a constant a `&` would promote to a static)
    fn mentions_local(&self, e: &Expr) -> bool {
        match strip(e) {
            Expr::Path(None, p) => p.segs.len() == 1 && self.lookup(&p.segs[0].name).is_some(),
            Expr::Ref(_, x) | Expr::Unary(_, x, _) | Expr::Field(x, _, _) | Expr::Cast(x, _) => self.mentions_local(x),
            Expr::Binary(_, a, b, _) => self.mentions_local(a) || self.mentions_local(b),
            Expr::Tuple(xs) | Expr::Array(xs) => xs.iter().any(|x| self.mentions_local(x)),
            Expr::Struct(_, fs, _) => fs.iter().any(|f| self.mentions_local(&f.1)),
            _ => false,
        }
    }

    /// A function the gate left out begins its check here, where something must be 'static: what it owns must not
    /// be borrowed for that (what came before has no trees and adds no constraint)
    pub(crate) fn rg_late_on(&mut self) {
        if !self.rg.on && self.rg.eligible {
            self.rg.on = true;
            self.rg.began_late = true;
            // (the parameters' trees: those the signature gives them)
            if let Some((self_elides, other_self)) = self.rg.late.take() {
                // (a parameter's binding is the outermost of its name: a later `let` may shadow it)
                let ps: Vec<(Loc, Ty)> = self
                    .bk
                    .params
                    .iter()
                    .filter_map(|&l| {
                        let n = &self.bk.locals[l as usize].name;
                        self.scopes.iter().find_map(|s| s.iter().find(|b| b.0 == *n)).map(|b| (b.1, b.2))
                    })
                    .collect();
                if debug() {
                    eprintln!("regionck: late begin with {} of {} parameters ({:?})", ps.len(), self.bk.params.len(), self.inst_params_len());
                }
                if ps.len() == self.bk.params.len() && self.inst_params_len() == Some(ps.len()) {
                    self.rg_begin(None, &ps, self_elides, true);
                    if other_self {
                        self.rg.ret = None;
                    }
                }
            }
        }
    }

    /// The number of parameters of the function being checked
    fn inst_params_len(&self) -> Option<usize> {
        self.prog.sigs.get(self.rg.base as usize).map(|s| s.params.len())
    }

    /// A parameter's trait object not behind a reference (`Box<dyn Tr>`): 'static by default, written or not
    pub(crate) fn static_object(t: Ty) -> bool {
        match t {
            Ty::Dyn(_) => true,
            Ty::Box(i) => Self::static_object(*i),
            _ => false,
        }
    }

    /// The signature requires a region of it to be 'static
    /// Of a callee: what it requires of its arguments writes 'static (not its result's 'static, which requires
    /// nothing of the caller)
    pub(crate) fn callee_needs_static(r: &RSigSyn) -> bool {
        let n = r.slots.len().saturating_sub(1);
        r.slots[..n].iter().flatten().any(|&c| c == R_STATIC) || r.ty_outlives.iter().any(|x| x.1 == R_STATIC) || r.outlives.iter().any(|x| x.1 == R_STATIC) || r.self_slots.contains(&R_STATIC)
    }

    pub(crate) fn needs_static(r: &RSigSyn) -> bool {
        r.slots.iter().flatten().any(|&c| c == R_STATIC) || r.ty_outlives.iter().any(|x| x.1 == R_STATIC) || r.outlives.iter().any(|x| x.1 == R_STATIC) || r.self_slots.contains(&R_STATIC)
    }

    /// The tree of a place, and the regions of the references it goes through
    pub(crate) fn rt_place(&mut self, e: &Expr) -> Option<(Ty, RT, Vec<u32>)> {
        match e {
            Expr::Paren(x) => self.rt_place(x),
            Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => {
                let (loc, t) = self.lookup(&p.segs[0].name)?;
                // (a closure's capture: reached through its environment, not owned)
                if self.rg.cap_locs.contains(&loc) {
                    return Some((t, self.rg.locals.get(&loc)?.clone(), vec![self.rg.env_r]));
                }
                let tr = match self.rg.locals.get(&loc) {
                    Some(tr) => tr.clone(),
                    // (a local whose type has no region (bound before the check began): its tree is empty)
                    None if count(self.prog, t, 0) == Some(0) => vec![],
                    None => {
                        if debug() {
                            eprintln!("regionck: local {} ({:?}) has no tree", p.segs[0].name, loc);
                        }
                        if self.rg.began_late {
                            self.rg.rerun = true;
                        }
                        return None;
                    }
                };
                Some((t, tr, vec![]))
            }
            Expr::Unary(op, x, _) if op == "*" => {
                let (t, tr, mut ds) = self.rt_place(x)?;
                match t {
                    Ty::Ref(m, i) => {
                        deref(&mut ds, *tr.first()?, m);
                        Some((*i, tr[1..].to_vec(), ds))
                    }
                    Ty::Box(i) => Some((*i, tr, ds)),
                    // (`*s` of a String or a Vec: what it owns, as a Box's)
                    Ty::Struct(sid) => {
                        let prog = self.prog;
                        let sd = &prog.structs[sid as usize];
                        match prog.structs[prog.base_of(sid) as usize].name.as_str() {
                            "String" if sd.args.is_empty() => Some((Ty::Str, tr, ds)),
                            "Vec" if sd.args.len() == 1 && lts_of(prog, prog.base_of(sid)) == 0 => Some((Ty::Slice(crate::types::intern(sd.args[0])), tr, ds)),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
            Expr::Field(x, f, _) => {
                let (mut t, mut tr, mut ds) = self.rt_place(x)?;
                // (auto-deref)
                loop {
                    match t {
                        Ty::Ref(m, i) => {
                            deref(&mut ds, *tr.first()?, m);
                            tr = tr[1..].to_vec();
                            t = *i;
                        }
                        Ty::Box(i) => t = *i,
                        _ => break,
                    }
                }
                let Ty::Struct(sid) = t else { return None };
                let sd = &self.prog.structs[sid as usize];
                if sd.is_enum {
                    return None;
                }
                let i = sd.fields.iter().position(|x| x.0 == *f)?;
                let ft = sd.fields[i].1;
                let prog = self.prog;
                let fr = self.rg.field(prog, sid, i, &tr)?;
                Some((ft, fr, ds))
            }
            // (an element of an array, a slice or a Vec (`&mut v[..]`): its tree is the element type's)
            Expr::Index(x, ix, _) => {
                let (mut t, mut tr, mut ds) = self.rt_place(x)?;
                loop {
                    match t {
                        Ty::Ref(m, i) => {
                            deref(&mut ds, *tr.first()?, m);
                            tr = tr[1..].to_vec();
                            t = *i;
                        }
                        Ty::Box(i) => t = *i,
                        _ => break,
                    }
                }
                let et = match t {
                    Ty::Array(e, _) | Ty::Slice(e) => *e,
                    Ty::Struct(sid) => {
                        let sd = &self.prog.structs[sid as usize];
                        if self.prog.structs[self.prog.base_of(sid) as usize].name != "Vec" || sd.args.len() != 1 || lts_of(self.prog, self.prog.base_of(sid)) != 0 {
                            return None;
                        }
                        sd.args[0]
                    }
                    _ => return None,
                };
                let t = if matches!(strip(ix), Expr::Range(..)) { Ty::Slice(crate::types::intern(et)) } else { et };
                Some((t, tr, ds))
            }
            Expr::Call(..) | Expr::MethodCall(..) => {
                let (t, tr) = self.rg.calls.get(&(e as *const Expr as usize)).cloned()?;
                Some((t, tr, vec![]))
            }
            _ => None,
        }
    }

    /// `let name[: ann] = init;` (after name is bound)
    pub(crate) fn rg_let(&mut self, name: &str, ann: Option<&PTy>, init: &Expr, init_tree: Option<(Ty, RT)>) {
        if !self.rg_on() {
            return;
        }
        let Some((loc, t)) = self.lookup(name) else { return };
        let prog = self.prog;
        let fn_item_init = self.fn_item(super::strip_paren(init)).is_some();
        let mut ann_codes: Option<Vec<u8>> = None;
        // (an annotation names the function's lifetimes; its elided ones are fresh)
        let tree = match ann {
            Some(a) => {
                let names = self.rg.lt_names.clone();
                let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                let mut codes = vec![];
                s.slots(a, false, &mut codes);
                ann_codes = Some(codes.clone());
                let lts = self.rg.lts.clone();
                self.rg.dyn_infer = true;
                let tr = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var());
                self.rg.dyn_infer = false;
                // (an annotation with holes whose regions are the inferred type's (`let f: fn(_) -> _ = foo;`): fresh
                // ones, which the initializer flows into. A fn pointer type's parts that are no hole keep the regions
                // they write: `fn(_, &'a T) -> &'static T`)
                let sp = crate::wf::pos_of(a).filter(|s| s.line != 0).unwrap_or(self.rg.let_sp);
                self.rg_proj_wf(a, sp);
                tr.or_else(|| self.rg_proj_ann(a, t)).or_else(|| if crate::ast_summary::has_infer(a) { self.rg_alias_ann(a, t).or_else(|| self.rg_fn_ann_holes(a, t)).or_else(|| self.rg.fresh(prog, t)) } else { None })
            }
            // (a fn item with no type written (`let g = id;`): the local is of the item's type, whose own regions a
            // call through it takes afresh, its type arguments' are fixed (`let f = id::<&'static u32>;`))
            None if fn_item_init => match &init_tree {
                Some((it, itr)) if *it == t => Some(itr.clone()),
                _ => self.rg.fresh(prog, t),
            },
            None => self.rg.fresh(prog, t),
        };
        let Some(tree) = tree else { return };
        if let Some(a) = ann {
            self.rg_closure_fnptr(a, init);
            self.rg_closure_passes(init, t, &tree);
        }
        // (a generic fn item whose type arguments the annotation gives (`let f: fn(_, &'a T) -> &'static T = foo;`):
        // its instance's tree, the type arguments' regions fresh)
        let init_tree = match init_tree {
            None if fn_item_init && ann.is_some() => self.rt_fn_item_as(init, t),
            x => x,
        };
        // (the annotation's type is well formed)
        if ann.is_some() {
            let sp = Some(super::borrowck::pos(init)).filter(|s| s.line != 0).unwrap_or(self.rg.let_sp);
            self.rg.wf(prog, t, &tree, sp);
        }
        // (the initializer's tree was taken before the binding: `let y = y;` reads the y it shadows)
        if let Some((it, itr)) = init_tree {
            self.rg.flow(prog, it, t, &itr, &tree, super::borrowck::pos(init));
            // (a fn item coerced to the annotation's fn pointer type: its signature's types are well formed, which
            // relates its regions (`foo` of `fn foo<'a, 'b, T>(_: &'a &'b (), v: &'b T) -> &'a T` needs 'b: 'a), as
            // rustc requires where an item is made a fn pointer). Not where the type binds a region (`fn(_, &'a T,
            // &())`): rustc does not check it there (#25860, implied-bounds-on-nested-references-plus-variance-2.rs)
            let binds = ann_codes.as_ref().is_some_and(|c| c.contains(&R_BOUND));
            if let (true, Some(_), Ty::Fn(ft), false) = (fn_item_init, ann, it, binds) {
                let sp = Some(super::borrowck::pos(init)).filter(|s| s.line != 0).unwrap_or(self.rg.let_sp);
                let mut k = 0;
                for &p in ft.params.iter().chain(std::iter::once(&ft.ret)) {
                    let Some(n) = count(prog, p, 0) else { break };
                    let Some(seg) = itr.get(k..k + n) else { break };
                    let seg = seg.to_vec();
                    self.rg.wf(prog, p, &seg, sp);
                    k += n;
                }
            }
            // (a fn item where the annotation's fn pointer type binds its regions (`let g: for<'cx> fn(&'cx S) ->
            // &'static S = baz;`): it must do for every region the type binds, a placeholder (as for an argument,
            // rg_call). The local keeps its own tree: each call through it takes them afresh)
            let bound = matches!(t, Ty::Fn(ft) if !ft.params.iter().chain(std::iter::once(&ft.ret)).any(|&x| has_fn_ty(x, 0)));
            if let (true, true, Some(codes)) = (fn_item_init, bound, ann_codes.filter(|c| c.contains(&R_BOUND))) {
                let u = self.rg.var();
                self.rg.univ.push((u, "'_".to_string()));
                self.rg.bound_univ = Some(u);
                let lts = self.rg.lts.clone();
                let ph = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var());
                self.rg.bound_univ = None;
                // (a tree of the item's own, not the one that flowed into the local's: the two do not share regions)
                if let (Some(ph), Some((it2, itr2))) = (ph, self.rt_expr(init)) {
                    self.rg.flow(prog, it2, t, &itr2, &ph, super::borrowck::pos(init));
                }
            }
        }
        self.rg.locals.insert(loc, tree);
    }

    /// E0261 in a body: the lifetimes a `let` annotation names (outside fn pointer, `impl` and `dyn` types, which may
    /// bind their own) are the function's (its impl's included). Not in a closure's body, whose function's are in
    /// scope too
    pub(crate) fn body_lifetimes(&self, t: &PTy, sp: Sp) -> super::R<()> {
        if !self.user_code || self.inline_depth > 0 || !crate::regions::names_lt(t) || self.prog.closures.contains_key(&self.rg.base) {
            return Ok(());
        }
        let mut used = vec![];
        crate::ast_summary::named_lifetimes(t, &mut used);
        let declared = self.prog.sigs.get(self.rg.base as usize).and_then(|s| s.regions.as_deref()).map(|r| r.lts.as_slice()).unwrap_or(&[]);
        match used.iter().find(|l| !matches!(l.as_str(), "'static" | "'_") && !l.starts_with("'r#") && !declared.iter().any(|d| d == *l)) {
            Some(l) => Err(self.at(sp, format!("use of undeclared lifetime name `{l}` (E0261)"))),
            None => Ok(()),
        }
    }

    /// A call through `dyn Fn(..)` whose bound writes lifetimes (s: the bound's text): the arguments flow into its
    /// parameters' types, with the regions written there (those it elides are its own)
    pub(crate) fn rg_dyn_fn_args(&mut self, s: &str, args: &[&Expr]) {
        // (the object's trait: `+ Send` after it would read as an addition to the cast below)
        let s = crate::types::dyn_autos(s).0;
        if !self.rg_on() || !s.contains('\'') {
            return;
        }
        let Ok(Expr::Cast(_, pt)) = crate::parse::parse_expr(&format!("0 as &dyn {s}")) else { return };
        let PTy::Ref(_, _, inner) = &*pt else { return };
        let PTy::Dyn(bs) = &**inner else { return };
        let Some(ps) = bs.iter().find_map(|b| match b {
            crate::parse::Bound::Trait(_, p) => p.segs.last().and_then(|sg| sg.fn_args.as_ref()).map(|f| f.0.clone()),
            _ => None,
        }) else {
            return;
        };
        let prog = self.prog;
        for (a, p) in args.iter().zip(&ps) {
            if !crate::regions::names_lt(p) {
                continue;
            }
            let Ok(t) = self.parse_ty(p) else { continue };
            let mut sl = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 1, derived: None };
            let mut codes = vec![];
            sl.slots(p, false, &mut codes);
            let lts = self.rg.lts.clone();
            let Some(tree) = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var()) else { continue };
            let Some((at, atr)) = self.rt_expr(a) else { continue };
            let sp = super::borrowck::pos(a);
            self.rg.flow(prog, at, t, &atr, &tree, sp);
        }
    }

    /// A fn item as a value (`f::<&'a i32>`): a fn pointer whose regions are its signature's, with the lifetimes
    /// the type arguments written name; its own lifetimes fresh
    /// The tree of a generic fn item's instance whose type is want (a fn pointer type it is coerced to): its type
    /// arguments solved from want, their regions fresh
    fn rt_fn_item_as(&mut self, e: &Expr, want: Ty) -> Option<(Ty, RT)> {
        let id = self.fn_item(strip(e))?;
        let prog = self.prog;
        let sig = &prog.sigs[id as usize];
        let Ty::Fn(ft) = want else { return None };
        if sig.generics.is_empty() || sig.trait_of.is_some() || sig.params.len() != ft.params.len() {
            return None;
        }
        let mut sol = vec![None; sig.generics.len()];
        for (&p, &q) in sig.params.iter().zip(&ft.params).chain(std::iter::once((&sig.ret, &ft.ret))) {
            if !prog.match_ty(p, q, &mut sol) {
                return None;
            }
        }
        let sol: Vec<Ty> = sol.into_iter().collect::<Option<Vec<Ty>>>()?;
        let targs: Vec<Option<RT>> = sol.iter().map(|&t| self.rg.fresh(prog, t)).collect();
        self.rt_fn_sig(id, &sol, &targs)
    }

    /// The tree of a projection written as a type (`<&'static () as Foo>::Out`) whose associated type's value names
    /// lifetimes (Program::assoc_regions): the value's codes, with the impl's lifetimes those its self type writes there
    /// (fresh where it elides them). Only where one impl's associated type of the name fits the self type
    pub(crate) fn rg_proj_ann(&mut self, ann: &PTy, t: Ty) -> Option<RT> {
        let (ai, lts, targs) = self.proj_impl(ann)?;
        let prog = self.prog;
        let codes = prog.assoc_regions.get(&ai)?.0.clone();
        let value = prog.assocs[ai as usize].value;
        let tr = self.rg.instantiate(prog, value, Some(&codes), &lts, &targs, &mut |me| me.var())?;
        (count(prog, t, 0) == Some(tr.len())).then_some(tr)
    }

    /// The impl a projection written in the function selects (`<&'a () as Visitor<'d>>::Value`), where one impl's
    /// associated type of the name fits and the summaries keep its regions: its index in assocs, its lifetimes, and
    /// its type parameters' trees. Its lifetimes and type arguments are the parts of the self type as written where
    /// the impl's self type has them (`<W<'static, &'static u32> as Foo>::Out` with `impl<'a, T> Foo for W<'a, T>`
    /// gives 'a 'static and T `&'static u32`; a blanket impl's `T` is all of it: `<&'a u8 as Tr>::Ty` with
    /// `impl<T> Tr for T`), and the lifetimes the trait path writes where the impl's trait path has them (`'d` of
    /// `Visitor<'d>`). What is not found is fresh
    fn proj_impl(&mut self, ann: &PTy) -> Option<(u32, Vec<u32>, Vec<Option<RT>>)> {
        let PTy::Path(Some(q), path) = ann else { return None };
        let prog = self.prog;
        if prog.assoc_regions.is_empty() {
            return None;
        }
        let name = path.segs.last()?.name.as_str();
        let qt = self.parse_ty(q).ok()?;
        let mut found = None;
        for (i, a) in prog.assocs.iter().enumerate() {
            if a.name != name || a.ngat != 0 {
                continue;
            }
            let mut sol = vec![None; a.ngen];
            if prog.match_ty(a.self_ty, qt, &mut sol) && !matches!((a.self_ty, qt), (Ty::Ref(m1, _), Ty::Ref(m2, _)) if m1 != m2) {
                if found.is_some() {
                    return None;
                }
                found = Some((i as u32, sol));
            }
        }
        let (ai, sol) = found?;
        let (codes, self_slots, outlives, trait_lts) = prog.assoc_regions.get(&ai)?.clone();
        let a = &prog.assocs[ai as usize];
        let mut sl = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
        let mut written = vec![];
        sl.slots(q, false, &mut written);
        let fl = self.rg.lts.clone();
        let qtree = self.rg.instantiate(prog, qt, Some(&written), &fl, &[], &mut |me| me.var());
        let n = codes
            .iter()
            .chain(&self_slots)
            .chain(&trait_lts)
            .chain(outlives.iter().flat_map(|(x, y)| [x, y]))
            .filter(|&&c| c < R_STATIC)
            .map(|&c| c as usize + 1)
            .max()
            .unwrap_or(0);
        let mut found_l: Vec<Option<u32>> = vec![None; n];
        let mut found_t: Vec<Option<RT>> = vec![None; sol.len()];
        if let Some(qtree) = qtree.filter(|q| count(prog, qt, 0) == Some(q.len())) {
            // (the marks of a path's elided lifetimes left out where the codes then match the impl's self type's
            // regions)
            // (and the marks a fn pointer type's codes begin with, R_FN, which stand for no region)
            let want = count(prog, a.self_ty, 0);
            let ss: Vec<u8> = self_slots.iter().copied().filter(|&c| c != R_FN).collect();
            let ss: Vec<u8> = if want == Some(ss.len()) { ss } else { ss.into_iter().filter(|&c| c != R_ELIDED_ADT).collect() };
            if want == Some(ss.len()) {
                let (mut at, mut si) = (0, 0);
                if proj_split(prog, a.self_ty, qt, &qtree, &mut at, &ss, &mut si, &mut found_l, &mut found_t, 0).is_none() {
                    found_l = vec![None; n];
                    found_t = vec![None; sol.len()];
                }
            }
        }
        // (the trait path's lifetime arguments: `<&'a () as Visitor<'d>>`'s 'd where the impl's trait path writes its
        // 'd)
        if let Some(tseg) = path.segs.len().checked_sub(2).and_then(|k| path.segs.get(k)) {
            let wl: Vec<&str> = tseg.args.iter().filter_map(|g| if let GArg::Lifetime(l) = g { Some(l.as_str()) } else { None }).collect();
            if wl.len() == trait_lts.len() {
                for (&c, l) in trait_lts.iter().zip(wl) {
                    let r = if l == "'static" { Some(STATIC) } else { self.rg.lt_names.iter().position(|x| x == l).and_then(|p| self.rg.lts.get(p).copied()) };
                    if let (Some(slot), Some(r)) = (found_l.get_mut(c as usize).filter(|x| x.is_none()), r) {
                        *slot = Some(r);
                    }
                }
            }
        }
        let mut targs: Vec<Option<RT>> = vec![];
        for (k, s0) in sol.iter().enumerate() {
            let Some(st) = *s0 else { return None };
            let tr = match found_t[k].take() {
                Some(tr) => Some(tr),
                None => self.rg.fresh(prog, st),
            };
            targs.push(tr);
        }
        let lts: Vec<u32> = found_l.into_iter().map(|v| v.unwrap_or_else(|| self.rg.var())).collect();
        Some((ai, lts, targs))
    }

    /// A projection written in the function (a `let`'s type, a parameter's, the result's) requires what the header of
    /// the impl it selects declares of its lifetimes: `<&'a () as Visitor<'d>>::Value` with `impl<'a, 'd: 'a> Visitor<'d>
    /// for &'a ()` requires `'d: 'a` (nll/normalization-bounds-error.rs)
    pub(crate) fn rg_proj_wf(&mut self, ann: &PTy, sp: Sp) {
        if self.prog.assoc_regions.is_empty() {
            return;
        }
        // (a projection within the type too: `Option<<&'a () as Visitor<'d>>::Value>`, `&'x <S as Tr<'y>>::Out`)
        match ann {
            PTy::Path(Some(q), p) => {
                self.rg_proj_wf(q, sp);
                for a in p.segs.iter().flat_map(|s| &s.args) {
                    if let GArg::Ty(t) = a {
                        self.rg_proj_wf(t, sp);
                    }
                }
            }
            PTy::Path(None, p) => {
                for a in p.segs.iter().flat_map(|s| &s.args) {
                    if let GArg::Ty(t) = a {
                        self.rg_proj_wf(t, sp);
                    }
                }
                return;
            }
            PTy::Ref(_, _, i) | PTy::Ptr(_, i) | PTy::Slice(i) | PTy::Array(i, _) => return self.rg_proj_wf(i, sp),
            PTy::Tuple(ts) => {
                for t in ts {
                    self.rg_proj_wf(t, sp);
                }
                return;
            }
            _ => return,
        }
        let Some((ai, lts, _)) = self.proj_impl(ann) else { return };
        let Some(outlives) = self.prog.assoc_regions.get(&ai).map(|r| r.2.clone()) else { return };
        let r = |c: u8| if c == R_STATIC { Some(STATIC) } else { lts.get(c as usize).copied() };
        for (a, b) in outlives {
            if let (Some(x), Some(y)) = (r(a), r(b)) {
                self.rg.edge(x, y, sp);
            }
        }
    }

    /// The tree of an annotation that is a generic type alias with holes among its arguments (`DoubleCell<_>` with
    /// `type DoubleCell<A> = Cell<(A, A)>`): a parameter's occurrences in the alias share one tree, its argument's (a
    /// hole's fresh). The alias's own regions are fresh: the summaries keep no lifetimes of its definition
    fn rg_alias_ann(&mut self, a: &PTy, t: Ty) -> Option<RT> {
        let PTy::Path(None, p) = a else { return None };
        let [seg] = p.segs.as_slice() else { return None };
        let prog = self.prog;
        let (gens, syn) = prog.aliases.get(&seg.name)?;
        if gens.is_empty() || gens.iter().any(|g| g.starts_with('\'')) || seg.args.len() != gens.len() {
            return None;
        }
        let args: Vec<&PTy> = seg.args.iter().filter_map(|x| if let GArg::Ty(t) = x { Some(t) } else { None }).collect();
        if args.len() != gens.len() {
            return None;
        }
        let gn: Vec<&str> = gens.iter().map(|s| s.as_str()).collect();
        let body = prog.resolve_g(syn, &gn).ok()?;
        let mut sol = vec![None; gens.len()];
        if !prog.match_ty(body, t, &mut sol) {
            return None;
        }
        let names = self.rg.lt_names.clone();
        let lts = self.rg.lts.clone();
        let mut targs = vec![];
        for (k, pa) in args.iter().enumerate() {
            let st = sol[k]?;
            let tr = if crate::ast_summary::has_infer(pa) {
                self.rg.fresh(prog, st)
            } else {
                let mut sl = crate::regions::Slots { lts: names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                let mut codes = vec![];
                sl.slots(pa, false, &mut codes);
                self.rg.instantiate(prog, st, Some(&codes), &lts, &[], &mut |me| me.var())
            };
            targs.push(Some(tr?));
        }
        let tr = self.rg.instantiate(prog, body, None, &lts, &targs, &mut |me| me.var())?;
        (count(prog, t, 0) == Some(tr.len())).then_some(tr)
    }

    /// The tree of a fn pointer annotation with holes (`fn(_, &'a T) -> &'static T`), part by part: a hole's regions
    /// fresh, the other parts' as written. None where a hole is not a whole part
    fn rg_fn_ann_holes(&mut self, a: &PTy, t: Ty) -> Option<RT> {
        let (PTy::Fn(ps, r), Ty::Fn(ft)) = (a, t) else { return None };
        if ps.len() != ft.params.len() {
            return None;
        }
        let prog = self.prog;
        // (a result that elides a region (`fn(&u32, _) -> &u32`): it is its parameter's, which one binder links; not
        // taken part by part (the whole is fresh, as before))
        let mut rs = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 1, derived: None };
        let mut rc = vec![];
        rs.slots(r, false, &mut rc);
        if rc.contains(&R_BOUND) {
            return None;
        }
        let lts = self.rg.lts.clone();
        let mut out = vec![];
        for (p, &pt) in ps.iter().chain(std::iter::once(&**r)).zip(ft.params.iter().chain(std::iter::once(&ft.ret))) {
            if matches!(p, PTy::Infer) {
                out.extend(self.rg.fresh(prog, pt)?);
                continue;
            }
            let mut s = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 1, derived: None };
            let mut codes = vec![];
            s.slots(p, false, &mut codes);
            out.extend(self.rg.instantiate(prog, pt, Some(&codes), &lts, &[], &mut |me| me.var())?);
        }
        Some(out)
    }

    pub(crate) fn rt_fn_item(&mut self, e: &Expr) -> Option<(Ty, RT)> {
        let Expr::Path(None, p) = strip(e) else { return None };
        let id = self.fn_item(e)?;
        let prog = self.prog;
        let sig = &prog.sigs[id as usize];
        let tf: Vec<&PTy> = p.segs.last()?.args.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None }).collect();
        // (the instance: its type arguments as written, all of them)
        if tf.len() != sig.generics.len() || sig.trait_of.is_some() {
            return if sig.generics.is_empty() { self.rt_fn_sig(id, &[], &[]) } else { None };
        }
        let mut sol = vec![];
        let mut targs = vec![];
        for pt in &tf {
            let t = self.parse_ty(pt).ok()?;
            let mut sl = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
            let mut codes = vec![];
            sl.slots(pt, false, &mut codes);
            let lts = self.rg.lts.clone();
            targs.push(self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var()));
            sol.push(t);
        }
        self.rt_fn_sig(id, &sol, &targs)
    }

    fn rt_fn_sig(&mut self, id: u32, sol: &[Ty], targs: &[Option<RT>]) -> Option<(Ty, RT)> {
        let prog = self.prog;
        let sig = &prog.sigs[id as usize];
        let rs = sig.regions.clone();
        let codes = |i: usize| rs.as_deref().map(|r| r.slots.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        // (its own lifetimes and those elided in it are its type's: a call through a local of it takes them afresh)
        let lts: Vec<u32> = rs.as_deref().map(|r| r.lts.iter().map(|_| self.rg.var()).collect()).unwrap_or_default();
        self.rg.late_bound.extend(lts.iter().copied());
        let opt: Vec<Option<Ty>> = sol.iter().map(|&t| Some(t)).collect();
        let mut tree = vec![];
        let mut params = vec![];
        let mut inputs: Vec<u32> = vec![];
        let mut self_r = None;
        for (i, &pt) in sig.params.iter().enumerate() {
            let t = self.rg.instantiate(prog, pt, codes(i), &lts, targs, &mut |me| {
                let v = me.var();
                me.late_bound.push(v);
                v
            })?;
            if i == 0 && sig.has_self && matches!(pt, Ty::Ref(..)) {
                self_r = t.first().copied();
            }
            inputs.extend(t.iter().copied().filter(|&v| v != STATIC));
            tree.extend(t);
            params.push(prog.partial_subst(pt, &opt));
        }
        // (the result's elided regions, as in the fn's signature: of `&self`, else of the one region among the
        // parameters (`fn baz(x: &S) -> &S` is `for<'a> fn(&'a S) -> &'a S`))
        inputs.sort_unstable();
        inputs.dedup();
        let one = if inputs.len() == 1 { Some(inputs[0]) } else { None };
        tree.extend(self.rg.instantiate(prog, sig.ret, codes(sig.params.len()), &lts, targs, &mut |me| match self_r.or(one) {
            Some(v) => v,
            None => {
                let v = me.var();
                me.late_bound.push(v);
                v
            }
        })?);
        let ret = prog.partial_subst(sig.ret, &opt);
        if params.iter().any(|&t| prog.has_param(t)) || prog.has_param(ret) {
            return None;
        }
        let t = Ty::Fn(crate::types::intern_fn(crate::types::FnTy { params, ret }));
        (count(prog, t, 0) == Some(tree.len())).then_some((t, tree))
    }

    /// A closure made where nothing was expected (by its definition base, of type arguments args): its calls share
    /// one signature (clo_targs, made here), and what its body stores of its parameters in places it captures
    /// (`x.0 = input`, `v.push(input)`) flows there in the function it is in: the function knows those places
    pub(crate) fn rg_closure_stores(&mut self, base: u32, args: &[Ty], link: Option<usize>, body: &Expr, params: &[(String, usize)], caps: &[String]) {
        if !self.rg_on() || params.is_empty() {
            return;
        }
        let prog = self.prog;
        let mut targs: Vec<Option<RT>> = args.iter().map(|&t| self.rg.fresh(prog, t)).collect();
        if let (Some(g), Some(last)) = (link, targs.len().checked_sub(1)) {
            if let Some(t) = targs.get(g).cloned() {
                targs[last] = t;
            }
        }
        self.rg.clo_targs.retain(|x| x.0 != base);
        self.rg.clo_targs.push((base, targs.clone()));
        let stmts: Vec<&Expr> = match strip(body) {
            Expr::Block(_, _, b) => b.stmts.iter().filter_map(|s| if let Stmt::Expr(e, _) = s { Some(e) } else { None }).collect(),
            e => vec![e],
        };
        fn root(e: &Expr) -> Option<&str> {
            match strip(e) {
                Expr::Path(None, p) if p.segs.len() == 1 => Some(p.segs[0].name.as_str()),
                Expr::Field(x, _, _) | Expr::Index(x, _, _) => root(x),
                Expr::Unary(op, x, _) if op == "*" => root(x),
                _ => None,
            }
        }
        let param = |e: &Expr| match strip(e) {
            Expr::Path(None, p) if p.segs.len() == 1 => params.iter().find(|x| x.0 == p.segs[0].name).map(|x| x.1),
            _ => None,
        };
        for e in stmts {
            let (place, g, push) = match strip(e) {
                Expr::Assign(l, r) => match param(r) {
                    Some(g) => (&**l, g, false),
                    None => continue,
                },
                Expr::MethodCall(recv, m, _, a, _) if m == "push" && a.len() == 1 => match param(&a[0]) {
                    Some(g) => (&**recv, g, true),
                    None => continue,
                },
                _ => continue,
            };
            if !root(place).is_some_and(|r| caps.iter().any(|c| c == r)) {
                continue;
            }
            let (Some(Some(ptr)), Some(&pty)) = (targs.get(g), args.get(g)) else { continue };
            let Some((mut t, mut tr, _)) = self.rt_place(place) else { continue };
            // (`v.push(input)` of a Vec: its elements')
            if push {
                while let Ty::Ref(_, i) = t {
                    if tr.is_empty() {
                        break;
                    }
                    tr.remove(0);
                    t = *i;
                }
                let Ty::Struct(sid) = t else { continue };
                let sd = &prog.structs[sid as usize];
                if prog.structs[prog.base_of(sid) as usize].name != "Vec" || sd.args.len() != 1 {
                    continue;
                }
                t = sd.args[0];
            }
            let sp = super::borrowck::pos(e);
            let ptr = ptr.clone();
            self.rg.flow(prog, pty, t, &ptr, &tr, sp);
        }
    }

    /// What a closure made where nothing was expected (by its definition base, of type arguments args) requires of its
    /// parameters, as its body writes it (closure_reqs): their shared trees (clo_targs) outlive the regions the
    /// annotation or the turbofish names, so a call's arguments must too (`f(&c)` with `let f = |x| W::<&'static
    /// u32>(x);`, or `let f = W::<&'static u32>;`: E0597, as rustc propagates a closure's requirements)
    pub(crate) fn rg_closure_reqs(&mut self, base: u32, args: &[Ty], reqs: &[ClosureReq]) {
        if !self.rg_on() || reqs.is_empty() {
            return;
        }
        let prog = self.prog;
        let Some(targs) = self.rg.clo_targs.iter().find(|x| x.0 == base).map(|x| x.1.clone()) else { return };
        for r in reqs {
            let pt: &PTy = match r.call {
                None => r.ann.unwrap(),
                // (a method's: of the receiver's type where it is known here, a local's or a struct's path)
                Some(call) if matches!(strip(call), Expr::MethodCall(..)) => {
                    let Expr::MethodCall(recv, m, gs, _, _) = strip(call) else { continue };
                    let sid = match strip(recv) {
                        Expr::Path(None, p) if p.segs.len() == 1 => match self.lookup(&p.segs[0].name) {
                            Some((_, t)) => {
                                let mut t = t;
                                while let Ty::Ref(_, i) = t {
                                    t = *i;
                                }
                                match t {
                                    Ty::Struct(sid) => Some(sid),
                                    _ => None,
                                }
                            }
                            None => prog.struct_by_p(&crate::program::PPath::of(p), None),
                        },
                        _ => None,
                    };
                    let Some(id) = sid.and_then(|sid| prog.self_method(sid, m)) else { continue };
                    let sig = &prog.sigs[id as usize];
                    let tys: Vec<&PTy> = gs.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None }).collect();
                    // (the method's own type parameters come after its impl's: the turbofish writes those)
                    let Some(own0) = sig.generics.len().checked_sub(tys.len()).filter(|_| sig.has_self) else { continue };
                    let Some(Ty::Param(k)) = sig.params.get(r.arg + 1).copied() else { continue };
                    let Some(&t) = (k as usize).checked_sub(own0).and_then(|j| tys.get(j)) else { continue };
                    t
                }
                Some(call) => {
                    let Expr::Call(f, _) = strip(call) else { continue };
                    let Expr::Path(None, p) = strip(f) else { continue };
                    let tys: Vec<&PTy> = p.segs.last().map(|s| s.args.iter().filter_map(|a| if let GArg::Ty(t) = a { Some(t) } else { None }).collect()).unwrap_or_default();
                    // (the callee's parameter the argument is passed to: one of its type parameters, which the
                    // turbofish writes)
                    let declared = if let Some(id) = self.fn_item(f) {
                        prog.sigs[id as usize].params.get(r.arg).copied()
                    } else {
                        // (the path without its turbofish names the type)
                        let mut q = p.clone();
                        for sg in &mut q.segs {
                            sg.args.clear();
                        }
                        let pp = crate::program::PPath::of(&q);
                        if let Some((sid, vi)) = prog.variant_by_p(&pp, None) {
                            prog.structs[sid as usize].variants[vi].fields.get(r.arg).map(|f| f.1)
                        } else if let Some(sid) = prog.struct_by_p(&pp, None) {
                            prog.structs[sid as usize].fields.get(r.arg).map(|f| f.1)
                        } else {
                            None
                        }
                    };
                    let Some(Ty::Param(k)) = declared else { continue };
                    let Some(&t) = tys.get(k as usize) else { continue };
                    t
                }
            };
            let mut sl = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
            let mut codes = vec![];
            sl.slots(pt, false, &mut codes);
            if !sl.any {
                continue;
            }
            let (Some(&pty), Some(Some(ptr))) = (args.get(r.param), targs.get(r.param)) else { continue };
            let lts = self.rg.lts.clone();
            let Some(req) = self.rg.instantiate(prog, pty, Some(&codes), &lts, &[], &mut |me| me.var()) else { continue };
            let ptr = ptr.clone();
            self.rg.flow(prog, pty, pty, &ptr, &req, r.sp);
        }
    }

    /// A call through a fn pointer (f: the callee's value, of type t once dereferenced): the arguments flow into its
    /// parameters' regions, the result is of its result's
    /// A `&mut` local passed where a `&mut` is expected is reborrowed (`f(x)` is `f(&mut *x)`): a region of its own,
    /// that the reference's outlives, recorded as a reborrow (the borrow check's loan of the argument: see
    /// Rg::forever)
    fn rg_reborrow_arg(&mut self, a: &Expr, t: &mut Option<(Ty, RT)>) {
        let Some((Ty::Ref(true, _), tr)) = t else { return };
        // (a field of a local too: `v.push(c.raw)`)
        let mut root = strip(a);
        while let Expr::Field(x, _, _) = root {
            root = strip(x);
        }
        if tr.is_empty() || !matches!(root, Expr::Path(..)) {
            return;
        }
        // (a capture of the closure being checked: reborrowed through its environment, which the closure has for no
        // longer than a call (`|| doit(data)` with `data: &'static mut ()` passed where a `&'static mut ()` is
        // expected: "lifetime may not live long enough"). Not a local of the closure's own (`let d = data; doit(d)`)
        if self.rg.closure.is_some() {
            let env = self.rg.env_r;
            if self.rt_place(root).is_some_and(|p| p.2.contains(&env)) {
                let r = self.rg.var();
                let sp = super::borrowck::pos(a);
                self.rg.edge(tr[0], r, sp);
                self.rg.edge(env, r, sp);
                tr[0] = r;
            }
            return;
        }
        let r = self.rg.var();
        self.rg.edge(tr[0], r, super::borrowck::pos(a));
        tr[0] = r;
        self.rg.reborrows.push((a as *const Expr as usize, r));
    }

    pub(crate) fn rg_fnptr_call(&mut self, f: &Expr, t: Ty, args: &[&Expr]) {
        if !self.rg_on() {
            return;
        }
        let Ty::Fn(ft) = t else { return };
        let Some((mut ft0, mut tr)) = self.rt_expr(f) else { return };
        while let Ty::Ref(_, i) = ft0 {
            if tr.is_empty() {
                return;
            }
            tr.remove(0);
            ft0 = *i;
        }
        if ft0 != t {
            return;
        }
        // (each call instantiates the regions the pointer's type binds afresh: `(g(a), g(b))` with
        // `let g: fn(&u32) -> &u32 = id;` relates neither a to b. The function's own regions and 'static stay what
        // they are; one region within the tree stays one)
        let mut map: Vec<(u32, u32)> = vec![];
        for v in tr.iter_mut() {
            if !self.rg.late_bound.contains(v) {
                continue;
            }
            *v = match map.iter().find(|m| m.0 == *v) {
                Some(m) => m.1,
                None => {
                    let w = self.rg.var();
                    map.push((*v, w));
                    w
                }
            };
        }
        let prog = self.prog;
        let mut k = 0;
        for (a, &pt) in args.iter().zip(&ft.params) {
            let Some(n) = count(prog, pt, 0) else { return };
            let Some(ptr) = tr.get(k..k + n).map(|x| x.to_vec()) else { return };
            k += n;
            let mut t = self.rt_expr(a);
            if matches!(pt, Ty::Ref(true, _)) {
                self.rg_reborrow_arg(a, &mut t);
            }
            if let Some((at, atr)) = t {
                self.rg.flow(prog, at, pt, &atr, &ptr, super::borrowck::pos(a));
            }
        }
        if let Some(r) = tr.get(k..) {
            if count(prog, ft.ret, 0) == Some(r.len()) {
                self.rg.last_call = Some((self.rg.depth + 1, ft.ret, r.to_vec()));
            }
        }
    }

    /// `let _: ann = init;`: init flows into a value of the annotation's type
    pub(crate) fn rg_let_wild(&mut self, ann: &PTy, init: &Expr) {
        self.rg_let_ann(ann, init, None);
    }

    /// `let PAT: ann = init;`: init flows into a value of the annotation's type, whose tree is returned (what the
    /// pattern binds from: `let (y, _): (&'a u32, u32) = (&22, 44);` makes y a `&'a u32`)
    /// A place init that a `ref mut` binding of a `let` annotated with type t (tree tr) borrows: the place's value is
    /// of that type both ways (a `&mut` is invariant), as well as flowing into it
    pub(crate) fn rg_place_is_ann(&mut self, init: &Expr, t: Ty, tr: &RT) {
        if !self.rg_on() || !super::is_place_expr(init) {
            return;
        }
        let Some((it, itr)) = self.rt_expr(init) else { return };
        if it != t {
            return;
        }
        let sp = Some(super::borrowck::pos(init)).filter(|s| s.line != 0).unwrap_or(self.rg.let_sp);
        let prog = self.prog;
        self.rg.flow(prog, t, it, tr, &itr, sp);
    }

    pub(crate) fn rg_let_ann(&mut self, ann: &PTy, init: &Expr, actual: Option<Ty>) -> Option<(Ty, RT)> {
        if !self.rg_on() {
            return None;
        }
        let init_tree = self.rt_expr(init);
        let prog = self.prog;
        // (the type of the value bound (a pattern's), else the annotation's: an annotation with holes (`(&'static u8,
        // _)`) gives its tree against the type inference found, element by element, a hole's regions its own)
        let t = match actual {
            Some(t) => t,
            None => self.parse_ty(ann).ok()?,
        };
        if let Some(sp) = crate::wf::pos_of(ann).filter(|s| s.line != 0) {
            self.rg_proj_wf(ann, sp);
        }
        let tree = self.rg_ann_tree(ann, t)?;
        self.rg_closure_fnptr(ann, init);
        let sp = Some(super::borrowck::pos(init)).filter(|s| s.line != 0).unwrap_or(self.rg.let_sp);
        self.rg.wf(prog, t, &tree, sp);
        if let Some((it, itr)) = init_tree {
            if it != t {
                self.rg_dyn_arg_lts(it, t, sp);
            }
            self.rg.flow(prog, it, t, &itr, &tree, sp);
        }
        Some((t, tree))
    }

    /// A value whose type differs from the annotation's in the lifetimes the arguments of a trait object write
    /// (`Box<dyn Foo<Bar = &'static u32>>` into `let _: Box<dyn Foo<Bar = &'a u32>>`): those arguments are invariant,
    /// so a `'static` of the value's there makes the annotation's lifetime 'static. (The trees give an object one
    /// region, its own; its arguments keep their lifetimes in its text. Only a `'static` of the value's is taken: a
    /// named one of it may be a callee's)
    fn rg_dyn_arg_lts(&mut self, from: Ty, to: Ty, sp: Sp) {
        let (x, y) = (self.prog.tn(from), self.prog.tn(to));
        if !x.contains("dyn") || !x.contains("'static") {
            return;
        }
        let (a, b) = (dyn_arg_tokens(&x), dyn_arg_tokens(&y));
        let (mut i, mut j) = (0, 0);
        let mut pairs: Vec<&str> = vec![];
        while i < a.len() || j < b.len() {
            let (p, inside) = a.get(i).map(|(p, d)| (p.as_str(), *d)).unwrap_or(("", false));
            let q = b.get(j).map(|(q, _)| q.as_str()).unwrap_or("");
            match (p.starts_with('\''), q.starts_with('\'')) {
                (true, true) => {
                    if inside && p == "'static" && q != p && q != "'_" {
                        pairs.push(q);
                    }
                    i += 1;
                    j += 1;
                }
                // (elided on the other side: inferred)
                (true, false) => i += 1,
                (false, true) => j += 1,
                _ if p == q && !p.is_empty() => {
                    i += 1;
                    j += 1;
                }
                _ => return,
            }
        }
        for q in pairs {
            if let Some(k) = self.rg.lt_names.iter().position(|n| n == q) {
                let r = self.rg.lts[k];
                self.rg.constrain(r, STATIC, Var::Co, sp);
            }
        }
    }

    /// The trait obligations of a call's bounds on the callee's type parameters that select an impl of the program's
    /// whose head has regions to relate (TraitDb::lt_traits): see rg_obligation. `lts`: the callee's lifetimes at the
    /// call; `targs`: the trees of its type arguments
    pub(crate) fn rg_obligations(&mut self, id: u32, sol: &[Option<Ty>], lts: &[u32], targs: &[Option<RT>], sp: Sp) {
        let prog = self.prog;
        // (not in a generic function's instance: rustc proves the bounds of its body's calls once, from its where
        // clauses, whatever its parameters are; an impl of the instance's types is no proof there
        // (implied-bounds-on-trait-hierarchy-2.rs, a check-pass test, with `S: for<'a, 'b> Subtrait<..>`))
        if prog.tdb.lt_traits.is_empty() || !self.rg.inst_args.is_empty() {
            return;
        }
        let sig0 = &prog.sigs[id as usize];
        if !sig0.generics.iter().any(|(_, bs)| bs.iter().any(|b| prog.tdb.lt_traits.contains(bound_trait(b)))) {
            return;
        }
        let names: Vec<String> = sig0.regions.as_deref().map(|r| r.lts.clone()).unwrap_or_default();
        let scope: Vec<(String, u32)> = names.into_iter().zip(lts.iter().copied()).collect();
        let gn: Vec<&str> = sig0.generics.iter().map(|g| g.0.as_str()).collect();
        let all: Option<Vec<Ty>> = sol.iter().copied().collect();
        for (gi, (_, bounds)) in sig0.generics.iter().enumerate() {
            let (Some(&Some(t)), Some(Some(tree))) = (sol.get(gi), targs.get(gi)) else { continue };
            if prog.has_param(t) || prog.holds_placeholder(t, 0) {
                continue;
            }
            for b in bounds {
                if !prog.tdb.lt_traits.contains(bound_trait(b)) {
                    continue;
                }
                let Some(mut pr) = crate::traits::preds_of(prog, t, &[b.as_str()], &gn).pop() else { continue };
                // (arguments with a tuple in them, which pred_of leaves for `Fn(..)` sugar: `Foo<(&'a isize, &'b isize)>`)
                if pr.args.is_none() && b.find('<').is_some_and(|i| b.find('(').is_some_and(|j| j > i)) {
                    pr.args = crate::ast_summary::bound_args(b).and_then(|(_, a)| a.iter().map(|x| prog.resolve_g(x, &gn).ok()).collect());
                }
                // (the bound's arguments with the call's types for the callee's parameters)
                let conc = match (&pr.args, &all) {
                    (Some(a), _) if a.iter().all(|&x| !prog.has_param(x)) => Some(a.clone()),
                    (Some(a), Some(all)) => Some(a.iter().map(|&x| prog.subst(x, all)).collect()),
                    (Some(_), None) => continue,
                    (None, _) => None,
                };
                if conc.as_ref().is_some_and(|a| a.iter().any(|&x| prog.holds_placeholder(x, 0))) {
                    continue;
                }
                self.rg_obligation(t, tree, b, pr.args.as_deref(), conc.as_deref(), &gn, targs, &scope, sp, 0);
            }
        }
    }

    /// One trait obligation `t: b` (tree: t's regions) that selects impl c of the program's: c's head is t (and its
    /// trait's arguments the bound's), so their regions are equal; c's `'b: 'a` must hold of its lifetimes there, and
    /// its where clauses of such traits are obligations in turn. A lifetime b names that is not in `scope` is the
    /// bound's own (`for<'a> Foo<&'a isize>`): a placeholder of the call, a universal region that only itself and
    /// 'static outlive. A region of the impl's that must be it and something else ('static, a lifetime of the caller,
    /// another placeholder) is "implementation of `Foo` is not general enough"
    #[allow(clippy::too_many_arguments)]
    fn rg_obligation(&mut self, t: Ty, tree: &[u32], b: &str, gen_args: Option<&[Ty]>, args: Option<&[Ty]>, tparams: &[&str], targs: &[Option<RT>], scope: &[(String, u32)], sp: Sp, depth: u32) {
        let prog = self.prog;
        if depth > 4 {
            return;
        }
        let tr = bound_trait(b);
        let sel = prog.tdb.select(prog, t, tr, args);
        if debug() {
            eprintln!("regionck: obligation {}: {b} (args {:?}) selects {:?}", prog.tn(t), args.map(|a| a.iter().map(|&x| prog.tn(x)).collect::<Vec<_>>()), sel.as_ref().map(|x| (x.0.item, x.0.head.is_some())));
        }
        let Some((c, isol)) = sel else { return };
        let (Some(st), Some(n)) = (c.self_ty, count(prog, t, 0)) else { return };
        if n != tree.len() {
            return;
        }
        let rg = &mut self.rg;
        let v0 = rg.n;
        let mut scope = scope.to_vec();
        for l in bound_names(b) {
            if !scope.iter().any(|x| x.0 == l) {
                let v = rg.var();
                rg.univ.push((v, format!("{l}{HR_MARK}{tr}")));
                rg.hr.push((v, v0, sp, tr.to_string()));
                scope.push((l, v));
            }
        }
        let h = c.head.as_deref();
        let hl: Vec<u32> = h.map(|h| h.lts.iter().map(|_| rg.var()).collect()).unwrap_or_default();
        let ptrees: Vec<Option<RT>> = isol.iter().map(|x| x.and_then(|t| rg.fresh(prog, t))).collect();
        let var_of = |c: u8| if c == R_STATIC { Some(STATIC) } else { hl.get(c as usize).copied() };
        // (a region a fn pointer type of the goal binds (`indirect::<fn(&())>()`, `T: Trait<for<'b> fn(&'b u32)>`) is
        // none of the impl's but one its head binds there too: `impl<'a> X for fn(&'a ())`, `impl<A> Foo for fn(A)`
        // are not general enough for it. Nor is a region the goal does not bind (a lifetime it names, 'static) one the
        // head binds. A region of the goal that inference made is either)
        let known: Vec<u32> = scope.iter().map(|x| x.1).collect();
        // (an impl's type parameter's regions are its own only where the head writes it within a fn pointer type
        // (`fn(A)`): one that stands for the whole of such a type (`impl<T> Foo for Box<T>` and `Box<fn(&u8)>`) has
        // its binder in it)
        let mut in_fn = vec![false; c.n];
        params_in_fn(prog, st, false, &mut in_fn, 0);
        for &a in c.args.iter().flatten() {
            params_in_fn(prog, a, false, &mut in_fn, 0);
        }
        let outside: Vec<u32> = ptrees.iter().enumerate().filter(|(i, _)| !in_fn.get(*i).copied().unwrap_or(false)).flat_map(|(_, t)| t.iter().flatten().copied()).collect();
        let not_general = |rg: &RgSt, g: u32, h: u32| {
            let (gb, hb) = (rg.late_bound.contains(&g), rg.late_bound.contains(&h));
            (gb && !hb && !outside.contains(&h)) || (hb && !gb && (g == STATIC || known.contains(&g) || rg.univ.iter().any(|u| u.0 == g)))
        };
        let msg = format!("implementation of `{tr}` is not general enough");
        // (the self type: the goal's regions are the head's)
        if let Some(ht) = rg.instantiate(prog, st, h.map(|h| h.self_slots.as_slice()), &hl, &ptrees, &mut |me| me.var()) {
            if ht.len() == tree.len() {
                for (&a, &x) in tree.iter().zip(&ht) {
                    if not_general(rg, a, x) {
                        rg.hr_errs.push((sp, msg.clone()));
                    }
                    rg.constrain(a, x, Var::Inv, sp);
                }
            }
        }
        if let Some(h) = h {
            // (the trait's type arguments: as the bound writes them, with its lifetimes, against the head's)
            if let (Some(ga), Some(ha), Some(pts)) = (gen_args, c.args.as_deref(), bound_type_args(b)) {
                if ga.len() == ha.len() && pts.len() == ga.len() && h.trait_args.len() == ha.len() {
                    let names: Vec<String> = scope.iter().map(|x| x.0.clone()).collect();
                    let vars: Vec<u32> = scope.iter().map(|x| x.1).collect();
                    for ((pt, &g), (&hx, hc)) in pts.iter().zip(ga).zip(ha.iter().zip(&h.trait_args)) {
                        let mut sl = crate::regions::Slots { lts: names.clone(), tparams: tparams.to_vec(), self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                        let mut codes = vec![];
                        sl.slots(pt, false, &mut codes);
                        let gt = rg.instantiate(prog, g, Some(&codes), &vars, targs, &mut |me| me.var());
                        let xt = rg.instantiate(prog, hx, Some(hc), &hl, &ptrees, &mut |me| me.var());
                        if let (Some(gt), Some(xt)) = (gt, xt) {
                            if gt.len() == xt.len() {
                                for (&a, &x) in gt.iter().zip(&xt) {
                                    if not_general(rg, a, x) {
                                        rg.hr_errs.push((sp, msg.clone()));
                                    }
                                    rg.constrain(a, x, Var::Inv, sp);
                                }
                            }
                        }
                    }
                }
            }
            // (its lifetime arguments)
            let wl = bound_lts(b);
            if wl.len() == h.trait_lts.len() {
                for (l, &hc) in wl.iter().zip(&h.trait_lts) {
                    let g = if l == "'static" { Some(STATIC) } else { scope.iter().find(|x| x.0 == *l).map(|x| x.1) };
                    if let (Some(g), Some(x)) = (g, var_of(hc)) {
                        rg.constrain(g, x, Var::Inv, sp);
                    }
                }
            }
            // (the header's `'b: 'a`)
            for &(a, bb) in &h.outlives {
                if let (Some(x), Some(y)) = (var_of(a), var_of(bb)) {
                    rg.constrain(x, y, Var::Co, sp);
                }
            }
        }
        // (its where clauses of such traits, on its parameters: obligations of the impl's lifetimes)
        let hscope: Vec<(String, u32)> = h.map(|h| h.lts.iter().cloned().zip(hl.iter().copied()).collect()).unwrap_or_default();
        let isol_all: Option<Vec<Ty>> = isol.iter().copied().collect();
        for w in &c.body {
            if !prog.tdb.lt_traits.contains(&w.tr) {
                continue;
            }
            let Some(all) = isol_all.as_ref() else { continue };
            let wt = prog.subst(w.ty, all);
            let Some(wtree) = self.rg.instantiate(prog, w.ty, None, &[], &ptrees, &mut |me| me.var()) else { continue };
            let conc: Option<Vec<Ty>> = w.args.as_ref().map(|a| a.iter().map(|&x| prog.subst(x, all)).collect());
            self.rg_obligation(wt, &wtree, &w.raw, w.args.as_deref(), conc.as_deref(), &[], &ptrees, &hscope, sp, depth + 1);
        }
    }

    /// The tree of a value of type t written as ann (the annotation of a `let`): the regions it writes, fresh ones for
    /// a hole (`_`) and for what it elides; through tuples element by element
    fn rg_ann_tree(&mut self, ann: &PTy, t: Ty) -> Option<RT> {
        let prog = self.prog;
        match (ann, t) {
            (PTy::Infer, _) => self.rg.fresh(prog, t),
            (PTy::Tuple(es), Ty::Struct(sid)) if prog.is_tuple(sid) && prog.structs[sid as usize].args.len() == es.len() => {
                let args = prog.structs[sid as usize].args.clone();
                let mut out = vec![];
                for (e, a) in es.iter().zip(args) {
                    out.extend(self.rg_ann_tree(e, a)?);
                }
                Some(out)
            }
            _ => {
                let names = self.rg.lt_names.clone();
                let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                let mut codes = vec![];
                s.slots(ann, false, &mut codes);
                let lts = self.rg.lts.clone();
                self.rg.dyn_infer = true;
                let tree = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var());
                self.rg.dyn_infer = false;
                tree.or_else(|| self.rg_proj_ann(ann, t))
            }
        }
    }

    /// The type arguments a path's segment or a method call writes are well formed (`None::<Static<T>>`,
    /// `r.use_ty::<Static<T>>()`)
    pub(crate) fn rg_targs_wf(&mut self, args: &[GArg], sp: Sp) {
        let prog = self.prog;
        for a in args {
            let GArg::Ty(pt) = a else { continue };
            if crate::ast_summary::has_infer(pt) {
                continue;
            }
            let Ok(t) = self.parse_ty(pt) else { continue };
            // (only a type with a struct or an enum that bounds a type parameter by 'static, or a reference to a
            // placeholder's type where every parameter's type is followed (`wf::<&'x T>()`: T: 'x,
            // regions/regions-implied-bounds-projection-gap-1.rs): other requirements (`None::<&'a A>`'s A: 'a)
            // may hold by what a parameter's projection implies, which is not followed
            // (implied-bounds/normalization.rs))
            if !has_static_bound_adt(prog, t, 0) && (self.rg.implied_partial || !refs_placeholder(prog, t, false, 0)) {
                continue;
            }
            let mut s = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
            let mut codes = vec![];
            s.slots(pt, false, &mut codes);
            let lts = self.rg.lts.clone();
            if let Some(tree) = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var()) {
                self.rg.wf(prog, t, &tree, sp);
            }
        }
    }

    /// `let x: ann;` (`let _: ann;`): the annotation's type is well formed; x's tree is of it
    pub(crate) fn rg_let_uninit(&mut self, name: Option<&str>, ann: &PTy, sp: Sp) {
        // (or a projection whose associated type's value names lifetimes, as for a `let` with an initializer:
        // `let x: <() as Foo>::Out; x = r;` with `type Out = &'static u32`)
        let proj = matches!(ann, PTy::Path(Some(_), _)) && !self.prog.assoc_regions.is_empty();
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && (crate::regions::mentions_static(ann) || proj) {
            self.rg_late_on();
        }
        if !self.rg_on() {
            return;
        }
        let Ok(t) = self.parse_ty(ann) else { return };
        let prog = self.prog;
        let names = self.rg.lt_names.clone();
        let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
        let mut codes = vec![];
        s.slots(ann, false, &mut codes);
        let lts = self.rg.lts.clone();
        self.rg.dyn_infer = true;
        let tree = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var());
        self.rg.dyn_infer = false;
        let tree = if proj { self.rg_proj_ann(ann, t).or(tree) } else { tree };
        // (a projection anywhere in the annotation: `let _x: Option<<&'a () as Visitor<'d>>::Value>;`)
        self.rg_proj_wf(ann, sp);
        let Some(tree) = tree else { return };
        self.rg.wf(prog, t, &tree, sp);
        if let Some((loc, _)) = name.and_then(|n| self.lookup(n)) {
            self.rg.locals.insert(loc, tree);
        }
    }

    /// `a == b` (and the other comparisons): both are of one type (`PartialEq<Self>`), each flows into it (so its
    /// invariant parts are the same: `*mut &'a i32 == *mut &'b i32` needs 'a == 'b)
    pub(crate) fn rg_cmp(&mut self, a: &Expr, b: &Expr, e: &Expr) {
        // (a literal on either side: a number, a string, of no region to relate)
        if !self.rg_on() || matches!(strip(a), Expr::Lit(..)) || matches!(strip(b), Expr::Lit(..)) {
            return;
        }
        let (Some((at, atr)), Some((bt, btr))) = (self.rt_expr(a), self.rt_expr(b)) else { return };
        let prog = self.prog;
        if at != bt || atr.is_empty() {
            return;
        }
        let Some(tree) = self.rg.fresh(prog, at) else { return };
        let sp = Some(epos(e)).filter(|s| s.line != 0).unwrap_or_else(|| epos(a));
        self.rg.relate(prog, at, &atr, &tree, sp);
        self.rg.relate(prog, bt, &btr, &tree, sp);
    }

    /// `place = value;`
    pub(crate) fn rg_assign(&mut self, place: &Expr, value: &Expr) {
        // (a static assigned (`BAR = &n;`): every region of its type is 'static)
        // (a static first, then not a local of its name: the scopes are looked up for the rare static only)
        let stat = match strip(place) {
            Expr::Path(None, p) if p.segs.len() == 1 && (self.rg.on || self.rg.eligible) => self
                .prog
                .const_index
                .get(&p.segs[0].name)
                .map(|&ci| &self.prog.consts[ci as usize])
                .filter(|c| c.stat.is_some())
                .map(|c| c.ty)
                .filter(|_| self.lookup(&p.segs[0].name).is_none()),
            _ => None,
        };
        let stat = stat.and_then(|t| Some((t, count(self.prog, t, 0)?))).filter(|&(_, n)| n > 0);
        if stat.is_some() && !self.rg.on && self.rg.eligible && self.inline_depth == 0 {
            self.rg_late_on();
        }
        if !self.rg_on() {
            return;
        }
        if let Some((t, n)) = stat {
            let Some((vt, vtr)) = self.rt_expr(value) else { return };
            let prog = self.prog;
            self.rg.flow(prog, vt, t, &vtr, &vec![STATIC; n], super::borrowck::pos(value));
            return;
        }
        let (Some((t, ptr, _)), Some((vt, vtr))) = (self.rt_place(place), self.rt_expr(value)) else { return };
        let prog = self.prog;
        self.rg.flow(prog, vt, t, &vtr, &ptr, super::borrowck::pos(value));
    }

    /// The value e is returned
    pub(crate) fn rg_ret(&mut self, e: &Expr) {
        self.rg_ret_at(e, None);
    }

    /// rg_ret, with the position its errors are reported at when it is not e's own: a `match` arm's after the first,
    /// whose value is coerced to the match's type, is reported at the match as rustc does (its blame prefers the
    /// return of the match's value over that coercion). Not an arm that makes its value (a block, an `if`, a
    /// constructor's call): rustc reports those inside
    fn rg_ret_at(&mut self, e: &Expr, at: Option<Sp>) {
        if !self.rg_on() {
            return;
        }
        match e {
            Expr::Paren(x) => return self.rg_ret_at(x, at),
            Expr::Block(None, _, b) => {
                if let Some(Stmt::Expr(t, false)) = b.stmts.last() {
                    self.rg_ret_at(t, at);
                }
                return;
            }
            Expr::If(_, b, els) => {
                if let Some(Stmt::Expr(t, false)) = b.stmts.last() {
                    self.rg_ret_at(t, at);
                }
                if let Some(x) = els {
                    self.rg_ret_at(x, at);
                }
                return;
            }
            Expr::Match(_, arms) => {
                if let Some(&m2) = self.rg.alias.get(&(e as *const Expr as usize)) {
                    return self.rg_ret_at(m2, at);
                }
                let made = |b: &Expr| match super::strip_paren(b) {
                    Expr::Block(..) | Expr::If(..) | Expr::Match(..) | Expr::Struct(..) | Expr::Tuple(..) | Expr::Array(..) => true,
                    Expr::Call(f, _) => matches!(&**f, Expr::Path(_, p) if p.segs.last().is_some_and(|s| s.name.starts_with(|c: char| c.is_ascii_uppercase()))),
                    _ => false,
                };
                let sp = super::borrowck::pos(e);
                for (i, a) in arms.iter().enumerate() {
                    let at = at.or((i > 0 && sp.line != 0 && !made(&a.body)).then_some(sp));
                    self.rg_ret_at(&a.body, at);
                }
                return;
            }
            _ => {}
        }
        let Some(rt) = self.rg.ret.clone() else { return };
        let Some((et, tr)) = self.rt_expr(e) else { return };
        let (prog, t) = (self.prog, self.ret);
        let sp = at.unwrap_or_else(|| super::borrowck::pos(e));
        // (a result with no regions: nothing escapes through it; the span may be a call's in it)
        if sp.line != 0 && !rt.is_empty() {
            self.rg.ret_sps.push(sp);
        }
        let e0 = self.rg.edges.len();
        self.rg.flow(prog, et, t, &tr, &rt, sp);
        let e1 = self.rg.edges.len();
        if e1 > e0 {
            self.rg.ret_edges.push((e0, e1));
        }
    }

    /// A call of id (generic signature sig0, type arguments sol, instance's parameter types params) with these
    /// arguments (the receiver's tree is rg.recv): relate the arguments to the parameters, and keep the result's tree
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rg_call(&mut self, id: u32, sol: &[Option<Ty>], turbofish: Option<&[GArg]>, args: &[&Expr], params: &[Ty], ret: Ty, recv: Option<(RT, Sp)>, has_recv: bool, sp: Sp) {
        if !self.rg_on() {
            return;
        }
        let recv_info = recv.as_ref().map(|r| r.1);
        // (a call whose span is not known: at its first argument)
        let sp = if sp.line == 0 { recv_info.into_iter().chain(args.first().map(|a| super::borrowck::pos(a))).find(|s| s.line != 0).unwrap_or(sp) } else { sp };
        self.rg.last_call = None;
        let prog = self.prog;
        let sig0 = &prog.sigs[id as usize];
        // (a generic function's instance calling the method of an impl that writes lifetimes in its trait
        // (`impl Trait<'static> for ()`): the generic body called the trait's method under its own bounds
        // (`(): Trait<'a>`), with the trait's signature, not the impl's. Not followed)
        // (only where the trait's declaration of the method names the trait's own lifetimes, which the impl sets and a
        // bound may set otherwise; a trait's `fn take(x: &'static u8)` is the impl's too: `S::take(x)` is followed)
        let decl_names_trait_lts = || {
            let tr = sig0.impl_trait.as_deref()?;
            let name = prog.names[id as usize].rsplit("::").next()?;
            let tail = format!("::{tr}");
            let decls = prog.traits.get(tr).or_else(|| prog.traits.iter().find(|(k, _)| k.ends_with(&tail)).map(|(_, v)| v))?;
            let d = decls.iter().copied().find(|&d| prog.names[d as usize].rsplit("::").next() == Some(name))?;
            let r = prog.sigs[d as usize].regions.as_deref()?;
            Some(r.slots.iter().flatten().any(|&c| c < r.outer))
        };
        if !self.rg.inst_args.is_empty() && sig0.impl_trait.is_some() && decl_names_trait_lts().unwrap_or(false) {
            self.rg.missed.push("trait impl");
            return;
        }
        let rs = sig0.regions.clone();
        let codes = |i: usize| rs.as_deref().map(|r| r.slots.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        // (the prelude writes no lifetimes: what its results borrow is not known from elision. A closure's call
        // elides as a fn)
        let prelude = prog.item_of.get(id as usize).copied().flatten().is_none_or(|i| i >= prog.prelude_from) && !prog.closures.contains_key(&id);
        // (a closure passed where a fn pointer type binds the region of a parameter it annotates with one of the
        // function's lifetimes (`want(|_: &'static u32| {})` with `fn want(f: fn(&u32))`): as in a `let`
        // (rg_closure_fnptr). Without a summary, every region of the type is one it binds)
        let mut clo_fnptr: Vec<(&Expr, Vec<Vec<u8>>, Vec<Vec<u8>>, Vec<Ty>)> = vec![];
        for (i, &pt) in sig0.params.iter().enumerate() {
            let Ty::Fn(ft) = pt else { continue };
            let Some(c) = i.checked_sub(has_recv as usize).and_then(|j| args.get(j)).map(|a| super::strip_paren(a)) else { continue };
            let Expr::Closure(_, inputs, ..) = c else { continue };
            if inputs.len() != ft.params.len() || !inputs.iter().any(|(_, x)| x.as_ref().is_some_and(crate::regions::names_lt)) {
                continue;
            }
            let Some(ns) = ft.params.iter().map(|&t| count(prog, t, 0)).collect::<Option<Vec<usize>>>() else { continue };
            // (the marks of paths written with no lifetime stand for no region where their types have none (`&S`))
            let (all, raw): (Vec<u8>, Vec<u8>) = match codes(i).map(|c| c.iter().copied().filter(|&x| x != R_ELIDED_ADT).collect::<Vec<u8>>()) {
                None => (vec![CLO_P; ns.iter().sum()], vec![R_BOUND; ns.iter().sum()]),
                Some(c) if c.first() == Some(&R_FN) && Some(c.len() - 1) == count(prog, ft.ret, 0).map(|r| r + ns.iter().sum::<usize>()) => {
                    (c[1..].iter().map(|&x| if x == R_BOUND { CLO_P } else { CLO_E }).collect(), c[1..].to_vec())
                }
                Some(_) => continue,
            };
            let mut per = vec![];
            let mut per_raw = vec![];
            let mut at = 0;
            for &n in &ns {
                per.push(all[at..at + n].to_vec());
                per_raw.push(raw[at..at + n].to_vec());
                at += n;
            }
            // (with the regions the parameter's type names, the callee's lifetimes once instantiated below:
            // `want(x, |_: &'static u32| {})` with `fn want<'a>(_: &'a u32, _f: fn(&'a u32))`)
            clo_fnptr.push((c, per, per_raw, ft.params.clone()));
        }
        // (which parameters a fn item is passed to (`want(baz)`), by the parameter's index)
        let fn_item_args: Vec<bool> = (0..sig0.params.len())
            .map(|i| i.checked_sub(has_recv as usize).and_then(|j| args.get(j)).is_some_and(|a| self.fn_item(super::strip_paren(a)).is_some()))
            .collect();
        let rg = &mut self.rg;
        let mut lts: Vec<u32> = rs.as_deref().map(|r| r.lts.iter().map(|_| rg.var()).collect()).unwrap_or_default();
        // (the lifetimes the path writes on the impl's self type: `A::<'a>::new` with `impl<'a> A<'a>` is the impl's
        // `'a`. The self type's first slots are its lifetime parameters')
        // (a call in the arguments, one level deeper, leaves them: it is made before this call's check)
        let written = if rg.call_self.as_ref().is_some_and(|x| x.0 == rg.depth && x.1 == id) { rg.call_self.take() } else { None };
        if let (Some((_, _, ls)), Some(r)) = (written, rs.as_deref()) {
            let n = match sig0.self_ty {
                Some(Ty::Struct(s)) => lts_of(prog, prog.base_of(s)),
                _ => 0,
            };
            for (j, l) in ls.iter().enumerate().take(n) {
                let Some(&c) = r.self_slots.get(j) else { break };
                if (c as usize) >= (r.outer as usize).min(lts.len()) || l == "'_" {
                    continue;
                }
                let v = if l == "'static" { Some(STATIC) } else { rg.lt_names.iter().position(|x| x == l).and_then(|p| rg.lts.get(p).copied()) };
                if let Some(v) = v {
                    lts[c as usize] = v;
                }
            }
        }
        // (a method of a placeholder's bound (`G: GetRef<'a>` calling `g.get()`): the trait's lifetimes are those the
        // bound writes, the caller's; what the declaration requires is the caller's to prove)
        let mut ph_self = false;
        if let (Some(tr), Some(&Some(Ty::Struct(s)))) = (sig0.trait_of.as_deref(), sol.first()) {
            if prog.is_placeholder(s) {
                ph_self = true;
                let outer = rs.as_deref().map_or(0, |r| r.outer as usize);
                let short = tr.rsplit("::").next().unwrap_or(tr);
                let bs = prog.structs[s as usize].placeholder.as_deref().unwrap_or(&[]);
                // (`Self` of the trait's own default method: bounded by the trait, whose lifetimes are the method's, by name)
                // (only for the `Self` placeholder: another type parameter of the method (`G: Get<'b>`) has its own bound)
                let own = prog.sigs.get(rg.base as usize).and_then(|c| c.trait_of.as_deref()) == Some(tr) && prog.structs[s as usize].name == "Self#";
                if let (true, Some(r)) = (own, rs.as_deref()) {
                    for (i, l) in r.lts.iter().enumerate().take(outer) {
                        if let (Some(p), Some(slot)) = (rg.lt_names.iter().position(|x| x == l), lts.get_mut(i)) {
                            if let Some(&v) = rg.lts.get(p) {
                                *slot = v;
                            }
                        }
                    }
                } else if let Some(b) = bs.iter().filter(|_| outer > 0).find(|b| b.split('<').next().unwrap_or(b).trim() == tr).or_else(|| bs.iter().filter(|_| outer > 0).find(|b| b.split('<').next().unwrap_or(b).trim().rsplit("::").next() == Some(short))) {
                    for (i, l) in bound_lts(b).iter().enumerate().take(outer) {
                        let v = if l == "'static" { Some(STATIC) } else { rg.lt_names.iter().position(|x| x == l).and_then(|p| rg.lts.get(p).copied()) };
                        if let (Some(v), Some(slot)) = (v, lts.get_mut(i)) {
                            *slot = v;
                        }
                    }
                }
            }
        }
        for (c, per, per_raw, params) in clo_fnptr {
            let tgts: Vec<Vec<Option<u32>>> = per_raw.iter().map(|v| v.iter().map(|&x| if x == R_STATIC { Some(STATIC) } else if x < R_STATIC { lts.get(x as usize).copied() } else { None }).collect()).collect();
            closure_ann_bound(prog, rg, c, &per, &tgts, &params);
        }
        // (type parameters the arguments did not solve (a bound's `G: GetRef<'a, isize>` gives the trait's `T`): from
        // the instantiated signature)
        let filled;
        let sol = if ph_self && sol.iter().any(|t| t.is_none()) {
            let mut s2 = sol.to_vec();
            for (&g, &c) in sig0.params.iter().zip(params).chain(std::iter::once((&sig0.ret, &ret))) {
                bind_params(prog, g, c, &mut s2, 0);
            }
            filled = s2;
            &filled[..]
        } else {
            sol
        };
        let mut targs: Vec<Option<RT>> = sol.iter().map(|t| t.and_then(|t| rg.fresh(prog, t))).collect();
        // (a method of a placeholder's bound's trait: the trait's type arguments as the bound writes them, with their
        // regions (`F: FnLike<&'a isize, &'a isize>`: the function's 'a, both). A lifetime the function does not name is
        // the bound's own (`for<'a> FnLike<..>`): one fresh region for the call, which its arguments share)
        if ph_self {
            if let (Some(tr), Some(&Some(Ty::Struct(s)))) = (sig0.trait_of.as_deref(), sol.first()) {
                if let Some((args, own)) = bound_targ_ptys(prog, s, tr) {
                    let mut names = rg.lt_names.clone();
                    let mut flts = rg.lts.clone();
                    for l in own {
                        if names.contains(&l) {
                            continue;
                        }
                        names.push(l);
                        let v = rg.var();
                        flts.push(v);
                    }
                    for (i, pt) in args.iter().enumerate() {
                        let Some(Some(t)) = sol.get(1 + i).copied() else { continue };
                        if !crate::regions::names_lt(pt) {
                            continue;
                        }
                        let mut sl = crate::regions::Slots { lts: names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                        let mut codes = vec![];
                        sl.slots(pt, false, &mut codes);
                        let tr = rg.instantiate(prog, t, Some(&codes), &flts, &[], &mut |me| me.var());
                        if let (Some(tr), Some(slot)) = (tr.filter(|tr| count(prog, t, 0) == Some(tr.len())), targs.get_mut(1 + i)) {
                            *slot = Some(tr);
                        }
                    }
                }
            }
        }
        // (type arguments written (`f::<&'a u8>()`): with the regions they name)
        if let Some(tf) = turbofish {
            let first = sig0.trait_of.is_some() as usize;
            for (i, a) in tf.iter().enumerate() {
                let (GArg::Ty(pt), Some(Some(t))) = (a, sol.get(first + i)) else { continue };
                if !crate::regions::names_lt(pt) {
                    continue;
                }
                let mut s = crate::regions::Slots { lts: rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                let mut codes = vec![];
                s.slots(pt, false, &mut codes);
                let flts = rg.lts.clone();
                rg.dyn_infer = true;
                let tr = rg.instantiate(prog, *t, Some(&codes), &flts, &[], &mut |me| me.var());
                rg.dyn_infer = false;
                if let Some(tr) = tr {
                    targs[first + i] = Some(tr);
                }
            }
        }
        // (the trait obligations of the callee's bounds: the impls they select, with their regions. A fn pointer type
        // written as a type argument (`indirect::<fn(&())>()`) gives its regions there, which it binds)
        if !prog.tdb.lt_traits.is_empty() {
            let lts2 = lts.clone();
            let mut targs2 = targs.clone();
            if let Some(tf) = turbofish {
                let first = sig0.trait_of.is_some() as usize;
                for (i, a) in tf.iter().enumerate() {
                    let (GArg::Ty(pt), Some(Some(t))) = (a, sol.get(first + i)) else { continue };
                    if crate::regions::names_lt(pt) || !crate::regions::fn_ptr_ref(pt, false, 0) {
                        continue;
                    }
                    let mut s = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                    let mut codes = vec![];
                    s.slots(pt, false, &mut codes);
                    let flts = self.rg.lts.clone();
                    if let Some(tr) = self.rg.instantiate(prog, *t, Some(&codes), &flts, &[], &mut |me| me.var()) {
                        if let Some(slot) = targs2.get_mut(first + i) {
                            *slot = Some(tr);
                        }
                    }
                }
            }
            self.rg_obligations(id, sol, &lts2, &targs2, sp);
        }
        let rg = &mut self.rg;
        // (the regions a type argument brings are not the signature's own: elision does not see them)
        let substituted: Vec<u32> = targs.iter().flatten().flatten().copied().collect();
        let mut ptrees: Vec<Option<RT>> = vec![];
        let mut inputs = vec![];
        // (a closure's of one region for all its calls: the same variables, in order)
        let shared = rg.shared_clo.take();
        // (its parameters' and result's types: the same trees at each call; the result's its parameter's for `|y| y`)
        if let Some((b, link)) = shared {
            match rg.clo_targs.iter().find(|x| x.0 == b) {
                Some((_, t)) if t.len() == targs.len() => targs = t.clone(),
                _ => {
                    if let (Some(g), Some(last)) = (link, targs.len().checked_sub(1)) {
                        if let Some(t) = targs.get(g).cloned() {
                            targs[last] = t;
                        }
                    }
                    rg.clo_targs.retain(|x| x.0 != b);
                    rg.clo_targs.push((b, targs.clone()));
                }
            }
        }
        let shared = shared.map(|s| s.0);
        let mut k = 0usize;
        for (i, &pt) in sig0.params.iter().enumerate() {
            // (a fn item passed where a fn pointer that binds its regions is expected (`want(baz)` with
            // `want(f: for<'cx> fn(&'cx S) -> &'static S)`): it must do for every region the type binds, so they are
            // a placeholder of the call, which the item's own regions must not make outlive anything else (E0308 in
            // rustc for `fn baz(x: &S) -> &S`). Not for a fn pointer type that holds another)
            let bound_ph = fn_item_args.get(i).copied().unwrap_or(false)
                && matches!(pt, Ty::Fn(ft) if !ft.params.iter().chain(std::iter::once(&ft.ret)).any(|&t| has_fn_ty(t, 0)))
                && codes(i).is_some_and(|c| c.contains(&R_BOUND));
            if bound_ph {
                let u = rg.var();
                rg.univ.push((u, "'_".to_string()));
                rg.bound_univ = Some(u);
            }
            let tr = rg.instantiate(prog, pt, codes(i), &lts, &targs, &mut |me| match shared {
                Some(b) => {
                    let pos = match me.clo_shared.iter().position(|x| x.0 == b) {
                        Some(p) => p,
                        None => {
                            me.clo_shared.push((b, vec![]));
                            me.clo_shared.len() - 1
                        }
                    };
                    let v = match me.clo_shared[pos].1.get(k) {
                        Some(&v) => v,
                        None => {
                            let v = me.var();
                            me.clo_shared[pos].1.push(v);
                            v
                        }
                    };
                    k += 1;
                    v
                }
                None => me.var(),
            });
            rg.bound_univ = None;
            if let Some(t) = &tr {
                inputs.extend(t.iter().copied().filter(|&v| v != STATIC && !substituted.contains(&v)));
            }
            ptrees.push(tr);
        }
        // (the arguments flow into the parameters)
        let mut atrees: Vec<(Option<(Option<Ty>, RT)>, Sp)> = vec![];
        if has_recv {
            atrees.push((recv.map(|r| (None, r.0)), recv_sp(&recv_info, sp)));
        }
        for (i, a) in args.iter().enumerate() {
            let mut t = self.rt_expr(a);
            if params.get(i + has_recv as usize).is_some_and(|p| matches!(p, Ty::Ref(true, _))) {
                self.rg_reborrow_arg(a, &mut t);
            }
            atrees.push((t.map(|x| (Some(x.0), x.1)), super::borrowck::pos(a)));
        }
        let rg = &mut self.rg;
        for (i, ((at, asp), pt)) in atrees.iter().zip(&ptrees).enumerate() {
            if let (Some((aty, at)), Some(pt), Some(&ty)) = (at, pt, params.get(i)) {
                rg.flow(prog, aty.unwrap_or(ty), ty, at, pt, *asp);
            }
        }
        // (the parameters' types are well formed, as the callee assumes: `&'a Foo<'b>` needs 'b: 'a of the call)
        if !prelude && rs.is_some() {
            for (pt, &ty) in ptrees.iter().zip(params) {
                if let Some(pt) = pt.as_ref().filter(|t| t.len() > 1) {
                    rg.wf(prog, ty, pt, sp);
                }
            }
        }
        // (an impl's method: its self type is well formed (`struct Foo<'a, 'b: 'a>`): the call makes sure)
        if let (Some(r), Some(isid)) = (rs.as_deref(), sig0.impl_of) {
            if !r.self_slots.is_empty() {
                if let Some(st) = rg.instantiate(prog, Ty::Struct(isid), Some(&r.self_slots), &lts, &targs, &mut |me| me.var()) {
                    rg.wf(prog, Ty::Struct(isid), &st, sp);
                }
            }
        }
        // (the callee's declared bounds hold of its arguments; not those of a trait's impl called from a generic
        // function's instance: the generic body called the trait's method, under its own bounds)
        let through_trait = !rg.inst_args.is_empty() && (sig0.impl_trait.is_some() || sig0.trait_of.is_some()) && !ph_self;
        if let Some(r) = rs.as_deref().filter(|_| !through_trait) {
            let m = |c: u8| if c == R_STATIC { Some(STATIC) } else { lts.get(c as usize).copied() };
            for &(a, b) in &r.outlives {
                if let (Some(x), Some(y)) = (m(a), m(b)) {
                    rg.edge(x, y, sp);
                }
            }
            for (tp, b) in &r.ty_outlives {
                let (Some(g), Some(y)) = (sig0.generics.iter().position(|x| x.0 == *tp), m(*b)) else { continue };
                if let Some(Some(at)) = targs.get(g) {
                    for &x in at {
                        rg.edge(x, y, sp);
                    }
                }
            }
        }
        // (a callee's trait bound on a type parameter the caller's own type parameter fills, as the caller's bound
        // of that trait (`G: Get<T>` with T = &'min i32, the caller's `G: Get<&'max i32>`): a trait's parameters are
        // invariant, so the regions are the same)
        if !through_trait && !ph_self {
            for (gi, (_, bs)) in sig0.generics.iter().enumerate() {
                let Some(&Some(Ty::Struct(s))) = sol.get(gi) else { continue };
                if !prog.is_placeholder(s) {
                    continue;
                }
                let cbs = prog.structs[s as usize].placeholder.as_deref().unwrap_or(&[]);
                for b in bs.iter().filter(|b| b.contains('<')) {
                    let tr = b.split('<').next().unwrap_or(b).trim();
                    let mut same = cbs.iter().filter(|c| c.split('<').next().unwrap_or(c).trim() == tr);
                    let (Some(cb), None) = (same.next(), same.next()) else { continue };
                    let (xs, ys) = (bound_args(b), bound_args(cb));
                    if xs.len() != ys.len() {
                        continue;
                    }
                    for (x, y) in xs.iter().zip(&ys) {
                        let callee: Option<RT> = if x.starts_with('\'') {
                            if x == "'static" { Some(vec![STATIC]) } else { rs.as_deref().and_then(|r| r.lts.iter().position(|l| l == x)).and_then(|p| lts.get(p).copied()).map(|v| vec![v]) }
                        } else {
                            sig0.generics.iter().position(|g| g.0 == *x).and_then(|p| targs.get(p).cloned().flatten())
                        };
                        let caller: Option<RT> = written_lts(y).and_then(|ls| {
                            ls.iter().map(|l| if l == "'static" { Some(STATIC) } else { rg.lt_names.iter().position(|n| n == l).and_then(|p| rg.lts.get(p).copied()) }).collect()
                        });
                        if let (Some(a), Some(c)) = (callee, caller) {
                            if a.len() == c.len() {
                                for (&a, &c) in a.iter().zip(&c) {
                                    rg.constrain(a, c, Var::Inv, sp);
                                }
                            }
                        }
                    }
                }
            }
        }
        // (a callee's trait bound on a type parameter filled by a type that only the caller's where clause makes
        // implement the trait (`impls_get::<&'min G>()` with `G: Get`, the caller's `&'max G: Get`, and no impl of Get
        // for a reference): the type is the where clause's, and a trait's `Self` is invariant, so the regions are
        // the same (variance/variance-*-self-trait-match.rs))
        if !through_trait && !rg.where_trees.is_empty() {
            for (gi, (_, bs)) in sig0.generics.iter().enumerate() {
                let (Some(&Some(t)), Some(Some(at))) = (sol.get(gi), targs.get(gi)) else { continue };
                for b in bs {
                    let tr = b.split('<').next().unwrap_or(b).trim();
                    let tr = tr.rsplit("::").next().unwrap_or(tr);
                    // (only where one clause fits: the trees keep no trait arguments, and with `&'a G: Get<u8>, &'b G:
                    // Get<u16>` which clause meets the bound is not known here)
                    let mut fits = rg.where_trees.iter().filter(|(wty, _, trs)| *wty == t && trs.iter().any(|x| x == tr));
                    let (Some((_, wt, _)), None) = (fits.next(), fits.next()) else { continue };
                    if wt.len() != at.len() || prog.implements(t, tr) {
                        continue;
                    }
                    let pairs: Vec<(u32, u32)> = at.iter().copied().zip(wt.iter().copied()).collect();
                    for (a, c) in pairs {
                        rg.constrain(a, c, Var::Inv, sp);
                    }
                }
            }
        }
        // (the result: its elided regions are of `&self`, else of the one region among the parameters)
        // (a receiver `&self` / `&mut self`: of the impl's own type; another receiver type leaves the result's
        // elided regions unknown)
        // (or of `impl dyn Tr`'s own object: `fn get(&self) -> &u8` borrows from the receiver, not only its object)
        // (or a trait's declaration, `&self` of its Self: a call through the vtable, a placeholder's or a default
        // through `impl Ext for dyn Tr {}`)
        let self_ref = sig0.has_self
            && (matches!((sig0.params.first(), sig0.impl_of), (Some(Ty::Ref(_, Ty::Struct(s))), Some(i)) if prog.base_of(*s) == prog.base_of(i))
                || matches!(sig0.params.first(), Some(Ty::Ref(_, Ty::Dyn(_)))) && prog.names[id as usize].starts_with("__dyn_")
                || sig0.trait_of.is_some() && sig0.impl_of.is_none() && matches!(sig0.params.first(), Some(Ty::Ref(_, Ty::Param(0)))) && sig0.generics.first().is_some_and(|g| g.0 == "Self"));
        let self_r = if self_ref { ptrees.first().and_then(|t| t.as_ref()).and_then(|t| t.first().copied()).filter(|v| !substituted.contains(v)) } else { None };
        let other_self = sig0.has_self && !self_ref;
        inputs.dedup();
        let one = if inputs.len() == 1 { Some(inputs[0]) } else { None };
        let n = sig0.params.len();
        // (the prelude writes no lifetimes: a method of it taking `&self` returns a borrow of self, as std's do;
        // what else its results borrow is not known)
        let rtr = rg.instantiate(prog, sig0.ret, codes(n), &lts, &targs, &mut |me| match (prelude, other_self && self_r.is_none(), self_r, one) {
            (true, _, Some(v), _) => v,
            (false, false, Some(v), _) | (false, false, None, Some(v)) => v,
            _ => me.var(),
        });
        if let Some(rtr) = rtr {
            if count(prog, ret, 0) == Some(rtr.len()) {
                rg.last_call = Some((rg.depth, ret, rtr));
            }
        }
    }

    /// The tree of a method call's receiver as passed: recv's place, through `derefs` references, then borrowed again
    /// when the method takes `&self` / `&mut self` (passed as recv_ty)
    pub(crate) fn rg_recv(&mut self, recv: &Expr, recv_ty: Ty, sid: u32, call: Option<&Expr>) {
        self.rg.recv = None;
        if !self.rg_on() {
            return;
        }
        let Some((mut t, mut tr, mut ds)) = self.rt_place(recv).or_else(|| self.rt_expr(recv).map(|(t, tr)| (t, tr, vec![]))) else { return };
        loop {
            match t {
                Ty::Struct(s) if s == sid => break,
                // (through a shared reference the borrow lives no longer than it alone: `x.get()` with
                // x: &'short mut &'long S borrows for 'long, as fields and indexes do)
                Ty::Ref(m, i) => {
                    let Some(&r) = tr.first() else { return };
                    deref(&mut ds, r, m);
                    tr = tr[1..].to_vec();
                    t = *i;
                }
                Ty::Box(i) => t = *i,
                _ => return,
            }
        }
        let tree = match recv_ty {
            Ty::Ref(..) => {
                let r = self.rg.var();
                let sp = super::borrowck::pos(recv);
                for d in ds {
                    self.rg.edge(d, r, sp);
                }
                // (the borrow of the receiver, by the call (the borrow check's loan of it): see Rg::holders)
                if let (None, Some(c)) = (&self.rg.closure, call) {
                    self.rg.reborrows.push((c as *const Expr as usize, r));
                }
                let mut v = vec![r];
                v.extend(tr);
                v
            }
            _ => tr,
        };
        self.rg.recv = Some((tree, super::borrowck::pos(recv)));
    }

    /// A struct literal of type t: a fresh tree, the fields' values flow into its fields
    pub(crate) fn rg_struct_lit(&mut self, e: &Expr, t: Ty) {
        // (a field of a type written `'static`: from here on, checked)
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 {
            if let Ty::Struct(sid) = t {
                if self.prog.adt_regions.get(&self.prog.base_of(sid)).is_some_and(|r| r.fields.iter().flatten().any(|&c| c == R_STATIC)) {
                    self.rg_late_on();
                }
            }
        }
        if !self.rg_on() {
            return;
        }
        let (Expr::Struct(_, fs, None), Ty::Struct(sid)) = (e, t) else { return };
        let prog = self.prog;
        if prog.structs[sid as usize].is_enum {
            return;
        }
        // (`Self { .. }` in an impl's method: of the impl's self type)
        let of_self = matches!(e, Expr::Struct(p, ..) if p.segs.len() == 1 && p.segs[0].name == "Self");
        let Some(tree) = self.self_tree(t, of_self) else { return };
        // (at a field's value, else at the literal: `Foo { x, y }` writes no value)
        let lit_sp = match e {
            Expr::Struct(p, ..) if p.pos.line != 0 => p.pos,
            _ => super::borrowck::pos(e),
        };
        for (n, v) in fs {
            let Some(i) = prog.structs[sid as usize].fields.iter().position(|f| f.0 == *n) else { continue };
            let ft = prog.structs[sid as usize].fields[i].1;
            let Some(ftr) = self.rg.field(prog, sid, i, &tree) else { continue };
            if let Some((vt, vtr)) = self.rt_expr(v) {
                let sp = Some(super::borrowck::pos(v)).filter(|s| s.line != 0).unwrap_or(lit_sp);
                self.rg.flow(prog, vt, ft, &vtr, &ftr, sp);
            }
        }
        // (the definition's bounds hold of the value built: `struct Foo<'a: 'b, 'b>`)
        self.rg.wf_declared(prog, t, &tree, lit_sp);
        self.rg.calls.insert(e as *const Expr as usize, (t, tree));
    }

    /// The tree of a value of type t built by a struct literal or a constructor: the impl's self type's for `Self`,
    /// else fresh
    fn self_tree(&mut self, t: Ty, of_self: bool) -> Option<RT> {
        let prog = self.prog;
        // (an impl of an instance (`impl A<&'static u8>`): its self type's codes, on the type built)
        if of_self && self.rg.self_tree.as_ref().is_none_or(|x| x.0 != t) {
            let rs = prog.sigs.get(self.rg.base as usize).and_then(|s| s.regions.clone());
            if let Some(r) = rs.filter(|r| !r.self_slots.is_empty()) {
                let lts = self.rg.lts.clone();
                if let Some(tr) = self.rg.instantiate(prog, t, Some(&r.self_slots), &lts, &[], &mut |me| me.var()) {
                    return Some(tr);
                }
            }
        }
        match &self.rg.self_tree {
            Some((st, tr)) if of_self && *st == t => Some(tr.clone()),
            _ => self.rg.fresh(prog, t),
        }
    }

    /// e (a path, or a call of one) is a value of type t, an ADT that bounds a type parameter by 'static (`struct
    /// St<T: 'static>`): t is well formed (`St::<T>(None)`, `En::A(x)` in a generic fn: T: 'static, E0310)
    pub(crate) fn rg_ctor_wf(&mut self, e: &Expr, t: Ty) {
        let prog = self.prog;
        let Ty::Struct(sid) = t else { return };
        if !prog.adt_regions.get(&prog.base_of(sid)).is_some_and(|r| r.ty_outlives.iter().any(|&(_, c)| c == R_STATIC)) {
            return;
        }
        let path = match e {
            Expr::Path(_, p) => p,
            Expr::Call(f, _) => match strip(f) {
                Expr::Path(_, p) => p,
                _ => return,
            },
            _ => return,
        };
        // (a constructor or a variant: the path names the type (`St`, `En::A`, `Self`), not a fn returning it)
        let name = &prog.structs[sid as usize].name;
        let name = name.split('<').next().unwrap_or(name);
        let short = name.rsplit("::").next().unwrap_or(name);
        let n = path.segs.len();
        let names_type = |k: usize| path.segs.get(k).is_some_and(|s| s.name == short || s.name == "Self");
        if !(names_type(n - 1) || (n >= 2 && names_type(n - 2))) {
            return;
        }
        let Some(tree) = self.rg.fresh(prog, t) else { return };
        self.rg.wf_declared(prog, t, &tree, super::borrowck::pos(e));
    }

    /// A tuple struct's value of type t built by its constructor (`B(a)`): the arguments flow into its fields
    pub(crate) fn rg_ctor(&mut self, c: &Expr, t: Ty, args: &[(usize, &Expr)]) {
        // (c: the call `Self(..)`)
        let of_self = matches!(strip(c), Expr::Call(f, _) if matches!(strip(f), Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "Self"));
        // (a field of a type written `'static`, or `Self` of an impl's self type that writes it: from here on,
        // checked)
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 {
            if let Ty::Struct(sid) = t {
                let self_static = of_self && self.prog.sigs.get(self.rg.base as usize).and_then(|s| s.regions.as_deref()).is_some_and(|r| r.self_slots.contains(&R_STATIC));
                if self_static || self.prog.adt_regions.get(&self.prog.base_of(sid)).is_some_and(|r| r.fields.iter().flatten().any(|&c| c == R_STATIC)) {
                    self.rg_late_on();
                }
            }
        }
        if !self.rg_on() {
            return;
        }
        let Ty::Struct(sid) = t else { return };
        let prog = self.prog;
        if prog.structs[sid as usize].is_enum {
            return;
        }
        let Some(tree) = self.self_tree(t, of_self) else { return };
        for &(i, v) in args {
            let Some(&(_, ft, _)) = prog.structs[sid as usize].fields.get(i) else { continue };
            let Some(ftr) = self.rg.field(prog, sid, i, &tree) else { continue };
            if let Some((vt, vtr)) = self.rt_expr(v) {
                self.rg.flow(prog, vt, ft, &vtr, &ftr, super::borrowck::pos(v));
            }
        }
        self.rg.calls.insert(c as *const Expr as usize, (t, tree));
    }

    /// A call of a trait's method through a trait object (decl: the trait's declaration; sol: its type arguments,
    /// params: the instance's parameters but the receiver): as a call of the declaration with Self the object
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rg_dyn_call(&mut self, mc: &Expr, decl: u32, sol: &[Option<Ty>], params: &[Ty], ret: Ty) {
        if !self.rg_on() {
            return;
        }
        let Expr::MethodCall(recv, _, _, margs, _) = mc else { return };
        let Some((rty, mut rtr)) = self.rt_expr(recv) else { return };
        let selft = match rty {
            Ty::Ref(_, i) | Ty::Box(i) => *i,
            t => t,
        };
        // (a Box's content or an object place (`(*b).get()`) is borrowed for a `&self` method)
        if let (Ty::Box(_) | Ty::Dyn(_), Some(Ty::Ref(..))) = (rty, self.prog.sigs[decl as usize].params.first()) {
            let r = self.rg_autoref(recv, rty);
            rtr.insert(0, r);
        }
        let mut sol = sol.to_vec();
        if let Some(s) = sol.first_mut() {
            *s = Some(selft);
        }
        let Some(&p0) = self.prog.sigs[decl as usize].params.first() else { return };
        let p0 = self.prog.partial_subst(p0, &sol);
        let ps: Vec<Ty> = std::iter::once(p0).chain(params.iter().copied()).collect();
        let args: Vec<&Expr> = margs.iter().collect();
        let sp = super::borrowck::pos(recv);
        self.rg.depth += 1;
        self.rg_call(decl, &sol, None, &args, &ps, ret, Some((rtr, sp)), true, super::borrowck::pos(mc));
        self.rg.depth -= 1;
        self.rg_took_call(mc);
    }

    /// The region of the borrow of a method's receiver place recv of type rty (a Box's content, or the object a
    /// place of type `dyn Tr` is) for its `&self`: as `&recv`, outlived by the references the place is reached
    /// through, and a loan of the place when the function owns it (`b.get()` and `(*b).get()` of a local Box, not
    /// `(*r).get()` of `r: &Box<dyn Tr>`)
    fn rg_autoref(&mut self, recv: &Expr, rty: Ty) -> u32 {
        let r = self.rg.var();
        let sp = super::borrowck::pos(recv);
        let call = matches!(strip(recv), Expr::Call(..) | Expr::MethodCall(..));
        let name = place_text(recv).map(|n| if matches!(rty, Ty::Box(_)) { format!("*{n}") } else { n });
        match self.rt_place(recv) {
            Some((_, _, ds)) => {
                if let (true, false, Some(n)) = (ds.is_empty(), call, name) {
                    self.rg.loans.push((r, n, sp));
                }
                for d in ds {
                    self.rg.edge(d, r, sp);
                }
            }
            None => {
                if let (Ty::Box(_), Some(n)) = (rty, name) {
                    self.rg.loans.push((r, n, sp));
                }
            }
        }
        r
    }

    /// The tree of the receiver of a call through a trait object to a method of `impl dyn Tr` (rg.recv): a
    /// reference's as it is, a Box's content or an object place borrowed for the call. need_static: the impl is for
    /// `dyn Tr + 'static`, which the object's lifetime (its tree's last region) must outlive
    pub(crate) fn rg_dyn_recv(&mut self, mc: &Expr, need_static: bool) {
        self.rg.recv = None;
        if !self.rg_on() {
            return;
        }
        let Expr::MethodCall(recv, ..) = mc else { return };
        let Some((rty, mut rtr)) = self.rt_expr(recv) else { return };
        if matches!(rty, Ty::Box(_) | Ty::Dyn(_)) {
            let r = self.rg_autoref(recv, rty);
            rtr.insert(0, r);
        }
        if let (true, Some(&obj)) = (need_static, rtr.last()) {
            self.rg.edge(obj, STATIC, super::borrowck::pos(recv));
        }
        self.rg.recv = Some((rtr, super::borrowck::pos(recv)));
    }

    /// A tuple literal of type t: its elements' trees, side by side (the tuple is covariant in them)
    pub(crate) fn rg_tuple(&mut self, e: &Expr, t: Ty) {
        if !self.rg_on() {
            return;
        }
        let (Expr::Tuple(xs), Ty::Struct(sid)) = (e, t) else { return };
        let prog = self.prog;
        let args = prog.structs[sid as usize].args.clone();
        if !prog.is_tuple(sid) || args.len() != xs.len() {
            return;
        }
        let Some(tree) = self.rg.fresh(prog, t) else { return };
        let mut k = 0;
        for (x, &at) in xs.iter().zip(&args) {
            let Some(c) = count(prog, at, 0) else { return };
            if let Some((xty, xt)) = self.rt_expr(x) {
                let part = tree[k..k + c].to_vec();
                self.rg.flow(prog, xty, at, &xt, &part, super::borrowck::pos(x));
            }
            k += c;
        }
        self.rg.calls.insert(e as *const Expr as usize, (t, tree));
    }

    /// A closure's region signature (Rg::closure) for its body's check, from the bound it is passed to (its text,
    /// the callee's lifetimes and type parameters) or else its annotations: kept with the instances (Insts::closure_rg)
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rg_closure_sig(&mut self, c: &Expr, cid: u32, params: &[Ty], ret: Ty, bound: Option<(String, Vec<String>, Vec<String>)>) {
        let Expr::Closure(_, inputs, ret_ann, body, _) = c else { return };
        let prog = self.prog;
        // (its captures' regions that are universal here: the function's own, related as it knows)
        let (caps, known, lts) = self.rg_capture_codes(cid);
        let any_cap = caps.iter().flatten().flatten().any(|&c| c >= CLO_U);
        // (nothing to check when no region is in its signature nor its captures are this function's, unless its
        // body writes `'static` (what it reaches through its environment is borrowed from outside, for less))
        // (or one that writes a lifetime the function names, or `'static`, with nothing else that asks: writes_lifetime)
        let ann_static = (!caps.is_empty() && crate::regions::annotates_static(body)) || (bound.is_none() && !inputs.iter().any(|x| x.1.is_some()) && crate::regions::writes_lifetime(body));
        if !any_cap && !ann_static && params.iter().chain([&ret]).all(|&t| count(prog, t, 0) == Some(0)) {
            return;
        }
        if bound.is_none() && !any_cap && !ann_static && !inputs.iter().any(|x| x.1.is_some()) {
            return;
        }
        let mut codes: Vec<Vec<u8>> = vec![];
        match bound {
            Some((b, lts, tps)) => {
                let Some(f) = crate::parse::parse_file(&format!("type __B = dyn {b};")).ok() else { return };
                let Some(crate::parse::ItemKind::Type(_, _, _, Some(PTy::Dyn(bs)))) = f.items.first().map(|i| &i.kind) else { return };
                let Some(crate::parse::Bound::Trait(_, p)) = bs.first() else { return };
                let Some((bps, br)) = p.segs.last().and_then(|s| s.fn_args.clone()) else { return };
                if bps.len() != params.len() {
                    return;
                }
                let mut cc = CloCodes { outer_lts: &lts, tparams: &tps, named: vec![], np: 0, ne: 0, elided: None };
                for (pt, &t) in bps.iter().zip(params) {
                    let mut v = vec![];
                    if !cc.of(prog, pt, t, &mut v, 0) {
                        return;
                    }
                    codes.push(v);
                }
                // (a parameter annotated with one of the function's lifetimes, or 'static, where the bound binds the
                // region (`|x: &'x u32|` for `F: FnOnce(&u32)`): the closure must take every region there, so 'x
                // would be all of them ("lifetime may not live long enough" both ways)
                self.rg_closure_ann_bound(c, &codes, &[], params);
                // (the result's elided regions: the one region of the inputs, as fn elision)
                let mut ins: Vec<u8> = codes.iter().flatten().copied().filter(|&c| c != R_ELIDED && c != R_STATIC).collect();
                ins.sort();
                ins.dedup();
                cc.elided = if ins.len() == 1 { Some(ins[0]) } else { None };
                let mut v = vec![];
                match &br {
                    Some(rt) => {
                        if !cc.of(prog, rt, ret, &mut v, 0) {
                            return;
                        }
                    }
                    None if count(prog, ret, 0) == Some(0) => {}
                    None => return,
                }
                // (an elided region of the result with no one input region: not followed)
                if cc.elided.is_none() && v.iter().any(|&c| c < CLO_E && cc.np as usize > ins.len()) {
                    return;
                }
                codes.push(v);
            }
            // (no bound: the annotated parameters' elided regions are bound by the closure; the result's regions
            // are from outside it)
            None => {
                let outer = self.rg.lt_names.clone();
                let mut cc = CloCodes { outer_lts: &outer, tparams: &[], named: vec![], np: 0, ne: 0, elided: None };
                for (i, &t) in params.iter().enumerate() {
                    let mut v = vec![];
                    let ok = match inputs.get(i).and_then(|x| x.1.as_ref()) {
                        Some(pt) => cc.of(prog, pt, t, &mut v, 0),
                        None => count(prog, t, 0).map(|n| v.extend(std::iter::repeat_n(R_ELIDED, n))).is_some(),
                    };
                    if !ok {
                        return;
                    }
                    codes.push(v);
                }
                // (the result's: from outside when the closure writes its type; else an expected type it is
                // coerced to may give its signature (`Box<dyn for<'r> Fn(&'r T) -> &'r U>`): not known)
                let Some(n) = count(prog, ret, 0) else { return };
                let v: Vec<u8> = (0..n.min(0x30)).map(|i| if ret_ann.is_some() { cc.e() } else { CLO_S + i as u8 }).collect();
                if v.len() != n {
                    return;
                }
                codes.push(v);
            }
        }
        let mut g = self.insts.lock().unwrap();
        g.closure_rg.insert(cid, Box::new(crate::program::CloRg { codes, caps, known, lts }));
        g.fx_log.push(cid);
    }

    /// rg_closure_sig: the annotated parameters of a closure whose bound binds a region where the annotation names one
    /// of the function's lifetimes or 'static: the bound's region is a placeholder, which that lifetime would have to
    /// be (codes: the bound's, per parameter)
    /// A closure where a fn pointer type binds the region of a parameter whose annotation names one of the function's
    /// lifetimes or 'static (`let _: fn(&()) = |_: &'a ()| {};`): as where a bound binds it (rg_closure_ann_bound)
    pub(crate) fn rg_closure_fnptr(&mut self, ann: &PTy, init: &Expr) {
        let c = super::strip_paren(init);
        let (PTy::Fn(ps, _), Expr::Closure(_, inputs, ..)) = (ann, c) else { return };
        if ps.len() != inputs.len() || !inputs.iter().any(|(_, a)| a.as_ref().is_some_and(crate::regions::names_lt)) {
            return;
        }
        let names = self.rg.lt_names.clone();
        let mut codes = vec![];
        let mut tgts = vec![];
        let mut params = vec![];
        for p in ps {
            let mut s = crate::regions::Slots { lts: names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 1, derived: None };
            let mut cs = vec![];
            s.slots(p, false, &mut cs);
            let Ok(t) = self.parse_ty(p) else { return };
            // (a path written with no lifetime is a mark that stands for no region when its type has none (`&S`):
            // the regions are the codes but those marks, as many as the type has)
            let cs: Vec<u8> = cs.into_iter().filter(|&x| x != R_ELIDED_ADT).collect();
            if count(self.prog, t, 0) != Some(cs.len()) {
                return;
            }
            codes.push(cs.iter().map(|&x| if x == R_BOUND { CLO_P } else { CLO_E }).collect::<Vec<u8>>());
            // (the regions the type names: 'static, or one of the function's lifetimes)
            tgts.push(cs.iter().map(|&x| if x == R_STATIC { Some(STATIC) } else if x < R_STATIC { self.rg.lts.get(x as usize).copied() } else { None }).collect::<Vec<Option<u32>>>());
            params.push(t);
        }
        self.rg_closure_ann_bound(c, &codes, &tgts, &params);
    }

    /// A closure held as a fn pointer (t, its tree) whose value is one of its parameters (`let g: fn(_, _) -> _ =
    /// |_x, y| y;`): that parameter flows into the result, so the pointer's parameter outlives its result (returned
    /// as `fn(&'a u32, &'b u32) -> &'a u32`, `'b: 'a`). Only for a body that is the parameter itself: what another
    /// body returns, the check does not know
    fn rg_closure_passes(&mut self, init: &Expr, t: Ty, tree: &[u32]) {
        let (Expr::Closure(_, inputs, None, body, pos), Ty::Fn(f)) = (super::strip_paren(init), t) else { return };
        if f.params.len() != inputs.len() {
            return;
        }
        let Expr::Path(None, p) = super::strip_paren(body) else { return };
        if p.segs.len() != 1 {
            return;
        }
        let Some(i) = inputs.iter().position(|(pt, _)| matches!(pt, crate::parse::Pat::Ident(false, _, n, None) if *n == p.segs[0].name)) else { return };
        let prog = self.prog;
        if f.params[i] != f.ret {
            return;
        }
        let mut at = 0;
        let mut offs = vec![];
        for &pt in f.params.iter().chain([&f.ret]) {
            let Some(c) = count(prog, pt, 0) else { return };
            offs.push((at, c));
            at += c;
        }
        if at != tree.len() {
            return;
        }
        let ((pi, pc), (ri, rc)) = (offs[i], offs[f.params.len()]);
        if pc != rc || pc == 0 {
            return;
        }
        let (ptr, rtr) = (tree[pi..pi + pc].to_vec(), tree[ri..ri + rc].to_vec());
        self.rg.flow(prog, f.params[i], f.ret, &ptr, &rtr, *pos);
    }

    fn rg_closure_ann_bound(&mut self, c: &Expr, codes: &[Vec<u8>], tgts: &[Vec<Option<u32>>], params: &[Ty]) {
        closure_ann_bound(self.prog, &mut self.rg, c, codes, tgts, params);
    }

    /// The codes of a closure's captures (cid) as trees of this function: its universal regions are CLO_U + i; what
    /// it knows of them; its named lifetimes
    #[allow(clippy::type_complexity)]
    fn rg_capture_codes(&mut self, cid: u32) -> (Vec<Option<Vec<u8>>>, Vec<(u8, u8)>, Vec<(String, u8)>) {
        let prog = self.prog;
        let Some(cd) = prog.closures.get(&cid) else { return (vec![], vec![], vec![]) };
        if !self.rg_on() {
            return (vec![None; cd.captures.len()], vec![], vec![]);
        }
        let mut us: Vec<u32> = vec![];
        let code = |us: &mut Vec<u32>, v: u32| -> u8 {
            match us.iter().position(|&x| x == v) {
                Some(i) => CLO_U + i as u8,
                None if us.len() < 0x3F => {
                    us.push(v);
                    CLO_U + us.len() as u8 - 1
                }
                None => R_ELIDED,
            }
        };
        let univ: Vec<u32> = self.rg.univ.iter().map(|u| u.0).collect();
        let mut caps = vec![];
        for n in &cd.captures {
            let seg = crate::parse::Seg { name: n.clone(), args: vec![], fn_args: None, text: n.clone() };
            let p = Expr::Path(None, crate::parse::Path { global: false, segs: vec![seg], pos: Default::default() });
            caps.push(self.rt_place(&p).map(|(_, tr, _)| {
                tr.iter()
                    .map(|&v| match v {
                        STATIC => R_STATIC,
                        v if univ.contains(&v) => code(&mut us, v),
                        _ => R_ELIDED,
                    })
                    .collect()
            }));
        }
        let mut lts = vec![];
        for (name, &v) in self.rg.lt_names.clone().iter().zip(self.rg.lts.clone().iter()) {
            lts.push((name.clone(), code(&mut us, v)));
        }
        let known = self
            .rg
            .known
            .iter()
            .filter_map(|&(a, b)| {
                let (i, j) = (us.iter().position(|&x| x == a)?, us.iter().position(|&x| x == b)?);
                Some((CLO_U + i as u8, CLO_U + j as u8))
            })
            .collect();
        (caps, known, lts)
    }

    /// A closure's body begins: its parameters' trees and its result's from its region signature; what it
    /// captures is from outside it (rg_capture)
    /// A use of an associated const of an impl for its type's 'static instance (`A::<'a>::IC` with
    /// `impl A<'static> { const IC: i32 = 10; }`): a lifetime written where the impl's self type has 'static must be
    /// 'static (the type must implement what the const is of)
    pub(crate) fn rg_static_self_const(&mut self, e: &Expr, ci: u32) {
        let Some(slots) = self.prog.static_self_consts.get(&ci) else { return };
        if !self.rg_on() {
            return;
        }
        // (the type's segment: `A::<'a>::IC`, or the self type of `<A<'a> as Tr>::IC`)
        let seg = match e {
            Expr::Path(None, p) if p.segs.len() >= 2 => &p.segs[p.segs.len() - 2],
            Expr::Path(Some(q), _) => match &**q {
                PTy::Path(None, qp) => match qp.segs.last() {
                    Some(s) => s,
                    None => return,
                },
                _ => return,
            },
            _ => return,
        };
        let lts = seg.args.iter().filter_map(|a| if let crate::parse::GArg::Lifetime(l) = a { Some(l) } else { None });
        for (i, l) in lts.enumerate() {
            if slots.get(i) != Some(&R_STATIC) || l == "'static" {
                continue;
            }
            if let Some(v) = self.rg.lt_names.iter().position(|x| x == l).and_then(|k| self.rg.lts.get(k).copied()) {
                self.rg.edge(v, STATIC, super::borrowck::pos(e));
            }
        }
    }

    /// A use of an impl's constant whose type names the impl's lifetimes (`<Foo<'a>>::C` with `impl<'a> Foo<'a> {
    /// const C: &'a u32 = &22; }`): its value is of the regions the path writes on the self type for them, a fresh one
    /// where it writes none (returned as `&'static u32`: "lifetime may not live long enough")
    pub(crate) fn rg_const_regions(&mut self, e: &Expr, ci: u32) {
        let Some((codes, self_slots)) = self.prog.const_regions.get(&ci) else { return };
        if !self.rg_on() {
            return;
        }
        let prog = self.prog;
        let seg = match e {
            Expr::Path(None, p) if p.segs.len() >= 2 => Some(&p.segs[p.segs.len() - 2]),
            Expr::Path(Some(q), _) => match &**q {
                PTy::Path(None, qp) => qp.segs.last(),
                _ => None,
            },
            _ => None,
        };
        // (the codes of the self type as the path writes it, against the function's lifetimes: a lifetime written
        // in a type argument too (`Foo::<&'b u32>::C` with `impl<'a> Foo<&'a u32>`), in the place of the impl's)
        let written: Vec<u8> = match seg {
            Some(sg) => {
                let t = PTy::Path(None, crate::parse::Path { global: false, segs: vec![sg.clone()], pos: Default::default() });
                let mut sl = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
                let mut v = vec![];
                sl.slots(&t, false, &mut v);
                v
            }
            None => vec![],
        };
        let n = codes.iter().chain(self_slots).filter(|&&c| c < R_STATIC).map(|&c| c as usize + 1).max().unwrap_or(0);
        let mut lts = vec![];
        for k in 0..n {
            let at = self_slots.iter().position(|&c| c as usize == k).filter(|_| written.len() == self_slots.len());
            let v = match at.map(|j| written[j]) {
                Some(R_STATIC) => STATIC,
                Some(c) if (c as usize) < self.rg.lts.len() => self.rg.lts[c as usize],
                _ => self.rg.var(),
            };
            lts.push(v);
        }
        let t = prog.consts[ci as usize].ty;
        let codes = codes.clone();
        if let Some(tree) = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var()) {
            self.rg.calls.insert(e as *const Expr as usize, (t, tree));
        }
    }

    pub(crate) fn rg_begin_closure(&mut self, params: &[(Loc, Ty)]) {
        let prog = self.prog;
        let Some(clo) = self.rg.closure.clone() else { return };
        let codes = &clo.codes;
        if codes.len() != params.len() + 1 {
            self.rg.on = false;
            return;
        }
        let mut rg = std::mem::take(&mut self.rg);
        rg.in_closure = true;
        let reg = |rg: &mut RgSt, c: u8| -> u32 { rg.clo_reg(c) };
        // (the function's named lifetimes, as a `let` annotation in the closure names them)
        for (n, c) in &clo.lts {
            let v = reg(&mut rg, *c);
            rg.lt_names.push(n.clone());
            rg.lts.push(v);
            if let Some(u) = rg.univ.iter_mut().find(|u| u.0 == v) {
                u.1 = n.clone();
            }
        }
        let mut trees = vec![];
        for (i, &(loc, t)) in params.iter().enumerate() {
            let tr: RT = codes[i].iter().map(|&c| reg(&mut rg, c)).collect();
            if count(prog, t, 0) != Some(tr.len()) {
                self.rg = rg;
                self.rg.on = false;
                return;
            }
            trees.push((loc, t, tr));
        }
        let rtr: RT = codes[params.len()].iter().map(|&c| reg(&mut rg, c)).collect();
        // (the environment is borrowed from outside)
        let env = rg.var();
        rg.univ.push((env, "'_".to_string()));
        rg.externals.push(env);
        rg.env_r = env;
        for (loc, t, tr) in trees {
            rg.implied(prog, t, &tr, 0);
            rg.locals.insert(loc, tr);
        }
        if count(prog, self.ret, 0) == Some(rtr.len()) {
            rg.ret = Some(rtr);
        }
        // (regions from outside the closure: what the caller knows of them is not known here, nor needed; but the
        // function's own regions are related as it knows)
        let ex = rg.externals.clone();
        for &a in &ex {
            for &b in &ex {
                rg.known.push((a, b));
            }
        }
        for &(a, b) in &clo.known {
            let (x, y) = (reg(&mut rg, a), reg(&mut rg, b));
            rg.known.push((x, y));
        }
        self.rg = rg;
    }

    /// A capture of the closure being checked, bound at loc: its regions are from outside, and it is reached through
    /// the environment
    pub(crate) fn rg_capture(&mut self, i: usize, loc: Loc, t: Ty) {
        if !self.rg_on() || self.rg.closure.is_none() {
            return;
        }
        let prog = self.prog;
        // (its regions that are of the function the closure is in: those)
        let codes = self.rg.closure.as_ref().and_then(|c| c.caps.get(i).cloned().flatten());
        // (a region of a local of the function the closure is in: from outside, related to the others from outside as
        // the function relates them, not known here; a placeholder of the closure's signature must not flow into it
        // ("borrowed data escapes outside of closure": `ay = z` with `ay = &y` captured and z of a `&isize` the bound
        // or the annotation binds))
        let cap = |rg: &mut RgSt| -> u32 {
            let v = rg.var();
            for a in rg.externals.clone() {
                rg.known.push((a, v));
                rg.known.push((v, a));
            }
            rg.univ.push((v, "'_".to_string()));
            rg.externals.push(v);
            rg.cap_regions.push(v);
            v
        };
        let tr = match codes {
            // (one of the function's universal regions: that; one of its locals' (not universal there): as above)
            Some(cs) if count(prog, t, 0) == Some(cs.len()) => Some(cs.iter().map(|&c| if c == R_ELIDED { cap(&mut self.rg) } else { self.rg.clo_reg(c) }).collect()),
            _ => self.rg.fresh(prog, t).map(|tr| tr.iter().map(|_| cap(&mut self.rg)).collect()),
        };
        if let Some(tr) = tr {
            self.rg.locals.insert(loc, tr);
            self.rg.cap_locs.push(loc);
        }
    }

    /// A closure's value of type t: what it captures by value, and borrows of what it captures by reference
    pub(crate) fn rg_closure(&mut self, c: &Expr, t: Ty) {
        if !self.rg_on() {
            return;
        }
        let prog = self.prog;
        let Ty::Struct(sid) = t else { return };
        let Some(cd) = prog.closure_of_struct.get(&prog.base_of(sid)).and_then(|cid| prog.closures.get(cid)) else { return };
        let sp = super::borrowck::pos(c);
        let mut tree = vec![];
        for (i, n) in cd.captures.iter().enumerate() {
            let seg = crate::parse::Seg { name: n.clone(), args: vec![], fn_args: None, text: n.clone() };
            let p = Expr::Path(None, crate::parse::Path { global: false, segs: vec![seg], pos: Default::default() });
            let Some((_, tr, derefs)) = self.rt_place(&p) else { return };
            if !cd.moved.get(i).copied().unwrap_or(false) {
                let r = self.rg.var();
                if derefs.is_empty() {
                    // (the borrow is the closure's; it is reported where the body uses it first, as rustc does)
                    let mut uses = vec![];
                    if let Expr::Closure(_, _, _, body, _) = c {
                        self.bk_cap_paths(body, n, &mut uses);
                    }
                    let at = uses.iter().map(|u| u.3).filter(|s| s.line != 0).min_by_key(|s| (s.line, s.col)).unwrap_or(sp);
                    self.rg.loans.push((r, n.clone(), at));
                }
                for d in derefs {
                    self.rg.edge(d, r, sp);
                }
                tree.push(r);
            }
            tree.extend(tr);
        }
        if count(prog, t, 0) == Some(tree.len()) {
            self.rg.calls.insert(c as *const Expr as usize, (t, tree));
        }
    }

    /// An array literal of type t: every element flows into the element's tree
    pub(crate) fn rg_array(&mut self, e: &Expr, t: Ty) {
        if !self.rg_on() {
            return;
        }
        let (Expr::Array(xs), Ty::Array(et, _)) = (e, t) else { return };
        let prog = self.prog;
        let Some(tree) = self.rg.fresh(prog, t) else { return };
        for x in xs {
            if let Some((xty, xt)) = self.rt_expr(x) {
                self.rg.flow(prog, xty, *et, &xt, &tree, super::borrowck::pos(x));
            }
        }
        self.rg.calls.insert(e as *const Expr as usize, (t, tree));
    }

    /// A builtin method of a reference (`buf.split_at(2)` of a slice, `s.trim()`): the references in its result (of
    /// type t) borrow what the receiver does
    pub(crate) fn rg_builtin(&mut self, e: &Expr, recv: &Expr, t: Ty) {
        if !self.rg_on() {
            return;
        }
        let prog = self.prog;
        let Some(n) = count(prog, t, 0).filter(|&n| n > 0) else { return };
        let Some((Ty::Ref(..), rtr)) = self.rt_expr(recv) else { return };
        let Some(&r) = rtr.first() else { return };
        // (a placeholder's region in the result is its own, not the receiver's: `v.iter()` of `&[T]` is `Iter<T>`)
        let tree = match self.rg.fresh(prog, t).filter(|_| !self.rg.ph.is_empty()) {
            Some(tr) => tr.into_iter().map(|v| if self.rg.ph.iter().any(|x| x.1 == v) { v } else { r }).collect(),
            None => vec![r; n],
        };
        self.rg.calls.insert(e as *const Expr as usize, (t, tree));
    }

    /// `Box::new(x)` (x of type t): the box's tree is x's
    pub(crate) fn rg_box(&mut self, c: &Expr, x: &Expr, t: Ty) {
        if !self.rg_on() {
            return;
        }
        if let Some((_, tr)) = self.rt_expr(x) {
            self.rg.calls.insert(c as *const Expr as usize, (Ty::Box(crate::types::intern(t)), tr));
        }
    }

    /// A variant's value of type t (`Some(x)`, `None`): a fresh tree, the fields' values flow into its fields; kept as
    /// a call's result (see rg_took_call)
    pub(crate) fn rg_variant(&mut self, t: Ty, vi: usize, args: &[(usize, &Expr)]) {
        if !self.rg_on() {
            return;
        }
        // (nothing a field's call left is this value's)
        self.rg.last_call = None;
        let Ty::Struct(sid) = t else { return };
        let prog = self.prog;
        let Some(tree) = self.rg.fresh(prog, t) else { return };
        let sd = &prog.structs[sid as usize];
        let Some(v) = sd.variants.get(vi) else { return };
        let off: usize = sd.fields.len() + sd.variants[..vi].iter().map(|v| v.fields.len()).sum::<usize>();
        let fts: Vec<Ty> = v.fields.iter().map(|f| f.1).collect();
        for &(fi, ex) in args {
            let (Some(&ft), Some(ftr)) = (fts.get(fi), self.rg.field(prog, sid, off + fi, &tree)) else { continue };
            if let Some((vt, vtr)) = self.rt_expr(ex) {
                self.rg.flow(prog, vt, ft, &vtr, &ftr, super::borrowck::pos(ex));
            }
        }
        self.rg.last_call = Some((self.rg.depth + 1, t, tree));
    }

    /// `for pat in e` binds items of type vt: of a Vec, an array or a slice (or a reference to one), its elements' tree
    /// (and the reference's region): the pattern's
    pub(crate) fn rg_for_item(&mut self, f: &Expr, vt: Ty) {
        if !self.rg_on() {
            return;
        }
        let Expr::For(_, _, x, _) = f else { return };
        let Some((it, itr)) = self.rt_expr(x) else { return };
        let prog = self.prog;
        let elem = |t: Ty| match t {
            Ty::Array(e, _) | Ty::Slice(e) => Some(*e),
            Ty::Struct(sid) if prog.structs[prog.base_of(sid) as usize].name == "Vec" && prog.structs[sid as usize].args.len() == 1 && lts_of(prog, prog.base_of(sid)) == 0 => Some(prog.structs[sid as usize].args[0]),
            _ => None,
        };
        let ok = match (it, vt) {
            (Ty::Ref(_, i), Ty::Ref(_, e)) => elem(*i) == Some(*e),
            (t, e) => elem(t) == Some(e),
        };
        if ok && count(prog, vt, 0) == Some(itr.len()) {
            self.rg.pat = Some((vt, itr));
        }
    }

    /// The element type and tree of an array or a slice matched (through references, as default binding modes)
    pub(crate) fn rg_slice_elem(&mut self, p: (Ty, RT)) -> Option<(Ty, RT)> {
        let (mut t, mut tr) = p;
        loop {
            match t {
                Ty::Ref(_, i) => {
                    self.rg.pat_ref = Some(*tr.first()?);
                    tr.remove(0);
                    t = *i;
                }
                Ty::Array(e, _) | Ty::Slice(e) => return Some((*e, tr)),
                _ => return None,
            }
        }
    }

    /// The pattern matches through references (default binding modes) down to a value of type t
    pub(crate) fn rg_pat_align(&mut self, t: Ty) {
        let Some((mut pt, mut tr)) = self.rg.pat.take() else { return };
        while pt != t {
            let Ty::Ref(_, i) = pt else { return };
            let Some(&r) = tr.first() else { return };
            self.rg.pat_ref = Some(r);
            tr.remove(0);
            pt = *i;
        }
        self.rg.pat = Some((pt, tr));
    }

    /// A pattern's path writes lifetimes (`Foo::Bar::<'static> { .. }`, a type annotation): the value it matches is
    /// of that type, its regions outlive them
    pub(crate) fn rg_pat_lifetimes(&mut self, path: &crate::parse::Path) {
        if !self.rg_on() {
            return;
        }
        let ls: Vec<&String> = path.segs.iter().flat_map(|s| s.args.iter()).filter_map(|a| if let GArg::Lifetime(l) = a { Some(l) } else { None }).collect();
        if ls.is_empty() {
            return;
        }
        let Some((Ty::Struct(sid), tr)) = self.rg.pat.clone() else { return };
        let prog = self.prog;
        let Some((own, _)) = self.rg.split_adt(prog, sid, &tr) else { return };
        if own.len() != ls.len() {
            return;
        }
        let sp = path.pos;
        for (&r, l) in own.iter().zip(ls) {
            let to = match l.as_str() {
                "'static" => STATIC,
                l => match self.rg.lt_names.iter().position(|x| x == l) {
                    Some(i) => self.rg.lts[i],
                    None => continue,
                },
            };
            self.rg.edge(r, to, sp);
        }
    }

    /// Descend into field fi of the value the pattern matches (of type parent; vi: the variant); returns what to
    /// restore
    pub(crate) fn rg_pat_field(&mut self, parent: Ty, vi: Option<usize>, fi: usize) -> (Option<(Ty, RT)>, Option<u32>) {
        let saved = (self.rg.pat.clone(), self.rg.pat_ref);
        let prog = self.prog;
        let next = (|| {
            let (pt, tr) = self.rg.pat.clone()?;
            let Ty::Struct(sid) = pt else { return None };
            if pt != parent {
                return None;
            }
            let sd = &prog.structs[sid as usize];
            if prog.is_tuple(sid) {
                let (_, args) = self.rg.split_adt(prog, sid, &tr)?;
                return Some((*sd.args.get(fi)?, args.get(fi)?.clone()?));
            }
            let (off, ft) = match vi {
                Some(vi) => (sd.fields.len() + sd.variants[..vi].iter().map(|v| v.fields.len()).sum::<usize>() + fi, sd.variants.get(vi)?.fields.get(fi)?.1),
                None => (fi, sd.fields.get(fi)?.1),
            };
            let ftr = self.rg.field(prog, sid, off, &tr)?;
            Some((ft, ftr))
        })();
        if debug() {
            eprintln!("rg_pat_field {:?} vi={vi:?} fi={fi}: {:?} -> {:?}", self.prog.tn(parent), saved.0.as_ref().map(|x| (self.prog.tn(x.0), x.1.clone())), next.as_ref().map(|x| x.1.clone()));
        }
        self.rg.pat = next;
        saved
    }

    /// The pattern binds name (by_ref: by reference, through the reference matched last)
    pub(crate) fn rg_pat_bind(&mut self, name: &str, by_ref: bool) {
        let Some((_, tr)) = self.rg.pat.clone() else { return };
        let Some((loc, t)) = self.lookup(name) else { return };
        let tree = if by_ref {
            // (by the reference matched through, or a `ref` binding's own borrow: a region of its own, a loan of the
            // place matched when the function owns it (`|ref line: String| ..`, `let ref r = s;`))
            let r = match self.rg.pat_ref {
                Some(r) => r,
                None => {
                    let r = self.rg.var();
                    // (the place the borrow check has the pattern at: one the function owns, through no reference
                    // (it drops the place where a pattern matches through one: `|&ref x| ..`, `match self { S(ref
                    // x) => .. }` of `self: &S`))
                    if let Some(pp) = self.bk.pat_path.as_ref().filter(|pp| pp.proj.iter().all(|x| matches!(x, super::borrowck::Proj::Field(_) | super::borrowck::Proj::Deref(true)))) {
                        let n = super::borrowck::path_str(&self.bk.locals, pp);
                        let sp = self.bk.locals.get(pp.local as usize).map(|l| l.decl).unwrap_or(super::SP);
                        self.rg.loans.push((r, n, sp));
                    }
                    r
                }
            };
            let mut v = vec![r];
            v.extend(tr);
            v
        } else {
            tr
        };
        if count(self.prog, t, 0) == Some(tree.len()) {
            self.rg.locals.insert(loc, tree);
        }
    }

    /// The tree of e, kept while what it names is in scope (an arm's or a block's value, related after the match or
    /// block is lowered)
    pub(crate) fn rg_keep(&mut self, e: &Expr) {
        if !self.rg_on() || self.rg.calls.contains_key(&(e as *const Expr as usize)) {
            return;
        }
        if let Some(t) = self.rt_expr(e) {
            self.rg.calls.insert(e as *const Expr as usize, t);
        }
    }

    /// Keep the tree of the call just lowered, as the value of e
    /// A value of an ADT whose field nests two of its lifetimes (`p: &'a mut &'b i32`) relates the borrows it is
    /// built from (a method may store one through the other): the region check of a function the gate left out is
    /// turned on before its fields are lowered, from the function's start if a local bound before has no tree
    /// (nll/capture-ref-in-struct.rs)
    pub(crate) fn rg_adt_on(&mut self, base: u32) {
        if self.rg.on || !self.rg.eligible || self.inline_depth != 0 {
            return;
        }
        let Some(r) = self.prog.adt_regions.get(&base) else { return };
        if r.lts.len() < 2 {
            return;
        }
        let nests = r.fields.iter().any(|f| {
            let mut first = None;
            f.iter().any(|&c| c < R_STATIC && *first.get_or_insert(c) != c)
        });
        if nests {
            self.rg_late_on();
        }
    }

    /// A constructor's path that writes lifetimes turns the region check on, before what it holds is lowered
    pub(crate) fn rg_written_on(&mut self, path: &crate::parse::Path) {
        if !self.rg.on && self.rg.eligible && self.inline_depth == 0 && path.segs.iter().any(|s| !s.args.is_empty()) && crate::regions::path_names_lt(path) {
            self.rg_late_on();
        }
    }

    /// A constructor's path writes lifetimes (`SomeEnum::SomeVariant::<&'static u32>(&c)`, `S::<&'a u32> { .. }`,
    /// `E::Unit::<Cell<&'a u32>>`): the value built (its tree, kept at e) is of the type written. What it holds flows
    /// into that type, and the value is of it from here on
    pub(crate) fn rg_written_ctor(&mut self, e: &Expr, path: &crate::parse::Path) {
        if !self.rg_on() || path.segs.iter().all(|s| s.args.is_empty()) || !crate::regions::path_names_lt(path) {
            return;
        }
        let key = e as *const Expr as usize;
        let Some((t, tr)) = self.rg.calls.get(&key).cloned() else { return };
        let (Ty::Struct(sid), Some(seg)) = (t, path.segs.iter().rev().find(|s| !s.args.is_empty())) else { return };
        let prog = self.prog;
        // (the arguments of the segment that has them (the enum's or the variant's), on the type's name)
        let mut seg = seg.clone();
        seg.name = prog.structs[prog.base_of(sid) as usize].name.clone();
        let pt = PTy::Path(None, crate::parse::Path { global: false, segs: vec![seg], pos: path.pos });
        let mut s = crate::regions::Slots { lts: self.rg.lt_names.clone(), tparams: vec![], self_slots: &[], self_paths: &[], any: false, in_fn: 0, derived: None };
        let mut codes = vec![];
        s.slots(&pt, false, &mut codes);
        let lts = self.rg.lts.clone();
        self.rg.dyn_infer = true;
        let w = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var());
        self.rg.dyn_infer = false;
        let Some(w) = w.filter(|w| w.len() == tr.len()) else { return };
        // (not its well-formedness: that may rest on implied bounds the check does not know, as of a normalized
        // parameter's type (implied-bounds/normalization.rs: `None::<&'a A>`))
        let sp = super::borrowck::pos(e);
        self.rg.flow(prog, t, t, &tr, &w, sp);
        self.rg.calls.insert(key, (t, w));
    }

    pub(crate) fn rg_took_call(&mut self, e: &Expr) {
        // (only the call made by this expression's own call_fn: one level below)
        // (read first: taking it would make the state of a check that wrote none)
        if self.rg.last_call.is_none() {
            return;
        }
        if let Some((d, t, tr)) = self.rg.last_call.take() {
            if d == self.rg.depth + 1 {
                self.rg.calls.insert(e as *const Expr as usize, (t, tr));
            }
        }
    }

    /// Report what the constraints require
    pub(crate) fn rg_check(&mut self) -> super::R<()> {
        if debug() {
            eprintln!("regionck {}: on={} st={} univ={} edges={} loans={} missed={:?}", self.prog.names.get(self.rg.base as usize).map(|s| s.as_str()).unwrap_or("?"), self.rg.on, self.rg.has_st(), self.rg.univ.len(), self.rg.edges.len(), self.rg.loans.len(), self.rg.missed);
            if std::env::var_os("ZUSTC_RG_EDGES").is_some() {
                eprintln!("  univ {:?} loans {:?} edges {:?} ret {:?}", self.rg.univ, self.rg.loans.iter().map(|l| (l.0, &l.1)).collect::<Vec<_>>(), self.rg.edges.iter().map(|e| (e.0, e.1, e.2.line)).collect::<Vec<_>>(), self.rg.ret);
            }
        }
        if !self.rg.on {
            return Ok(());
        }
        match self.rg.check() {
            // (a placeholder's type by its parameter's name: `T`, `<T as Tr>::Out`, not `T#`)
            // (and a name the program shares with the prelude's, kept apart as `__u_Iter`, as written)
            Some((sp, m)) => Err(self.at(sp, if self.rg.ph.is_empty() { m } else { m.replace("#`", "`").replace("#>", ">").replace("__u_", "") })),
            None => Ok(()),
        }
    }
}

/// The declaration in its trait of the method an impl's method implements (by name)
pub fn trait_decl_of(prog: &Program, mi: u32) -> Option<u32> {
    let ms = prog.sigs.get(mi as usize)?;
    let tr = ms.impl_trait.as_deref()?;
    let m = prog.names[mi as usize].rsplit("::").next()?.split('#').next()?;
    let ds = prog.traits.get(tr)?;
    ds.iter().copied().find(|&d| d != mi && prog.names[d as usize].rsplit("::").next() == Some(m))
}

/// An impl's method against its trait's declaration, by regions (rustc's "`impl` item signature doesn't match
/// `trait` item signature"): the declaration's regions are universal, the method's own lifetimes and elided regions
/// are variables; each of the declaration's parameters flows into the impl's, and the impl's result into the
/// declaration's. True when that needs a region of the declaration to outlive another it is not known to
/// (`fn foo<'a>(x: &'a i32, y: &'a i32) -> &'a i32` for `fn foo<'a>(x: &i32, y: &'a i32) -> &'a i32`). Not compared:
/// signatures that name the impl's or the trait's own lifetimes, or declare bounds among lifetimes, and parameters
/// whose types do not have as many regions on both sides (`Self`, a trait's type parameter)
/// An impl's `Self` and the trait's arguments its header gives (`impl HaveRelationship<&ProofReader> for Article`),
/// when one of them has a region and they are all elided: the header writes no lifetime (`'static`, `'a`) and the
/// impl declares none. None for two impls of the trait for the type (`impl Tr<&u8> for S`, `impl Tr<&u16> for S`)
fn header_targs(prog: &Program, ms: &crate::program::FnSig, mr: Option<&RSigSyn>) -> Option<(Ty, Vec<Ty>)> {
    let tr = ms.impl_trait.as_deref()?;
    let st = ms.self_ty?;
    if mr.is_some_and(|r| r.outer > 0) || prog.impl_targs_lts.contains(&(tr.to_string(), st)) {
        return None;
    }
    let mut es = prog.impl_targs.get(tr)?.iter().filter(|e| e.0 == st);
    let e = es.next()?;
    if es.next().is_some() {
        return None;
    }
    let regions = |t: Ty| count(prog, t, 0).is_some_and(|n| n > 0);
    if !regions(st) && !e.1.iter().any(|&t| regions(t)) {
        return None;
    }
    Some((st, e.1.clone()))
}

pub fn impl_sig_mismatch(prog: &Program, decl: u32, imp: u32) -> bool {
    let (ds, ms) = (&prog.sigs[decl as usize], &prog.sigs[imp as usize]);
    if ds.params.len() != ms.params.len() || ds.has_self != ms.has_self {
        return false;
    }
    let (dr, mr) = (ds.regions.as_deref(), ms.regions.as_deref());
    // (the trait's type parameters as the impl's header gives them: `To` of `fn get_relation(&self) -> To` is the
    // header's `&ProofReader` in `impl HaveRelationship<&ProofReader> for Article`)
    let hdr = header_targs(prog, ms, mr);
    // (nothing written on either side, nor in the header: every region is elided the same way)
    if dr.is_none() && mr.is_none() && hdr.is_none() {
        return false;
    }
    let plain = |r: Option<&RSigSyn>| {
        r.is_none_or(|r| {
            r.outlives.is_empty() && r.ty_outlives.is_empty() && r.slots.iter().flatten().all(|&c| c == R_STATIC || c == R_ELIDED || c == R_ELIDED_ADT || (c >= r.outer && (c as usize) < r.lts.len()))
        })
    };
    if !plain(dr) || !plain(mr) {
        return false;
    }
    fn codes(r: Option<&RSigSyn>, i: usize) -> Option<&[u8]> {
        r.map(|r| r.slots.get(i).map(|c| c.as_slice()).unwrap_or(&[]))
    }
    let mut rg = RgSt::default();
    // (`Self` and the trait's type parameters (Param(0), Param(1), ..): their regions are the impl's, universal, each
    // elided one of the header its own lifetime parameter of the impl)
    let targs: Vec<Option<RT>> = match &hdr {
        Some((st, args)) => std::iter::once(st)
            .chain(args.iter())
            .map(|&t| {
                rg.instantiate(prog, t, None, &[], &[], &mut |me| {
                    let v = me.var();
                    me.univ.push((v, "'_".to_string()));
                    v
                })
            })
            .collect(),
        None => vec![],
    };
    let dl: Vec<u32> = dr.map(|r| r.lts.as_slice()).unwrap_or(&[]).iter().map(|l| {
        let v = rg.var();
        rg.univ.push((v, l.clone()));
        v
    }).collect();
    let ml: Vec<u32> = mr.map(|r| r.lts.as_slice()).unwrap_or(&[]).iter().map(|_| rg.var()).collect();
    let sp = super::SP;
    let (mut d_in, mut m_in): (Vec<u32>, Vec<u32>) = (vec![], vec![]);
    let (mut d_self, mut m_self) = (None, None);
    for i in 0..ds.params.len() {
        let (dt, mt) = (ds.params[i], ms.params[i]);
        let Some(dtr) = rg.instantiate(prog, dt, codes(dr, i), &dl, &targs, &mut |me| {
            let v = me.var();
            me.univ.push((v, "'_".to_string()));
            v
        }) else {
            return false;
        };
        let Some(mtr) = rg.instantiate(prog, mt, codes(mr, i), &ml, &[], &mut |me| me.var()) else { return false };
        if dtr.len() != mtr.len() || count(prog, mt, 0) != Some(mtr.len()) {
            return false;
        }
        if i == 0 && ds.has_self && matches!(dt, Ty::Ref(..)) && matches!(mt, Ty::Ref(..)) {
            d_self = dtr.first().copied();
            m_self = mtr.first().copied();
        }
        // (`'static` too: the one region of the inputs, written or elided, is the result's elided one)
        d_in.extend(dtr.iter().copied());
        m_in.extend(mtr.iter().copied());
        rg.implied(prog, dt, &dtr, 0);
        rg.relate(prog, mt, &dtr, &mtr, sp);
    }
    // (the results' elided regions: of `&self`, else of the one region of the parameters)
    let one = |v: &mut Vec<u32>| {
        v.sort();
        v.dedup();
        if v.len() == 1 { Some(v[0]) } else { None }
    };
    let (d_el, m_el) = (d_self.or_else(|| one(&mut d_in)), m_self.or_else(|| one(&mut m_in)));
    let mut bad = false;
    let n = ds.params.len();
    let dret = rg.instantiate(prog, ds.ret, codes(dr, n), &dl, &targs, &mut |_| d_el.unwrap_or_else(|| {
        bad = true;
        STATIC
    }));
    let mret = rg.instantiate(prog, ms.ret, codes(mr, n), &ml, &[], &mut |_| m_el.unwrap_or_else(|| {
        bad = true;
        STATIC
    }));
    let (Some(dret), Some(mret)) = (dret, mret) else { return false };
    if bad || dret.len() != mret.len() || count(prog, ms.ret, 0) != Some(mret.len()) {
        return false;
    }
    rg.relate(prog, ms.ret, &mret, &dret, sp);
    rg.check().is_some()
}

/// t holds a fn pointer type (`Option<fn()>`, `&fn(&u8)`)
/// The tokens of a type's text (lifetimes, names, punctuation; `->` one), each with whether it is inside the
/// arguments of a trait object (`dyn Foo<..>`, `dyn Fn(..)`), not its own `+ 'a`
fn dyn_arg_tokens(s: &str) -> Vec<(String, bool)> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut depth = 0usize;
    let mut dyn_at: Vec<usize> = vec![];
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        if c == '\'' || c.is_alphanumeric() || c == '_' {
            while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
        } else if c == '-' && cs.get(i) == Some(&'>') {
            i += 1;
        }
        let tok: String = cs[start..i].iter().collect();
        match tok.as_str() {
            "<" | "(" | "[" => depth += 1,
            ">" | ")" | "]" => {
                depth = depth.saturating_sub(1);
                while dyn_at.last().is_some_and(|&d| d > depth) {
                    dyn_at.pop();
                }
            }
            "dyn" => dyn_at.push(depth),
            _ => {}
        }
        let inside = dyn_at.iter().any(|&d| depth > d);
        out.push((tok, inside));
    }
    out
}

fn has_fn_ty(t: Ty, depth: u32) -> bool {
    if depth > 8 {
        return false;
    }
    match t {
        Ty::Fn(_) => true,
        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => has_fn_ty(*i, depth + 1),
        _ => false,
    }
}
