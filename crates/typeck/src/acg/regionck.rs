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

#[derive(Default)]
pub struct Rg {
    pub on: bool,
    /// The function may be checked (its own body, not a closure's), though the gate may have left it out
    pub eligible: bool,
    /// The `let` being lowered: where an error is reported for what has no position of its own (a block)
    pub let_sp: Sp,
    /// Region variables made (0 is `'static`)
    pub n: u32,
    /// The universal regions (not `'static`), with their names
    pub univ: Vec<(u32, String)>,
    /// Outlives relations among universal regions the function may assume (a: b)
    pub known: Vec<(u32, u32)>,
    /// Constraints a: b, with where they come from
    pub edges: Vec<(u32, u32, Sp)>,
    pub locals: HashMap<Loc, RT>,
    /// The trees of call results (by the address of the call expression)
    pub calls: HashMap<usize, (Ty, RT)>,
    /// The result of the last call lowered (at the depth of call_fn it was made at)
    pub last_call: Option<(u32, Ty, RT)>,
    /// Nesting of call_fn
    pub depth: u32,
    /// The receiver of the method call being lowered, as passed, and its position
    pub recv: Option<(RT, Sp)>,
    /// The tree of the return type
    pub ret: Option<RT>,
    /// The function's own lifetime parameters (in `RSigSyn::lts` order)
    pub lts: Vec<u32>,
    pub lt_names: Vec<String>,
    /// Variance of a struct / enum definition: its lifetime parameters, then its type parameters
    pub variance: HashMap<u32, Vec<Var>>,
    /// The definitions whose variance is being computed; whether a provisional one was read; the ones computed, in
    /// order (see variance)
    pub var_open: Vec<u32>,
    pub var_cycle: bool,
    pub var_log: Vec<u32>,
    /// The function's own definition (of an instance: the generic one) and an instance's type arguments
    pub base: u32,
    pub inst_args: Vec<Ty>,
    /// Borrows of what the function owns (a local, not through a reference): (region, the place's text, where)
    pub loans: Vec<(u32, String, Sp)>,
    /// Instantiating a type written in a body (`let` annotation, cast): a trait object's default lifetime is inferred
    pub dyn_infer: bool,
    /// ZUSTC_RG_DEBUG: the kinds of values not followed
    pub missed: Vec<&'static str>,
    /// The last scrutinee's tree; the part of it the pattern being bound matches, and the region of the reference
    /// matched through last (bindings in it are by reference)
    pub scrut: Option<(Ty, RT)>,
    pub pat: Option<(Ty, RT)>,
    pub pat_ref: Option<u32>,
    /// A match lowered as another (its or-pattern arms with guards split): the one lowered
    pub alias: HashMap<usize, &'static Expr>,
    /// Values of no region borrowed as temporaries no constant promotion makes static (`&(x + 1)`, `&(5.f())`)
    pub temps: HashMap<usize, Ty>,
    /// The method call being made (call_fn is given no span for it)
    pub call_sp: Sp,
    /// Where values are returned (a borrow that escapes there is reported there: E0515)
    pub ret_sps: Vec<Sp>,
    /// A function the gate left out: how its receiver elides (`&self`), and whether another receiver leaves the
    /// result's regions unknown, for a check begun later (rg_late_on)
    pub late: Option<(bool, bool)>,
    /// A closure's body: its region signature (codes CLO_P + i: a region the bound binds, a placeholder; CLO_E + i:
    /// one from outside the closure), the locals that are its captures, and the region its environment is borrowed for
    pub closure: Option<Box<crate::program::CloRg>>,
    pub cap_locs: Vec<Loc>,
    pub env_r: u32,
    pub externals: Vec<u32>,
    /// A closure's body: the regions its codes name (CLO_U: of the function it is in, related as there)
    pub clo_regs: Vec<(u8, u32)>,
    pub outer_univ: Vec<u32>,
    /// CLO_S regions: only borrows of what the closure owns must not reach them
    pub soft: Vec<u32>,
    /// The elements' tree of the array or slice a slice pattern matches
    pub pat_elem: Option<(Ty, RT)>,
    /// Rg::wf: imp's relations are required, at this span (not those of the fields: wf_declared)
    pub wf_sp: Option<Sp>,
    pub wf_no_fields: bool,
    /// The check is on from the function's start whatever the gate says (compile's second run); the check began late
    /// (rg_late_on); a local bound before it with no tree was met (the function is compiled again)
    pub force: bool,
    pub began_late: bool,
    pub rerun: bool,
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
    prog.adt_regions.get(&base).map_or(0, |r| r.lts.len())
}

fn is_closure(prog: &Program, sid: u32) -> bool {
    prog.closure_of_struct.contains_key(&prog.base_of(sid))
}

/// A closure's captures: their types (among its type arguments: a capture by reference is a pointer to it) and
/// whether each is held by value
fn captures(prog: &Program, sid: u32) -> Option<(&[Ty], &[bool])> {
    let cd = prog.closures.get(prog.closure_of_struct.get(&prog.base_of(sid))?)?;
    let args = prog.structs[sid as usize].args.get(cd.n_parent..cd.n_parent + cd.captures.len())?;
    Some((args, &cd.moved))
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
            // (a closure: the regions of what it captures, a capture by reference the borrow's region first)
            if is_closure(prog, sid) {
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
            let mut n = lts_of(prog, prog.base_of(sid));
            for &a in &prog.structs[sid as usize].args {
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

/// e has no value: it panics, returns, breaks or continues (as written)
fn diverges(e: &Expr) -> bool {
    match strip(e) {
        Expr::Return(_) | Expr::Break(..) | Expr::Continue(_) => true,
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

impl Rg {
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
        let n = count(prog, t, 0)?;
        Some((0..n).map(|_| self.var()).collect())
    }

    /// The variance of the lifetime and type parameters of a definition (computed once)
    pub fn variance(&mut self, prog: &Program, base: u32) -> Vec<Var> {
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
        let prelude = prog.struct_item.get(base as usize).is_some_and(|&i| i >= prog.prelude_from);
        // (the prelude's types stand for std's: raw pointers in them are as `NonNull` (covariant); its cells are
        // invariant)
        if prelude {
            let inv = matches!(sd.name.as_str(), "Cell" | "RefCell" | "UnsafeCell" | "Mutex" | "RwLock" | "OnceCell" | "OnceLock" | "AtomicPtr" | "MutexGuard");
            let v = vec![if inv { Var::Inv } else { Var::Co }; nl + ng];
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
            self.variance.insert(base, cur.clone());
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
        self.variance.insert(base, cur.clone());
        self.var_log.push(base);
        cur
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
                if is_closure(prog, sid) {
                    c.ok = false;
                    return;
                }
                let base = prog.base_of(sid);
                let n2 = lts_of(prog, base);
                let nv = self.variance(prog, base);
                if !prog.is_tuple(sid) {
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
                let args = prog.structs[sid as usize].args.clone();
                for (j, a) in args.into_iter().enumerate() {
                    let w = if prog.is_tuple(sid) { Var::Co } else { nv.get(n2 + j).copied().unwrap_or(Var::Inv) };
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
                    // (a type alias: the regions in it are not followed, its type parameters are)
                    let mut none = Codes { codes: None, pos: 0, ok: true, parent: None };
                    for &p in &ft.params {
                        self.var_walk(prog, p, &mut none, v.then(Var::Contra), 0, acc, depth + 1);
                    }
                    self.var_walk(prog, ft.ret, &mut none, v, 0, acc, depth + 1);
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
    fn inst(&mut self, prog: &Program, t: Ty, c: &mut Codes, lts: &[u32], targs: &[Option<RT>], elided: &mut dyn FnMut(&mut Rg) -> u32, out: &mut RT, depth: u32) {
        if depth > 24 || !c.ok {
            c.ok = false;
            return;
        }
        let parent = c.parent.take();
        let region = |me: &mut Rg, c: &mut Codes, elided: &mut dyn FnMut(&mut Rg) -> u32| -> u32 {
            match c.next() {
                R_STATIC => STATIC,
                R_ELIDED => elided(me),
                R_BOUND => me.var(),
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
                if is_closure(prog, sid) {
                    c.ok = false;
                    return;
                }
                let base = prog.base_of(sid);
                let n = lts_of(prog, base);
                let own0 = out.len();
                if !prog.is_tuple(sid) {
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
                let ar = prog.adt_regions.get(&base).cloned();
                for (j, &a) in prog.structs[sid as usize].args.clone().iter().enumerate() {
                    // (a trait object as a type argument: its default lifetime is the parameter's one lifetime bound
                    // (`struct Ref<'a, T: 'a>` makes `Ref<'a, dyn Tr>` `dyn Tr + 'a`); with two, a variable of its own)
                    if matches!(a, Ty::Dyn(_)) {
                        if let (Some(ar), Some(gn)) = (&ar, prog.structs[base as usize].generics.get(j)) {
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
                    R_DYN_DEFAULT | R_ELIDED => match parent {
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
                let mut own = |me: &mut Rg| me.var();
                if c.next() == R_FN {
                    for &p in &ft.params {
                        self.inst(prog, p, c, lts, targs, &mut own, out, depth + 1);
                    }
                    self.inst(prog, ft.ret, c, lts, targs, &mut own, out, depth + 1);
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
            Ty::Assoc(_) => c.ok = false,
            _ => {}
        }
    }

    /// Instantiate t with codes (all of them must be read)
    #[allow(clippy::too_many_arguments)]
    pub fn instantiate(&mut self, prog: &Program, t: Ty, codes: Option<&[u8]>, lts: &[u32], targs: &[Option<RT>], elided: &mut dyn FnMut(&mut Rg) -> u32) -> Option<RT> {
        // (a parameter whose type is not followed)
        if codes.is_some_and(|c| c.contains(&R_UNKNOWN)) {
            return None;
        }
        let mut c = Codes { codes, pos: 0, ok: true, parent: None };
        let mut out = vec![];
        self.inst(prog, t, &mut c, lts, targs, elided, &mut out, 0);
        (c.ok && codes.is_none_or(|x| c.pos == x.len())).then_some(out)
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
        match (from, to) {
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
                let base = prog.base_of(sid);
                let n = lts_of(prog, base);
                let nv = self.variance(prog, base);
                for j in 0..n {
                    self.constrain(sub[*k], sup[*k], v.then(nv[j]), sp);
                    *k += 1;
                }
                let tuple = prog.is_tuple(sid);
                for (j, &a) in prog.structs[sid as usize].args.clone().iter().enumerate() {
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

    /// x outlives y: known (implied bounds), or required (wf)
    fn imp_rel(&mut self, x: u32, y: u32) {
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
                // (its type arguments are well formed: `Option<&'s &'l ()>` implies 'l: 's)
                if depth < 4 && !is_closure(prog, sid) {
                    if let Some((_, args)) = self.split_adt(prog, sid, &me) {
                        for (&a, at) in prog.structs[sid as usize].args.clone().iter().zip(args) {
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
                            let (Some(g), Some(&y)) = (bd.generics.iter().position(|x| x == tp), own.get(*b as usize)) else { continue };
                            if let Some(Some(at)) = args.get(g) {
                                for &x in at {
                                    self.imp_rel(x, y);
                                }
                            }
                        }
                    }
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
        if self.edges.is_empty() {
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
        let n = self.n as usize + 1;
        let mut adj: Vec<Vec<(u32, Sp)>> = vec![vec![]; n];
        for &(a, b, sp) in &self.edges {
            if (a as usize) < n && (b as usize) < n {
                adj[a as usize].push((b, sp));
            }
        }
        let mut errs: Vec<(Sp, String)> = vec![];
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
                        let outside = |a: u32, b: u32| self.externals.contains(&a) && (b == STATIC || self.outer_univ.contains(&b));
                        // (nor one of the function's that must outlive a placeholder: the callee's lifetimes in the
                        // bound may be it, and what they imply is of them)
                        let placeholder = |b: u32| self.closure.is_some() && !self.externals.contains(&b) && !self.outer_univ.contains(&b) && b != STATIC;
                        if !k[i][j] && !outside(u, y) && !outside(y, u) && !(self.outer_univ.contains(&u) && placeholder(y)) && !self.soft.contains(&u) && !self.soft.contains(&y) {
                            let other = if j == 0 { "'static".to_string() } else { self.univ[j - 1].1.clone() };
                            errs.push((sp, format!("lifetime may not live long enough (`{uname}` must outlive `{other}`)")));
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
                Some(sp) if bad => errs.push((sp, if name.is_empty() { "cannot return value referencing temporary value (E0515)".to_string() } else { format!("cannot return value referencing local variable `{name}` (E0515)") })),
                _ if bad => errs.push((*lsp, if name.is_empty() { "temporary value dropped while borrowed (it must outlive the function)".to_string() } else { format!("`{name}` does not live long enough (borrowed value must outlive the function)") })),
                _ => {}
            }
        }
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
        let rs = regions;
        let codes = |i: usize| rs.map(|r| r.slots.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        let mut rg = std::mem::take(&mut self.rg);
        for l in rs.map(|r| r.lts.as_slice()).unwrap_or(&[]) {
            let v = rg.var();
            rg.univ.push((v, l.clone()));
            rg.lts.push(v);
            rg.lt_names.push(l.clone());
        }
        let lts = rg.lts.clone();
        // (an instance: the generic signature's types, with fresh trees for the type parameters)
        let gsig = &prog.sigs[rg.base as usize];
        let generic = !rg.inst_args.is_empty() && gsig.params.len() == params.len();
        let targs: Vec<Option<RT>> = if generic { rg.inst_args.clone().into_iter().map(|t| rg.fresh(prog, t)).collect() } else { vec![] };
        let mut trees: Vec<Option<RT>> = vec![];
        let mut inputs: Vec<u32> = vec![];
        for (i, &(_, t)) in params.iter().enumerate() {
            let t = if generic { gsig.params[i] } else { t };
            let mut el = |me: &mut Rg| {
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
        let mut el = |_: &mut Rg| match self_r.or(one) {
            Some(v) => v,
            None => {
                bad = true;
                STATIC
            }
        };
        let gret = if generic { gsig.ret } else { ret_t };
        let ret = rg.instantiate(prog, gret, codes(params.len()), &lts, &targs, &mut el);
        rg.ret = ret.filter(|_| !bad);
        let nuniv = rg.univ.len();
        // (or a signature that writes 'static: `fn f(x: isize) -> &'static [isize]`)
        rg.on = force || nuniv >= 2 || (nuniv >= 1 && (rs.is_some() || has_dyn(prog, ret_t, 0))) || rs.is_some_and(Self::needs_static);
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
                if to != it {
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
                let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], any: false, in_fn: 0 };
                let mut codes = vec![];
                s.slots(ty, false, &mut codes);
                let lts = self.rg.lts.clone();
                self.rg.dyn_infer = true;
                let tree = self.rg.instantiate(prog, to, Some(&codes), &lts, &[], &mut |me| me.var());
                self.rg.dyn_infer = false;
                let tree = tree?;
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
                // (a unit variant: `None`)
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
        if self.rg.calls.contains_key(&k) || !(self.not_const(x) || dropped()) || super::borrowck::has_refs(self.prog, t, 0) {
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
                Expr::Path(_, p) if self.prog.variant_by_p(&crate::program::PPath::of(p), self.self_sid).is_some() || self.prog.struct_by_p(&crate::program::PPath::of(p), self.self_sid).is_some() => args.iter().any(|a| self.not_const(a)),
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
    pub(crate) fn needs_static(r: &RSigSyn) -> bool {
        r.slots.iter().flatten().any(|&c| c == R_STATIC) || r.ty_outlives.iter().any(|x| x.1 == R_STATIC) || r.outlives.iter().any(|x| x.1 == R_STATIC)
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
        // (an annotation names the function's lifetimes; its elided ones are fresh)
        let tree = match ann {
            Some(a) => {
                let names = self.rg.lt_names.clone();
                let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], any: false, in_fn: 0 };
                let mut codes = vec![];
                s.slots(a, false, &mut codes);
                let lts = self.rg.lts.clone();
                self.rg.dyn_infer = true;
                let tr = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var());
                self.rg.dyn_infer = false;
                tr
            }
            None => self.rg.fresh(prog, t),
        };
        let Some(tree) = tree else { return };
        // (the annotation's type is well formed)
        if ann.is_some() {
            self.rg.wf(prog, t, &tree, super::borrowck::pos(init));
        }
        // (the initializer's tree was taken before the binding: `let y = y;` reads the y it shadows)
        if let Some((it, itr)) = init_tree {
            self.rg.flow(prog, it, t, &itr, &tree, super::borrowck::pos(init));
        }
        self.rg.locals.insert(loc, tree);
    }

    /// `let _: ann = init;`: init flows into a value of the annotation's type
    pub(crate) fn rg_let_wild(&mut self, ann: &PTy, init: &Expr) {
        if !self.rg_on() {
            return;
        }
        let Some((it, itr)) = self.rt_expr(init) else { return };
        let Ok(t) = self.parse_ty(ann) else { return };
        let prog = self.prog;
        let names = self.rg.lt_names.clone();
        let mut s = crate::regions::Slots { lts: names, tparams: vec![], self_slots: &[], any: false, in_fn: 0 };
        let mut codes = vec![];
        s.slots(ann, false, &mut codes);
        let lts = self.rg.lts.clone();
        self.rg.dyn_infer = true;
        let tree = self.rg.instantiate(prog, t, Some(&codes), &lts, &[], &mut |me| me.var());
        self.rg.dyn_infer = false;
        if let Some(tree) = tree {
            self.rg.wf(prog, t, &tree, super::borrowck::pos(init));
            self.rg.flow(prog, it, t, &itr, &tree, super::borrowck::pos(init));
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
        if !self.rg_on() {
            return;
        }
        let (Some((t, ptr, _)), Some((vt, vtr))) = (self.rt_place(place), self.rt_expr(value)) else { return };
        let prog = self.prog;
        self.rg.flow(prog, vt, t, &vtr, &ptr, super::borrowck::pos(value));
    }

    /// The value e is returned
    pub(crate) fn rg_ret(&mut self, e: &Expr) {
        if !self.rg_on() {
            return;
        }
        match e {
            Expr::Paren(x) => return self.rg_ret(x),
            Expr::Block(None, _, b) => {
                if let Some(Stmt::Expr(t, false)) = b.stmts.last() {
                    self.rg_ret(t);
                }
                return;
            }
            Expr::If(_, b, els) => {
                if let Some(Stmt::Expr(t, false)) = b.stmts.last() {
                    self.rg_ret(t);
                }
                if let Some(x) = els {
                    self.rg_ret(x);
                }
                return;
            }
            Expr::Match(_, arms) => {
                if let Some(&m2) = self.rg.alias.get(&(e as *const Expr as usize)) {
                    return self.rg_ret(m2);
                }
                for a in arms {
                    self.rg_ret(&a.body);
                }
                return;
            }
            _ => {}
        }
        let Some(rt) = self.rg.ret.clone() else { return };
        let Some((et, tr)) = self.rt_expr(e) else { return };
        let (prog, t) = (self.prog, self.ret);
        let sp = super::borrowck::pos(e);
        if sp.line != 0 {
            self.rg.ret_sps.push(sp);
        }
        self.rg.flow(prog, et, t, &tr, &rt, sp);
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
        let rs = sig0.regions.clone();
        let codes = |i: usize| rs.as_deref().map(|r| r.slots.get(i).map(|c| c.as_slice()).unwrap_or(&[]));
        // (the prelude writes no lifetimes: what its results borrow is not known from elision)
        let prelude = prog.item_of.get(id as usize).copied().flatten().is_none_or(|i| i >= prog.prelude_from);
        let mut rg = std::mem::take(&mut self.rg);
        let lts: Vec<u32> = rs.as_deref().map(|r| r.lts.iter().map(|_| rg.var()).collect()).unwrap_or_default();
        let mut targs: Vec<Option<RT>> = sol.iter().map(|t| t.and_then(|t| rg.fresh(prog, t))).collect();
        // (type arguments written (`f::<&'a u8>()`): with the regions they name)
        if let Some(tf) = turbofish {
            let first = sig0.trait_of.is_some() as usize;
            for (i, a) in tf.iter().enumerate() {
                let (GArg::Ty(pt), Some(Some(t))) = (a, sol.get(first + i)) else { continue };
                if !crate::regions::names_lt(pt) {
                    continue;
                }
                let mut s = crate::regions::Slots { lts: rg.lt_names.clone(), tparams: vec![], self_slots: &[], any: false, in_fn: 0 };
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
        // (the regions a type argument brings are not the signature's own: elision does not see them)
        let substituted: Vec<u32> = targs.iter().flatten().flatten().copied().collect();
        let mut ptrees: Vec<Option<RT>> = vec![];
        let mut inputs = vec![];
        for (i, &pt) in sig0.params.iter().enumerate() {
            let tr = rg.instantiate(prog, pt, codes(i), &lts, &targs, &mut |me| me.var());
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
        self.rg = rg;
        for a in args {
            let t = self.rt_expr(a).map(|x| (Some(x.0), x.1));
            atrees.push((t, super::borrowck::pos(a)));
        }
        let mut rg = std::mem::take(&mut self.rg);
        for (i, ((at, asp), pt)) in atrees.iter().zip(&ptrees).enumerate() {
            if let (Some((aty, at)), Some(pt), Some(&ty)) = (at, pt, params.get(i)) {
                rg.flow(prog, aty.unwrap_or(ty), ty, at, pt, *asp);
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
        let through_trait = !rg.inst_args.is_empty() && (sig0.impl_trait.is_some() || sig0.trait_of.is_some());
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
        // (the result: its elided regions are of `&self`, else of the one region among the parameters)
        // (a receiver `&self` / `&mut self`: of the impl's own type; another receiver type leaves the result's
        // elided regions unknown)
        let self_ref = sig0.has_self && matches!((sig0.params.first(), sig0.impl_of), (Some(Ty::Ref(_, Ty::Struct(s))), Some(i)) if prog.base_of(*s) == prog.base_of(i));
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
        self.rg = rg;
    }

    /// The tree of a method call's receiver as passed: recv's place, through `derefs` references, then borrowed again
    /// when the method takes `&self` / `&mut self` (passed as recv_ty)
    pub(crate) fn rg_recv(&mut self, recv: &Expr, recv_ty: Ty, sid: u32) {
        self.rg.recv = None;
        if !self.rg_on() {
            return;
        }
        let Some((mut t, mut tr, mut ds)) = self.rt_place(recv).or_else(|| self.rt_expr(recv).map(|(t, tr)| (t, tr, vec![]))) else { return };
        loop {
            match t {
                Ty::Struct(s) if s == sid => break,
                Ty::Ref(_, i) => {
                    let Some(&r) = tr.first() else { return };
                    ds.push(r);
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
        let Some(tree) = self.rg.fresh(prog, t) else { return };
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

    /// A tuple struct's value of type t built by its constructor (`B(a)`): the arguments flow into its fields
    pub(crate) fn rg_ctor(&mut self, c: &Expr, t: Ty, args: &[(usize, &Expr)]) {
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
        let Ty::Struct(sid) = t else { return };
        let prog = self.prog;
        if prog.structs[sid as usize].is_enum {
            return;
        }
        let Some(tree) = self.rg.fresh(prog, t) else { return };
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
        // (a Box's content is borrowed for a `&self` method: of what the function owns, a loan)
        if let (Ty::Box(_), Some(Ty::Ref(..))) = (rty, self.prog.sigs[decl as usize].params.first()) {
            let r = self.rg.var();
            if let Some(n) = place_text(recv) {
                self.rg.loans.push((r, format!("*{n}"), super::borrowck::pos(recv)));
            }
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
        let Expr::Closure(_, inputs, ret_ann, _, _) = c else { return };
        let prog = self.prog;
        // (its captures' regions that are universal here: the function's own, related as it knows)
        let (caps, known, lts) = self.rg_capture_codes(cid);
        let any_cap = caps.iter().flatten().flatten().any(|&c| c >= CLO_U);
        // (nothing to check when no region is in its signature nor its captures are this function's)
        if !any_cap && params.iter().chain([&ret]).all(|&t| count(prog, t, 0) == Some(0)) {
            return;
        }
        if bound.is_none() && !any_cap && !inputs.iter().any(|x| x.1.is_some()) {
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
        self.insts.lock().unwrap().closure_rg.insert(cid, Box::new(crate::program::CloRg { codes, caps, known, lts }));
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
    pub(crate) fn rg_begin_closure(&mut self, params: &[(Loc, Ty)]) {
        let prog = self.prog;
        let Some(clo) = self.rg.closure.clone() else { return };
        let codes = &clo.codes;
        if codes.len() != params.len() + 1 {
            self.rg.on = false;
            return;
        }
        let mut rg = std::mem::take(&mut self.rg);
        let reg = |rg: &mut Rg, c: u8| -> u32 { rg.clo_reg(c) };
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
        let tr = match codes {
            Some(cs) if count(prog, t, 0) == Some(cs.len()) => Some(cs.iter().map(|&c| self.rg.clo_reg(c)).collect()),
            _ => self.rg.fresh(prog, t),
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
        self.rg.calls.insert(e as *const Expr as usize, (t, vec![r; n]));
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
            // (by the reference matched through, or a `ref` binding's own borrow: a region of its own)
            let r = match self.rg.pat_ref {
                Some(r) => r,
                None => self.rg.var(),
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
    pub(crate) fn rg_took_call(&mut self, e: &Expr) {
        // (only the call made by this expression's own call_fn: one level below)
        if let Some((d, t, tr)) = self.rg.last_call.take() {
            if d == self.rg.depth + 1 {
                self.rg.calls.insert(e as *const Expr as usize, (t, tr));
            }
        }
    }

    /// Report what the constraints require
    pub(crate) fn rg_check(&mut self) -> super::R<()> {
        if debug() {
            eprintln!("regionck {}: on={} univ={} edges={} loans={} missed={:?}", self.prog.names.get(self.rg.base as usize).map(|s| s.as_str()).unwrap_or("?"), self.rg.on, self.rg.univ.len(), self.rg.edges.len(), self.rg.loans.len(), self.rg.missed);
        }
        if !self.rg.on {
            return Ok(());
        }
        match self.rg.check() {
            Some((sp, m)) => Err(self.at(sp, m)),
            None => Ok(()),
        }
    }
}
