//! Tables for the whole program (fn signatures, struct / enum definitions and layouts, trait impls and so on).
//! Built from the item summaries (`Summary`). This part does not depend on syn (zustc can compile it);
//! the lookups that walk syn's syntax trees (`fn_by_path` and so on) are in another `impl Program` in front.rs.

// (Fx hashing throughout: deterministic iteration and instruction counts, unlike the std maps' random seeds)
type HashMap<K, V> = crate::fxhash::FxMap<K, V>;
type HashSet<K> = crate::fxhash::FxSet<K>;
use std::sync::Mutex;

use crate::fxhash::{FxMap, FxSet};
use crate::lexer::SrcItem;
use crate::types::*;
use crate::key::{Key, KeyHasher};
use crate::ast_summary::parse_fn_bound;

pub type R<T> = Result<T, String>;


/// `zustc --test`'s entry point (zust_syntax::test_harness): the program's `main` when it is there
pub const TEST_ENTRY: &str = "__zust_test_entry";

#[derive(Clone, Debug)]
pub struct FnSig {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// The symbol name in the object file
    pub symbol: String,
    pub is_extern: bool,
    pub is_main: bool,
    /// For a fn in an impl, the self type (struct id)
    pub impl_of: Option<u32>,
    pub has_self: bool,
    pub generics: Vec<(String, Vec<String>)>,
    pub trait_of: Option<String>,
    pub has_body: bool,
    pub impl_trait: Option<String>,
    /// The `Self` of a fn in an impl (may contain type parameters)
    pub self_ty: Option<Ty>,
    pub export: bool,
    pub panic_handler: bool,
    /// SigSyn::is_unsafe
    pub is_unsafe: bool,
    /// SigSyn::ret_from (which reference arguments the return value may borrow from)
    pub ret_from: u64,
    /// SigSyn::regions
    pub regions: Option<std::sync::Arc<crate::types::RSigSyn>>,
}

impl FnSig {
    /// Whether it needs monomorphization (it is not compiled directly)
    pub fn is_generic(&self) -> bool {
        !self.generics.is_empty()
    }
}

/// The environment for resolving type names in a fn body (`Self` and the type parameters)
#[derive(Clone, Debug, Default)]
pub struct TyEnv {
    pub self_sid: Option<u32>,
    pub params: Vec<(String, Ty)>,
    /// The type of `Self` (Struct(self_sid) for a struct, u32 and so on for a trait impl on a primitive)
    pub self_ty: Option<Ty>,
}

#[derive(Clone)]
pub struct VariantDef {
    pub name: String,
    pub kind: VariantKind,
    /// (name, type, offset from the start of the enum)
    pub fields: Vec<(String, Ty, u32)>,
    pub discr: i64,
}

/// The fields a module of the program may not read (E0616): the inline modules by their lines, and of each struct or
/// union of a module whose name is the program's only item of that name, its fields that are not `pub`
#[derive(Default, Debug, Clone)]
pub struct FieldPriv {
    /// (the line of a module's `{`, of its `}`, its parent); the crate root is module 0, of every line
    pub mods: Vec<(u32, u32, u32)>,
    /// the struct's or union's name -> (`struct` / `union`, [(a field, the module it is visible within)])
    pub fields: FxMap<String, (&'static str, Vec<(String, u32)>)>,
    /// E0624: a type's name -> [(a fn of its inherent impls that is not `pub`, the module it is visible within)]
    pub methods: FxMap<String, Vec<(String, u32)>>,
}

impl FieldPriv {
    /// the innermost module of a line (None on a line where a module starts or ends, which two modules share)
    pub fn module_of(&self, line: u32) -> Option<u32> {
        let mut best = 0u32;
        for (i, &(a, e, _)) in self.mods.iter().enumerate().skip(1) {
            if line == a || line == e {
                return None;
            }
            if a < line && line < e {
                best = i as u32;
            }
        }
        Some(best)
    }
    /// whether module `m` is `scope` or inside it
    pub fn within(&self, mut m: u32, scope: u32) -> bool {
        loop {
            if m == scope {
                return true;
            }
            if m == 0 || m as usize >= self.mods.len() {
                return false;
            }
            m = self.mods[m as usize].2;
        }
    }
}

/// Definitions of structs and enums (named aggregate types). Keeping enums in the same table lets them share
/// copying, argument passing, impls, deep hashing and so on with structs.
#[derive(Clone)]
pub struct StructDef {
    pub name: String,
    /// (name, type, offset)
    pub fields: Vec<(String, Ty, u32)>,
    pub size: u32,
    pub align: u32,
    /// A hash of the definition and of every struct definition its fields reach (changes when the layout changes)
    pub deep: Key,
    /// For an enum, its variants (empty for a struct)
    pub variants: Vec<VariantDef>,
    pub is_enum: bool,
    /// The type of the enum's tag (U8 or I32), placed at offset 0
    pub tag: Ty,
    /// For a generic definition (`Option<T>`), its type parameter names. Field types hold Param; no layout
    pub generics: Vec<String>,
    /// For an instance (`Option<i64>`), the base definition and the type arguments
    pub base: Option<u32>,
    pub args: Vec<Ty>,
    /// Whether the instance's type arguments still hold type parameters (so has_param need not walk them again:
    /// nested closure types share parts, so walking them again is exponential in the nesting depth)
    pub symbolic: bool,
    /// A placeholder for a type parameter of a generic fn `zustc check` instantiates though nothing calls it: the
    /// traits the parameter's bounds name (Program::placeholder)
    pub placeholder: Option<Vec<String>>,
    /// That placeholder's parameter is declared `?Sized` (Program::fn_unsized_gens): it may be unsized
    pub maybe_unsized: bool,
}

impl StructDef {
    fn blank(name: String, is_enum: bool) -> StructDef {
        StructDef {
            name,
            fields: vec![],
            size: 0,
            align: 1,
            deep: 0,
            variants: vec![],
            is_enum,
            tag: Ty::U8,
            generics: vec![],
            base: None,
            args: vec![],
            symbolic: false,
            placeholder: None,
            maybe_unsized: false,
        }
    }
}

/// The untyped locals of the blocks being evaluated (Program::eval_block_env): their expressions, and their values
/// at each type a use wanted (memoized, so a chain `let x1 = x0 & x0; let x2 = x1 & x1; ..` is linear)
type LazyLocals = (u64, FxMap<u64, (crate::parse::Expr, Vec<(Option<It>, Result<(i128, Option<It>), String>)>, Option<String>, bool)>);
// (the third: the error of its untyped value, reported if no use evaluates it: `let _x = 1 / 0;`; the fourth: whether
// the block refers to it, where it may not be evaluated (an `if` arm not taken))

thread_local! {
    static LAZY: std::cell::RefCell<LazyLocals> = std::cell::RefCell::new((0, FxMap::default()));
    /// The instances under construction (base type, type arguments) and the ids reserved for them midway
    static INSTANTIATING: std::cell::RefCell<Vec<((u32, Vec<Ty>), Option<u32>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The struct / enum table. Instances such as `Option<i64>` can be added later (also during parallel codegen).
/// Elements are boxed and never removed or moved, so a reference to an element can be returned from `&self`
pub struct AdtTable {
    v: Mutex<Vec<Box<StructDef>>>,
    inst: Mutex<HashMap<(u32, Vec<Ty>), u32>>,
    tuples: Mutex<HashMap<usize, u32>>,
    /// The placeholders made, by (fn, type parameter)
    placeholders: Mutex<HashMap<(u32, u32), u32>>,
    /// The placeholders of placeholders' associated types, by (placeholder, name), and what each is of: (placeholder,
    /// the bound whose trait declares it, name)
    assoc_placeholders: Mutex<(HashMap<(u32, String), u32>, HashMap<u32, (u32, String, String)>)>,
    /// Whether any was made (read without the lock, on hot paths)
    any_placeholder: std::sync::atomic::AtomicBool,
}

impl Clone for AdtTable {
    fn clone(&self) -> Self {
        AdtTable {
            v: Mutex::new(self.v.lock().unwrap().iter().map(|b| Box::new((**b).clone())).collect()),
            inst: Mutex::new(self.inst.lock().unwrap().clone()),
            tuples: Mutex::new(self.tuples.lock().unwrap().clone()),
            placeholders: Mutex::new(self.placeholders.lock().unwrap().clone()),
            assoc_placeholders: Mutex::new(self.assoc_placeholders.lock().unwrap().clone()),
            any_placeholder: std::sync::atomic::AtomicBool::new(self.any_placeholder.load(std::sync::atomic::Ordering::Relaxed)),
        }
    }
}

impl AdtTable {
    fn new() -> Self {
        AdtTable {
            v: Mutex::new(vec![]),
            inst: Mutex::new(HashMap::default()),
            tuples: Mutex::new(HashMap::default()),
            placeholders: Mutex::new(HashMap::default()),
            assoc_placeholders: Mutex::new((HashMap::default(), HashMap::default())),
            any_placeholder: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn len(&self) -> usize {
        self.v.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn push(&self, d: StructDef) -> u32 {
        let mut g = self.v.lock().unwrap();
        g.push(Box::new(d));
        g.len() as u32 - 1
    }
    fn get_mut(&mut self, i: usize) -> &mut StructDef {
        &mut self.v.get_mut().unwrap()[i & !(DST_TAG as usize)]
    }
    /// Fills in an instance whose id was reserved first (only for ids not yet handed out to anyone)
    fn replace(&self, i: u32, d: StructDef) {
        let mut g = self.v.lock().unwrap();
        *g[(i & !DST_TAG) as usize] = d;
    }
}

impl std::ops::Index<usize> for AdtTable {
    type Output = StructDef;
    fn index(&self, i: usize) -> &StructDef {
        let g = self.v.lock().unwrap();
        let p: *const StructDef = &*g[i & !(DST_TAG as usize)];
        drop(g);
        // SAFETY: the elements are boxed and never removed (a reallocation of the Vec moves only the Box pointers).
        // Only get_mut, which takes &mut self (during build), changes an element after it is published
        unsafe { &*p }
    }
}

/// The input to the layout computation
#[derive(Clone)]
enum AdtShape {
    Struct(Vec<(String, TySyn)>),
    Enum(Vec<VariantSyn>),
}

/// The definition of a const / static
impl ConstDef {
    /// The type holds a fn pointer somewhere (written out: `fn(` in the type's name is enough for statics)
    pub fn ty_has_fn(&self) -> bool {
        fn has(t: Ty, d: u32) -> bool {
            d < 8 && match t {
                Ty::Fn(_) => true,
                Ty::Array(e, _) | Ty::Ref(_, e) => has(*e, d + 1),
                _ => false,
            }
        }
        has(self.ty, 0) || matches!(self.ty, Ty::Struct(_))
    }
}

#[derive(Clone)]
pub struct ConstDef {
    pub name: String,
    pub ty: Ty,
    /// The source of the initializer expression (parsed again where it is used and at initialization)
    pub expr: String,
    /// The item that defines it (for the cache key)
    pub item: u32,
    /// For a static, Some(whether it is mut)
    pub stat: Option<bool>,
}

/// The impl block of an associated const of a group (Program::const_groups)
#[derive(Clone, Debug)]
pub struct ConstImpl {
    /// Its self type, with the impl's parameters as Param (None: it does not resolve)
    pub self_ty: Option<Ty>,
    /// Its trait's own name (None for an inherent impl) and the trait's arguments over the impl's parameters, when they
    /// resolve
    pub trait_name: Option<String>,
    pub trait_args: Option<Vec<Ty>>,
    /// The impl's parameters
    pub ngen: usize,
}

/// Associated consts such as `u32::MAX`
/// `const _: () = { .. };` (nameless: only a compile-time check, so skipped; also a flattened module's `m___`)
fn anon_const(n: &str) -> bool {
    n == "_" || n.ends_with("::_") || (n.len() > 3 && n.ends_with("___"))
}

/// A constant's declared type, when it is an integer type written as one or as an alias of one (`u8`, `type L = u16`)
fn early_int_ty(t: &TySyn, aliases: &FxMap<String, (Vec<String>, TySyn)>) -> Option<Ty> {
    let mut t = t.clone();
    for _ in 0..16 {
        t = match t {
            TySyn::Prim(p) if It::of_ty(p).is_some() => return Some(p),
            TySyn::Named(n) => match aliases.get(&n) {
                Some((g, a)) if g.is_empty() => a.clone(),
                _ => return None,
            },
            // (a generic alias whose body is a parameter or a type: `type Id<T> = T;` makes `Id<i8>` an i8)
            TySyn::App(n, args) => match aliases.get(&n) {
                Some((g, TySyn::Named(p))) if g.len() == args.len() => match g.iter().position(|x| x == p) {
                    Some(i) => args[i].clone(),
                    None => TySyn::Named(p.clone()),
                },
                Some((g, a)) if g.len() == args.len() => a.clone(),
                _ => return None,
            },
            _ => return None,
        }
    }
    None
}

/// The names of core::arch's vector type t (`Ty::Vec(128)`: `__m128i`, `__m128`, `__m128d`, `v128`), which an impl
/// may use for it
pub fn vector_names(t: Ty) -> &'static [&'static str] {
    match t {
        Ty::Vec(128) => &["__m128i", "__m128", "__m128d", "v128"],
        Ty::Vec(256) => &["__m256i", "__m256", "__m256d"],
        Ty::Vec(512) => &["__m512i", "__m512", "__m512d"],
        _ => &[],
    }
}

/// An integer type in constant evaluation: its width and sign (usize / isize are 64 bits; ptr tells them from u64 /
/// i64 only in their names, `usize::MAX`: they are one type otherwise)
#[derive(Clone, Copy, Debug)]
pub struct It {
    pub bits: u32,
    pub signed: bool,
    pub ptr: bool,
}

impl PartialEq for It {
    fn eq(&self, o: &It) -> bool {
        (self.bits, self.signed) == (o.bits, o.signed)
    }
}

impl Eq for It {}

impl std::hash::Hash for It {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        (self.bits, self.signed).hash(h);
    }
}

impl It {
    pub const U8: It = It { bits: 8, signed: false, ptr: false };
    pub const U32: It = It { bits: 32, signed: false, ptr: false };
    pub const I32: It = It { bits: 32, signed: true, ptr: false };
    pub fn of_name(n: &str) -> Option<It> {
        let (bits, signed) = match n {
            "u8" => (8, false),
            "u16" => (16, false),
            "u32" => (32, false),
            "u64" | "usize" => (64, false),
            "u128" => (128, false),
            "i8" => (8, true),
            "i16" => (16, true),
            "i32" => (32, true),
            "i64" | "isize" => (64, true),
            "i128" => (128, true),
            _ => return None,
        };
        Some(It { bits, signed, ptr: n == "usize" || n == "isize" })
    }
    pub fn of_ty(t: Ty) -> Option<It> {
        match t {
            Ty::U8 => It::of_name("u8"),
            Ty::U16 => It::of_name("u16"),
            Ty::U32 => It::of_name("u32"),
            Ty::U64 => It::of_name("u64"),
            Ty::Usize => It::of_name("usize"),
            Ty::U128 => It::of_name("u128"),
            Ty::I8 => It::of_name("i8"),
            Ty::I16 => It::of_name("i16"),
            Ty::I32 => It::of_name("i32"),
            Ty::I64 => It::of_name("i64"),
            Ty::Isize => It::of_name("isize"),
            Ty::I128 => It::of_name("i128"),
            _ => None,
        }
    }
    /// The type's name, as a literal's suffix
    pub fn name(self) -> &'static str {
        match (self.bits, self.signed) {
            (8, false) => "u8",
            (16, false) => "u16",
            (32, false) => "u32",
            (64, false) if self.ptr => "usize",
            (64, false) => "u64",
            (128, false) => "u128",
            (8, true) => "i8",
            (16, true) => "i16",
            (32, true) => "i32",
            (64, true) if self.ptr => "isize",
            (64, true) => "i64",
            _ => "i128",
        }
    }
    /// v as rustc shows a value of this type in E0080: `200_u8`, `-1_i32`, the type's MIN and MAX by name
    /// (`i8::MIN`, `u8::MAX`; an unsigned 0 is `0_u8`)
    fn show(self, v: i128) -> String {
        let n = self.name();
        let min = self.wrap(1i128 << (self.bits - 1));
        if self.signed && v == min {
            format!("{n}::MIN")
        } else if v == if self.signed { !min } else { self.wrap(-1) } {
            format!("{n}::MAX")
        } else if self.signed {
            format!("{v}_{n}")
        } else {
            // (a u128 past i128::MAX is held as its bit pattern)
            format!("{}_{n}", v as u128)
        }
    }
    /// v's low bits as this type (two's complement; a u128 as its bit pattern)
    pub fn wrap(self, v: i128) -> i128 {
        if self.bits == 128 {
            return v;
        }
        let m = (1u128 << self.bits) - 1;
        let u = (v as u128) & m;
        if self.signed && u >> (self.bits - 1) & 1 == 1 { (u | !m) as i128 } else { u as i128 }
    }
    /// v is in this type's range (a 128-bit type takes any held value)
    pub fn fits(self, v: i128) -> bool {
        self.bits == 128 || self.wrap(v) == v
    }
    /// v, which must be in this type's range ("overflow in constant", as rustc's error)
    pub fn check(self, v: Option<i128>) -> R<i128> {
        v.filter(|&v| self.fits(v)).ok_or_else(|| "overflow in constant".to_string())
    }
    /// rustc's E0080 for `l op r` of this type that overflows or divides by zero (`attempt to compute `u8::MAX +
    /// 1_u8`, which would overflow`; op "neg" is `-l`), at the operation's place in the text it was parsed from: its
    /// start and its operator's, `1:14:1:22: ..` (see unplaced; 1-based columns); r is of type rt (a shift's own, i32
    /// when unsuffixed)
    #[cold]
    #[inline(never)]
    fn arith_err(self, op: &str, l: i128, r: i128, rt: Option<It>, sp: crate::parse::Span) -> String {
        let m = match op {
            "neg" => format!("attempt to negate `{}`, which would overflow", self.show(l)),
            "/" if r == 0 => format!("attempt to divide `{}` by zero", self.show(l)),
            "%" if r == 0 => format!("attempt to calculate the remainder of `{}` with a divisor of zero", self.show(l)),
            "<<" | ">>" => format!("attempt to shift {} by `{}`, which would overflow", if op == "<<" { "left" } else { "right" }, rt.unwrap_or(It::I32).show(r)),
            _ => format!("attempt to compute `{} {op} {}`, which would overflow", self.show(l), self.show(r)),
        };
        let (p, a) = (sp.start, sp.at);
        if p.line == 0 { format!("{m} (E0080)") } else { format!("{}:{}:{}:{}: {m} (E0080)", p.line, p.col + 1, a.line, a.col + 1) }
    }
    /// rustc's E0080 of `v.pow(e)` past this type, at the call sp: the first multiplication that overflows in core's
    /// pow as a constant evaluates it (`acc * base` for each set bit of e, low to high, then `base * base` until the
    /// last one: `2u8.pow(8)` is `16_u8 * 16_u8`)
    #[cold]
    #[inline(never)]
    fn pow_err(self, v: i128, mut e: u32, sp: crate::parse::Span) -> String {
        let mul = |a: i128, b: i128| {
            if self.bits == 128 && !self.signed { (a as u128).checked_mul(b as u128).map(|x| x as i128) } else { a.checked_mul(b).filter(|&x| self.fits(x)) }
        };
        let (mut base, mut acc) = (v, 1);
        while e != 0 {
            if e & 1 == 1 {
                acc = match mul(acc, base) {
                    Some(x) => x,
                    None => return self.arith_err("*", acc, base, None, sp),
                };
                if e == 1 {
                    break;
                }
            }
            e /= 2;
            base = match mul(base, base) {
                Some(x) => x,
                None => return self.arith_err("*", base, base, None, sp),
            };
        }
        "overflow in constant".into()
    }
}

/// An error of constant evaluation that is rustc's at the constant (E0080): an overflow or a division by zero
pub fn const_eval_err(e: &str) -> bool {
    e.contains("overflow in constant") || e.ends_with("(E0080)")
}

/// An E0080 without the place of its operation (`1:14:1:22: attempt to ..`, It::arith_err), which is in the text the
/// expression was parsed from: for one parsed from other text than the caller's (another constant's initializer, a
/// const fn's body), and for the callers that place the error themselves
#[cold]
fn unplaced(e: String) -> String {
    match e.split_once(": ") {
        Some((p, m)) if e.ends_with("(E0080)") && p.split(':').count() == 4 && p.split(':').all(|x| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit())) => m.to_string(),
        _ => e,
    }
}

/// An E0080 that has no place, at p (start and operator both)
#[cold]
fn placed_at(e: String, p: crate::parse::Pos) -> String {
    if !e.ends_with("(E0080)") || p.line == 0 {
        return e;
    }
    format!("{}:{}:{}:{}: {e}", p.line, p.col + 1, p.line, p.col + 1)
}

pub fn prim_assoc_const(t: Ty, name: &str) -> Option<i128> {
    let bits = t.bits();
    Some(match (name, t.signed()) {
        ("BITS", _) => bits as i128,
        // (`char::REPLACEMENT_CHARACTER`, U+FFFD, and `core::char::REPLACEMENT_CHARACTER` (its deprecated module's))
        ("REPLACEMENT_CHARACTER", _) if t == Ty::Char => 0xFFFD,
        // (u128::MAX is held as its bit pattern: -1)
        ("MAX", true) => (u128::MAX >> (129 - bits)) as i128,
        ("MIN", true) => -((u128::MAX >> (129 - bits)) as i128) - 1,
        ("MAX", false) if t == Ty::Char => 0x10FFFF,
        ("MAX", false) => (u128::MAX >> (128 - bits)) as i128,
        ("MIN", false) => 0,
        _ => return None,
    })
}

/// A Mutex that clones its content (Program's caches)
#[derive(Default)]
pub struct CloneMutex<T>(pub Mutex<T>);
impl<T: Clone> Clone for CloneMutex<T> {
    fn clone(&self) -> Self {
        CloneMutex(Mutex::new(self.0.lock().unwrap().clone()))
    }
}
impl<T> std::ops::Deref for CloneMutex<T> {
    type Target = Mutex<T>;
    fn deref(&self) -> &Mutex<T> {
        &self.0
    }
}

#[derive(Clone)]
pub struct Program {
    /// All fns (user-defined + extern). The index is the u32 fn ID
    pub names: Vec<String>,
    pub index: FxMap<String, u32>,
    pub sigs: Vec<FnSig>,
    /// The number of the item that defines each user-defined fn (None for an extern)
    pub item_of: Vec<Option<u32>>,
    /// The associated types of trait impls by name (indices into assocs), made once for normalize: every name is
    /// looked for again by each instance's goals
    pub assoc_index: std::sync::OnceLock<FxMap<(String, u32), Vec<u32>>>,
    /// normalize's answers so far, by (type, trait, associated type)
    pub normalized: CloneMutex<FxMap<(Ty, String, String), Option<Ty>>>,
    /// A fn's where clauses on other than a parameter (SigSyn::preds), by fn id: goals of each instance (issue #2)
    pub fn_preds: FxMap<u32, Vec<(crate::types::TySyn, String, Vec<String>)>>,
    pub structs: AdtTable,
    pub struct_index: FxMap<String, u32>,
    /// Variant name → (enum, index): finds variants from `use E::*;` by short name (not when several share it)
    pub variant_names: FxMap<String, Option<(u32, usize)>>,
    /// The prelude enums' variants a variant of the program's own enum took the short name from, (name, (enum,
    /// index)): the session gives the name back to the prelude's where the program imports that and not its own
    pub variant_shadowed: Vec<(String, (u32, usize))>,
    /// The fns whose `impl Trait` return type their body determines
    pub opaque: Vec<u32>,
    /// The items from here on are the prelude's (no overflow checks)
    pub prelude_from: u32,
    /// Whether arithmetic is checked for overflow (when not -O)
    pub overflow_checks: bool,
    /// `#![deny(unreachable_patterns)]` (or forbid) in the source: an arm no value can reach is an error
    pub deny_unreachable: bool,
    /// structs / enums with `#[derive(PartialEq)]` (base ids): a constant of such a type (with fields of such types) is
    /// structural, usable as a pattern
    pub derived_peq: FxSet<u32>,
    /// the derived impls (base ids and traits): `#[derive(Clone)] struct S<T>` is Clone when each type argument is
    pub derived: FxMap<u32, Vec<String>>,
    /// a struct's / enum's const parameters of other types than usize (base ids): (the parameter's place, its type)
    pub const_param_tys: FxMap<u32, Vec<(usize, String)>>,
    /// The defaults of a struct's type parameters, by place (`struct W<T, F = fn() -> T>`): `W<u8>` is `W<u8, fn() -> u8>`
    pub struct_defaults: FxMap<u32, Vec<Option<TySyn>>>,
    /// A struct / enum / union of the program's: the places of its type parameters bounded by Copy (`union U<T: Copy>`)
    pub copy_params: FxMap<u32, Vec<usize>>,
    /// A struct's type parameters declared `?Sized` (base ids: the places), which its last field may make unsized
    pub unsized_params: FxMap<u32, Vec<usize>>,
    /// A fn's own type parameters, by their index in its generics: those that must be sized (a call's type for one is
    /// E0277 when unsized) and those declared `?Sized` (its placeholder may be unsized). The program's fns only
    pub fn_sized_gens: FxMap<u32, Vec<usize>>,
    pub fn_unsized_gens: FxMap<u32, Vec<usize>>,
    /// A struct's parameter names and its last field's type as written (base ids, the structs with `?Sized`
    /// parameters): whether an instance's tail is unsized, asked before the definition's fields are resolved
    /// (`struct Holder<T> { p: *const Tail<[u8]> }` laid out before `struct Tail<T: ?Sized>(T)`)
    pub tail_syn: FxMap<u32, (Vec<String>, TySyn)>,
    /// the program's source declares modules (`mod m { .. }`), whose scopes are flattened away: a name's item may then
    /// be out of scope where the name is written
    pub has_mods: bool,
    /// The program's inline modules and the fields of its structs and unions it may not see everywhere, for E0616
    /// (set by the driver where it reads the modules)
    pub field_priv: Option<Box<FieldPriv>>,
    /// some trait other than Clone has a fn `clone` (`trait Duplicate { fn clone(&self) -> S { .. } }`), or a blanket
    /// impl one: only then can default_method find a `clone` for a type
    pub trait_clone_fns: bool,
    /// method names that two traits of the program's declare (`trait A { fn foo(self); } trait B { fn foo(self); }`):
    /// only a call of one of these can be ambiguous (E0034)
    pub multi_trait_methods: FxSet<String>,
    /// of those, the types (a struct's base, a primitive) two of the traits that declare the name are implemented for
    pub ambiguous_methods: FxMap<String, Vec<Ty>>,
    /// the program's traits with a blanket impl without bounds (`impl<T> A for T {}`): every type implements them
    pub unbounded_blankets: FxSet<String>,
    /// the names the program's `use` declarations write (fn bodies' too): a value of one is imported, not undeclared,
    /// though zustc may not have it (`use std::convert::identity;`)
    pub use_names: FxSet<String>,
    /// the fmt traits the program's `use` declarations import (fn bodies' too; of the whole program, not each
    /// scope's): 1 Display, 2 Debug (`use std::fmt::Debug;`, `use core::fmt::{self, Display as _}`; a glob `fmt::*`
    /// both). Which one `x.fmt(f)` calls where x's type has both (#256)
    pub fmt_imports: u8,
    /// a constant with a lowercase name exists (`const or: usize`): an identifier pattern of any case may name one;
    /// without, only an uppercase one is looked up
    pub lower_consts: bool,
    /// `#![deny(unused_must_use)]`: a statement whose value is a `Result` is an error
    pub deny_must_use: bool,
    /// The edition (2021 unless given): from 2024, a block's tail's temporaries go away before its locals
    pub edition: u16,
    /// The source writes a lifetime in a path's generic arguments (regions::src_path_lts)
    pub path_lts: bool,
    /// `#![deny(unused_allocation)]`: `Box::new(x).m()` with m of `&self` on what the box holds is an error
    pub deny_unused_alloc: bool,
    /// `#![deny(unused_mut)]` (or `unused` / `warnings`), with no `allow` of it anywhere in the source: a `let mut`
    /// that nothing needs mutable is an error (the borrow check finds it)
    pub deny_unused_mut: bool,
    /// the source allows (or warns of) `overflowing_literals`, at the crate or an item: no "literal out of range"
    pub allow_overflowing_literals: bool,
    /// The names a `use` of the program's has a path through (`E` of `use E::*;` / `use m::E::{A, B};`): an enum of
    /// one of these names may have its variants in scope, so a binding of a variant's name may be the variant
    pub use_prefixes: Vec<String>,
    /// The crate enables `fn_traits` (`#![feature(fn_traits)]`): a call of a closure's `call` / `call_mut` /
    /// `call_once` is allowed
    pub fn_traits: bool,
    /// `#[allow(dangerous_implicit_autorefs)]` (or warn / expect) somewhere in the program: the deny lint is not an error
    pub allow_implicit_autorefs: bool,
    /// `#![deny(trivial_casts)]` / `#![deny(trivial_numeric_casts)]`: a cast a coercion or nothing would do
    pub deny_trivial_casts: (bool, bool),
    /// the program's `#[must_use]` fns (by name), for unused_must_use
    pub must_use_fns: Vec<String>,
    /// the names of the methods of numbers / bool / char that take `&mut self` (`impl Inc for i32 { fn inc(&mut
    /// self) }`): a local that calls one is kept in memory, and the method gets its address
    pub prim_mut_methods: HashSet<String>,
    /// -O (optimized build)
    pub opt: bool,
    /// The methods of blanket impls (name → fn id), and the type parameter name of their self type
    pub blanket: FxMap<String, u32>,
    /// A blanket impl's trait → the bounds of its self type's parameter, its where clauses' too
    /// (`impl<T: ?Sized> FutureExt for T where T: Future`: ?Sized, Future)
    pub blanket_bounds: FxMap<String, Vec<Vec<String>>>,
    pub blanket_selfs: HashSet<String>,
    /// Traits with a blanket impl (`impl<T> Tr for T`): every type implements them (bounds are not checked)
    pub blanket_traits: HashSet<String>,
    /// The structs that are unions
    pub unions: HashSet<String>,
    /// The tag type of enums with `#[repr(u8)]` and the like
    pub reprs: HashMap<String, Ty>,
    /// `#[repr(packed(N))]` structs / unions: their fields aligned to at most N
    pub packed: HashMap<String, u32>,
    /// `#[repr(align(N))]` types: aligned to at least N
    pub min_align: HashMap<String, u32>,
    /// `#[repr(C)]` types (an enum's `#[repr(C, u8)]` lays its fields out after the tag, as one union)
    pub repr_c: HashSet<String>,
    /// `const fn` bodies a constant may call: name -> (parameter names, body text)
    pub const_fns: HashMap<String, (Vec<String>, String)>,
    /// The parameter and result type names of those const fns (see Summary::const_fn_tys)
    pub const_fn_tys: HashMap<String, (Vec<String>, String)>,
    /// Same-named fns of separate trait impls for one type (`impl From<&str> for P`, `impl From<String> for P`):
    /// the first name → the candidates (later ones are named like `P::from#1`). The caller picks by argument types
    pub overloads: FxMap<String, Vec<u32>>,
    /// The overloads of impls for other self types, instances of one type (simdutf8's `impl SimdU8Value<__m256i> { fn
    /// from_32_cut_off_leading(..) }` beside `.. <__m128i>`, and its `impl From<__m256i> for SimdU8Value<__m256i>` beside
    /// `From<__m128i>`'s), by the first name: the self type a path names chooses (fn_for_self_ty), as no receiver does
    pub self_ty_groups: FxSet<String>,
    /// Of the overloads that are trait impls' fns, the trait's arguments of each one's impl, over the fn's generics
    /// (`impl ToInt<i16> for Z0 { fn to_int() }`: [i16]): a qualified path's `<Z0 as ToInt<i16>>::to_int` chooses by them
    pub fn_targs: FxMap<u32, Vec<Ty>>,
    /// zust_syntax::lexer::trait_scopes of the program's own source: where two scopes define traits of one name, the
    /// traits in scope on each line (pick_overload chooses the impl of the one in scope at the call)
    pub trait_scopes: Vec<(u32, u32, Vec<String>)>,
    /// The source line (1-based) of each item, for trait_scopes: the diagnostic line base of a function is not it (a
    /// dependency's is tagged, and it is the line before). Empty when trait_scopes is
    pub item_lines: Vec<u32>,
    /// The last segment of a name → its group of fns (ref_groups: `len` → `A::len`, `B::len`). For over-approximating
    /// references
    pub by_last: FxMap<String, u32>,
    /// The groups of fns of one last segment (by_last), in the order the names were first met: the nodes of the
    /// reference graph between fns. A fn refers to a name, not to each fn of that name: `x.area()` to `area`, not to
    /// its 2400 definitions in a program that defines it 2400 times, so the graph, and the cache keys over it, grow
    /// with the calls plus the definitions, not with their product (#188). The same over-approximation: a change to
    /// any `area` still reaches every caller of `area`. A fn is in its name's group, and in a second one for a
    /// `__core_` name (`swap` too). Each group's fns ascend; an edit that adds a name keeps the others' order
    pub ref_groups: Vec<Vec<u32>>,
    /// Trait name → the fns declared in that trait
    pub traits: FxMap<String, Vec<u32>>,
    /// impls whose self type is a reference to a concrete type (`impl Tr for &i32` / `for &S`): (that type, trait)
    pub ref_impls: HashSet<(Ty, String)>,
    /// generic impls for references: (self type with the impl's parameters, trait, number of parameters, each
    /// parameter's bounds)
    pub ref_impl_pats: Vec<(Ty, String, usize, Vec<Vec<String>>)>,
    /// The impls for references to a type parameter, `impl<T: Tr> Tr for &T` / `for &mut T`: (trait, whether `&mut`).
    /// A method call whose Self is such a reference takes the impl's method (or the trait's default with that Self),
    /// not T's through the deref (Cx::ref_param_method_call, #144)
    pub ref_param_impls: Vec<(String, bool)>,
    /// Their parameter's bounds, by (trait, whether `&mut`): `impl<T: Tr + Marker> Tr for &T` is for `&S` only where S
    /// is Marker too
    pub ref_param_bounds: FxMap<(String, bool), Vec<String>>,
    /// The program's generic impls for trait objects, `impl<R> WasmClosure for dyn Fn() -> R + '_`: (trait, the
    /// object's trait's name, `Fn`), which a `dyn Fn(..)` then implements
    pub dyn_impls: FxSet<(String, String)>,
    /// And the ones with no parameters, `impl Marker for dyn Obj<u8>`: (trait, the object's first trait as written,
    /// without spaces), which only that object implements (not `dyn Obj<u16>`)
    pub dyn_impls_exact: FxSet<(String, String)>,
    /// Of both, the ones whose object writes a lifetime (`impl Tr for dyn Obj + '_`), by the trait and the object's
    /// trait's name: the others are for `dyn Obj + 'static` only
    pub dyn_impls_lt: FxSet<(String, String)>,
    /// traits implemented for fn pointers (`impl Tr for fn(A) -> B`; generic ones count for every fn pointer)
    pub fnptr_impls: HashSet<(Option<Ty>, String)>,
    /// Supertraits (`trait Bar: Foo`)
    pub trait_supers: FxMap<String, Vec<String>>,
    /// trait → its supertraits Copy / Clone (`trait Num: Copy`), which trait_supers leaves out
    pub trait_marker_supers: FxMap<String, Vec<String>>,
    /// What a trait's `Self` must outlive (`trait Tr<'a>: 'a`): "static", or the index of the trait's lifetime parameter
    pub trait_self_outlives: FxMap<String, Vec<String>>,
    /// The program (not the prelude) has a static not declared `mut`: an assignment is checked against it
    pub any_immutable_static: bool,
    /// An alias of the program's names a lifetime that is not its own (`type S = &'static u8;`): see
    /// Summary::alias_fixed_lts
    pub alias_fixed_lt: bool,
    /// the generics a method of the trait starts with: Self and the trait's own parameters
    pub trait_ngen: FxMap<String, usize>,
    /// a generic trait's impls: trait → (self type, the trait's arguments, both with the impl's parameters as
    /// `Param(i)`, and their number) (`impl Getter<isize> for isize`: a default method's T for an isize receiver)
    pub impl_targs: FxMap<String, Vec<(Ty, Vec<Ty>, usize)>>,
    /// Of those, the (trait, self type) whose header's trait path writes a lifetime (SigSyn::impl_trait_lts)
    pub impl_targs_lts: HashSet<(String, Ty)>,
    /// The bounds of each impl_targs entry's generic parameters, in the same order (`impl<T: Marker> From<W<T>> for
    /// J`: `[["Marker"]]`)
    pub impl_targ_bounds: FxMap<String, Vec<Vec<Vec<String>>>>,
    /// The impls, as (type, trait)
    pub impls: HashSet<(u32, String)>,
    /// the self types of the impls of a trait for a struct's instances (`impl A for W<u8>`), by (struct, trait), with
    /// the impl's parameters as `Param(i)` and their number
    pub impl_selfs: FxMap<(u32, String), Vec<(Ty, usize, Vec<(usize, String)>)>>,
    /// the traits the program declares (not the prelude's): a bound of one is checked by its impls
    pub user_traits: FxSet<String>,
    /// The program's impls of its own traits as clauses, for the trait solver (crate::traits)
    pub tdb: crate::traits::TraitDb,
    /// Every trait the program and the prelude declare, known before the structs' fields are resolved (bare_dyn)
    pub trait_decls: FxSet<String>,
    /// The `bare_trait_objects` lint is denied: a trait object without `dyn` (bare_dyn) is an error
    pub deny_bare_trait: bool,
    /// Types whose `Copy` impl names 'static in its self type (`impl Copy for Foo<'static>`): a copy of one requires
    /// what it borrows to be 'static
    pub static_copy: FxSet<u32>,
    /// Associated consts of impls whose self type names 'static (`impl A<'static> { const IC: i32 = 10; }`), with
    /// the region slots of that self type: `A::<'a>::IC` requires 'a to be 'static
    pub static_self_consts: FxMap<u32, Vec<u8>>,
    /// Summary::const_regions, by constant: the codes of its type against its impl's lifetimes, and the impl's self
    /// type's slots (which say which of them a use's path writes)
    pub const_regions: FxMap<u32, (Vec<u8>, Vec<u8>)>,
    /// Summary::assoc_regions, by associated type (its index in assocs): the codes of its value against its impl's
    /// lifetimes, the impl's self type's codes, the `'d: 'a` its header declares of them, and the codes of the
    /// lifetimes its trait path writes (RImplSyn::trait_lts)
    pub assoc_regions: FxMap<u32, AssocRegions>,
    /// The item (an impl's member) each associated type is defined in, by its index in assocs: a function's cache
    /// key holds the definitions of the program's own, whose values (and their lifetimes) the function's types and
    /// region check read
    pub assoc_items: Vec<u32>,
    /// The declared type (as written) of a constant whose type is not known yet during layout (`[u8; 1]`, `(u8,)`):
    /// what an element or field of it is before its type is resolved
    pub early_syn: FxMap<u32, crate::types::TySyn>,
    /// The `#[panic_handler]` fn (no_std). Panics (out-of-bounds access, unwrap and so on) call it
    pub panic_handler: Option<u32>,
    /// Consts and statics
    pub consts: Vec<ConstDef>,
    pub const_index: FxMap<String, u32>,
    /// Associated consts (`S::N`) by their last name (`N`) (for over-approximating references)
    pub const_by_last: FxMap<String, Vec<u32>>,
    /// Associated consts of one name in impl blocks rustc tells apart, which a name alone does not: impls of one trait
    /// with other arguments (`impl ToInt<i8> for Z0`, `impl ToInt<i16> for Z0`) or self types (`impl Tr for S`, `impl
    /// Tr for &S`), inherent impls for disjoint instances (`impl W<u8>`, `impl W<u16>`). Each const of such a group →
    /// the group; the first keeps its name, a later one is `S::C#k`, and a use chooses among them (pick_const)
    pub const_group: FxMap<u32, u32>,
    pub const_groups: Vec<Vec<u32>>,
    /// The impls of those consts, by const: see ConstImpl
    pub const_impls: FxMap<u32, ConstImpl>,
    /// Trait impls for primitive types (type, trait)
    pub prim_impls: HashSet<(Ty, String)>,
    /// prim_impls that only an impl for a reference to the type makes (`impl Tr for &mut i32`): the type itself does not
    /// implement the trait (implements_std)
    pub prim_ref_only: HashSet<(Ty, String)>,
    /// The traits implemented for `&[T]` (`impl BufRead for &[u8]`)
    pub ref_slice_traits: HashSet<String>,
    /// Closure fn id → definition
    pub closures: FxMap<u32, ClosureDef>,
    /// (parent fn id, preorder index) → closure fn id
    pub closure_by: FxMap<(u32, u32), u32>,
    /// A closure's (generic) struct id → fn id
    pub closure_of_struct: FxMap<u32, u32>,
    /// hash_ty's memo per instance (nested closure types share parts, so walking them again is exponential)
    pub inst_keys: CloneMutex<FxMap<u32, Key>>,
    /// The structs / enums with an `impl Drop for S` (the base definition when generic)
    pub drop_impls: crate::fxhash::FxSet<u32>,
    /// The structs (base ids) whose `deref` returns a reference to an array, slice or str: `t.len()` on one of them is
    /// that target's method
    pub deref_to_seq: crate::fxhash::FxSet<u32>,
    /// The program's structs (base ids) with a `deref_mut`: a method call's receiver of another one reaches no
    /// program's `DerefMut`
    pub deref_mut_of: crate::fxhash::FxSet<u32>,
    /// Whether any type may need drop (whether the prelude has drop glue: false with neither Box nor Drop)
    pub has_drop: bool,
    /// The definitions of impls' associated types
    pub assocs: Vec<AssocDef>,
    /// (trait, associated type) -> the bounds of its declaration (`type Y: Clone;`)
    pub assoc_bounds: HashMap<(String, String), Vec<String>>,
    /// generic fns returning `impl Trait`: (parameters, body, has self), to type each instance's body
    pub opaque_generic: HashMap<u32, std::sync::Arc<(Vec<(crate::parse::Pat, crate::parse::Ty)>, crate::parse::Block, bool)>>,
    /// Comparison impls' eq for a right-hand type (`Vec::<PartialEq<[T;N]>>::eq`): tried when `a == b`'s types differ
    pub peq: Vec<u32>,
    /// Type aliases: name → (type parameters, body)
    pub aliases: FxMap<String, (Vec<String>, TySyn)>,
    /// The defaults of an alias's type parameters, by place (`type Result<T, E = Error>`: [None, Some(Error)])
    pub alias_defaults: FxMap<String, Vec<Option<TySyn>>>,
    /// Likewise `S::<PartialOrd<R>>::partial_cmp`, looked up when the types of `a < b` differ
    pub pord: Vec<u32>,
    /// The number of the item that writes each struct / enum definition (the low ids)
    pub struct_item: Vec<u32>,
    /// The regions of the struct / enum definitions that name lifetimes (by definition id): region checking
    pub adt_regions: FxMap<u32, std::sync::Arc<RAdtSyn>>,
}

/// Program::assoc_regions' entry: (value codes, impl self type codes, header outlives, trait path lifetime codes)
pub type AssocRegions = (Vec<u8>, Vec<u8>, Vec<(u8, u8)>, Vec<u8>);

/// `impl<..> Trait for S<..> { type Name = Value; }`
#[derive(Clone)]
pub struct AssocDef {
    pub name: String,
    /// The impl's self type and value (with the impl's type parameters as Param)
    pub self_ty: Ty,
    pub value: Ty,
    /// the impl's parameters
    pub ngen: usize,
    /// a generic associated type's own parameters (`type Pointer<T>`: 1), Params after the impl's and a mark
    pub ngat: usize,
    /// the impl's trait (its own name) and its type arguments over the impl's parameters (`impl Tr<u8> for S`), when
    /// they resolve: a qualified path with the trait's arguments chooses the impl by them
    pub trait_name: String,
    pub trait_args: Option<Vec<Ty>>,
}

/// t with `Self::X` (the impl's self type's X) replaced by the impl's own definition of X (own), recursively
fn own_assoc_syn(t: &TySyn, self_ty: &TySyn, own: &[(String, Vec<String>, TySyn)], depth: u32) -> TySyn {
    if depth > 8 {
        return t.clone();
    }
    let r = |x: &TySyn| own_assoc_syn(x, self_ty, own, depth + 1);
    match t {
        TySyn::Assoc(b, n) if **b == *self_ty => match own.iter().find(|(m, g, _)| m == n && g.is_empty()) {
            Some((_, _, v)) => own_assoc_syn(v, self_ty, own, depth + 1),
            None => t.clone(),
        },
        // (`Self::Output<X>`: the generic associated type `#gat:Output` of (Self, X), its parameters given)
        TySyn::Assoc(b, n) if n.starts_with(crate::ast_types::GAT_PREFIX) && matches!(&**b, TySyn::Tuple(p) if p.first() == Some(self_ty)) => {
            let TySyn::Tuple(parts) = &**b else { unreachable!() };
            let name = &n[crate::ast_types::GAT_PREFIX.len()..];
            match own.iter().find(|(m, g, _)| m == name && g.len() == parts.len() - 1 && !g.is_empty()) {
                Some((_, g, v)) => {
                    let args: Vec<TySyn> = parts[1..].iter().map(r).collect();
                    let v = subst_names_syn(v, g, &args, 0);
                    own_assoc_syn(&v, self_ty, own, depth + 1)
                }
                None => t.clone(),
            }
        }
        TySyn::Ref(m, i) => TySyn::Ref(*m, Box::new(r(i))),
        TySyn::Ptr(m, i) => TySyn::Ptr(*m, Box::new(r(i))),
        TySyn::Slice(i) => TySyn::Slice(Box::new(r(i))),
        TySyn::Array(i, n) => TySyn::Array(Box::new(r(i)), n.clone()),
        TySyn::App(n, v) => TySyn::App(n.clone(), v.iter().map(r).collect()),
        TySyn::Tuple(v) => TySyn::Tuple(v.iter().map(r).collect()),
        TySyn::FnPtr(v) => TySyn::FnPtr(v.iter().map(r).collect()),
        TySyn::Assoc(b, n) => TySyn::Assoc(Box::new(r(b)), n.clone()),
        t => t.clone(),
    }
}

/// t with the names ns (type parameters) replaced by args
fn subst_names_syn(t: &TySyn, ns: &[String], args: &[TySyn], depth: u32) -> TySyn {
    if depth > 16 {
        return t.clone();
    }
    let r = |x: &TySyn| subst_names_syn(x, ns, args, depth + 1);
    match t {
        TySyn::Named(n) => match ns.iter().position(|x| x == n) {
            Some(k) => args.get(k).cloned().unwrap_or_else(|| t.clone()),
            None => t.clone(),
        },
        TySyn::Ref(m, i) => TySyn::Ref(*m, Box::new(r(i))),
        TySyn::Ptr(m, i) => TySyn::Ptr(*m, Box::new(r(i))),
        TySyn::Slice(i) => TySyn::Slice(Box::new(r(i))),
        TySyn::Array(i, n) => TySyn::Array(Box::new(r(i)), n.clone()),
        TySyn::App(n, v) => TySyn::App(n.clone(), v.iter().map(r).collect()),
        TySyn::Tuple(v) => TySyn::Tuple(v.iter().map(r).collect()),
        TySyn::FnPtr(v) => TySyn::FnPtr(v.iter().map(r).collect()),
        TySyn::Assoc(b, n) => TySyn::Assoc(Box::new(r(b)), n.clone()),
        t => t.clone(),
    }
}

/// The trait of the impl whose member item ii is (its own name) and the trait's type arguments resolved over the impl's
/// parameters gn (None when one does not resolve)
fn impl_trait_of(p: &Program, items: &[SrcItem], sums: &[Summary], ii: usize, gn: &[&str]) -> (String, Option<Vec<Ty>>) {
    let h = match items.get(ii).map(|it| it.kind) {
        Some(crate::lexer::ItemKind::ImplMember(h)) => h as usize,
        _ => ii,
    };
    let Some(sm) = sums.get(h) else { return (String::new(), None) };
    let tr = sm.impl_trait.as_deref().unwrap_or("");
    let tr = tr.split('<').next().unwrap_or(tr).trim();
    let tr = tr.rsplit("::").next().unwrap_or(tr).to_string();
    let args: Option<Vec<Ty>> = sm.impl_trait_args.iter().map(|a| p.resolve_g(a, gn).ok()).collect();
    (tr, args)
}

/// (the impl's parameter count, the generic associated type's) of an associated type's parameter names: those after
/// the mark `#gat` are its own
fn gat_split(gn: &[&str]) -> (usize, usize) {
    match gn.iter().position(|g| *g == "#gat") {
        Some(k) => (k, gn.len() - k - 1),
        None => (gn.len(), 0),
    }
}

/// The mark of a `_` left to type inference (a value never used as a type parameter index)
pub const HOLE: u8 = 250;
/// the key mark of a closure's fn-pointer instance in the instance table (not a type parameter)
pub const FNPTR_MARK: u8 = 251;

/// A lifted closure ([`crate::closure`])
#[derive(Clone)]
pub struct ClosureDef {
    /// The id of the parent (the fn the closure is written in), and the closure's preorder index in its body
    pub parent: u32,
    pub k: u32,
    /// The generic struct
    pub sid: u32,
    /// The names of the captures (in the struct's field order)
    pub captures: Vec<String>,
    pub by_move: bool,
    /// For each capture, whether it is held by value (the rest are held by reference)
    pub moved: Vec<bool>,
    /// For each capture, whether the body assigns to it or borrows it mutably (ClosureSyn::mutated)
    pub mutated: Vec<bool>,
    /// The number of the parent's type parameters (they lead the type arguments)
    pub n_parent: usize,
    /// The annotated return type (may contain the parent's type parameters as Param)
    pub ret_ann: Option<Ty>,
    /// ClosureSyn::fake_reads
    pub fake_reads: Vec<String>,
}

/// A monomorphized instance (the generic fn base with the type arguments args)
pub struct Inst {
    pub base: u32,
    pub args: Vec<Ty>,
    pub sig: FnSig,
    /// a closure with no captures called through a fn pointer: no environment parameter (`let f: fn(i32) -> i32 = |x| x;`)
    pub fnptr: bool,
}

/// The instance table. Fn IDs continue after the Program's fns.
/// What codegen requests (in parallel) is pushed on pending, and the driver processes it to a fixpoint
pub struct Insts {
    pub n_fns: u32,
    pub list: Vec<Inst>,
    map: HashMap<(u32, Vec<Ty>), u32>,
    pub pending: Vec<u32>,
    /// A closure's region signature, by its fn id, from the bound it is passed to or its annotations, and the regions
    /// of the function it is in that its captures hold (typeck's regionck)
    pub closure_rg: HashMap<u32, Box<CloRg>>,
    /// The region check's implied relations of struct types, by position (typeck's Rg::imp_cache): kept between
    /// the functions of a build
    pub imp_cache: HashMap<(u32, u32, bool), Option<Vec<(u32, u32)>>>,
    /// zustc check's last round: the instances with placeholders for type parameters (Program::placeholder), and
    /// what they request. A typeck error of one is not reported (acg::compile)
    pub lenient: bool,
    /// Of those, the instances of fns that instances were made of with `Copy` types only (session's
    /// placeholder_wave): checked for what moving a T does, nothing else
    pub recheck: FxSet<u32>,
    /// Of those, the instances of fns that instances were made of whose bodies name a struct or an enum of the
    /// program's that bounds a type parameter by 'static (session's placeholder_wave): checked for the T: 'static
    /// that its well-formedness needs (E0310), nothing else (an instance's T may be a type with a region the check
    /// takes as fresh)
    pub recheck_static: FxSet<u32>,
    /// Of those, the instances of fns that a where clause bounds (the fn's, its impl's): a parameter's bounds there
    /// may not be its own (`where &'a T: Trait`)
    pub where_bound: FxSet<u32>,
    /// Closures' bodies (by their fn id) whose kind no bound nor expected type decides, made in the program's code: one that
    /// returns a mutable borrow of what it captures by reference is `FnMut`, and the borrow escapes its body
    pub kind_free: FxSet<u32>,
    /// The closures' bodies kind_free or closure_rg was given a fact for, in order: what a function's lowering told
    /// the closures it makes, kept with its Blob (acg::Blob::clo_facts), as one taken from a cache, a check's record
    /// or another process is not lowered here
    pub fx_log: Vec<u32>,
}

/// What a function's lowering told a closure's body it makes (Insts::kind_free, Insts::closure_rg)
#[derive(Clone, Debug)]
pub struct CloFact {
    pub body: u32,
    pub kind_free: bool,
    pub rg: Option<Box<CloRg>>,
}

/// Insts::closure_rg (codes: see typeck's regionck)
#[derive(Clone, Debug, Default)]
pub struct CloRg {
    /// The parameters' codes, then the result's
    pub codes: Vec<Vec<u8>>,
    /// Each capture's codes (None: not followed)
    pub caps: Vec<Option<Vec<u8>>>,
    /// What the function it is in knows of those of its regions: (longer, shorter)
    pub known: Vec<(u8, u8)>,
    /// The function's named lifetimes (a `let` annotation in the closure names them)
    pub lts: Vec<(String, u8)>,
}

/// How sure the answer to a trait goal is (Program::evaluate; rustc's next solver's `Certainty`, issue #2)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Certainty {
    Yes,
    /// Neither proved nor disproved
    Maybe(MaybeCause),
    No,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MaybeCause {
    /// The goal was met again while it was being proved: `impl<T: Q> P for T` with `impl<T: P> Q for T`. rustc
    /// reports E0275 for an obligation that needs it (a program trait's goals are inductive)
    Cycle,
    /// More than 64 goals deep
    Overflow,
}

impl Certainty {
    /// Either holds: `Yes` over `Maybe` over `No`
    pub fn or(self, o: Certainty) -> Certainty {
        match (self, o) {
            (Certainty::Yes, _) | (_, Certainty::Yes) => Certainty::Yes,
            (Certainty::Maybe(c), _) | (_, Certainty::Maybe(c)) => Certainty::Maybe(c),
            _ => Certainty::No,
        }
    }

    /// Both hold: `No` over `Maybe` over `Yes`
    pub fn and(self, o: Certainty) -> Certainty {
        match (self, o) {
            (Certainty::No, _) | (_, Certainty::No) => Certainty::No,
            (Certainty::Maybe(c), _) | (_, Certainty::Maybe(c)) => Certainty::Maybe(c),
            _ => Certainty::Yes,
        }
    }

    /// Some one holds (stops at the first `Yes`)
    pub fn any(it: impl Iterator<Item = Certainty>) -> Certainty {
        let mut r = Certainty::No;
        for c in it {
            r = r.or(c);
            if r == Certainty::Yes {
                break;
            }
        }
        r
    }

    /// All hold (stops at the first `No`)
    pub fn all(it: impl Iterator<Item = Certainty>) -> Certainty {
        let mut r = Certainty::Yes;
        for c in it {
            r = r.and(c);
            if r == Certainty::No {
                break;
            }
        }
        r
    }
}

thread_local! {
    /// How sure implements_concrete's last struct or tuple impl answer was (Program::impl_applies_noted)
    static NOTED: std::cell::Cell<Option<Certainty>> = const { std::cell::Cell::new(None) };
}

impl Insts {
    pub fn new(prog: &Program) -> Insts {
        Insts {
            n_fns: prog.names.len() as u32,
            list: vec![],
            map: HashMap::default(),
            pending: vec![],
            closure_rg: HashMap::default(),
            imp_cache: HashMap::default(),
            lenient: false,
            recheck: FxSet::default(),
            recheck_static: FxSet::default(),
            where_bound: FxSet::default(),
            kind_free: FxSet::default(),
            fx_log: vec![],
        }
    }

    pub fn get(&self, id: u32) -> Option<&Inst> {
        id.checked_sub(self.n_fns)
            .and_then(|i| self.list.get(i as usize))
    }

    /// Requests an instance (makes it and pushes it on pending if absent). Trait bounds are checked here too
    pub fn request(&mut self, prog: &Program, base: u32, args: Vec<Ty>) -> R<u32> {
        if let Some(&id) = self.map.get(&(base, args.clone())) {
            return Ok(id);
        }
        let b = &prog.sigs[base as usize];
        if b.generics.len() != args.len() {
            return Err(format!(
                "`{}` takes {} type arguments",
                prog.names[base as usize],
                b.generics.len()
            ));
        }
        let gn: Vec<&str> = b.generics.iter().map(|g| g.0.as_str()).collect();
        for ((pn, bounds), &t) in b.generics.iter().zip(&args) {
            for tr in bounds {
                // (a trait's default method for an impl for a reference to a primitive (`impl Third for &&u32`) is an
                // instance whose Self is the primitive: the impl is for it there (prim_ref_only))
                if pn == "Self" && prog.prim_ref_only.contains(&(t, tr.clone())) {
                    continue;
                }
                // (the bound needs a goal that needs itself: inductive, so it does not hold. Overflow stays
                // accepted: the limit is not rustc's)
                if !prog.has_param(t) && prog.evaluate(t, tr) == Certainty::Maybe(MaybeCause::Cycle) {
                    return Err(format!("overflow evaluating the requirement `{}: {tr}` (E0275)", prog.tn(t)));
                }
                // (zustc check's last round: a type with a placeholder in it (`&G#`) may meet the bound by a where clause
                // of the generic fn the request comes from (`&'max G: Get`), which is not known here)
                if self.lenient && prog.holds_placeholder(t, 0) && !prog.is_placeholder_ty(t) {
                    continue;
                }
                // (a trait of the program's: by the trait solver, so an impl's where clauses count (`impl<T: Bar> Foo
                // for T` is no `(): Foo` without `(): Bar`), and a goal that needs itself fails (E0275))
                let own = {
                    let n = tr.split('<').next().unwrap_or(tr).trim();
                    prog.tdb.user.contains(n.rsplit("::").next().unwrap_or(n))
                };
                // (the solver's `No` holds whatever the caller's type parameters are when t names no placeholder: it
                // is reported from a placeholder instance too, marked so)
                // (std's trait of a type parameter's placeholder that none of its bounds gives (`T: Copy` of `fn
                // g<T>`): it holds whatever the caller's parameters are, so it is reported from a placeholder instance)
                if prog.tdb.placeholder_lacks(prog, t, tr.trim()) {
                    return Err(format!("{} {}", crate::traits::not_held(&crate::traits::display_ty(prog, t), tr.trim()), crate::traits::SOLVED_NO));
                }
                // (an auto trait of a type parameter's placeholder: only by its bounds (Coinductive.lean's csolve, with
                // the bounds as the environment); it holds whatever the caller's parameters are, so it is reported from a
                // placeholder instance too)
                if matches!(tr.as_str(), "Send" | "Sync") && matches!(t, Ty::Struct(s) if prog.structs[s as usize].placeholder.is_some()) && prog.tdb.holds(prog, t, tr) == crate::traits::Ans::No {
                    return Err(format!("{} {}", crate::traits::not_held(&crate::traits::display_ty(prog, t), tr), crate::traits::SOLVED_NO));
                }
                // (a binding that normalizes to another type: E0271, `<W<i32> as Tr>::Assoc` is u32, not ())
                if own && tr.contains('=') && !prog.has_param(t) && !prog.names_placeholder(t, 0) {
                    if let Some((proj, got, want)) = prog.tdb.binding_mismatch(prog, t, tr, &gn, &args) {
                        if !prog.names_placeholder(want, 0) {
                            return Err(format!("type mismatch resolving `{proj} == {}`: it is `{}` {}", prog.tn(want), prog.tn(got), crate::traits::SOLVED_NO.replace("E0277", "E0271")));
                        }
                    }
                }
                // (the solver's answer, asked once: the check below asks the same goal again)
                let mut held: Option<crate::traits::Ans> = None;
                if own && !prog.has_param(t) && !prog.names_placeholder(t, 0) && !args.iter().any(|&a| prog.names_placeholder(a, 0) && tr.contains('<')) {
                    let h = prog.tdb.holds_in(prog, t, tr, &gn, &args);
                    held = Some(h);
                    match h {
                        crate::traits::Ans::No => {
                            return Err(format!("the trait bound `{}: {}` is not satisfied (for `{pn}`) {}", crate::traits::display_ty(prog, t), crate::traits::display_name(&crate::traits::bound_text(tr)), crate::traits::SOLVED_NO));
                        }
                        // (the solver ran out of fuel (a long chain of blanket impls): `evaluate`'s no, which proves
                        // each goal once, decides, for a bound without arguments (it does not read lifetimes:
                        // `for<'a> Yokeable<'a>` would be a false no))
                        crate::traits::Ans::Overflow if !tr.contains('<') && prog.evaluate(t, tr) == Certainty::No => {
                            return Err(format!("the trait bound `{}: {}` is not satisfied (for `{pn}`)", prog.tn(t), crate::traits::bound_text(tr)));
                        }
                        _ => {}
                    }
                }
                // (a trait of the program's: only the trait solver's No (its impls, a placeholder's bounds through their
                // supertraits). std's: `implements`' no, asked of the bound with the instance's types for the callee's
                // parameters (`AsRef<T>` with T = str is `AsRef<str>`) and without lifetimes (`Into<&'a str>`), but where
                // it is known to miss impls: std's blanket impls it does not model (`T: Borrow<T>`, `T: Any`), a Fn trait
                // of a reference to a closure or a type parameter (`&F: Fn()`), and std's derivable traits of `()`, whose
                // impls it does not know (the solver answers those))
                let std_no = !own && !prog.has_param(t) && !prog.implements(t, tr) && {
                    let n = tr.split('<').next().unwrap_or(tr).split('(').next().unwrap_or(tr).trim();
                    let n = n.rsplit("::").next().unwrap_or(n);
                    let inst_tr = || crate::traits::instantiate_bound(prog, tr, &gn, &args);
                    let blanket = matches!(n, "Borrow" | "BorrowMut" | "Any" | "DoubleEndedIterator");
                    // (`&F` is every Fn trait F is; `&mut F` is FnMut and FnOnce when F is FnMut, never Fn of itself)
                    let fn_of_ref = match t {
                        Ty::Ref(m, Ty::Struct(s)) if prog.closure_of_struct.contains_key(&prog.base_of(*s)) || prog.structs[*s as usize].placeholder.is_some() => {
                            if m { matches!(n, "FnMut" | "FnOnce") } else { matches!(n, "Fn" | "FnMut" | "FnOnce") }
                        }
                        _ => false,
                    };
                    let unit_known = t == Ty::Unit && crate::traits::unit_judged(n);
                    // (never more than `implements`' own no on the bound as written: the instantiated bound is only a
                    // second opinion that can take a no back)
                    let r = !blanket && !fn_of_ref && !unit_known && inst_tr().as_deref().is_none_or(|b| b == tr || !prog.implements(t, b));
                    r
                };
                if std_no || ((own || t == Ty::Unit) && !prog.has_param(t) && held.unwrap_or_else(|| prog.tdb.holds_in(prog, t, tr, &gn, &args)) == crate::traits::Ans::No) {
                    return Err(format!(
                        "the trait bound `{}: {}` is not satisfied (for `{pn}`)",
                        prog.tn(t),
                        crate::traits::bound_text(tr)
                    ));
                }
                // (the bound's bindings, `T: Trait<AssociatedType = u32>`: normalizes-to goals, issue #2. Of the
                // program's fns only: the prelude's bindings are hints for inference, not std's bounds (`sum<S>` asks
                // `Self: Iterator<Item = S>` where std asks `S: Sum<Self::Item>`)
                let own = prog.item_of.get(base as usize).copied().flatten().is_some_and(|i| i < prog.prelude_from);
                let names: Vec<&str> = b.generics.iter().map(|g| g.0.as_str()).collect();
                if let Some((proj, want, got)) = own.then(|| prog.binding_mismatch(t, tr, &names, &args)).flatten() {
                    return Err(format!("type mismatch resolving `{proj} == {}`: it is `{}` (E0271)", prog.tn(want), prog.tn(got)));
                }
            }
        }
        // (the where clauses on other than a parameter, `where T::A: Bar`: the type each names for these arguments,
        // normalized by the trait that gives it, must meet the bound. Bounds of the program's traits only, as for the
        // nested goals of impls)
        if let Some(preds) = prog.fn_preds.get(&base) {
            let names: Vec<&str> = b.generics.iter().map(|g| g.0.as_str()).collect();
            for (syn, qtr, bounds) in preds {
                let Some(st) = prog.pred_subject(syn, qtr, b, &names, &args) else { continue };
                // (nor a type implements does not see the program's impls for: a Box, an array, a slice, a type with a
                // trait object in it; as for the supertraits and item bounds of impls)
                if prog.opaque_to_impls(st) {
                    continue;
                }
                // (nor one with a placeholder in it: the instance checks the generic fn's own body, where its where
                // clauses are what it may assume, not what it must prove (`where &'max G: Get`))
                if prog.holds_placeholder(st, 0) {
                    continue;
                }
                for tr in bounds {
                    // (its bindings too, `where W<T>: Tr<A = u32>`, as for a parameter's bounds: of the program's fns)
                    let own = prog.item_of.get(base as usize).copied().flatten().is_some_and(|i| i < prog.prelude_from);
                    if let Some((proj, want, got)) = own.then(|| prog.binding_mismatch(st, tr, &names, &args)).flatten() {
                        return Err(format!("type mismatch resolving `{proj} == {}`: it is `{}` (E0271)", prog.tn(want), prog.tn(got)));
                    }
                    let tname = tr.split('<').next().unwrap_or(tr).trim();
                    if !prog.user_traits.contains(tname) {
                        continue;
                    }
                    match prog.evaluate(st, tr) {
                        Certainty::No => return Err(format!("the trait bound `{}: {tr}` is not satisfied (E0277)", prog.tn(st))),
                        Certainty::Maybe(MaybeCause::Cycle) => {
                            return Err(format!("overflow evaluating the requirement `{}: {tr}` (E0275)", prog.tn(st)));
                        }
                        _ => {}
                    }
                }
            }
        }
        let names: Vec<String> = args.iter().map(|&t| prog.tn(t)).collect();
        let impl_of = if b.trait_of.is_some() {
            match args[0] {
                Ty::Struct(sid) => Some(sid),
                _ => None,
            }
        } else {
            match b.self_ty.map(|t| prog.subst(t, &args)) {
                Some(Ty::Struct(sid)) => Some(sid),
                _ => b.impl_of,
            }
        };
        let sig = FnSig {
            params: b.params.iter().map(|&t| prog.subst(t, &args)).collect(),
            ret: prog.subst(b.ret, &args),
            symbol: format!("{}<{}>", b.symbol, names.join(",")),
            is_extern: false,
            is_main: false,
            impl_of,
            has_self: b.has_self,
            generics: vec![],
            trait_of: None,
            has_body: true,
            impl_trait: None,
            // Self: a trait default method's first type argument, or an impl method's self type with the type arguments
            self_ty: if b.trait_of.is_some() {
                Some(args[0])
            } else {
                b.self_ty.map(|t| prog.subst(t, &args))
            },
            export: false,
            panic_handler: false,
            ret_from: b.ret_from,
            is_unsafe: b.is_unsafe,
            // (of the generic signature: its slots leave out the type parameters' own)
            regions: b.regions.clone(),
        };
        // (a generic fn returning `impl Trait`: the type of this instance's body)
        let mut sig = sig;
        if let Some(body) = prog.opaque_generic.get(&base) {
            let env = TyEnv {
                self_sid: sig.impl_of,
                params: b.generics.iter().map(|g| g.0.clone()).zip(args.iter().copied()).collect(),
                self_ty: sig.self_ty,
            };
            let (_, r, _) = crate::ast_infer::infer_ctx(prog, body.2, &body.0, &body.1, &sig, &env, &[], None, Some((base, 0)));
            match r.filter(|&t| !prog.has_param(t)) {
                Some(t) => sig.ret = t,
                None => return Err(format!("cannot infer the `impl Trait` return type of `{}`", prog.names[base as usize])),
            }
        }
        let id = self.n_fns + self.list.len() as u32;
        self.list.push(Inst {
            base,
            args: args.clone(),
            sig,
            fnptr: false,
        });
        self.map.insert((base, args), id);
        self.pending.push(id);
        Ok(id)
    }

    /// The fn-pointer instance of a closure with no captures: its signature without the environment parameter
    pub fn request_fnptr(&mut self, prog: &Program, base: u32, args: Vec<Ty>) -> R<u32> {
        let mut key = args.clone();
        key.push(Ty::Param(FNPTR_MARK));
        if let Some(&id) = self.map.get(&(base, key.clone())) {
            return Ok(id);
        }
        let id0 = self.request(prog, base, args.clone())?;
        let mut sig = self.sig(prog, id0).clone();
        if !sig.params.is_empty() {
            sig.params.remove(0);
        }
        sig.has_self = false;
        sig.symbol = format!("{}#fnptr", sig.symbol);
        let id = self.n_fns + self.list.len() as u32;
        self.list.push(Inst { base, args, sig, fnptr: true });
        self.map.insert((base, key), id);
        self.pending.push(id);
        Ok(id)
    }

    /// The signature of fn (or instance) id
    pub fn sig<'a>(&'a self, prog: &'a Program, id: u32) -> &'a FnSig {
        match self.get(id) {
            Some(i) => &i.sig,
            None => &prog.sigs[id as usize],
        }
    }

    /// The id of the fn holding the body (for an instance, its generic base fn)
    /// The facts a lowering told closures' bodies (from fx_log: those logged from `from` on)
    pub fn facts_since(&self, from: usize) -> Vec<CloFact> {
        let mut out: Vec<CloFact> = vec![];
        for &b in &self.fx_log[from.min(self.fx_log.len())..] {
            if !out.iter().any(|f| f.body == b) {
                out.extend(self.facts_of(b));
            }
        }
        out
    }

    /// A closure's body's facts
    pub fn facts_of(&self, b: u32) -> Vec<CloFact> {
        vec![CloFact { body: b, kind_free: self.kind_free.contains(&b), rg: self.closure_rg.get(&b).cloned() }]
    }

    /// Facts of a lowering not made here (a Blob's): as it would have set them
    pub fn give_facts(&mut self, fs: &[CloFact]) {
        for f in fs {
            if f.kind_free {
                self.kind_free.insert(f.body);
            }
            if let Some(rg) = &f.rg {
                self.closure_rg.insert(f.body, rg.clone());
            }
        }
    }

    pub fn body_of(&self, id: u32) -> u32 {
        self.get(id).map(|i| i.base).unwrap_or(id)
    }

    /// The type environment for type-checking the body
    pub fn env(&self, prog: &Program, id: u32) -> TyEnv {
        match self.get(id) {
            Some(i) => {
                let b = &prog.sigs[i.base as usize];
                TyEnv {
                    self_sid: i.sig.impl_of,
                    params: b
                        .generics
                        .iter()
                        .map(|g| g.0.clone())
                        .zip(i.args.iter().copied())
                        .collect(),
                    self_ty: i.sig.self_ty,
                }
            }
            None => {
                let s = &prog.sigs[id as usize];
                TyEnv {
                    self_sid: s.impl_of,
                    params: vec![],
                    self_ty: s.self_ty.filter(|t| !prog.has_param(*t)),
                }
            }
        }
    }
}


/// Whether rebase_aliased may change sm: it names an alias of the map where rebase_aliased looks (or more: the
/// type parameters it leaves alone are not looked at)
fn names_aliased(sm: &Summary, map: &FxMap<String, String>) -> bool {
    let named = |n: &Option<String>| n.as_ref().is_some_and(|n| map.contains_key(n.as_str()));
    named(&sm.impl_self)
        || sm.fns.iter().any(|(_, s)| named(&s.impl_of))
        || sm.closures.iter().any(|c| named(&c.sig.impl_of))
        || sm.consts.iter().any(|c| c.0.split_once("::").is_some_and(|(a, _)| map.contains_key(a)))
}

/// sm with the items of `impl Alias { .. }` moved to the aliased type (`Alias::f` -> `Foo::f`)
fn rebase_aliased(sm: &mut Summary, map: &FxMap<String, String>) {
    let re = |n: &mut String| {
        if let Some((a, rest)) = n.split_once("::") {
            if let Some(b) = map.get(a) {
                *n = format!("{b}::{rest}");
            }
        }
    };
    // (not a type parameter of the same name: `impl<T> Tr for T` beside `type T = S;`)
    if let Some(n) = &mut sm.impl_self {
        if let Some(b) = map.get(n.as_str()).filter(|_| !sm.impl_generics.iter().any(|g| g.0 == *n)) {
            *n = b.clone();
        }
    }
    for (n, s) in &mut sm.fns {
        if let Some(o) = &mut s.impl_of {
            if let Some(b) = map.get(o.as_str()).filter(|_| !s.generics.iter().any(|g| g.0 == *o)) {
                *o = b.clone();
                re(n);
            }
        }
    }
    for c in &mut sm.closures {
        if let Some(o) = &mut c.sig.impl_of {
            if let Some(b) = map.get(o.as_str()) {
                *o = b.clone();
            }
        }
    }
    for c in &mut sm.consts {
        re(&mut c.0);
    }
}

impl Program {
    /// trait_scopes at item ii: the traits of a colliding name in scope there (the innermost scope around its source
    /// line). None when no trait name collides
    pub fn traits_at_item(&self, ii: u32) -> Option<&[String]> {
        if self.trait_scopes.is_empty() {
            return None;
        }
        let line = *self.item_lines.get(ii as usize)?;
        self.trait_scopes
            .iter()
            .filter(|(lo, hi, _)| *lo <= line && line <= *hi)
            .min_by_key(|(lo, hi, _)| hi - lo)
            .map(|(_, _, v)| v.as_slice())
    }
}

/// `from` (a type name) as `to` everywhere in t
fn rename_tysyn(t: &mut TySyn, from: &str, to: &str) {
    match t {
        TySyn::Named(n) if n == from => *n = to.to_string(),
        TySyn::App(n, args) => {
            if n == from {
                *n = to.to_string();
            }
            for a in args {
                rename_tysyn(a, from, to);
            }
        }
        TySyn::Ref(_, i) | TySyn::Ptr(_, i) | TySyn::Slice(i) | TySyn::Array(i, _) | TySyn::Assoc(i, _) => rename_tysyn(i, from, to),
        TySyn::Tuple(v) | TySyn::FnPtr(v) => {
            for a in v {
                rename_tysyn(a, from, to);
            }
        }
        _ => {}
    }
}

/// `Self` inside a struct / enum definition
pub fn self_of(name: &str, generics: &[String]) -> TySyn {
    if generics.is_empty() {
        TySyn::Named(name.to_string())
    } else {
        TySyn::App(
            name.to_string(),
            generics.iter().map(|g| TySyn::Named(g.clone())).collect(),
        )
    }
}

/// arithmetic operator traits of integers and floats (core implements them)
const NUM_OPS: &[&str] = &["Add", "Sub", "Mul", "Div", "Rem", "Neg", "AddAssign", "SubAssign", "MulAssign", "DivAssign", "RemAssign", "Sum", "Product"];
/// bit operator traits of integers
const INT_OPS: &[&str] = &["BitAnd", "BitOr", "BitXor", "Shl", "Shr", "Not", "BitAndAssign", "BitOrAssign", "BitXorAssign", "ShlAssign", "ShrAssign"];

impl Program {
    /// The type an alias names, as an impl's self type: `type A = Foo<'static>` -> `Foo`, `type T = u32` -> `u32`
    fn alias_base(&self, n: &str) -> Option<String> {
        let mut t = &self.aliases.get(n)?.1;
        for _ in 0..8 {
            match t {
                TySyn::Named(s) | TySyn::App(s, _) => match self.aliases.get(s) {
                    Some(a) => t = &a.1,
                    None => return Some(s.clone()),
                },
                TySyn::Prim(Ty::Unit) => return Some("#tuple0".into()),
                TySyn::Tuple(v) => return Some(format!("#tuple{}", v.len())),
                TySyn::Prim(pt) => return Some(format!("{pt:?}").to_lowercase()),
                TySyn::Str => return Some("str".into()),
                _ => return None,
            }
        }
        None
    }

    pub fn build(items: &[SrcItem], sums: &mut [Summary]) -> R<Program> {
        Program::build_with(items, sums, true, items.len())
    }

    /// need_main: false means a library (no main needed)
    /// `user`: how many of the items are the program's (the rest are the prelude's)
    pub fn build_with(items: &[SrcItem], sums: &mut [Summary], need_main: bool, user: usize) -> R<Program> {
        Program::build_with_edition(items, sums, need_main, user, 2021, false)
    }

    /// build_with in an edition: the types of the signatures and the fields are resolved in it (a trait's name behind
    /// a pointer is its trait object before 2021, bare_dyn)
    /// (deny_bare: the `bare_trait_objects` lint is denied, so such an object is an error)
    /// (sums: the build rewrites a few of them for itself (`impl Alias`, closures of one name) and puts them back
    /// before it returns, so the caller's are as they were; only those are copied)
    pub fn build_with_edition(items: &[SrcItem], sums: &mut [Summary], need_main: bool, user: usize, edition: u16, deny_bare: bool) -> R<Program> {
        let mut saved: Vec<(usize, Summary)> = vec![];
        let r = Program::build_in(items, sums, &mut saved, need_main, user, edition, deny_bare);
        // (the last saved first: a summary saved twice gets the first copy, the one from before the build)
        for (i, sm) in saved.into_iter().rev() {
            sums[i] = sm;
        }
        r
    }

    /// build_with_edition, saving each summary it rewrites in `saved` (its index, the summary before) first
    fn build_in(items: &[SrcItem], sums_mut: &mut [Summary], saved: &mut Vec<(usize, Summary)>, need_main: bool, user: usize, edition: u16, deny_bare: bool) -> R<Program> {
        let sums: &[Summary] = &*sums_mut;
        // (the crate's `main`, summed up as an ordinary function because it cannot be the entry (ast_summary): an error
        //  when it would be, so not under `--test`. First, as it was when the summary reported it)
        if need_main && !sums.iter().any(|s| s.fns.iter().any(|(n, _)| n == TEST_ENTRY)) {
            for (i, s) in sums[..user.min(sums.len())].iter().enumerate() {
                if let Some((_, sig)) = s.fns.iter().find(|(n, sig)| n == "main" && !sig.is_main && !sig.export && !sig.is_extern && sig.symbol.starts_with("_xmain")) {
                    let msg = if sig.symbol.ends_with(".async") {
                        "`main` function is not allowed to be `async` (E0752)"
                    } else if sig.generics.is_empty() {
                        "`main` must be `fn main()`"
                    } else {
                        "`main` cannot be generic"
                    };
                    return Err(format!("{}:1: {msg}", items.get(i).map_or(1, |it| it.line)));
                }
            }
        }
        let mut p = Program {
            names: vec![],
            index: FxMap::default(),
            sigs: vec![],
            item_of: vec![],
            structs: AdtTable::new(),
            struct_index: FxMap::default(),
            variant_names: FxMap::default(),
            variant_shadowed: vec![],
            opaque: vec![],
            prelude_from: u32::MAX,
            fn_preds: FxMap::default(),
            assoc_index: std::sync::OnceLock::new(),
            normalized: Default::default(),
            overflow_checks: false,
            trait_supers: FxMap::default(),
            trait_marker_supers: FxMap::default(),
            trait_self_outlives: FxMap::default(),
            any_immutable_static: false,
            static_copy: FxSet::default(),
            static_self_consts: FxMap::default(),
            const_regions: FxMap::default(),
            assoc_regions: FxMap::default(),
            assoc_items: Vec::new(),
            early_syn: FxMap::default(),
            alias_fixed_lt: false,
            trait_ngen: FxMap::default(),
            impl_targs: FxMap::default(),
            impl_targ_bounds: FxMap::default(),
            impl_targs_lts: HashSet::default(),
            ref_impls: Default::default(),
            ref_impl_pats: vec![],
            ref_param_impls: vec![],
            ref_param_bounds: Default::default(),
            dyn_impls: FxSet::default(),
            dyn_impls_exact: FxSet::default(),
            dyn_impls_lt: FxSet::default(),
            fnptr_impls: Default::default(),
            deny_unreachable: false,
            deny_must_use: false,
            edition,
            deny_bare_trait: deny_bare,
            path_lts: false,
            deny_unused_alloc: false,
            deny_unused_mut: false,
            allow_overflowing_literals: false,
            use_prefixes: Vec::new(),
            fn_traits: false,
            allow_implicit_autorefs: false,
            deny_trivial_casts: (false, false),
            must_use_fns: vec![],
            prim_mut_methods: HashSet::default(),
            opt: false,
            blanket: FxMap::default(),
            blanket_bounds: FxMap::default(),
            blanket_selfs: Default::default(),
            blanket_traits: Default::default(),
            unions: Default::default(),
            reprs: Default::default(),
            packed: Default::default(),
            min_align: Default::default(),
            repr_c: Default::default(),
            const_fns: Default::default(),
            const_fn_tys: Default::default(),
            overloads: Default::default(),
            self_ty_groups: FxSet::default(),
            fn_targs: FxMap::default(),
            trait_scopes: vec![],
            item_lines: vec![],
            by_last: FxMap::default(),
            ref_groups: vec![],
            traits: FxMap::default(),
            impls: HashSet::default(),
            panic_handler: None,
            consts: vec![],
            const_index: FxMap::default(),
            derived_peq: FxSet::default(),
            derived: FxMap::default(),
            const_param_tys: FxMap::default(),
            struct_defaults: FxMap::default(),
            alias_defaults: FxMap::default(),
            copy_params: FxMap::default(),
            unsized_params: FxMap::default(),
            fn_sized_gens: FxMap::default(),
            fn_unsized_gens: FxMap::default(),
            tail_syn: FxMap::default(),
            has_mods: false,
            field_priv: None,
            trait_clone_fns: false,
            multi_trait_methods: FxSet::default(),
            ambiguous_methods: FxMap::default(),
            unbounded_blankets: FxSet::default(),
            use_names: FxSet::default(),
            fmt_imports: 0,
            impl_selfs: FxMap::default(),
            user_traits: FxSet::default(),
            tdb: Default::default(),
            trait_decls: FxSet::default(),
            lower_consts: false,
            const_by_last: FxMap::default(),
            const_group: FxMap::default(),
            const_groups: vec![],
            const_impls: FxMap::default(),
            prim_impls: HashSet::default(),
            prim_ref_only: HashSet::default(),
            ref_slice_traits: HashSet::default(),
            closures: FxMap::default(),
            closure_by: FxMap::default(),
            closure_of_struct: FxMap::default(),
            inst_keys: Default::default(),
            drop_impls: Default::default(),
            deref_to_seq: Default::default(),
            deref_mut_of: Default::default(),
            has_drop: false,
            assocs: vec![],
            assoc_bounds: Default::default(),
            opaque_generic: Default::default(),
            peq: vec![],
            aliases: FxMap::default(),
            pord: vec![],
            struct_item: vec![],
            adt_regions: FxMap::default(),
        };
        // 0. Type aliases (before the types of struct fields)
        // (a struct / enum of the same name wins: the user's `struct Path` over the prelude's `type Path = str`)
        let adt_names: HashSet<&str> = sums
            .iter()
            .flat_map(|sm| sm.structs.iter().map(|x| x.0.as_str()).chain(sm.enums.iter().map(|x| x.0.as_str())))
            .collect();
        for sm in sums {
            p.unions.extend(sm.unions.iter().cloned());
            for (n, ps, b) in &sm.const_fns {
                p.const_fns.insert(n.clone(), (ps.clone(), b.clone()));
            }
            for (n, ps, r) in &sm.const_fn_tys {
                p.const_fn_tys.insert(n.clone(), (ps.clone(), r.clone()));
            }
            for (tr, n, bs) in &sm.assoc_bounds {
                p.assoc_bounds.insert((tr.clone(), n.clone()), bs.clone());
            }
            for (n, t) in &sm.reprs {
                if let Some(t) = Ty::from_suffix(t) {
                    p.reprs.insert(n.clone(), t);
                } else if t == "C" {
                    p.repr_c.insert(n.clone());
                } else if let Some(k) = t.strip_prefix("packed:").and_then(|x| x.parse::<u32>().ok()) {
                    // (several: the least packing, the greatest alignment, as rustc takes them)
                    let e = p.packed.entry(n.clone()).or_insert(k);
                    *e = (*e).min(k);
                } else if let Some(k) = t.strip_prefix("align:").and_then(|x| x.parse::<u32>().ok()) {
                    let e = p.min_align.entry(n.clone()).or_insert(k);
                    *e = (*e).max(k);
                }
            }
            p.alias_fixed_lt |= !sm.alias_fixed_lts.is_empty();
            for (n, g, t) in &sm.aliases {
                if !adt_names.contains(n.as_str()) {
                    p.aliases.insert(n.clone(), (g.clone(), t.clone()));
                    if !sm.adt_defaults.is_empty() {
                        p.add_alias_defaults(n, &sm.adt_defaults);
                    }
                }
            }
        }
        // (`type Alias = Foo<'static>; impl Alias { .. }`: the impl's items belong to the aliased type)
        let alias_owner: FxMap<String, String> = if p.aliases.is_empty() {
            FxMap::default()
        } else {
            sums.iter()
                .filter_map(|sm| sm.impl_self.as_ref())
                .filter_map(|n| p.alias_base(n).map(|b| (n.clone(), b)))
                .collect()
        };
        if !alias_owner.is_empty() {
            for (ii, sm) in sums_mut.iter_mut().enumerate() {
                if names_aliased(sm, &alias_owner) {
                    saved.push((ii, sm.clone()));
                    rebase_aliased(sm, &alias_owner);
                }
            }
        }
        let sums: &[Summary] = &*sums_mut;
        // (a closure is named after the fn it is in, `Vec::m::{closure#0}`, which leaves out the trait: two traits of
        // one name (in two modules) implemented for Vec, each with a closure in `m`, would share the name. A later
        // one also gets its item's number. It was an error before, so no program that built is renamed)
        {
            let mut seen: FxMap<&str, usize> = FxMap::default();
            let dup = sums.iter().enumerate().any(|(ii, sm)| sm.closures.iter().any(|c| *seen.entry(c.name.as_str()).or_insert(ii) != ii));
            if dup {
                // (the first summary with each name keeps it: the others are renamed, and only they are rewritten)
                let mut first: FxMap<String, usize> = FxMap::default();
                for (ii, sm) in sums.iter().enumerate() {
                    for c in &sm.closures {
                        first.entry(c.name.clone()).or_insert(ii);
                    }
                }
                for (ii, sm) in sums_mut.iter_mut().enumerate() {
                    if sm.closures.iter().all(|c| first[&c.name] == ii) {
                        continue;
                    }
                    saved.push((ii, sm.clone()));
                    let mut renames: Vec<(String, String)> = vec![];
                    for c in &mut sm.closures {
                        if first[&c.name] != ii {
                            let to = format!("{}_{ii}", c.name);
                            // (its `call` takes the closure struct by name)
                            let from = std::mem::replace(&mut c.name, to.clone());
                            for t in c.sig.params.iter_mut().chain(std::iter::once(&mut c.sig.ret)).chain(c.sig.self_ty.iter_mut()) {
                                rename_tysyn(t, &from, &to);
                            }
                            if c.sig.impl_of.as_deref() == Some(from.as_str()) {
                                c.sig.impl_of = Some(to.clone());
                            }
                            // (and its symbol, `<the fn's symbol>.c<k>`, which is the same for the two too)
                            c.sig.symbol = format!("{}_{ii}", c.sig.symbol);
                            renames.push((from, to));
                        }
                    }
                    // (the fn a closure is in names it in its return type when it returns it, `-> impl Fn`)
                    for (from, to) in &renames {
                        for (_, sig) in &mut sm.fns {
                            for t in sig.params.iter_mut().chain(std::iter::once(&mut sig.ret)) {
                                rename_tysyn(t, from, to);
                            }
                        }
                        for c in &mut sm.closures {
                            for t in c.sig.params.iter_mut().chain(std::iter::once(&mut c.sig.ret)) {
                                rename_tysyn(t, from, to);
                            }
                        }
                    }
                }
            }
        }
        let sums: &[Summary] = &*sums_mut;
        // 0. The names of consts / statics (before types, since array lengths use them)
        for (ii, sm) in sums.iter().enumerate() {
            let cs = sm
                .consts
                .iter()
                .map(|(n, t, e)| (n, t, e, None))
                .chain(sm.statics.iter().map(|(n, t, e, m)| (n, t, e, Some(*m))));
            for (n, t, e, stat) in cs {
                // (`const _: () = { assert!(..) };` is only a compile-time check: skipped)
                if anon_const(n) {
                    continue;
                }
                // (`impl Foo { const C }` beside `impl Tr for Foo { const C }`: rustc's `Foo::C` and `<Foo as Tr>::C`.
                // The trait impl's goes by `Foo as Tr::C`, which a qualified path reads)
                let trait_of = |k: usize| match items[k].kind {
                    crate::lexer::ItemKind::ImplMember(h) => sums[h as usize].impl_trait.clone(),
                    _ => None,
                };
                let header = |k: usize| match items[k].kind {
                    crate::lexer::ItemKind::ImplMember(h) => Some(h),
                    _ => None,
                };
                let mut n = n.clone();
                // (the const of another impl block this one stands beside under one name: see const_groups)
                let mut beside: Option<u32> = None;
                if let Some(&prev) = p.const_index.get(&n) {
                    let pk = p.consts[prev as usize].item as usize;
                    let other_block = header(ii).is_some() && header(pk).is_some() && header(ii) != header(pk);
                    match (trait_of(ii), trait_of(pk)) {
                        // (two of one trait impl's: a real duplicate. But impls for trait objects of each arity,
                        // `impl Tr for dyn Fn(A) -> R` and `.. for dyn Fn(A, B) -> R`, are one `__dyn_Fn` here: the
                        // first one's stands when they give it alike, `const IS_MUT: bool = false;`)
                        (Some(a), Some(b)) if a == b => {
                            // (not two in one impl: `impl Tr for dyn Obj { const N: bool = false; const N: bool = false; }`
                            // is E0201's duplicate)
                            if n.starts_with("__dyn_") && ii != pk && other_block && sums[pk].consts.iter().any(|c| c.0 == n && c.2 == *e) {
                                // (kept under a name no path reaches: the consts stay in the summaries' order)
                                n = format!("{n}#dyn{}", p.consts.len());
                            } else if other_block {
                                // (two impls of one trait for one type, with other trait arguments or self types:
                                // `impl ToInt<i8> for Z0` and `impl ToInt<i16> for Z0` (typenum), `impl FixedTag for S`
                                // and `impl FixedTag for &S` (der). Each has its own; a use chooses (pick_const))
                                beside = Some(prev);
                            } else {
                                return Err(format!("{}:1: duplicate definition of `{n}`", items[ii].line));
                            }
                        }
                        (Some(tr), _) => {
                            let (base, last) = n.rsplit_once("::").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
                            n = format!("{base} as {tr}::{last}");
                        }
                        (None, Some(tr)) => {
                            // (the trait impl's came first: it moves to its qualified name)
                            let (base, last) = n.rsplit_once("::").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
                            let q = format!("{base} as {tr}::{last}");
                            p.const_index.remove(&n);
                            p.consts[prev as usize].name = q.clone();
                            p.const_index.insert(q, prev);
                        }
                        // (two inherent impls' (`impl W<u8> { const C }`, `impl W<u16> { const C }`): E0592 unless
                        // their self types are apart, which is known once types are (const_group_impls))
                        (None, None) if other_block => beside = Some(prev),
                        (None, None) => {
                            return Err(format!("{}:1: duplicate definition of `{n}`", items[ii].line));
                        }
                    }
                    // (the qualified name another impl of the trait has: `impl Foo { const C }` with `impl Tr<u8> for
                    // Foo` and `impl Tr<u16> for Foo`, each `Foo as Tr::C`)
                    if let (None, Some(&q)) = (beside, p.const_index.get(&n)) {
                        let qk = p.consts[q as usize].item as usize;
                        if trait_of(ii).is_none() || trait_of(qk) != trait_of(ii) || header(qk).is_none() || header(qk) == header(ii) {
                            return Err(format!("{}:1: duplicate definition of `{n}`", items[ii].line));
                        }
                        beside = Some(q);
                    }
                }
                let real_last = beside.map(|_| n.rsplit_once("::").map_or(n.as_str(), |x| x.1).to_string());
                if let Some(prev) = beside {
                    let g = match p.const_group.get(&prev) {
                        Some(&g) => g,
                        None => {
                            p.const_groups.push(vec![prev]);
                            p.const_group.insert(prev, p.const_groups.len() as u32 - 1);
                            p.const_groups.len() as u32 - 1
                        }
                    };
                    let me = p.consts.len() as u32;
                    p.const_groups[g as usize].push(me);
                    p.const_group.insert(me, g);
                    // (a name of its own, which only the group reaches: `Z0::INT#1`)
                    n = format!("{n}#{}", p.const_groups[g as usize].len() - 1);
                }
                let n = &n;
                if let Some((_, last)) = n.rsplit_once("::") {
                    p.const_by_last.entry(real_last.unwrap_or_else(|| last.to_string())).or_default().push(p.consts.len() as u32);
                }
                p.const_index.insert(n.clone(), p.consts.len() as u32);
                // (a trait impl's goes by `Foo as Tr::C` too: a qualified path that misses it reads the trait's default)
                if let (Some(tr), false) = (trait_of(ii), n.contains(" as ")) {
                    if let Some((base, last)) = n.rsplit_once("::") {
                        p.const_index.insert(format!("{base} as {tr}::{last}"), p.consts.len() as u32);
                    }
                }
                // (an impl's for its type's 'static instance (`impl A<'static> { const IC }`): a use names the
                // lifetimes it is used at)
                if let crate::lexer::ItemKind::ImplMember(h) = items[ii].kind {
                    if let Some(r) = sums[h as usize].hdr_regions.as_ref().filter(|r| r.self_slots.contains(&R_STATIC)) {
                        p.static_self_consts.insert(p.consts.len() as u32, r.self_slots.clone());
                    }
                    // (one whose type names the impl's lifetimes: a use instantiates them)
                    if let (Some(r), Some((_, codes))) = (sums[h as usize].hdr_regions.as_ref(), sm.const_regions.iter().find(|x| x.0 == *n)) {
                        p.const_regions.insert(p.consts.len() as u32, (codes.clone(), r.self_slots.clone()));
                    }
                }
                // (an integer type is known already, before the layouts that evaluate constants (`[u8; N]`); the other
                // types are resolved below)
                let early = early_int_ty(t, &p.aliases).unwrap_or(Ty::Unit);
                if early == Ty::Unit && matches!(t, TySyn::Array(..) | TySyn::Tuple(_) | TySyn::Ref(..)) {
                    p.early_syn.insert(p.consts.len() as u32, t.clone());
                }
                p.consts.push(ConstDef {
                    name: n.clone(),
                    ty: early,
                    expr: e.clone(),
                    item: ii as u32,
                    stat,
                });
            }
        }
        // 1. Register the struct / enum names
        p.trait_decls = sums.iter().filter_map(|sm| sm.trait_decl.clone()).collect();
        let mut sdefs: Vec<(usize, String, AdtShape, Vec<String>)> = vec![];
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            let adts = sm
                .structs
                .iter()
                .map(|(n, f)| (n, AdtShape::Struct(f.clone())))
                .chain(sm.enums.iter().map(|(n, v)| (n, AdtShape::Enum(v.clone()))));
            for (name, shape) in adts {
                if p.struct_index.contains_key(name) {
                    // (rustc's E0428, at the second's name: the driver moves it to that line)
                    return Err(format!("{}:1: the name `{name}` is defined multiple times (E0428)", it.line));
                }
                p.struct_index.insert(name.clone(), sdefs.len() as u32);
                if let Some(r) = &sm.adt_regions {
                    p.adt_regions.insert(sdefs.len() as u32, std::sync::Arc::new((**r).clone()));
                }
                // (the declared types of its const parameters other than usize, by the parameter's place)
                for (pn, bs) in &sm.adt_bounds {
                    let decl = bs.iter().find_map(|b| b.strip_prefix(CONST_GENERIC)?.strip_prefix(':'));
                    if let (Some(d), Some(k)) = (decl, sm.adt_generics.iter().position(|g| g == pn)) {
                        p.const_param_tys.entry(sdefs.len() as u32).or_default().push((k, d.to_string()));
                    }
                    if let (true, Some(k)) = (bs.iter().any(|b| b == "Copy" || b.ends_with("::Copy")), sm.adt_generics.iter().position(|g| g == pn)) {
                        p.copy_params.entry(sdefs.len() as u32).or_default().push(k);
                    }
                }
                for pn in &sm.adt_unsized {
                    if let Some(k) = sm.adt_generics.iter().position(|g| g == pn) {
                        p.unsized_params.entry(sdefs.len() as u32).or_default().push(k);
                    }
                }
                if !sm.adt_defaults.is_empty() {
                    p.struct_defaults.insert(sdefs.len() as u32, sm.adt_defaults.clone());
                }
                if !sm.adt_unsized.is_empty() {
                    if let AdtShape::Struct(fs) = &shape {
                        if let Some((_, last)) = fs.last() {
                            p.tail_syn.insert(sdefs.len() as u32, (sm.adt_generics.clone(), last.clone()));
                        }
                    }
                }
                sdefs.push((ii, name.clone(), shape, sm.adt_generics.clone()));
            }
            for c in &sm.closures {
                p.struct_index.insert(c.name.clone(), sdefs.len() as u32);
                sdefs.push((
                    ii,
                    c.name.clone(),
                    AdtShape::Struct(c.fields.clone()),
                    c.generics.clone(),
                ));
            }
        }
        p.struct_item = sdefs.iter().map(|d| d.0 as u32).collect();
        // (the concrete types in where clauses: `where Missing: Trait`)
        for (it, sm) in items.iter().zip(sums) {
            // (and an impl's self type, at its header: `impl Tr for xyz::T`)
            if let Some(sn) = &sm.impl_self {
                let builtin = Ty::prim(sn).is_some() || sn == "str" || sn == "Box" || sn.starts_with("#tuple") || sn.starts_with("__") || crate::simd_sig::vector_name(sn).is_some();
                if !builtin && !p.struct_index.contains_key(sn) && !p.aliases.contains_key(sn) && !sm.impl_generics.iter().any(|g| g.0 == *sn) {
                    return Err(format!("{}:1: cannot find type `{sn}` in this scope", it.line));
                }
            }
            for n in &sm.where_types {
                if !p.struct_index.contains_key(n) && !p.aliases.contains_key(n) && !p.traits.contains_key(n) {
                    return Err(format!("{}:1: cannot find type `{n}` in this scope", it.line));
                }
            }
        }
        // 2. Resolve field types and lay the types out in dependency order
        for (_, n, sh, g) in &sdefs {
            let mut d = StructDef::blank(n.clone(), matches!(sh, AdtShape::Enum(_)));
            d.generics = g.clone();
            p.structs.push(d);
        }
        // (associated types without generic structs, `impl Tr for i32 { type Ty = i32; }`, before the fields: a field
        // or a const may be `<i32 as Tr>::Ty`. The others below, once the definitions are laid out)
        // (the traits the program declares, known before the associated types are read: a qualified path's
        // `<T as Tr>::Abi` keeps Tr when it is one of them)
        for sm in &sums[..user.min(sums.len())] {
            if let Some(tr) = &sm.trait_decl {
                p.user_traits.insert(tr.clone());
            }
        }
        let mut early = vec![];
        // (the values that name other associated types, `type Prim1 = <WasmWord as WasmAbi>::Prim1;`: read again once
        // all are known, as one read before WasmWord's impl took a blanket impl's: (index in p.assocs, item, place))
        let mut reread: Vec<(usize, usize, usize)> = vec![];
        for (ii, sm) in sums.iter().enumerate() {
            // (of a trait impl: an inherent `impl S { type P = (); }` is unstable, `S::P` is not looked up through it)
            let of_trait = matches!(items[ii].kind, crate::lexer::ItemKind::ImplMember(h) if sums[h as usize].impl_trait.is_some());
            if sm.assocs.is_empty() || !of_trait {
                continue;
            }
            for (k, a) in sm.assocs.iter().enumerate() {
                if syn_has_app(&a.self_ty, &p.aliases) || syn_has_app(&a.value, &p.aliases) {
                    continue;
                }
                let gn: Vec<&str> = a.generics.iter().map(|g| g.as_str()).collect();
                if let (Ok(self_ty), Ok(value)) = (p.resolve_g(&a.self_ty, &gn), p.resolve_g(&a.value, &gn)) {
                    let (ngen, ngat) = gat_split(&gn);
                    let (trait_name, trait_args) = impl_trait_of(&p, items, sums, ii, &gn);
                    if syn_has_assoc(&a.value) {
                        reread.push((p.assocs.len(), ii, k));
                    }
                    p.assocs.push(AssocDef { name: a.name.clone(), self_ty, value, ngen, ngat, trait_name, trait_args });
                    p.note_assoc_regions(items, sums, ii, k);
                    p.assoc_items.push(ii as u32);
                    early.push((ii, k));
                }
            }
        }
        // Generic definitions first (this only resolves their field types with Param; layout is per instance)
        let mut state = vec![0u8; sdefs.len()]; // 0 not done, 1 in progress, 2 done
        for id in 0..sdefs.len() {
            if !sdefs[id].3.is_empty() {
                p.layout(id, &sdefs, items, &mut state)?;
            }
        }
        // (then the associated types whose definitions name a generic struct (`impl<R: D> A<R> for AT { type AS =
        // As<R>; }`), now that the generic definitions are resolved: before the other structs' fields, which may name
        // them (`struct S(<AT as A<DT>>::AS)`). One that does not resolve yet is left to the pass below)
        // (and those of generic types, `impl<R> Generator for Core<R> { type Output = [u32; 64]; }`, once the generic
        // definitions are there: a field `BlockRng<Core<R8>>` of a non-generic struct, laid out next, has a field of type
        // `G::Output` (rand_core 0.10 with chacha20). Those that do not resolve yet are taken below, as before)
        for (ii, sm) in sums.iter().enumerate() {
            let of_trait = matches!(items[ii].kind, crate::lexer::ItemKind::ImplMember(h) if sums[h as usize].impl_trait.is_some());
            if sm.assocs.is_empty() || !of_trait {
                continue;
            }
            for (k, a) in sm.assocs.iter().enumerate() {
                if early.contains(&(ii, k)) {
                    continue;
                }
                let gn: Vec<&str> = a.generics.iter().map(|g| g.as_str()).collect();
                if let (Ok(self_ty), Ok(value)) = (p.resolve_g(&a.self_ty, &gn), p.resolve_g(&a.value, &gn)) {
                    let (ngen, ngat) = gat_split(&gn);
                    let (trait_name, trait_args) = impl_trait_of(&p, items, sums, ii, &gn);
                    if syn_has_assoc(&a.value) {
                        reread.push((p.assocs.len(), ii, k));
                    }
                    p.assocs.push(AssocDef { name: a.name.clone(), self_ty, value, ngen, ngat, trait_name, trait_args });
                    p.note_assoc_regions(items, sums, ii, k);
                    p.assoc_items.push(ii as u32);
                    early.push((ii, k));
                }
            }
        }
        for id in 0..sdefs.len() {
            p.layout(id, &sdefs, items, &mut state)?;
        }
        // Instances made midway through layout (`RefCell<Node>` in `Node`) were computed with types whose size was
        // not settled yet, so they are redone once everything is settled
        p.relayout_instances();
        // (a variant of the program's own enum wins over a prelude enum's of the same name: `use ControlFlow::*;` of its
        // own `enum ControlFlow { Break, .. }`, beside the prelude's that the name brings in. The prelude's it took the
        // name from are kept in variant_shadowed, for the imports to decide (session). When two of one side share the
        // name, neither is found)
        let mut own_names: FxSet<&str> = FxSet::default();
        for sid in 0..p.structs.len() {
            let sd = &p.structs[sid];
            if !sd.is_enum || sd.base.is_some() || sd.name.starts_with('#') {
                continue;
            }
            let own = p.struct_item.get(sid).is_some_and(|&i| (i as usize) < user);
            for (vi, v) in sd.variants.iter().enumerate() {
                if own && own_names.insert(v.name.as_str()) {
                    if let Some(Some(prev)) = p.variant_names.insert(v.name.clone(), Some((sid as u32, vi))) {
                        p.variant_shadowed.push((v.name.clone(), prev));
                    }
                    continue;
                }
                if !own && own_names.contains(v.name.as_str()) {
                    p.variant_shadowed.push((v.name.clone(), (sid as u32, vi)));
                    continue;
                }
                p.variant_names
                    .entry(v.name.clone())
                    .and_modify(|e| *e = None)
                    .or_insert(Some((sid as u32, vi)));
            }
        }
        // deep hash = the definition hashes of all structs the fields reach transitively, by value or by reference.
        // A reference (`&'a Other`) counts too, since field offsets are embedded in the code.
        let n_defs = sdefs.len();
        // An `impl Drop` also changes the code of the fns using the type (drop calls), so it goes in the deep hash
        let drop_names: HashSet<&str> = sums
            .iter()
            .filter(|sm| sm.impl_trait.as_deref() == Some("Drop"))
            .filter_map(|sm| sm.impl_self.as_deref())
            .collect();
        let deeps: Vec<Key> = (0..n_defs)
            .map(|id| {
                let mut seen = HashSet::default();
                let mut stack = vec![Ty::Struct(id as u32)];
                let mut hs = vec![];
                while let Some(t) = stack.pop() {
                    match t {
                        Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => {
                            stack.push(*i)
                        }
                        Ty::Struct(sid) => {
                            if !seen.insert(sid) {
                                continue;
                            }
                            let sd = &p.structs[sid as usize];
                            if let Some(b) = sd.base {
                                stack.push(Ty::Struct(b));
                                stack.extend(sd.args.iter().copied());
                                continue;
                            }
                            if (sid as usize) < n_defs && sdefs[sid as usize].1.contains("{closure#") {
                                // A closure's struct: its layout follows from the type arguments (the capture types), and its body goes in the
                                // key of the closure's fn, so the parent item's hash is left out
                                // (editing one closure does not rebuild the instances of map and the like that take it)
                                let mut kh = KeyHasher::new();
                                kh.add(&sdefs[sid as usize].1);
                                hs.push(kh.finish());
                            } else if (sid as usize) < n_defs {
                                hs.push(items[sdefs[sid as usize].0].hash);
                                if drop_names.contains(sdefs[sid as usize].1.as_str()) {
                                    hs.push(0xd0d0);
                                }
                            }
                            let vf = sd.variants.iter().flat_map(|v| v.fields.iter());
                            stack.extend(sd.fields.iter().chain(vf).map(|f| f.1));
                        }
                        _ => {}
                    }
                }
                hs.sort_unstable();
                let mut h = KeyHasher::new();
                h.add(&hs);
                // (mutually recursive types (`Ty` ↔ `FnTy`) reach the same definitions, so the own name tells them apart)
                if id < n_defs {
                    h.add(&sdefs[id].1);
                }
                h.finish()
            })
            .collect();
        for (i, d) in deeps.into_iter().enumerate() {
            p.structs.get_mut(i).deep = d;
        }
        // The types of consts / statics
        let mut k = 0;
        for (it, sm) in items.iter().zip(sums) {
            for (n, t, _) in &sm.consts {
                if anon_const(n) {
                    continue;
                }
                // (an impl's const whose type names the impl's parameters: a hole, typed at each use)
                p.consts[k].ty = match p.resolve(t) {
                    // (a projection `C::M` stays an error: its type is not the hole a use could fill)
                    Err(e) if n.contains("::") && e.starts_with("cannot find type") && matches!(it.kind, crate::lexer::ItemKind::ImplMember(_)) && !format!("{t:?}").contains("Assoc(") => Ty::Param(HOLE),
                    r => r.map_err(|e| format!("{}:1: {e}", it.line))?,
                };
                k += 1;
            }
            for (n, t, _, _) in &sm.statics {
                if anon_const(n) {
                    continue;
                }
                p.consts[k].ty = p.resolve(t).map_err(|e| format!("{}:1: {e}", it.line))?;
                k += 1;
            }
        }
        if !p.const_groups.is_empty() {
            p.const_group_impls(items, sums)?;
        }
        // Associated types (first, since they resolve `Self::Item` in fn signatures)
        let early: FxSet<(usize, usize)> = early.into_iter().collect();
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            for (k, a) in sm.assocs.iter().enumerate() {
                // (registered above)
                if early.contains(&(ii, k)) {
                    continue;
                }
                let gn: Vec<&str> = a.generics.iter().map(|g| g.as_str()).collect();
                let self_ty = p
                    .resolve_g(&a.self_ty, &gn)
                    .map_err(|e| format!("{}:1: {e}", it.line))?;
                let value = p
                    .resolve_g(&a.value, &gn)
                    .map_err(|e| format!("{}:1: {e}", it.line))?;
                let (ngen, ngat) = gat_split(&gn);
                let (trait_name, trait_args) = impl_trait_of(&p, items, sums, ii, &gn);
                if syn_has_assoc(&a.value) {
                    reread.push((p.assocs.len(), ii, k));
                }
                p.assocs.push(AssocDef {
                    name: a.name.clone(),
                    self_ty,
                    value,
                    ngen,
                    ngat,
                    trait_name,
                    trait_args,
                });
                p.note_assoc_regions(items, sums, ii, k);
                p.assoc_items.push(ii as u32);
            }
        }
        // (twice: a value may name one read again here)
        for _ in 0..2 {
            for &(at, ii, k) in &reread {
                let a = &sums[ii].assocs[k];
                let gn: Vec<&str> = a.generics.iter().map(|g| g.as_str()).collect();
                if let Ok(v) = p.resolve_g(&a.value, &gn) {
                    p.assocs[at].value = v;
                }
            }
        }
        // (the impls of each trait for each type so far: their self types and trait arguments, and parameters)
        let mut self_tys: FxMap<(&str, &str), Vec<usize>> = FxMap::default();
        // 3. Functions
        // (each trait impl's own associated types, by its header: `Self::X` in its methods' signatures is its own X, with
        // the impl's parameters as they are, also when X names a parameter only the trait's arguments bind (`impl<E>
        // Structure<E> for Slice { type FfiPtr = (*const E::FfiUnit, usize); }`, which no lookup by Slice can fill))
        let mut own_assocs: FxMap<usize, Vec<(String, Vec<String>, TySyn)>> = FxMap::default();
        for (ii, sm) in sums.iter().enumerate() {
            if let Some(crate::lexer::ItemKind::ImplMember(h)) = items.get(ii).map(|it| it.kind) {
                if sums.get(h as usize).is_some_and(|x| x.impl_trait.is_some()) {
                    for a in &sm.assocs {
                        own_assocs.entry(h as usize).or_default().push((a.name.clone(), a.generics.clone(), a.value.clone()));
                    }
                }
            }
        }
        // (the program's fns' parameters that must be sized, classified once the traits' supertraits are known)
        let mut pending_sized: Vec<(u32, usize, Vec<(String, Vec<String>)>, Vec<String>)> = vec![];
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            let first = p.names.len() as u32;
            let cnames: Vec<String> = sm.closures.iter().map(|c| format!("{}::call", c.name)).collect();
            let fns = sm
                .fns
                .iter()
                .map(|(n, s)| (n, s))
                .chain(cnames.iter().zip(sm.closures.iter().map(|c| &c.sig)));
            for (name, ss) in fns {
                let mut sym_suffix = String::new();
                let name = &if let Some(&prev) = p.index.get(name) {
                    if ss.is_extern && p.sigs[prev as usize].is_extern {
                        // (the same foreign fn declared in two `extern` blocks: one more name for the same symbol)
                        format!("{name}#extern{}", p.names.len())
                    } else {
                        // (inherent methods with `self` in impls for two instances of one type (either's
                        // `impl<L, R> Either<&L, &R> { fn cloned(self) }` and `.. Either<&mut L, &mut R>`, or `impl W<i32>` and
                        // `impl W<u8>`): overloads, told apart by the receiver as trait impls' are. The same self type twice is
                        // E0592 still. A function without `self` (simdutf8's `from_32_cut_off_leading` of
                        // `SimdU8Value<__m256i>` and of `SimdU8Value<__m128i>`) is told apart by the self type its path
                        // names: self_ty_groups)
                        // (against every inherent method of the name so far, not the first alone: a third `W<u8>::f` is
                        // E0592 too. Self types overlap when one could be the other, `Cake<T>` and `Cake<Box<U>>` (tests/ui
                        // coherence-overlap-issue-23516-inherent): E0592 as well)
                        let mine = ss.self_ty.as_ref().and_then(|t| {
                            let gn: Vec<&str> = ss.generics.iter().map(|g| g.0.as_str()).collect();
                            p.resolve_g(t, &gn).ok()
                        });
                        let group: Vec<u32> = p.overloads.get(name.as_str()).cloned().unwrap_or_else(|| vec![prev]);
                        let block_of = |c: u32| match p.item_of.get(c as usize).copied().flatten().map(|k| items[k as usize].kind) {
                            Some(crate::lexer::ItemKind::ImplMember(h)) => Some(h),
                            _ => None,
                        };
                        let own_block = match it.kind {
                            crate::lexer::ItemKind::ImplMember(h) => Some(h),
                            _ => None,
                        };
                        // (the inherent method of the name this one cannot stand beside: E0592, at that one's line when it is
                        // in another impl block)
                        let clash = ss.impl_trait.is_none().then(|| {
                            group.iter().copied().find(|&c| {
                                let cs = &p.sigs[c as usize];
                                if cs.impl_trait.is_some() {
                                    return false;
                                }
                                let apart = block_of(c).is_some()
                                    && block_of(c) != own_block
                                    && matches!((mine, cs.self_ty), (Some(a), Some(b)) if !p.self_tys_overlap(a, b));
                                !apart
                            })
                        });
                        if let Some(Some(c)) = clash {
                            let line = match p.item_of.get(c as usize).copied().flatten().map(|k| &items[k as usize]) {
                                Some(pit) if block_of(c).is_some() && block_of(c) != own_block => pit.line,
                                _ => it.line,
                            };
                            return Err(format!("{line}:1: duplicate definition of `{name}`"));
                        }
                        if group.iter().any(|&c| p.sigs[c as usize].self_ty != mine) {
                            p.self_ty_groups.insert(name.clone());
                        }
                        // (of trait impls' fns, the trait's arguments of each impl: `<Z0 as ToInt<i16>>::to_int()` chooses
                        // by them (typenum's `impl ToInt<i8> for Z0` beside `ToInt<i16>`'s))
                        if ss.impl_trait.is_some() {
                            let targs = |p: &Program, k: usize, gens: &[(String, Vec<String>)]| {
                                let gn: Vec<&str> = gens.iter().map(|g| g.0.as_str()).collect();
                                impl_trait_of(p, items, sums, k, &gn).1.filter(|a| !a.is_empty())
                            };
                            if !p.fn_targs.contains_key(&prev) && p.sigs[prev as usize].impl_trait.is_some() {
                                if let Some(a) = p.item_of.get(prev as usize).copied().flatten().and_then(|k| targs(&p, k as usize, &p.sigs[prev as usize].generics)) {
                                    p.fn_targs.insert(prev, a);
                                }
                            }
                            if let Some(a) = targs(&p, ii, &ss.generics) {
                                p.fn_targs.insert(p.names.len() as u32, a);
                            }
                        }
                        let k = p.overloads.entry(name.clone()).or_insert_with(|| vec![prev]);
                        k.push(p.names.len() as u32);
                        sym_suffix = format!("#{}", k.len() - 1);
                        format!("{name}{sym_suffix}")
                    }
                } else {
                    name.clone()
                };
                let gnames: Vec<&str> = ss.generics.iter().map(|g| g.0.as_str()).collect();
                // (`Self::X` of the impl's own X: see own_assocs)
                // (a generic associated type, `type Output<O> = O`: its own parameters after the mark, which the
                // signature's `Self::Output<X>` gives)
                let own: Vec<(String, Vec<String>, TySyn)> = match (it.kind, &ss.self_ty) {
                    (crate::lexer::ItemKind::ImplMember(h), Some(_)) => own_assocs
                        .get(&(h as usize))
                        .into_iter()
                        .flatten()
                        .filter_map(|(n, g, v)| {
                            let k = g.iter().position(|x| x == "#gat").unwrap_or(g.len());
                            let (impl_g, gat_g) = (&g[..k], g.get(k + 1..).unwrap_or(&[]));
                            (impl_g.len() <= gnames.len() && impl_g.iter().zip(&gnames).all(|(a, b)| a == b)).then(|| (n.clone(), gat_g.to_vec(), v.clone()))
                        })
                        .collect(),
                    _ => vec![],
                };
                let ss_own;
                let ss = if own.is_empty() {
                    ss
                } else {
                    let st = ss.self_ty.clone().unwrap();
                    let mut c = ss.clone();
                    c.params = c.params.iter().map(|t| own_assoc_syn(t, &st, &own, 0)).collect();
                    c.ret = own_assoc_syn(&c.ret, &st, &own, 0);
                    ss_own = c;
                    &ss_own
                };
                let mut params = vec![];
                for t in &ss.params {
                    params.push(
                        p.resolve_g(t, &gnames)
                            .map_err(|e| format!("{}:1: {e}", it.line))?,
                    );
                }
                // (an `impl Trait` the body determines is () for now: resolve_opaque fills it in later)
                let nested = match &ss.ret {
                    TySyn::App(n, d) if n == OPAQUE_NESTED => {
                        p.resolve_g(&d[0], &gnames).map_err(|e| format!("{}:1: {e}", it.line))?;
                        true
                    }
                    _ => false,
                };
                // (a trait's declaration without a body: a placeholder, as for `-> impl Tr;`)
                let ret = if nested && !ss.has_body {
                    Ty::Unit
                } else if nested || ss.ret == TySyn::Named(OPAQUE_RET.into()) {
                    p.opaque.push(p.names.len() as u32);
                    Ty::Unit
                } else {
                    p.resolve_g(&ss.ret, &gnames)
                        .map_err(|e| format!("{}:1: {e}", it.line))?
                };
                let self_ty = match &ss.self_ty {
                    Some(t) => Some(
                        p.resolve_g(t, &gnames)
                            .map_err(|e| format!("{}:1: {e}", it.line))?,
                    ),
                    None => None,
                };
                let mut blanket = false;
                let impl_of = match &ss.impl_of {
                    // (`impl Tr for Box<S>`: Box is not a struct; its methods are found by the self type, as for primitives)
                    Some(n) if Ty::prim(n).is_some() || n == "__array" || n == "__slice" || n == "__fnptr" || n == "__ptr" || n == "__never" || n.starts_with("__dyn_") || n == "str" || (n == "Box" && !p.struct_index.contains_key("Box")) => None,
                    // (`impl Vector for __m128i`: a vector type is not a struct either, unless the program's own)
                    Some(n) if crate::simd_sig::vector_name(n).is_some() && !p.struct_index.contains_key(n) => None,
                    Some(n) => match p.struct_index.get(n).copied().or_else(|| {
                        n.strip_prefix("#tuple").and_then(|k| k.parse().ok()).map(|k| p.tuple_base(k))
                    }).as_ref() {
                        Some(&sid) => Some(sid),
                        // A blanket impl (`impl<T: Display> Describe for T`): looked up as a method of any type
                        // (the fns lifted from closures inside it have no self: not looked up as methods)
                        None if ss.generics.iter().any(|g| &g.0 == n) => {
                            blanket = ss.has_self;
                            p.blanket_selfs.insert(n.clone());
                            None
                        }
                        None => {
                            return Err(format!("{}:1: cannot find struct `{n}` for impl", it.line));
                        }
                    },
                    None => None,
                };
                // (the fn `P::from#1` of a same-named trait impl is found by `from` too)
                let last = name.rsplit("::").next().unwrap().split('#').next().unwrap().to_string();
                // (`io::stdin()` / `mem::swap(..)` are the prelude's `__core_*`: make them findable by the name's tail)
                if let Some(short) = last.strip_prefix("__core_") {
                    let short = short.to_string();
                    p.add_to_group(short);
                }
                p.add_to_group(last);
                if blanket {
                    let m = name.rsplit("::").next().unwrap().to_string();
                    p.blanket.entry(m).or_insert(p.names.len() as u32);
                }
                p.index.insert(name.clone(), p.names.len() as u32);
                p.names.push(name.clone());
                p.item_of.push((!ss.is_extern).then_some(ii as u32));
                if !ss.preds.is_empty() {
                    p.fn_preds.insert(p.sigs.len() as u32, ss.preds.clone());
                }
                // (closure `call`s take their parent's parameters: not their own)
                if ii < user && !ss.generics.is_empty() && !cnames.contains(name) {
                    pending_sized.push((p.sigs.len() as u32, ii, ss.generics.clone(), ss.own_unsized.clone()));
                }
                p.sigs.push(FnSig {
                    params,
                    ret,
                    symbol: format!("{}{sym_suffix}", ss.symbol),
                    is_extern: ss.is_extern,
                    is_main: ss.is_main,
                    impl_of,
                    has_self: ss.has_self,
                    generics: ss.generics.clone(),
                    trait_of: ss.trait_of.clone(),
                    has_body: ss.has_body,
                    impl_trait: ss.impl_trait.clone(),
                    self_ty,
                    export: ss.export,
                    panic_handler: ss.panic_handler,
                    ret_from: ss.ret_from,
                    is_unsafe: ss.is_unsafe,
                    regions: ss.regions.as_deref().map(|r| {
                        let mut r = r.clone();
                        // (what bounds on types imply of their parts, where no path the part is in is a type alias
                        // (`Erased<'a>: 'static` with `type Erased<'a> = ();` says nothing of 'a))
                        if !r.derived.is_empty() || !r.derived_ty.is_empty() {
                            // (a path names only types with lifetime arguments, so never a type parameter, even one
                            // that shadows a type alias's name)
                            let alias = |paths: &str| paths.split(',').any(|n| p.aliases.contains_key(n));
                            for (paths, a, c) in std::mem::take(&mut r.derived) {
                                if !alias(&paths) && !r.outlives.contains(&(a, c)) {
                                    r.outlives.push((a, c));
                                }
                            }
                            for (paths, t, c) in std::mem::take(&mut r.derived_ty) {
                                if !alias(&paths) && !r.ty_outlives.contains(&(t.clone(), c)) {
                                    r.ty_outlives.push((t, c));
                                }
                            }
                        }
                        std::sync::Arc::new(r)
                    }),
                });
                if ss.panic_handler {
                    p.panic_handler = Some(p.names.len() as u32 - 1);
                }
                if let Some(tr) = &ss.trait_of {
                    p.traits
                        .entry(tr.clone())
                        .or_default()
                        .push(p.names.len() as u32 - 1);
                }
            }
            for (k, c) in sm.closures.iter().enumerate() {
                let fid = first + sm.fns.len() as u32 + k as u32;
                let sid = p.struct_index[&c.name];
                let gnames: Vec<&str> = c.generics.iter().map(|g| g.as_str()).collect();
                let ret_ann = match &c.ret_ann {
                    Some(t) => Some(
                        p.resolve_g(t, &gnames)
                            .map_err(|e| format!("{}:1: {e}", it.line))?,
                    ),
                    None => None,
                };
                p.closures.insert(
                    fid,
                    ClosureDef {
                        parent: first,
                        k: k as u32,
                        sid,
                        captures: c.fields.iter().map(|f| f.0.clone()).collect(),
                        // (from edition 2021, a shared reference used only through a deref is held by value)
                        by_move: c.by_move || (p.edition >= 2021 && !c.moved.is_empty() && c.moved.iter().zip(&c.deref).all(|(m, d)| *m || *d)),
                        moved: if p.edition >= 2021 { c.moved.iter().zip(c.deref.iter().chain(std::iter::repeat(&false))).map(|(m, d)| *m || *d).collect() } else { c.moved.clone() },
                        n_parent: c.n_parent as usize,
                        ret_ann,
                        fake_reads: c.fake_reads.clone(),
                        mutated: c.mutated.clone(),
                    },
                );
                p.closure_by.insert((first, k as u32), fid);
                p.closure_of_struct.insert(sid, fid);
            }
            if let Some(tr) = &sm.trait_decl {
                p.traits.entry(tr.clone()).or_default();
                if ii < user {
                    p.user_traits.insert(tr.clone());
                }
                if let Some((_, sup)) = sm.trait_generics.iter().find(|g| g.0 == "#supers") {
                    p.trait_supers.insert(tr.clone(), sup.clone());
                }
                if let Some((_, ms)) = sm.trait_generics.iter().find(|g| g.0 == "#marker_supers") {
                    p.trait_marker_supers.insert(tr.clone(), ms.clone());
                }
                if let Some((_, so)) = sm.trait_generics.iter().find(|g| g.0 == crate::types::SELF_OUTLIVES) {
                    p.trait_self_outlives.insert(tr.clone(), so.clone());
                }
                p.trait_ngen.insert(tr.clone(), 1 + sm.trait_generics.iter().filter(|g| !g.0.starts_with('#')).count());
            }
        }
        // (own parameters follow the impl's (impl_generics) or the trait's (`Self` and the trait's); one declared
        // `?Sized` whose other bounds imply Sized is sized: `T: Clone + ?Sized`, `T: Dup + ?Sized` with `trait Dup: Clone`)
        for (fid, ii, generics, own_unsized) in std::mem::take(&mut pending_sized) {
            let owner = match items[ii].kind {
                crate::lexer::ItemKind::ImplMember(h) => sums.get(h as usize).map_or(0, |h| h.impl_generics.len()),
                crate::lexer::ItemKind::TraitMember(h) => sums.get(h as usize).map_or(0, |h| 1 + h.trait_generics.iter().filter(|g| !g.0.starts_with('#')).count()),
                _ => 0,
            };
            let sized_by = |bs: &[String]| {
                bs.iter().any(|b| {
                    matches!(b.split('<').next().unwrap_or(b).trim().rsplit("::").next().unwrap_or(""), "Sized" | "Clone" | "Copy" | "Default" | "From" | "Into" | "TryFrom" | "TryInto" | "FromStr" | "FromIterator")
                        || p.bound_implies(b, &["Sized", "Clone", "Copy", "Default"], false, 0)
                })
            };
            let un: Vec<usize> = own_unsized.iter().filter_map(|n| generics.iter().rposition(|g| &g.0 == n)).filter(|&k| !sized_by(&generics[k].1)).collect();
            // (a const parameter is no type: `__const` among its bounds)
            let sz: Vec<usize> = (owner.min(generics.len())..generics.len()).filter(|k| !un.contains(k) && !generics[*k].1.iter().any(|b| b.starts_with(crate::types::CONST_GENERIC))).collect();
            if !sz.is_empty() {
                p.fn_sized_gens.insert(fid, sz);
            }
            if !un.is_empty() {
                p.fn_unsized_gens.insert(fid, un);
            }
        }
        p.peq = (0..p.names.len() as u32)
            .filter(|&i| {
                let n = &p.names[i as usize];
                n.ends_with("::eq") && n.contains("::<PartialEq<")
            })
            .collect();
        p.pord = (0..p.names.len() as u32)
            .filter(|&i| {
                let n = &p.names[i as usize];
                n.ends_with("::partial_cmp") && n.contains("::<PartialOrd<")
            })
            .collect();
        // 4. Trait impls (from the summaries of the impl headers)
        // (traits with an impl for `&T` of a type parameter T)
        // (the traits of `impl<T> Tr for &T` / `&mut T` (whether mut), and of `impl<T> Tr for T`)
        let mut ref_blankets: HashSet<(String, bool)> = Default::default();
        let mut bare_blankets: HashSet<String> = Default::default();
        // (impls for Box: (trait, line), checked against blanket impls of the same trait after the loop)
        let mut box_impls: Vec<(String, usize)> = vec![];
        let mut ref_box_impls: Vec<(String, usize, Ty)> = vec![];
        // (unbounded blanket impls (trait, line), and every impl of the program's (trait, line))
        let mut unbounded_blankets: Vec<(String, usize)> = vec![];
        let mut other_impls: Vec<(String, usize)> = vec![];
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            let (Some(sn), Some(tr)) = (&sm.impl_self, &sm.impl_trait) else {
                continue;
            };
            // (an impl for a type, not another blanket one: whose where clauses the summary may not keep)
            if ii < user && !sm.impl_generics.iter().any(|g| g.0 == *sn) {
                other_impls.push((tr.clone(), it.line));
            }
            // (comparison impls with a right-hand type and impls for arrays / slices are not counted: `==` picks the fn)
            // (an impl for a trait object, `impl<A, R> Tr for dyn Fn(A) -> R`: by the object's trait's name; one with no
            // parameters by the object as written)
            if sn.starts_with("__dyn_") {
                if let Some(TySyn::Dyn(d)) = &sm.impl_self_ty {
                    // (whether its object writes a lifetime other than 'static: the header's regions say, as the
                    // text the summary keeps of the object does not)
                    let any_lt = !sm.hdr_regions.as_deref().is_some_and(|r| r.self_slots.first() == Some(&crate::types::R_STATIC));
                    if sm.impl_generics.is_empty() {
                        p.dyn_impls_exact.insert((tr.clone(), dyn_first(d)));
                        if any_lt {
                            p.dyn_impls_lt.insert((tr.clone(), dyn_first(d)));
                        }
                    } else {
                        let first = d.split('+').next().unwrap_or(d).trim();
                        let name = first.split(|c: char| c == '(' || c == '<').next().unwrap_or(first).trim();
                        p.dyn_impls.insert((tr.clone(), name.rsplit("::").next().unwrap_or(name).to_string()));
                        if any_lt {
                            p.dyn_impls_lt.insert((tr.clone(), name.rsplit("::").next().unwrap_or(name).to_string()));
                        }
                    }
                }
            }
            if tr.starts_with("PartialEq<") || tr.starts_with("PartialOrd<") || sn == "__array" || sn == "__slice" || sn.starts_with("__dyn_") {
                // (`impl BufRead for &[u8]`: remembered for whether bounds hold)
                if sn == "__slice" && matches!(sm.impl_self_ty, Some(TySyn::Ref(false, _))) && !tr.contains('<') {
                    p.ref_slice_traits.insert(tr.clone());
                }
                // (`impl<'a, T> Tr for &'a [T; 3]` of a trait of the program's: the self type with the impl's
                // parameters, matched against a type for bounds)
                if (sn == "__array" || sn == "__slice") && matches!(sm.impl_self_ty, Some(TySyn::Ref(..))) && !tr.contains('<') && p.traits.contains_key(tr) {
                    let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                    if let Some(t) = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &gn).ok()) {
                        p.ref_impl_pats.push((t, tr.clone(), gn.len(), sm.impl_generics.iter().map(|g| g.1.clone()).collect()));
                    }
                }
                // (`impl Ext<u8> for dyn Obj {}` of a generic trait of the program's: its arguments, for a default
                // method's parameters through the object (`x.value()` with `fn value(&self) -> Option<T>`))
                if sn.starts_with("__dyn_") && !sm.impl_trait_args.is_empty() && p.traits.contains_key(tr) {
                    let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                    let st = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &gn).ok());
                    let args: Option<Vec<Ty>> = sm.impl_trait_args.iter().map(|a| p.resolve_g(a, &gn).ok()).collect();
                    if let (Some(st), Some(args)) = (st, args) {
                        if sm.impl_trait_lts {
                            p.impl_targs_lts.insert((tr.clone(), st));
                        }
                        p.impl_targs.entry(tr.clone()).or_default().push((st, args, gn.len()));
                        p.impl_targ_bounds.entry(tr.clone()).or_default().push(sm.impl_generics.iter().map(|g| g.1.clone()).collect());
                    }
                }
                continue;
            }
            // (marker traits the compiler knows without a declaration: `unsafe impl Send for P {}`)
            if matches!(tr.as_str(), "Send" | "Sync" | "UnwindSafe" | "RefUnwindSafe" | "Unpin") {
                // (the self type must still name known types: `unsafe impl Send for Foo<T> {}` without `<T>`)
                if let Some(st) = &sm.impl_self_ty {
                    let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                    p.resolve_g(st, &gn).map_err(|e| format!("{}:1: {e}", it.line))?;
                }
                if let Some(&sid) = p.struct_index.get(sn) {
                    p.impls.insert((sid, tr.clone()));
                }
                continue;
            }
            if !p.traits.contains_key(tr) {
                return Err(format!("{}:1: cannot find trait `{tr}`", it.line));
            }
            // (the trait's arguments name types: `impl Generic<Type> for S` without a `Type`)
            {
                let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                for a in sm.impl_trait_args.iter().chain(sm.impl_self_ty.as_ref()) {
                    if let Err(e) = p.resolve_g(a, &gn) {
                        if e.starts_with("cannot find type") {
                            return Err(format!("{}:1: {e}", it.line));
                        }
                    }
                }
                if !sm.impl_trait_args.is_empty() {
                    let st = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &gn).ok());
                    let args: Option<Vec<Ty>> = sm.impl_trait_args.iter().map(|a| p.resolve_g(a, &gn).ok()).collect();
                    if let (Some(st), Some(args)) = (st, args) {
                        // (one std's blanket impls give already, E0119: `impl<T, U: Into<T>> TryFrom<U> for T` with the
                        // program's `impl From<U> for T`, `impl<T> AsRef<T> for Box<T>`. The reflexive `From<S> for S` /
                        // `TryFrom<S> for S` below; a self type written with a path is another crate's)
                        let from_too = || {
                            sums.iter().any(|o| o.impl_trait.as_deref() == Some("From") && o.impl_self_ty == sm.impl_self_ty && o.impl_trait_args == sm.impl_trait_args)
                        };
                        let arg0 = sm.impl_trait_args.first();
                        let boxed_syn = matches!((&sm.impl_self_ty, arg0), (Some(TySyn::App(b, xs)), Some(a)) if b == "Box" && xs.len() == 1 && xs[0] == *a);
                        let std_has = !sm.impl_self_path
                            && match (tr.as_str(), args.as_slice()) {
                                ("TryFrom", [a]) => *a != st && from_too(),
                                ("AsRef", [a]) => st == Ty::Box(intern(*a)) && boxed_syn,
                                _ => false,
                            };
                        if std_has && ii < user {
                            return Err(format!("{}:1: conflicting implementations of trait `{tr}<{}>` for type `{}` (E0119)", it.line, p.tn(args[0]), p.tn(st)));
                        }
                        if sm.impl_trait_lts {
                            p.impl_targs_lts.insert((tr.clone(), st));
                        }
                        p.impl_targs.entry(tr.clone()).or_default().push((st, args, gn.len()));
                        p.impl_targ_bounds.entry(tr.clone()).or_default().push(sm.impl_generics.iter().map(|g| g.1.clone()).collect());
                    }
                }
            }
            // (`impl From<S> for S` / `impl TryFrom<S> for S`: std's `impl<T> From<T> for T`, and the TryFrom it gives,
            //  already cover it (E0119))
            // (not `impl From<TokenStream> for proc_macro::TokenStream`: a path names another crate's type, proc-macro2)
            if (tr == "From" || tr == "TryFrom") && !sm.impl_self_path && sm.impl_trait_args.len() == 1 && sm.impl_self_ty.as_ref() == Some(&sm.impl_trait_args[0]) {
                return Err(format!("{}:1: conflicting implementations of trait `{tr}<{sn}>` for type `{sn}` (E0119)", it.line));
            }
            // Trait impls for primitive types (`impl Tr for u32`)
            // (`impl Hash for str`: str is kept as a built-in type too)
            if matches!(sn.as_str(), "__fnptr" | "__ptr" | "__never") {
                // (a std trait for a fn pointer / raw pointer / `!`: none of them is local (E0117))
                const STD: &[&str] = &["Display", "Debug", "Clone", "Copy", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash", "Default", "Drop", "Iterator", "From", "Into", "Add", "Sub", "Mul", "Div", "Deref", "Send", "Sync", "Fn", "FnMut", "FnOnce"];
                if STD.contains(&tr.as_str()) {
                    return Err(format!("{}:1: only traits defined in the current crate can be implemented for arbitrary types (E0117)", it.line));
                }
                let t = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &[]).ok());
                p.fnptr_impls.insert((t, tr.clone()));
                continue;
            }
            // (`impl Tr for &i32` / `impl Tr for &S`: the reference type implements Tr)
            if let Some(TySyn::Ref(..)) = &sm.impl_self_ty {
                if let Some(t) = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &[]).ok()) {
                    p.ref_impls.insert((t, tr.clone()));
                } else {
                    // (one with the impl's parameters, `impl<T> Tr for &ScopedClosure<'_, T>`: its self type as a
                    // pattern, matched against a type for bounds)
                    let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                    if let Some(t) = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &gn).ok()) {
                        if !matches!(t, Ty::Ref(_, Ty::Param(_))) {
                            p.ref_impl_pats.push((t, tr.clone(), gn.len(), sm.impl_generics.iter().map(|g| g.1.clone()).collect()));
                        }
                    }
                }
            }
            // (and a vector type of core::arch, when the program has no type of the name: `impl Vector for __m128i`.
            // `__m128`, `__m128i`, `__m128d` and `v128` are one type here, so impls of a trait for two of them conflict)
            // (not `impl Tr for &__m128i`, which the reference implements: registered above)
            let vec_ty = crate::simd_sig::vector_name(sn).filter(|_| !p.struct_index.contains_key(sn.as_str()) && !matches!(sm.impl_self_ty, Some(TySyn::Ref(..))));
            if let Some(pt) = Ty::prim(sn).or((sn == "str").then_some(Ty::Str)).or((sn == "#tuple0").then_some(Ty::Unit)).or(vec_ty) {
                // (`impl Tr for &mut i32` is for the reference, registered above: not for i32 itself, which a bound
                // asks of (prim_ref_only); the method lookup still finds it from i32 (issue #2))
                if matches!(sm.impl_self_ty, Some(TySyn::Ref(..))) {
                    if !p.prim_impls.contains(&(pt, tr.clone())) {
                        p.prim_ref_only.insert((pt, tr.clone()));
                    }
                } else {
                    p.prim_ref_only.remove(&(pt, tr.clone()));
                }
                let dup = !p.prim_impls.insert((pt, tr.clone()));
                if impl_overlaps(&mut self_tys, sums, user, ii, sn, tr, dup) {
                    return Err(format!(
                        "{}:1: conflicting implementations of trait `{tr}` for `{sn}`",
                        it.line
                    ));
                }
                continue;
            }
            // (`impl Tr for Box<S>`: a trait of that Box type, as for primitives; `impl<T> Tr for Box<T>` has no single type)
            if sn == "Box" && !p.struct_index.contains_key("Box") {
                // (one for a reference to a Box, `impl Tr for &Box<str>`, is a reference's, which a blanket impl of
                // `T` can also cover only when the reference meets its bounds)
                if !matches!(sm.impl_self_ty, Some(TySyn::Ref(..))) {
                    box_impls.push((tr.clone(), it.line as usize));
                }
                if let Some(t) = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &[]).ok()) {
                    p.prim_impls.insert((t, tr.clone()));
                    // (`impl Tr for &Box<str>`: the reference implements Tr, as other references' impls say)
                    if matches!(t, Ty::Ref(..)) {
                        p.ref_impls.insert((t, tr.clone()));
                        ref_box_impls.push((tr.clone(), it.line as usize, t));
                    }
                }
                continue;
            }
            // (`impl<T> Tr for T`, with or without members)
            // (`impl<T: Tr> Tr for &T` / `&mut T`: a blanket impl for the references; a method call whose Self is such a
            // reference takes its methods (or the trait's defaults it leaves out) by ref_param_impls (#144))
            let ref_of_param = matches!(&sm.impl_self_ty, Some(TySyn::Ref(_, i)) if matches!(&**i, TySyn::Named(n) if n == sn))
                && sm.impl_generics.iter().any(|g| g.0 == *sn);
            if ref_of_param {
                // (`impl<T> Foo for T` covers `&U` too; `&T` and `&mut T` are two types)
                let m = matches!(&sm.impl_self_ty, Some(TySyn::Ref(true, _)));
                if bare_blankets.contains(tr) || !ref_blankets.insert((tr.clone(), m)) {
                    return Err(format!("{}:1: conflicting implementations of trait `{tr}` for type `&{}_` (E0119)", it.line, if m { "mut " } else { "" }));
                }
                p.blanket_selfs.insert(sn.clone());
                p.blanket_traits.insert(tr.clone());
                if ii < user {
                    p.ref_param_impls.push((tr.clone(), m));
                    let bs = sm.impl_generics.iter().find(|g| g.0 == *sn).map(|g| g.1.clone()).unwrap_or_default();
                    p.ref_param_bounds.insert((tr.clone(), m), bs);
                }
                continue;
            }
            if sm.impl_generics.iter().any(|g| g.0 == *sn) && matches!(sm.impl_self_ty, Some(TySyn::Named(_))) {
                // (`impl<T> Tr for T` with no bound and no where clause: every other impl of Tr overlaps it, E0119)
                if ii < user && sm.impl_generics.iter().any(|g| g.0 == *sn && g.1.is_empty()) && sm.where_types.is_empty() && sm.impl_trait_args.is_empty() {
                    unbounded_blankets.push((tr.clone(), it.line));
                }
                if ref_blankets.contains(&(tr.clone(), false)) || ref_blankets.contains(&(tr.clone(), true)) {
                    return Err(format!("{}:1: conflicting implementations of trait `{tr}` for type `&_` (E0119)", it.line));
                }
                bare_blankets.insert(tr.clone());
                p.blanket_selfs.insert(sn.clone());
                p.blanket_traits.insert(tr.clone());
                // (one list of bounds per impl: `impl<T: A> Tr<u8> for T` and `impl<T: B> Tr<u16> for T` are two)
                let bs = sm.impl_generics.iter().find(|g| g.0 == *sn).map(|g| g.1.clone()).unwrap_or_default();
                p.blanket_bounds.entry(tr.clone()).or_default().push(bs);
                // (the trait's default methods are methods of every type)
                let ids: Vec<u32> = p.traits.get(tr).cloned().unwrap_or_default();
                for id in ids {
                    let sg = &p.sigs[id as usize];
                    if sg.has_self && sg.has_body {
                        let m = p.names[id as usize].rsplit("::").next().unwrap_or("").to_string();
                        p.blanket.entry(m).or_insert(id);
                    }
                }
                continue;
            }
            let Some(&sid) = p.struct_index.get(sn).or(sn.strip_prefix("#tuple").and_then(|k| k.parse().ok()).map(|k| p.tuple_base(k)).as_ref()) else {
                if p.blanket_selfs.contains(sn) {
                    continue;
                }
                return Err(format!("{}:1: cannot find type `{sn}`", it.line));
            };
            // (an impl for a reference does not count as an impl of the trait: only operators call `__ref_*`)
            if matches!(sm.impl_self_ty, Some(TySyn::Ref(..))) {
                continue;
            }
            // (a generic trait may have an impl per argument: `impl From<&str> for P` and `impl From<String> for P`)
            // (a trait with parameters, `impl Tr<u8> for S` and `impl Tr<u16> for S`, or self types that differ in their
            // arguments, `impl Tr for Vec<i32>` and `impl Tr for Vec<Box<i32>>`)
            let dup = !p.impls.insert((sid, tr.clone()));
            // (an impl for an instance, `impl A for W<u8>`: its self type, for the method lookup to tell W<u16> apart;
            // one it cannot read is for every instance)
            // (an alias of an instance too, `impl A for Alias`; and the bounds of the impl's parameters, `impl<T: Marker>
            // A for W<T>` is not for a W<i32> without `impl Marker for i32`)
            {
                let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                let resolved = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &gn).ok());
                let inst = matches!(resolved, Some(Ty::Struct(s)) if p.structs[s as usize].base.is_some());
                if inst || matches!(sm.impl_self_ty, Some(TySyn::App(..))) {
                    let st = resolved.unwrap_or(Ty::Param(0));
                    let bounds: Vec<(usize, String)> =
                        sm.impl_generics.iter().enumerate().flat_map(|(i, (_, bs))| bs.iter().map(move |b| (i, b.split('<').next().unwrap_or(b).trim().to_string()))).collect();
                    p.impl_selfs.entry((sid, tr.clone())).or_default().push((st, gn.len().max(1), bounds));
                }
            }
            // (all of its lifetimes 'static: `impl Copy for Foo<'static>`, `impl Copy for Foo<&'static u32>` (a path
            // written without lifetimes is no lifetime of its own). One with others (`impl<'b> Copy for Foo<'static,
            // 'b>`) is not followed: the borrow check would ask all of a copy's borrows to be 'static)
            if tr == "Copy" && sm.hdr_regions.as_ref().is_some_and(|r| r.self_slots.contains(&R_STATIC) && r.self_slots.iter().all(|&c| c == R_STATIC || c == R_ELIDED_ADT)) {
                p.static_copy.insert(sid);
            }

            if impl_overlaps(&mut self_tys, sums, user, ii, sn, tr, dup)
                && !matches!(
                    tr.as_str(),
                    "From" | "TryFrom" | "Add" | "Sub" | "Mul" | "Div" | "Rem" | "BitAnd" | "BitOr" | "BitXor" | "Shl" | "Shr"
                        | "AddAssign" | "SubAssign" | "MulAssign" | "DivAssign" | "Extend" | "FromIterator" | "AsRef"
                )
            {
                return Err(format!(
                    "{}:1: conflicting implementations of trait `{tr}` for `{sn}`",
                    it.line
                ));
            }
        }
        // (an unbounded blanket impl and any other impl of its trait in the program)
        for (tr, bl) in &unbounded_blankets {
            if let Some(line) = other_impls.iter().filter(|(t, l)| t == tr && l != bl).map(|(_, l)| *l).min() {
                let at = line.max(*bl);
                return Err(format!("{at}:1: conflicting implementations of trait `{tr}` (E0119)"));
            }
        }
        // (`impl<T> Tr for T` and `impl Tr for Box<U>` overlap: Box<U> is one of the T, E0119)
        // (not one for `&T` / `&mut T`: a Box is no reference)
        if let Some((tr, line)) = box_impls.iter().find(|(tr, _)| bare_blankets.contains(tr)) {
            return Err(format!("{line}:1: conflicting implementations of trait `{tr}` for type `Box<_>`"));
        }
        // (and `impl Tr for &Box<S>` when the reference meets the blanket impl's bounds, or may: `impl<T: Marker> Tr for
        // T` with `impl Marker for &Box<S>`)
        if let Some((tr, line, _)) = ref_box_impls.iter().find(|(tr, _, t)| bare_blankets.contains(tr) && !matches!(p.blanket_certainty(*t, tr), Certainty::No)) {
            return Err(format!("{line}:1: conflicting implementations of trait `{tr}` for type `&Box<_>` (E0119)"));
        }
        // A derive counts as an impl (Clone / Copy and so on: the content is always a bit copy, so no body is needed)
        for sm in sums {
            for (tn, ds) in &sm.derives {
                if let Some(&sid) = p.struct_index.get(tn) {
                    for d in ds {
                        p.impls.insert((sid, d.clone()));
                        p.derived.entry(sid).or_default().push(d.clone());
                        if d == "PartialEq" {
                            p.derived_peq.insert(sid);
                        }
                    }
                }
            }
        }
        // (E0204: `Copy` for a type of the program's with a field that is not: derived, at the type's name; an
        // `impl Copy for S {}`, at its self type. A field of the type's parameter is Copy where the derive bounds the
        // parameter, and where the impl's own parameter for it has a bound or a where clause)
        for (ii, sm) in sums.iter().enumerate().take(user) {
            let not_copy = |p: &Program, sid: u32, args: Option<&[TySyn]>, own_gen: &[(String, Vec<String>)]| {
                let s = &p.structs[sid as usize];
                let fields: Vec<Ty> = if s.is_enum { s.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1)).collect() } else { s.fields.iter().map(|f| f.1).collect() };
                // (std's `PhantomData<T>` is Copy for every T, `ManuallyDrop<T>` when T is: the prelude's are not
                // written so)
                fn copy(p: &Program, t: Ty) -> bool {
                    match t {
                        Ty::Struct(sid) => match p.structs[p.base_of(sid) as usize].name.as_str() {
                            "PhantomData" => true,
                            "ManuallyDrop" => p.structs[sid as usize].args.iter().all(|&a| copy(p, a)),
                            _ => p.implements(t, "Copy"),
                        },
                        _ => p.implements(t, "Copy"),
                    }
                }
                fields.into_iter().any(|t| match t {
                    Ty::Param(k) => args.and_then(|a| a.get(k as usize)).is_some_and(|a| matches!(a, TySyn::Named(n) if own_gen.iter().any(|g| g.0 == *n && g.1.is_empty()))),
                    t => !p.has_param(t) && !copy(p, t),
                })
            };
            let e = |n: &str| format!("{}:1: \u{5}{}\u{5}the trait `Copy` cannot be implemented for this type (E0204)", items[ii].line, n.rsplit("::").next().unwrap_or(n));
            // (E0184: `Copy` for a type with a Drop impl)
            let dtor = |p: &Program, n: &str| p.struct_index.get(n).is_some_and(|&sid| p.impls.contains(&(sid, "Drop".to_string())));
            let e184 = |n: &str| format!("{}:1: \u{5}{}\u{5}the trait `Copy` cannot be implemented for this type; the type has a destructor (E0184)", items[ii].line, n.rsplit("::").next().unwrap_or(n));
            for (tn, ds) in &sm.derives {
                if ds.iter().any(|d| d == "Copy") && p.struct_index.get(tn).is_some_and(|&sid| not_copy(&p, sid, None, &[])) {
                    return Err(e(tn));
                }
                if ds.iter().any(|d| d == "Copy") && dtor(&p, tn) {
                    return Err(e184(tn));
                }
            }
            if let (Some("Copy"), Some(sn)) = (sm.impl_trait.as_deref(), &sm.impl_self) {
                let args = match &sm.impl_self_ty {
                    Some(TySyn::App(_, a)) => Some(a.as_slice()),
                    _ => None,
                };
                let own_gen = if sm.where_types.is_empty() { sm.impl_generics.as_slice() } else { &[] };
                if p.struct_index.get(sn).is_some_and(|&sid| not_copy(&p, sid, args, own_gen)) {
                    return Err(e(sn));
                }
                if dtor(&p, sn) {
                    return Err(e184(sn));
                }
            }
        }
        // Whether the required methods are implemented (codegen makes the bodies of derived ones)
        let derived: HashSet<(u32, String)> = sums
            .iter()
            .flat_map(|sm| sm.derives.iter())
            .filter_map(|(tn, ds)| p.struct_index.get(tn).map(|&sid| (sid, ds)))
            .flat_map(|(sid, ds)| ds.iter().map(move |d| (sid, d.clone())))
            .collect();
        // (E0195 for the methods of an impl of a type that is not a struct (`impl<'a> Foo<'a> for &'a isize`), which
        // the loop below does not reach: the method's own early-bound lifetimes are the trait's)
        {
            for (mi, ms) in p.sigs.iter().enumerate() {
                let (Some(tr), None, true) = (ms.impl_trait.as_deref(), ms.impl_of, ms.has_body) else { continue };
                let Some(ii) = p.item_of[mi] else { continue };
                if ii as usize >= user {
                    continue;
                }
                let m = p.names[mi].rsplit("::").next().unwrap_or("").split('#').next().unwrap_or("");
                // (an impl of a reference names its methods `__ref_m`)
                // (the declaration of the same name first: a method may be named `__ref_m` itself)
                let decl_name = |d: u32| p.names[d as usize].rsplit("::").next().unwrap_or("");
                let ds = p.traits.get(tr).map(|v| v.as_slice()).unwrap_or(&[]);
                let found = ds.iter().find(|&&d| decl_name(d) == m).or_else(|| ds.iter().find(|&&d| m.strip_suffix(decl_name(d)).is_some_and(|pre| pre.starts_with("__") && pre.ends_with('_'))));
                let Some(&d) = found else { continue };
                let m = p.names[d as usize].rsplit("::").next().unwrap_or(m);
                let ds = &p.sigs[d as usize];
                if let Some(e) = early_mismatch(ds, ms, m) {
                    return Err(format!("{}:1: {e}", items[ii as usize].line));
                }
                if let Some(at) = sig_incompatible(&p, ds, ms, tr) {
                    return Err(format!("{}:1: \u{6}{at}\u{6}method `{m}` has an incompatible type for trait `{tr}` (E0053)", items[ii as usize].line));
                }
            }
        }
        for (sid, tr) in &p.impls {
            if derived.contains(&(*sid, tr.clone())) {
                continue;
            }
            let Some(decls) = p.traits.get(tr) else {
                continue;
            }; // derived built-in traits
            for &d in decls {
                let m = p.names[d as usize].rsplit("::").next().unwrap();
                // (Display / Debug methods are `S::<Display>::fmt`)
                let found = if matches!(tr.as_str(), "Display" | "Debug") {
                    p.index
                        .contains_key(&format!("{}::<{tr}>::{m}", p.structs[*sid as usize].name))
                } else {
                    p.method(*sid, m).is_some()
                };
                // (the impl's method against the trait's: the number of parameters (E0050) and the kind of receiver (E0053))
                if !matches!(tr.as_str(), "Display" | "Debug") {
                    if let Some(mi) = p.method(*sid, m).filter(|&mi| p.sigs[mi as usize].impl_trait.as_deref() == Some(tr.as_str()) && mi != d) {
                        let (ds, ms) = (&p.sigs[d as usize], &p.sigs[mi as usize]);
                        let line = p.item_of[mi as usize].map_or(String::new(), |ii| format!("{}:1: ", items[ii as usize].line));
                        // (E0185 / E0186: a receiver on one side only)
                        if ds.has_self != ms.has_self {
                            return Err(receiver_mismatch(&line, m, if ms.has_self { ms } else { ds }, ms.has_self));
                        }
                        if ds.params.len() != ms.params.len() && ds.has_self == ms.has_self {
                            return Err(format!(
                                "{line}method `{m}` has {} parameters but the declaration in trait `{tr}::{m}` has {} (E0050)",
                                ms.params.len(),
                                ds.params.len()
                            ));
                        }
                        let kind = |prog: &Program, t: Ty| match t {
                            Ty::Ref(false, _) => 1,
                            Ty::Ref(true, _) => 2,
                            Ty::Box(_) => 3,
                            Ty::Struct(x) if prog.structs[prog.base_of(x) as usize].name == "Pin" => 4,
                            _ => 0,
                        };
                        // (at the receiver, or the part sig_incompatible names)
                        let at = (ds.has_self && ms.has_self && kind(&p, ds.params[0]) != kind(&p, ms.params[0])).then(|| "p0".to_string());
                        if let Some(at) = at.or_else(|| sig_incompatible(&p, ds, ms, tr)) {
                            return Err(format!("{line}\u{6}{at}\u{6}method `{m}` has an incompatible type for trait `{tr}` (E0053)"));
                        }
                        // (E0195: the method's own lifetimes that bounds make early-bound, and how many bounds each has,
                        // are the trait's: `fn bar<'a, 'b: 'a>` in the trait, `fn bar<'a, 'b>` in the impl)
                        if let Some(e) = early_mismatch(ds, ms, m) {
                            return Err(format!("{line}{e}"));
                        }
                        // (a parameter's reference the trait leaves to the caller (elided, or the method's own lifetime)
                        // that the impl ties to one of its header's (`fn next(&'a mut self)`): less general, E0308)
                        if let Some(mr) = ms.regions.as_deref().filter(|r| r.outer > 0) {
                            let dr = ds.regions.as_deref();
                            for (i, (&dt, &mt)) in ds.params.iter().zip(&ms.params).enumerate() {
                                if !(matches!(dt, Ty::Ref(..)) && matches!(mt, Ty::Ref(..))) {
                                    continue;
                                }
                                let dc = dr.and_then(|r| r.slots.get(i)).and_then(|s| s.first().copied()).unwrap_or(crate::types::R_ELIDED);
                                let free = dc == crate::types::R_ELIDED || dr.is_some_and(|r| dc >= r.outer && (dc as usize) < r.lts.len());
                                if free && mr.slots.get(i).and_then(|s| s.first()).is_some_and(|&c| c < mr.outer) {
                                    return Err(format!("{line}method not compatible with trait: lifetime mismatch in `{m}` of `{tr}` (E0308)"));
                                }
                            }
                        }
                    }
                }
                if !p.sigs[d as usize].has_body && !found {
                    let sn = &p.structs[*sid as usize].name;
                    // (the line of the impl block)
                    let line = sums
                        .iter()
                        .position(|sm| sm.impl_trait.as_deref() == Some(tr.as_str()) && sm.impl_self.as_deref() == Some(sn.as_str()))
                        .map_or(String::new(), |ii| format!("{}:1: ", items[ii].line));
                    return Err(format!("{line}not all trait items implemented, missing: `{m}` in `impl {tr} for {sn}`"));
                }
            }
        }
        // (E0407: a method of a trait impl that the trait does not declare; for the program's own traits)
        let user_traits: FxSet<&str> = sums[..user].iter().filter_map(|sm| sm.trait_decl.as_deref()).collect();
        if !user_traits.is_empty() {
            for (fi, s) in p.sigs.iter().enumerate() {
                let Some(tr) = s.impl_trait.as_deref() else { continue };
                // (`impl Tr<u8> for S`: the trait's own name)
                let tr = tr.split('<').next().unwrap_or(tr).trim();
                let Some(ii) = p.item_of[fi] else { continue };
                if ii as usize >= user || !user_traits.contains(tr) {
                    continue;
                }
                // (the internal names of overloads `get#1` and of impls for references `__ref_foo`: the method's own)
                let m = p.names[fi].rsplit("::").next().unwrap_or("");
                let m = m.split('#').next().unwrap_or(m);
                // (`__ref_foo` is the generated name of an impl for a reference; a method the trait declares with that
                // name matches as written)
                let mut bare = m;
                while let Some(b) = bare.strip_prefix("__ref_") {
                    bare = b;
                }
                let decls = p.traits.get(tr).map(|v| v.as_slice()).unwrap_or(&[]);
                let declared = |n: &str| decls.iter().any(|&d| p.names[d as usize].rsplit("::").next() == Some(n));
                if declared(m) || declared(bare) {
                    continue;
                }
                let m = bare;
                {
                    return Err(format!("{}:1: method `{m}` is not a member of trait `{tr}` (E0407)", items[ii as usize].line));
                }
            }
        }
        // (E0391: a trait that is its own supertrait, through any number of them; reported at the first trait of the
        // cycle in the source. A cycle through a name declared twice (in two modules, flattened to one name) is left
        // out: counted only when a cycle is found)
        if !user_traits.is_empty() && !p.trait_supers.is_empty() {
            for (ii, sm) in sums[..user].iter().enumerate() {
                let Some(tr) = sm.trait_decl.as_deref() else { continue };
                let Some(sup) = p.trait_supers.get(tr) else { continue };
                let mut seen: FxSet<&str> = FxSet::default();
                let mut todo: Vec<&str> = sup.iter().map(|s| s.as_str()).collect();
                let mut cycle = false;
                while let Some(t) = todo.pop() {
                    if t == tr {
                        cycle = true;
                        break;
                    }
                    if user_traits.contains(t) && seen.insert(t) {
                        todo.extend(p.trait_supers.get(t).into_iter().flatten().map(|s| s.as_str()));
                    }
                }
                let once = |t: &str| sums[..user].iter().filter(|s| s.trait_decl.as_deref() == Some(t)).count() == 1;
                if cycle && once(tr) && seen.iter().all(|t| once(t)) {
                    return Err(format!("{}:1: cycle detected when computing the super predicates of `{tr}` (E0391)", items[ii].line));
                }
            }
        }
        // (whether a type with no parameters implements a trait of the program's, for the checks of impls below: by
        // implements, or an impl for exactly this type written out (`impl A for [u8; 1]`). A type with a trait
        // object in it is taken to: `dyn T2` and `dyn T2 + 'static` are told apart by their text here, where rustc
        // sees one type with two lifetimes. So is a Box, an array or a slice: implements does not know the program's
        // impls for them)
        let holds = |p: &Program, t: Ty, name: &str| {
            p.has_param(t)
                || p.opaque_to_impls(t)
                || p.implements(t, name)
                || sums[..user].iter().any(|o| {
                    o.impl_generics.is_empty()
                        && o.impl_trait.as_deref().is_some_and(|x| x.split('<').next().unwrap_or(x).trim() == name)
                        && o.impl_self_ty.as_ref().and_then(|x| p.resolve_g(x, &[]).ok()) == Some(t)
                })
        };
        // (an impl of a trait with supertraits needs the self type to implement them: `trait B: A` and `impl B for C`
        // without `impl A for C` (E0277). For impls of a type with no parameters, and supertraits of the program's: a
        // nested goal of the impl, as the trait solver proves it (issue #2))
        if !user_traits.is_empty() && !p.trait_supers.is_empty() {
            for (ii, sm) in sums[..user].iter().enumerate() {
                let (Some(tr), Some(st)) = (sm.impl_trait.as_deref(), sm.impl_self_ty.as_ref()) else { continue };
                if !sm.impl_generics.is_empty() {
                    continue;
                }
                let Some(sup) = p.trait_supers.get(tr) else { continue };
                let Ok(t) = p.resolve_g(st, &[]) else { continue };
                for s in sup {
                    let name = s.split('<').next().unwrap_or(s).trim();
                    if s.contains('<') || !user_traits.contains(name) || holds(&p, t, name) {
                        continue;
                    }
                    return Err(format!("{}:1: the trait bound `{}: {name}` is not satisfied (E0277)", items[ii].line, p.tn(t)));
                }
            }
        }
        // (an impl's associated type meets the bounds its trait declares for it: `trait Tr { type A: Bar; }` and
        // `impl Tr for u8 { type A = u16; }` without `impl Bar for u16` (E0277). For impls with no parameters, and
        // bounds of traits of the program's, as for supertraits: the item bounds the trait solver proves, issue #2)
        if !user_traits.is_empty() && !p.assoc_bounds.is_empty() {
            for (ii, sm) in sums[..user].iter().enumerate() {
                let crate::lexer::ItemKind::ImplMember(h) = items[ii].kind else { continue };
                let imp = &sums[h as usize];
                let Some(tr) = imp.impl_trait.as_deref() else { continue };
                let tr = tr.split('<').next().unwrap_or(tr).trim();
                if !imp.impl_generics.is_empty() || !user_traits.contains(tr) {
                    continue;
                }
                for a in &sm.assocs {
                    let Some(bs) = p.assoc_bounds.get(&(tr.to_string(), a.name.clone())) else { continue };
                    if !a.generics.is_empty() {
                        continue;
                    }
                    let Ok(v) = p.resolve_g(&a.value, &[]) else { continue };
                    for bnd in bs {
                        let name = bnd.split('<').next().unwrap_or(bnd).trim();
                        if bnd.contains('<') || !user_traits.contains(name) || holds(&p, v, name) {
                            continue;
                        }
                        return Err(format!("{}:1: the trait bound `{}: {name}` is not satisfied (E0277)", items[ii].line, p.tn(v)));
                    }
                }
            }
        }
        // (a free constant of an uninhabited type (`!`, an enum with no variants) has no value to evaluate to; an
        // associated one is evaluated only where it is used)
        for c in p.consts.iter().filter(|c| c.stat.is_none() && (c.item as usize) < user && !c.name.contains("::")) {
            let empty = match c.ty {
                Ty::Never => true,
                Ty::Struct(sid) => p.structs[sid as usize].is_enum && p.structs[sid as usize].variants.is_empty(),
                _ => false,
            };
            if empty {
                // (`const C: ! = panic!();`: the panic is what its evaluation reaches)
                let why = if c.expr.trim_start().starts_with("panic") {
                    "evaluation panicked: explicit panic".to_string()
                } else {
                    format!("evaluation of constant value failed: attempted to instantiate uninhabited type `{}`", p.tn(c.ty))
                };
                return Err(format!("{}:1: {why} (E0080)", items[c.item as usize].line));
            }
        }
        let header = |ii: usize| match items.get(ii).map(|it| it.kind) {
            Some(crate::lexer::ItemKind::ImplMember(h)) => Some(h as usize),
            _ => None,
        };
        p.tdb = crate::traits::TraitDb::build(&p, sums, user, &header);
        // (a static's type is well-formed: the bounds of its struct's parameters hold, `static S: W<u8>` with
        // `struct W<T: Tr>` and no `impl Tr for u8` (E0277))
        for c in p.consts.iter().filter(|c| c.stat.is_some()) {
            let Ty::Struct(sid) = c.ty else { continue };
            let sd = &p.structs[sid as usize];
            let Some(base) = sd.base.filter(|_| !sd.symbolic) else { continue };
            let bn = &p.structs[base as usize].name;
            let Some(sm) = sums.iter().find(|sm| !sm.adt_bounds.is_empty() && sm.structs.iter().map(|x| &x.0).chain(sm.enums.iter().map(|x| &x.0)).any(|n| n == bn)) else {
                continue;
            };
            for (pn, trs) in &sm.adt_bounds {
                let Some(&a) = sm.adt_generics.iter().position(|g| g == pn).and_then(|i| sd.args.get(i)) else { continue };
                for tr in trs {
                    // (the program's own traits, for any argument; the prelude's (Eq ..), for the arguments `implements`
                    // judges exactly: scalars and non-generic structs)
                    let own = sums[..user.min(sums.len())].iter().any(|s| s.trait_decl.as_deref() == Some(tr.as_str()));
                    let judged = a.is_int() || a.is_float() || matches!(a, Ty::Bool | Ty::Char | Ty::Unit) || matches!(a, Ty::Struct(x) if p.structs[x as usize].base.is_none() && p.structs[x as usize].generics.is_empty());
                    let prelude = !own && judged && sums.iter().any(|s| s.trait_decl.as_deref() == Some(tr.as_str()));
                    if (own || prelude) && !p.bound_holds(sums, user, a, tr, 0) {
                        // (an extern static: at its own line in the block)
                        let line = items[c.item as usize].line + crate::parse::extern_static(&c.expr).map_or(0, |x| x.0.saturating_sub(1));
                        return Err(format!("{line}:1: the trait bound `{}: {tr}` is not satisfied (E0277)", p.tn(a)));
                    }
                }
            }
        }
        // (`zustc --test`: the harness's entry point is the program's `main`; the crate's own `main` is an ordinary
        //  function, under its own name, with an ordinary symbol)
        if let Some(&e) = p.index.get(TEST_ENTRY) {
            if p.sigs[e as usize].is_main {
                for (i, s) in p.sigs.iter_mut().enumerate() {
                    if i != e as usize && s.is_main {
                        s.is_main = false;
                        s.symbol = format!("_x{}", s.symbol);
                    }
                }
                p.sigs[e as usize].symbol = "main".to_string();
            }
        }
        if need_main && !p.index.contains_key("main") && !p.index.contains_key(TEST_ENTRY) {
            return Err("no `main` function".into());
        }
        p.deref_to_seq = p
            .names
            .iter()
            .enumerate()
            .filter(|(i, n)| n.ends_with("::deref") && matches!(p.sigs[*i].ret, Ty::Ref(_, Ty::Array(..) | Ty::Slice(_) | Ty::Str)))
            .filter_map(|(_, n)| p.struct_index.get(n.trim_end_matches("::deref")).copied())
            .collect();
        p.deref_mut_of = p
            .names
            .iter()
            .filter(|n| n.ends_with("::deref_mut"))
            .filter_map(|n| p.struct_index.get(n.trim_end_matches("::deref_mut")).copied())
            .filter(|&sid| p.struct_item.get(sid as usize).is_some_and(|&i| i < p.prelude_from))
            .collect();
        p.drop_impls = p
            .impls
            .iter()
            .filter(|(_, tr)| tr == "Drop")
            .map(|(sid, _)| *sid)
            .collect();
        p.has_drop = p.index.contains_key(crate::prelude::DROP_GLUE);
        for (i, sg) in p.sigs.iter().enumerate() {
            if let (true, Some(Ty::Ref(true, t))) = (sg.has_self, sg.params.first()) {
                if t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char) {
                    if let Some(n) = p.names.get(i).and_then(|n| n.rsplit("::").next()) {
                        p.prim_mut_methods.insert(n.split('#').next().unwrap_or(n).to_string());
                    }
                }
            }
        }
        p.any_immutable_static = p.consts.iter().any(|c| c.stat == Some(false) && (c.item as usize) < user);
        // (a body, whatever the trait's name: the prelude's `Clone::clone` has none, a program's own `trait Clone` may)
        p.trait_clone_fns = p.blanket.contains_key("clone") || p.traits.values().flatten().any(|&d| p.sigs[d as usize].has_body && p.names.get(d as usize).is_some_and(|n| n.rsplit("::").next() == Some("clone")));
        let mut first_trait: FxMap<&str, &str> = FxMap::default();
        let mut multi: FxSet<String> = FxSet::default();
        for (tr, ds) in &p.traits {
            for &d in ds {
                if !p.item_of.get(d as usize).copied().flatten().is_some_and(|i| i < p.prelude_from) {
                    continue;
                }
                let Some(n) = p.names.get(d as usize).and_then(|n| n.rsplit("::").next()) else { continue };
                match first_trait.get(n) {
                    Some(&t) if t != tr.as_str() => {
                        multi.insert(n.to_string());
                    }
                    Some(_) => {}
                    None => {
                        first_trait.insert(n, tr.as_str());
                    }
                }
            }
        }
        // (for each of those names, the types (a struct's base, a primitive) that implement two of the traits declaring
        // it: only a call on one of these can be E0034)
        if !multi.is_empty() {
            let mut declaring: FxMap<&str, Vec<&str>> = FxMap::default();
            for (tr, ds) in &p.traits {
                for &d in ds {
                    if let Some(n) = p.names.get(d as usize).and_then(|n| n.rsplit("::").next()).filter(|n| multi.contains(*n)) {
                        declaring.entry(n).or_default().push(tr.as_str());
                    }
                }
            }
            let mut amb: FxMap<String, Vec<Ty>> = FxMap::default();
            for (n, trs) in &declaring {
                let mut seen: FxMap<Ty, u32> = FxMap::default();
                for (b, tr) in &p.impls {
                    if trs.contains(&tr.as_str()) {
                        *seen.entry(Ty::Struct(*b)).or_default() += 1;
                    }
                }
                for (t, tr) in &p.prim_impls {
                    if trs.contains(&tr.as_str()) {
                        *seen.entry(*t).or_default() += 1;
                    }
                }
                // (with `impl<T> A for T {}`, a type that implements B: `S.foo()` is A's and B's)
                let ub = trs.iter().filter(|tr| unbounded_blankets.iter().any(|(u, _)| u == *tr)).count() as u32;
                let ts: Vec<Ty> = seen.into_iter().filter(|&(_, c)| c + ub >= 2).map(|(t, _)| t).collect();
                if !ts.is_empty() {
                    amb.insert(n.to_string(), ts);
                }
            }
            p.ambiguous_methods = amb;
        }
        p.multi_trait_methods = multi;
        p.unbounded_blankets = unbounded_blankets.into_iter().map(|(tr, _)| tr).collect();
        Ok(p)
    }

    /// Whether discarding a value of type t needs drop glue (Box / `impl Drop` / types holding them)
    pub fn needs_drop(&self, t: Ty) -> bool {
        if !self.has_drop {
            return false;
        }
        match t {
            Ty::Box(_) => true,
            Ty::Array(e, n) => n > 0 && self.needs_drop(*e),
            // (ManuallyDrop / MaybeUninit never drop what they hold)
            Ty::Struct(sid) if matches!(self.structs[self.base_of(sid) as usize].name.as_str(), "ManuallyDrop" | "MaybeUninit")
                && self.struct_item.get(self.base_of(sid) as usize).is_some_and(|&i| i >= self.prelude_from) =>
            {
                false
            }
            Ty::Struct(sid) => {
                let sd = &self.structs[sid as usize];
                self.drop_impls.contains(&self.base_of(sid))
                    || sd.fields.iter().any(|f| self.needs_drop(f.1))
                    || sd
                        .variants
                        .iter()
                        .any(|v| v.fields.iter().any(|f| self.needs_drop(f.1)))
            }
            _ => false,
        }
    }

    /// Whether type t implements trait tr
    /// whether a generic impl of tr exists for t's kind (fn pointer / raw pointer / `!`): one of its methods' self types
    fn has_kind_impl(&self, t: Ty, tr: &str) -> bool {
        let base = match t {
            Ty::Fn(_) => "__fnptr",
            Ty::Ptr(..) => "__ptr",
            _ => "__never",
        };
        let decls = self.traits.get(tr).map(|v| v.as_slice()).unwrap_or(&[]);
        // (a marker trait has no methods to tell the kinds apart: any generic impl of a kind counts)
        decls.is_empty() || decls.iter().any(|&d| {
            let m = self.names[d as usize].rsplit("::").next().unwrap_or("");
            self.index.contains_key(&format!("{base}::{m}"))
        })
    }

    /// What core / std implement for references, smart pointers and iterables (None: not decided here)
    fn implements_std(&self, t: Ty, tr: &str) -> Option<bool> {
        let base_name = |t: Ty| match t {
            Ty::Struct(sid) => Some(self.structs[self.base_of(sid) as usize].name.as_str()),
            _ => None,
        };
        // (`u16: From<u8>` / `u8: Into<u32>`: std's lossless conversions between primitives, and `T: From<T>`)
        if (tr.starts_with("From") || tr.starts_with("Into")) && (t.is_int() || t.is_float() || matches!(t, Ty::Bool | Ty::Char)) {
            let tr2: String = tr.chars().filter(|c| !c.is_whitespace()).collect();
            let arg = |p: &str| tr2.strip_prefix(p).and_then(|r| r.strip_prefix('<')).and_then(|r| r.strip_suffix('>')).and_then(Ty::prim);
            // (only a conversion std has answers here; the rest goes to the impls, a program's own `From` among them)
            if arg("From").is_some_and(|a| prim_from(a, t)) || arg("Into").is_some_and(|a| prim_from(t, a)) {
                return Some(true);
            }
        }
        match (t, tr) {
            // (`&str: ToString`, `&T: ToString` for T: Display)
            (Ty::Ref(_, i), "ToString") => Some(*i == Ty::Str || self.implements(*i, "Display") || self.implements(*i, "ToString")),
            // (`&mut T` compares, formats and hashes as T does)
            (Ty::Ref(true, i), "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Display" | "Hash") => Some(self.implements(*i, tr)),
            // (`&mut I` is an iterator when I is)
            (Ty::Ref(true, i), "Iterator") => Some(self.implements(*i, "Iterator")),
            // (`&dyn Fn(..)` / `&mut dyn FnMut(..)` call as the trait object does)
            (Ty::Ref(_, Ty::Dyn(_)), _) if parse_fn_bound(tr).is_some() || matches!(tr, "Fn" | "FnMut" | "FnOnce") => Some(self.implements(match t {
                Ty::Ref(_, i) => *i,
                _ => t,
            }, tr)),
            // (Rc / Arc compare, format and hash as their contents)
            (Ty::Struct(sid), "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Display" | "Hash")
                if matches!(base_name(t), Some("Rc" | "Arc")) =>
            {
                Some(self.structs[sid as usize].args.first().is_some_and(|&a| self.implements(a, tr)))
            }
            (Ty::Box(_), "Deref" | "DerefMut") => Some(true),
            // (a derived Copy of a generic type holds when its arguments are Copy, as `#[derive(Copy)]` bounds them:
            // `Option<String>` is not Copy; PhantomData<T> always is)
            (Ty::Struct(sid), "Copy")
                if !self.structs[sid as usize].args.is_empty()
                    && base_name(t) != Some("PhantomData")
                    && !self.closure_of_struct.contains_key(&self.base_of(sid))
                    && !self.is_tuple(sid)
                    && self.impls.contains(&(self.base_of(sid), "Copy".to_string())) =>
            {
                Some(self.structs[sid as usize].args.iter().all(|&a| self.implements(a, "Copy")))
            }
            // (iterables: iterators, `&[T; N]` / `&[T]` / `&Vec<T>`, Option, ranges and the collections with into_iter)
            (_, "IntoIterator") => {
                if self.implements(t, "Iterator") {
                    return Some(true);
                }
                let inner = match t {
                    Ty::Ref(_, i) => *i,
                    t => t,
                };
                match inner {
                    Ty::Array(..) | Ty::Slice(_) => Some(true),
                    Ty::Struct(sid) => Some(
                        self.method(sid, "into_iter").is_some()
                            || self.method(sid, "__ref_into_iter").is_some()
                            || (matches!(t, Ty::Ref(..)) && self.method(sid, "iter").is_some()),
                    ),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Whether t: tr holds, for a static's bound or a field's: false only when the trait solver proves it does not
    /// (crate::traits: `No`). For a trait the program declares, its impls are the clauses; others (std's) are asked
    /// of `implements`
    pub fn bound_holds(&self, sums: &[Summary], user: usize, t: Ty, tr: &str, _depth: u32) -> bool {
        // (the program's own traits: the first `user` summaries are its items, the rest the prelude's, whose traits
        // (Default ..) builtin types implement without an impl to find)
        if matches!(t, Ty::Param(_)) || self.has_param(t) || !sums[..user.min(sums.len())].iter().any(|s| s.trait_decl.as_deref() == Some(tr)) {
            return self.implements(t, tr);
        }
        self.tdb.holds(self, t, tr) != crate::traits::Ans::No
    }

    /// The primitive t implements tr by an impl of the program's (prim_impls), not one for a reference to t only
    fn prim_has(&self, t: Ty, tr: &str) -> bool {
        let k = (t, tr.to_string());
        self.prim_impls.contains(&k) && !self.prim_ref_only.contains(&k)
    }

    /// A `dyn Any` (+ Send / Sync) with no Debug of its own: std's own Debug impl for it writes `Any { .. }` (a
    /// thread's join error, a panic's payload). Only std's Any, the prelude's trait (`__p_Any` where a program defines
    /// an `Any` of its own): a program's `trait Any` has no Debug (E0277)
    pub fn dyn_any_debug(&self, s: &str) -> bool {
        // (std's impls are for `dyn Any`, `dyn Any + Send` and `dyn Any + Send + Sync`: `dyn Any + Sync` has none)
        let (n, send, sync) = dyn_autos(s);
        if sync && !send {
            return false;
        }
        let n = n.trim();
        let last = n.rsplit("::").next().unwrap_or(n);
        matches!(last, "Any" | "__p_Any")
            && self.traits.get(n).or_else(|| self.traits.get(last)).is_some_and(|fs| {
                !fs.is_empty() && fs.iter().all(|&f| self.item_of.get(f as usize).copied().flatten().is_some_and(|i| i >= self.prelude_from))
            })
    }

    pub fn implements(&self, t: Ty, tr: &str) -> bool {
        self.evaluate(t, tr) != Certainty::No
    }

    /// The goal `t: tr`, with how sure the answer is (issue #2). `Maybe` comes from a goal met again while it is
    /// being proved (a cycle), or one past 64 deep (overflow). implements takes `Maybe` to hold, the old answer.
    ///
    /// Only the goals of the program's traits carry `Maybe` (blanket impls' and struct and tuple impls' bounds);
    /// the rest of the answers are written case by case and are `Yes` or `No`. A `Maybe` lost on the way (through
    /// a derived impl's or a `Box`'s arguments) becomes `Yes`, which accepts, as before
    pub fn evaluate(&self, t: Ty, tr: &str) -> Certainty {
        let b = if self.blanket_traits.contains(tr) { self.blanket_certainty(t, tr) } else { Certainty::No };
        if b == Certainty::Yes {
            return b;
        }
        NOTED.with(|n| n.set(None));
        let c = self.implements_concrete(t, tr);
        let noted = NOTED.with(|n| n.take());
        let c = match (c, noted) {
            (false, _) => Certainty::No,
            (true, Some(m @ Certainty::Maybe(_))) => m,
            (true, _) => Certainty::Yes,
        };
        b.or(c)
    }

    /// A blanket impl of tr (`impl<T: Bar> Tr for T`) is for t when t meets its bounds, its where clauses' too: those
    /// of traits of the program's, nested goals of their own (goal); std's are taken to hold (issue #2)
    fn blanket_certainty(&self, t: Ty, tr: &str) -> Certainty {
        let Some(bs) = self.blanket_bounds.get(tr) else { return Certainty::Yes };
        // (a trait object implements its trait's supertraits itself, whatever the blanket impl asks: `dyn Trait:
        // Supertrait` with `trait Trait: Supertrait`)
        if matches!(t, Ty::Dyn(_)) {
            return Certainty::Yes;
        }
        // (some one impl's bounds all hold)
        Certainty::any(bs.iter().map(|bs| {
            Certainty::all(bs.iter().map(|b| {
                let name = b.split('<').next().unwrap_or(b).trim();
                if !self.user_traits.contains(name) { Certainty::Yes } else { self.goal(t, name) }
            }))
        }))
    }

    /// A nested goal `t: tr` of an impl's bounds. One already being proved (a cycle: `impl<T: Bar> Foo for T` with
    /// `impl<T: Foo> Bar for T`), or past 64 goals deep (overflow), is `Maybe`.
    ///
    /// A goal answered without meeting a cycle or the overflow is kept for the rest of the outermost goal (`T2: T1 +
    /// T0`, `T3: T2 + T1`, ... asks T1 of one type again and again). One that did meet either is not: its answer
    /// leaned on a guess. The cache starts empty with each outermost goal, so it never outlives the impls it was
    /// proved by
    fn goal(&self, t: Ty, tr: &str) -> Certainty {
        #[derive(Default)]
        struct Goals {
            stack: Vec<(Ty, String)>,
            /// A goal's answer, and how many goals deep its proof went below it: reused only where the same proof
            /// would stay within 64 (with fewer goals left above it, the proof could have met the overflow)
            done: FxMap<(Ty, String), (Certainty, usize)>,
            guesses: u64,
            /// The deepest the stack went in the proof being made
            peak: usize,
        }
        thread_local! {
            static GOALS: std::cell::RefCell<Goals> = std::cell::RefCell::new(Goals::default());
        }
        let key = (t, tr.to_string());
        let before = GOALS.with(|g| {
            let mut g = g.borrow_mut();
            let d = g.stack.len();
            if d == 0 {
                g.done.clear();
                g.peak = 0;
            }
            // (the overflow first: a goal past 64 deep is a guess, whatever an earlier, shallower proof of it found)
            if d >= 64 {
                g.guesses += 1;
                return Err(Certainty::Maybe(MaybeCause::Overflow));
            }
            if g.stack.contains(&key) {
                g.guesses += 1;
                return Err(Certainty::Maybe(MaybeCause::Cycle));
            }
            if let Some(&(r, h)) = g.done.get(&key) {
                if d + h < 64 {
                    g.peak = g.peak.max(d + h);
                    return Err(r);
                }
            }
            g.stack.push(key.clone());
            let peak = std::mem::replace(&mut g.peak, d + 1);
            Ok((g.guesses, d, peak))
        });
        let (before, d, outer_peak) = match before {
            Ok(x) => x,
            Err(r) => return r,
        };
        let r = self.evaluate(t, tr);
        GOALS.with(|g| {
            let mut g = g.borrow_mut();
            g.stack.pop();
            let h = g.peak - d;
            if g.guesses == before {
                g.done.insert(key, (r, h));
            }
            g.peak = g.peak.max(outer_peak);
        });
        r
    }

    /// An impl of tr for t's struct is for t itself: for an instance (`W<u8>`), an impl whose self type matches it, or
    /// one for every instance (`impl<T> A for W<T>`, `impl A for W`)
    pub fn impl_applies(&self, t: Ty, tr: &str) -> bool {
        self.impl_certainty(t, tr) != Certainty::No
    }

    /// impl_applies, with how sure it is: the bounds of the impl that matches are nested goals
    fn impl_certainty(&self, t: Ty, tr: &str) -> Certainty {
        let Ty::Struct(sid) = t else { return Certainty::Yes };
        let b = self.base_of(sid);
        match self.impl_selfs.get(&(b, tr.to_string())) {
            Some(v) if b != sid => Certainty::any(v.iter().map(|(st, n, bounds)| {
                let mut sol = vec![None; *n];
                if !(self.match_ty(*st, t, &mut sol) && self.refs_agree(*st, t, 0)) {
                    return Certainty::No;
                }
                // (a bound of a trait of the program's holds by its impls; std's are not checked)
                Certainty::all(bounds.iter().map(|(i, b)| match sol.get(*i).copied().flatten() {
                    Some(a) if self.user_traits.contains(b) => self.goal(a, b),
                    _ => Certainty::Yes,
                }))
            })),
            _ => Certainty::Yes,
        }
    }

    /// impl_applies for implements_concrete's answer, noting how sure it is for evaluate (NOTED): the answer is
    /// implements_concrete's own, so nothing runs between this and evaluate reading it but returns
    fn impl_applies_noted(&self, t: Ty, tr: &str) -> bool {
        let c = self.impl_certainty(t, tr);
        NOTED.with(|n| n.set(Some(c)));
        c != Certainty::No
    }

    /// The references and raw pointers in a and b (a pattern with `Param`s, a type) agree in mutability, as match_ty
    /// does not check: `W<&mut i32>` is not `W<&i32>`, nor `W<*mut i32>` `W<*const i32>`, nor `W<&i32>` `W<*const i32>`
    pub fn refs_agree(&self, a: Ty, b: Ty, depth: u32) -> bool {
        match (a, b) {
            (Ty::Ref(m, x), Ty::Ref(n, y)) | (Ty::Ptr(m, x), Ty::Ptr(n, y)) => m == n && self.refs_agree(*x, *y, depth + 1),
            // (match_ty lets a raw pointer pattern take a reference, as a call coerces it; a self type does not)
            (Ty::Ptr(..), Ty::Ref(..)) | (Ty::Ref(..), Ty::Ptr(..)) => false,
            (Ty::Array(x, _), Ty::Array(y, _)) | (Ty::Slice(x), Ty::Slice(y)) | (Ty::Box(x), Ty::Box(y)) => self.refs_agree(*x, *y, depth + 1),
            (Ty::Fn(f), Ty::Fn(g)) if f != g && depth < 16 => {
                f.params.len() == g.params.len() && f.params.iter().zip(&g.params).chain([(&f.ret, &g.ret)]).all(|(&p, &q)| self.refs_agree(p, q, depth + 1))
            }
            (Ty::Struct(x), Ty::Struct(y)) if x != y && depth < 16 => {
                self.structs[x as usize].args.iter().zip(&self.structs[y as usize].args).all(|(&p, &q)| self.refs_agree(p, q, depth + 1))
            }
            _ => true,
        }
    }

    /// implements, not counting a blanket impl of tr (`impl<T: Marker> Other for T`): an impl for t's type itself
    pub fn implements_concrete(&self, t: Ty, tr: &str) -> bool {
        // (a placeholder implements the traits its parameter's bounds name, and the auto traits)
        if let Ty::Struct(sid) = t {
            if let Some(bs) = &self.structs[sid as usize].placeholder {
                let name = |b: &str| b.split('<').next().unwrap_or(b).trim().to_string();
                return matches!(tr, "Sized" | "Send" | "Sync" | "Unpin") || bs.iter().any(|b| b == tr || name(b) == name(tr));
            }
        }
        // (std's Pointer of references, raw pointers and Box, whose address the compiler writes itself; `__p_Pointer`
        // where the program has a Pointer of its own, which this is not)
        let std_pointer = if self.traits.contains_key("__p_Pointer") { tr == "__p_Pointer" } else { tr == "Pointer" };
        if std_pointer && matches!(t, Ty::Ref(..) | Ty::Ptr(..) | Ty::Box(_)) {
            return true;
        }
        if matches!(t, Ty::Ref(..)) && !self.ref_impls.is_empty() && self.ref_impls.contains(&(t, tr.to_string())) {
            return true;
        }
        if matches!(t, Ty::Ref(..)) && self.ref_impl_pat_holds(t, tr) {
            return true;
        }
        if let Some(r) = self.implements_std(t, tr) {
            return r;
        }
        // (every type is Sized but str, slices and dyn Trait)
        if tr == "Sized" {
            return !matches!(t, Ty::Str | Ty::Slice(_) | Ty::Dyn(_));
        }
        // (auto traits: structurally, Rc / raw pointers are not Send, also Cell / RefCell are not Sync)
        if tr == "Send" || tr == "Sync" {
            return self.auto_trait(t, tr == "Sync", 0);
        }
        // (the unwind-safety auto traits: structurally, as std's impls say)
        if tr == "UnwindSafe" || tr == "RefUnwindSafe" {
            return self.unwind_safe(t, tr == "RefUnwindSafe", 0);
        }
        // (Unpin: every type here; nothing is pinned behind the program's back)
        if tr == "Unpin" {
            return true;
        }
        // (`Box<T>` is Clone / PartialEq / Eq / PartialOrd / Ord / Debug / Display / Hash / Default when T is)
        if let Ty::Box(i) = t {
            if matches!(tr, "Clone" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Display" | "Hash" | "Default") {
                return self.implements(*i, tr);
            }
        }
        // (`#[derive(Clone)] struct S<T>(PhantomData<T>)`: a derived impl of the program's requires the trait of each
        // type argument, `S<X>: Clone` needs `X: Clone`)
        if let Ty::Struct(sid) = t {
            let b = self.base_of(sid);
            // (`#[derive(Default)]` on an enum takes its `#[default]` unit variant: no bound on the arguments)
            if b != sid && !(tr == "Default" && self.structs[b as usize].is_enum) && self.derived.get(&b).is_some_and(|ds| ds.iter().any(|d| d == tr)) && self.struct_item.get(b as usize).is_some_and(|&i| i < self.prelude_from) {
                return self.structs[sid as usize].args.iter().all(|&a| matches!(a, Ty::Array(Ty::Unit, _)) || self.has_param(a) || self.implements(a, tr));
            }
        }
        // (a const generic argument is an array type carrying the length)
        if tr == CONST_GENERIC || tr.strip_prefix(CONST_GENERIC).is_some_and(|r| r.starts_with(':')) {
            return matches!(t, Ty::Array(Ty::Unit, _));
        }
        // The built-in AsRef<str> / AsRef<[T]> (strings, Vec, arrays, slices and references to them)
        if tr.starts_with("AsRef") && tr.contains('<') {
            let inner: String = tr["AsRef".len()..].chars().filter(|c| !c.is_whitespace()).collect();
            let mut b = t;
            while let Ty::Ref(_, i) = b {
                b = *i;
            }
            let name = match b {
                Ty::Struct(sid) => self.structs[self.base_of(sid) as usize].name.as_str(),
                _ => "",
            };
            let text = b == Ty::Str || name == "String";
            let seq = matches!(b, Ty::Array(..) | Ty::Slice(_)) || name == "Vec";
            if (text && (inner == "<str>" || inner == "<[u8]>")) || (seq && inner.starts_with("<[")) {
                return true;
            }
            if let Ty::Struct(sid) = b {
                return self.impls.contains(&(self.base_of(sid), "AsRef".to_string()));
            }
            return false;
        }
        // The built-in AsMut<[T]> of arrays, slices and Vec, AsMut<str> of str and String, and std's `impl AsMut<U>
        // for &mut T` (through `&mut` only, not `&`). A program's own impls are asked below, by the trait's name
        if tr.starts_with("AsMut") && tr.contains('<') {
            let squeeze = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
            let inner = squeeze(&tr["AsMut".len()..]);
            let mut b = t;
            while let Ty::Ref(true, i) = b {
                b = *i;
            }
            // (std's String / Vec, not a program's type of the name)
            let name = match b {
                Ty::Struct(sid) if self.struct_item.get(self.base_of(sid) as usize).is_some_and(|&i| i >= self.prelude_from) => {
                    self.structs[self.base_of(sid) as usize].name.as_str()
                }
                _ => "",
            };
            let elem = match b {
                Ty::Array(e, _) | Ty::Slice(e) => Some(*e),
                Ty::Struct(sid) if name == "Vec" => self.structs[sid as usize].args.first().copied(),
                _ => None,
            };
            // (and a Vec's AsMut<Vec<T>>)
            if elem.is_some_and(|e| inner == squeeze(&format!("<[{}]>", self.tn(e))))
                || ((b == Ty::Str || name == "String") && inner == "<str>")
                || (name == "Vec" && inner == squeeze(&format!("<{}>", self.tn(b))))
            {
                return true;
            }
        }
        // std's Extend of Vec<T> (`Extend<T>`, and `Extend<&T>` of a Copy T) and of String (`char`, `&char`, `&str`,
        // `String`), whose own `extend` takes any of them; a program's own impls by their argument (a type with none is
        // asked below, by the trait's name)
        if let Some(x) = tr.strip_prefix("Extend").map(str::trim).and_then(|r| r.strip_prefix('<')).and_then(|r| r.strip_suffix('>')) {
            if let Ty::Struct(sid) = t {
                // (the argument's text without lifetimes or white space: `&'a bool` is `&bool`)
                let mut x2 = String::new();
                let mut cs = x.chars().peekable();
                while let Some(c) = cs.next() {
                    if c == '\'' {
                        while cs.next_if(|c| c.is_alphanumeric() || *c == '_').is_some() {}
                    } else if !c.is_whitespace() {
                        x2.push(c);
                    }
                }
                match self.structs[self.base_of(sid) as usize].name.as_str() {
                    "Vec" => {
                        if let Some(&e) = self.structs[sid as usize].args.first() {
                            let en: String = self.tn(e).chars().filter(|c| !c.is_whitespace()).collect();
                            if x2 == en || (x2 == format!("&{en}") && self.implements(e, "Copy")) {
                                return true;
                            }
                        }
                    }
                    "String" if matches!(x2.as_str(), "char" | "&char" | "&str" | "String") => return true,
                    _ => {}
                }
                // (a program's own impls for the type: one whose argument is the bound's (`impl Extend<u8> for Bits`
                // is no `Extend<bool>`), or one with the impl's parameters in it, which is taken to fit)
                let own: Vec<&(Ty, Vec<Ty>, usize)> =
                    self.impl_targs.get("Extend").map_or(vec![], |v| v.iter().filter(|(s, _, n)| self.match_ty(*s, t, &mut vec![None; *n])).collect());
                if !own.is_empty() {
                    let squeeze = |s: String| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                    return own.iter().any(|(_, a, _)| a.len() != 1 || self.has_param(a[0]) || squeeze(self.tn(a[0])) == x2);
                }
            }
        }
        // The targets of collect (built-in): String / Vec / HashMap and so on
        if tr.starts_with("FromIterator") {
            if let Ty::Struct(sid) = t {
                let n = self.structs[self.base_of(sid) as usize].name.as_str();
                if matches!(n, "String" | "Vec" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque" | "BinaryHeap" | "Option" | "Result") {
                    return true;
                }
            }
        }
        // (`t: Into<X>` for a type X of the program's: std's `impl<T, U: From<T>> Into<U> for T`, so X's `From<t>`, X
        // itself, or the program's `impl Into<X> for t`; anything else as before)
        if let Some(x) = tr.strip_prefix("Into").and_then(|r| r.trim().strip_prefix('<')).and_then(|r| r.strip_suffix('>')).map(str::trim) {
            if let Some(&xs) = self.struct_index.get(x).filter(|&&xs| self.structs[xs as usize].generics.is_empty()) {
                let xt = Ty::Struct(xs);
                // (an impl whose parameters' bounds hold of what they matched: `impl<T: Marker> From<W<T>> for J` is
                // not `W<u8>`'s without `u8: Marker`)
                let hit = |trn: &str, st: Ty, arg: Ty| {
                    let bounds = self.impl_targ_bounds.get(trn);
                    self.impl_targs.get(trn).is_some_and(|v| {
                        v.iter().enumerate().any(|(k, (s, a, n))| {
                            let mut sol = vec![None; *n];
                            a.len() == 1
                                && self.match_ty(*s, st, &mut sol)
                                && self.match_ty(a[0], arg, &mut sol)
                                && bounds.and_then(|b| b.get(k)).is_none_or(|bs| {
                                    bs.iter().zip(&sol).all(|(bs, a)| match a {
                                        Some(a) if !self.has_param(*a) => bs.iter().all(|b| relaxed_bound(b) || self.implements(*a, b)),
                                        _ => true,
                                    })
                                })
                        })
                    })
                };
                if t == xt || hit("From", xt, t) || hit("Into", t, xt) {
                    return true;
                }
            }
        }
        // `Iterator<Item = T>` is checked by the trait's name (the caller solves the associated type constraint)
        if let Some(i) = tr.find('<') {
            if parse_fn_bound(tr).is_none() {
                // (the trait's name alone: its certainty goes on to evaluate, as impl_applies_noted's does)
                let c = self.evaluate(t, tr[..i].trim());
                NOTED.with(|n| n.set(Some(c)));
                return c != Certainty::No;
            }
        }
        match t {
            Ty::Struct(sid) if self.closure_of_struct.contains_key(&self.base_of(sid)) => {
                // Closures: the Fn traits, and Copy / Clone when every capture is Copy
                parse_fn_bound(tr).is_some()
                    || matches!(tr, "Fn" | "FnMut" | "FnOnce" | "Sized")
                    || (matches!(tr, "Copy" | "Clone")
                        && self.structs[sid as usize]
                            .fields
                            .iter()
                            .all(|f| self.implements(f.1, tr)))
            }
            // The inside of flat_map / flatten: an iterator also fits
            Ty::Struct(_) if tr == "__Flat" && self.implements(t, "Iterator") => true,
            // Tuples: when every element implements it (as core's impls do)
            Ty::Struct(sid) if self.is_tuple(sid) => {
                if matches!(
                    tr,
                    "Copy" | "Clone" | "Sized" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" | "Debug" | "Default"
                ) {
                    self.structs[sid as usize].fields.iter().all(|f| self.implements(f.1, tr))
                } else {
                    // The user's `impl<A, B> Tr for (A, B)`: its self type matches t, and its bounds hold (as a struct's)
                    self.impls.contains(&(self.base_of(sid), tr.to_string())) && self.impl_applies_noted(t, tr)
                }
            }
            // The prelude's collections and Option are Default (empty / None: an all-zero bit pattern)
            Ty::Struct(sid)
                if tr == "Default"
                    && matches!(
                        self.structs[self.base_of(sid) as usize].name.as_str(),
                        "Vec" | "String" | "HashMap" | "BinaryHeap" | "Option"
                    ) =>
            {
                true
            }
            // Types with eq / clone methods (the prelude's String / Vec and so on)
            Ty::Struct(sid)
                if (tr == "PartialEq" && self.method(sid, "eq").is_some())
                    || (tr == "Clone" && self.method(sid, "clone").is_some()) =>
            {
                true
            }
            // Vec / String compare depending on their elements
            Ty::Struct(sid)
                if matches!(tr, "PartialOrd" | "Ord" | "Eq" | "Hash" | "PartialEq" | "Clone")
                    && matches!(self.structs[self.base_of(sid) as usize].name.as_str(), "Vec" | "String") =>
            {
                self.structs[sid as usize].args.iter().all(|&a| self.implements(a, tr))
            }
            // (an impl for the struct, whose self type t matches and whose bounds t's arguments meet: `impl<T: Bar> Foo
            // for W<T>` is not for W<u8> without `impl Bar for u8`, `impl Foo for W<i32>` is not for W<u8> (issue #2))
            Ty::Struct(sid) => self.impls.contains(&(self.base_of(sid), tr.to_string())) && self.impl_applies_noted(t, tr),
            // (arrays / slices: comparison, Debug and Hash when the element implements them)
            Ty::Array(e, _) => {
                tr == "Sized"
                    || (matches!(tr, "Copy" | "Clone" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Hash" | "Default")
                        && self.implements(*e, tr))
            }
            Ty::Str => {
                matches!(tr, "Display" | "Debug" | "PartialEq" | "Eq" | "Hash" | "PartialOrd" | "Ord")
                    || self.prim_has(Ty::Str, tr)
            }
            Ty::Slice(e) => matches!(tr, "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Hash") && self.implements(*e, tr),
            // (`dyn Tr` implements Tr, and the other traits written in it: `dyn Tr + Send`; the bindings of
            // `dyn Tr<Y = T>` must meet the bounds of `type Y: B;`)
            Ty::Dyn(d) => d.split('+').any(|p| {
                let p = p.trim();
                let (name, args) = match p.find('<') {
                    Some(i) => (p[..i].trim(), p[i + 1..].trim_end_matches('>')),
                    None => (p, ""),
                };
                let name = name.rsplit("::").next().unwrap_or(name);
                name == tr && self.dyn_bindings_ok(name, args)
            }) || (!self.dyn_impls.is_empty() && {
                // (the program's impl for objects of its trait, `impl WasmClosure for dyn Fn() -> R`)
                let first = d.split('+').next().unwrap_or(d).trim();
                let name = first.split(|c: char| c == '(' || c == '<').next().unwrap_or(first).trim();
                self.dyn_impls.contains(&(tr.to_string(), name.rsplit("::").next().unwrap_or(name).to_string()))
            }) || (!self.dyn_impls_exact.is_empty() && self.dyn_impls_exact.contains(&(tr.to_string(), dyn_first(d))))
                || (tr == "Debug" && self.dyn_any_debug(d)),
            // Fn pointers: the Fn traits (with a matching argument count) and Copy
            // (impls for fn pointers / raw pointers / `!`: the exact type, or a generic one (None) of that kind)
            Ty::Fn(_) | Ty::Ptr(..) | Ty::Never
                if !self.fnptr_impls.is_empty()
                    && (self.fnptr_impls.contains(&(Some(t), tr.to_string()))
                        || (self.fnptr_impls.contains(&(None, tr.to_string())) && self.has_kind_impl(t, tr))) =>
            {
                true
            }
            Ty::Fn(f) => match parse_fn_bound(tr) {
                Some((_, ps, _)) => ps.len() == f.params.len(),
                None => matches!(tr, "Fn" | "FnMut" | "FnOnce" | "Copy" | "Clone" | "Sized" | "PartialEq" | "Eq"),
            },
            Ty::Ref(false, Ty::Slice(_)) if self.ref_slice_traits.contains(tr) => true,
            // Built-in types: marker traits, comparison, defaults and so on (what core implements)
            Ty::Ref(true, _) => tr == "Sized",
            Ty::Box(_) => matches!(tr, "Sized" | "Drop"),
            // `&T: Display` / `Debug` / `Hash` depend on T
            Ty::Ref(_, i) if matches!(tr, "Display" | "Debug" | "Hash" | "PartialOrd" | "Ord") => {
                self.implements(*i, tr)
            }
            Ty::Ptr(..) | Ty::Ref(..) => {
                matches!(tr, "Copy" | "Clone" | "Sized" | "PartialEq" | "Eq")
            }
            Ty::F32 | Ty::F64 => matches!(
                tr,
                "Copy" | "Clone" | "Sized" | "PartialEq" | "PartialOrd" | "Default" | "Debug" | "Display"
            ) || NUM_OPS.contains(&tr) || matches!(tr, "FromStr" | "ToString") || self.prim_has(t, tr),
            // (the operators core implements for integers: `T: Add<Output = T>` with T = i32)
            t if t.is_int() && (NUM_OPS.contains(&tr) || INT_OPS.contains(&tr) || matches!(tr, "FromStr" | "ToString")) => true,
            Ty::Bool | Ty::Char if matches!(tr, "FromStr" | "ToString") || (t == Ty::Bool && matches!(tr, "Not" | "BitAnd" | "BitOr" | "BitXor")) => true,
            t if t.is_int() || matches!(t, Ty::Bool | Ty::Char) => {
                matches!(
                    tr,
                    "Copy"
                        | "Clone"
                        | "Sized"
                        | "PartialEq"
                        | "Eq"
                        | "PartialOrd"
                        | "Ord"
                        | "Default"
                        | "Debug"
                        | "Display"
                        | "Hash"
                ) || self.prim_has(t, tr)
            }
            _ => matches!(tr, "Copy" | "Clone" | "Sized") || self.prim_has(t, tr),
        }
    }

    /// `Y = str, Z = S` (the arguments of `dyn Tr<..>`): each binding's type meets the bounds the trait declares
    /// for it (types other than a primitive, `str` or a struct name are not checked)
    fn dyn_bindings_ok(&self, tr: &str, args: &str) -> bool {
        if self.assoc_bounds.is_empty() {
            return true;
        }
        args.split(',').all(|a| {
            let Some((n, t)) = a.split_once('=') else { return true };
            let (n, t) = (n.trim(), t.trim());
            let Some(bs) = self.assoc_bounds.get(&(tr.to_string(), n.to_string())) else { return true };
            let ty = match Ty::prim(t) {
                Some(p) => p,
                None if t == "str" => Ty::Str,
                None => match self.struct_index.get(t) {
                    Some(&sid) => Ty::Struct(sid),
                    None => return true,
                },
            };
            // (not its lifetime bounds, `'static` / `'0`: of no type written here)
            bs.iter().filter(|b| !b.starts_with('\'')).all(|b| self.implements(ty, b))
        })
    }

    /// UnwindSafe (refs: RefUnwindSafe) as std has them: `&mut T` is not UnwindSafe; `&T`, raw pointers, Rc and Arc
    /// are when T is RefUnwindSafe; interior mutability (UnsafeCell, Cell, RefCell, OnceCell, LazyCell) is not
    /// RefUnwindSafe, but Mutex, RwLock and the atomics are; AssertUnwindSafe is both; a user's impl counts
    fn unwind_safe(&self, t: Ty, refs: bool, d: u32) -> bool {
        if d > 12 {
            return true;
        }
        match t {
            Ty::Ref(true, i) => refs && self.unwind_safe(*i, true, d + 1),
            Ty::Ref(false, i) | Ty::Ptr(_, i) => self.unwind_safe(*i, true, d + 1),
            Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.unwind_safe(*i, refs, d + 1),
            Ty::Struct(sid) => {
                let b = self.base_of(sid);
                let name = self.structs[b as usize].name.as_str();
                if self.impls.contains(&(b, if refs { "RefUnwindSafe" } else { "UnwindSafe" }.to_string())) {
                    return true;
                }
                let sd = &self.structs[sid as usize];
                let arg = |k: usize, r: bool| sd.args.get(k).is_none_or(|&a| self.unwind_safe(a, r, d + 1));
                match name {
                    "AssertUnwindSafe" => return true,
                    // (std's locks and their writing guards are both whatever they hold: a panic poisons them. A read
                    // guard does not poison: it is as its contents are RefUnwindSafe)
                    "Mutex" | "RwLock" | "MutexGuard" | "RwLockWriteGuard" => return true,
                    "RwLockReadGuard" => return arg(0, true),
                    n if n.starts_with("Atomic") => return true,
                    "UnsafeCell" | "Cell" | "RefCell" | "OnceCell" | "LazyCell" => return !refs && arg(0, false),
                    "Rc" | "Arc" => return arg(0, true),
                    _ => {}
                }
                let prelude = self.struct_item.get(b as usize).is_some_and(|&i| i >= self.prelude_from);
                if prelude {
                    return sd.args.iter().all(|&a| self.unwind_safe(a, refs, d + 1));
                }
                sd.fields.iter().all(|f| self.unwind_safe(f.1, refs, d + 1))
                    && sd.variants.iter().all(|v| v.fields.iter().all(|f| self.unwind_safe(f.1, refs, d + 1)))
            }
            _ => true,
        }
    }

    fn auto_trait(&self, t: Ty, sync: bool, d: u32) -> bool {
        if d > 12 {
            return true;
        }
        match t {
            Ty::Ptr(..) => false,
            // (a trait object has an auto trait it is written with (`dyn Tr + Send`), or one its trait has as a
            // supertrait: it may hold any type of its trait, one without it too)
            Ty::Dyn(d) => {
                let (core, send, sync_) = crate::types::dyn_autos(d);
                (if sync { sync_ } else { send }) || self.tdb.gives_auto(self, core, if sync { "Sync" } else { "Send" }) == Some(true)
            }
            // (`&T` is Send and Sync when T is Sync; `&mut T` is Send when T is Send, Sync when T is Sync)
            Ty::Ref(m, i) => self.auto_trait(*i, sync || !m, d + 1),
            Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.auto_trait(*i, sync, d + 1),
            Ty::Struct(sid) => {
                let b = self.base_of(sid);
                let name = self.structs[b as usize].name.as_str();
                // (a user's `unsafe impl Send for T`)
                if self.impls.contains(&(b, if sync { "Sync" } else { "Send" }.to_string())) {
                    return true;
                }
                // (the prelude's, not a program's own type of the name)
                let ours = self.struct_item.get(b as usize).is_some_and(|&i| i >= self.prelude_from);
                // (as acg's auto_trait_fails: a lock's guard is not Send, a RefCell's guards neither Send nor Sync, a
                // Mutex is Sync when T is Send, an Arc or RwLock when T is Send and Sync)
                let args = &self.structs[sid as usize].args;
                match name {
                    // (std's atomics, AtomicPtr<T> too, are Send and Sync whatever they point to)
                    n if ours && n.starts_with("Atomic") => return true,
                    "Rc" | "Ref" | "RefMut" if ours => return false,
                    "MutexGuard" | "RwLockReadGuard" | "RwLockWriteGuard" | "StdoutLock" if !sync && ours => return false,
                    "Cell" | "RefCell" | "LazyCell" | "OnceCell" | "UnsafeCell" | "__Rcvr__" if sync && ours => return false,
                    "Mutex" if sync && ours => return args.iter().all(|&a| self.auto_trait(a, false, d + 1)),
                    "Arc" | "RwLock" | "OnceLock" if ours && (sync || name == "Arc") => {
                        return args.iter().all(|&a| self.auto_trait(a, false, d + 1) && self.auto_trait(a, true, d + 1));
                    }
                    // (the prelude's own types hold raw pointers: what they hold decides)
                    _ => {}
                }
                let sd = &self.structs[sid as usize];
                let prelude = self.struct_item.get(b as usize).is_some_and(|&i| i >= self.prelude_from);
                if prelude {
                    return sd.args.iter().all(|&a| self.auto_trait(a, sync, d + 1));
                }
                // (a closure's capture held by reference is a raw pointer: the reference it stands for. Sync needs
                // `T: Sync` (as `&T` and `&mut T`); Send needs `T: Send` of a `&mut T`, and of one that may be either
                // kind `T: Sync` (`&T`) too)
                sd.fields.iter().enumerate().all(|(i, f)| match (f.1, self.ref_capture(sid, i)) {
                    (Ty::Ptr(_, inner), Some(true)) => self.auto_trait(*inner, sync, d + 1),
                    (Ty::Ptr(_, inner), Some(false)) => self.auto_trait(*inner, true, d + 1) && (sync || self.auto_trait(*inner, false, d + 1)),
                    _ => self.auto_trait(f.1, sync, d + 1),
                }) && sd.variants.iter().all(|v| v.fields.iter().all(|f| self.auto_trait(f.1, sync, d + 1)))
            }
            _ => true,
        }
    }

    #[cold]
    #[inline(never)]
    fn add_alias_defaults(&mut self, n: &str, ds: &[Option<TySyn>]) {
        self.alias_defaults.insert(n.to_string(), ds.to_vec());
    }

    /// The types of alias n's parameters a use leaves out (after vals), from their defaults, which may name the
    /// parameters before them. Any other name in a default is the type where the alias is declared, never a generic
    /// parameter of the user's (`type Pair<T, E = Error>` used as `Pair<u8>` in `fn make<Error>()` is the struct)
    #[cold]
    #[inline(never)]
    fn fill_alias_defaults(&self, n: &str, g: &[String], vals: &mut Vec<Ty>) -> R<()> {
        let ds = self.alias_defaults[n].clone();
        for d in &ds[vals.len()..] {
            let d = d.as_ref().ok_or("an alias parameter without a default")?;
            let k = vals.len();
            let v = self.resolve_with(d, &|x| g.iter().position(|y| y == x).filter(|&i| i < k).map(|i| vals[i]))?;
            vals.push(v);
        }
        Ok(())
    }

    /// Whether alias n's type parameters from the k-th on all have defaults (a use may give only the first k)
    pub fn alias_fills(&self, n: &str, k: usize) -> bool {
        self.alias_defaults.get(n).is_some_and(|ds| k < ds.len() && ds[k..].iter().all(Option::is_some))
    }

    /// A method of primitive type t (from a trait impl, or else the trait's default method)
    /// prim_method for a method call: of the functions of that name, one that takes self (`impl Foo for isize`
    /// with `fn new() -> bool` and `impl Bar for isize` with `fn new(&self) -> bool`: `1.new()` is Bar's)
    pub fn prim_self_method(&self, t: Ty, name: &str) -> Option<u32> {
        // (`impl<T> Fake for T {}`: the trait's methods are methods of every type, numbers too)
        let Some(id) = self.prim_method(t, name) else {
            return self.blanket.get(name).copied().filter(|&i| self.sigs[i as usize].has_self);
        };
        if self.sigs[id as usize].has_self {
            return Some(id);
        }
        if let Some(ids) = self.overloads.get(&self.names[id as usize]) {
            if let Some(&i) = ids.iter().find(|&&i| self.sigs[i as usize].has_self) {
                return Some(i);
            }
        }
        // (the default methods of the traits implemented for t)
        for (pt, tr) in &self.prim_impls {
            if *pt != t {
                continue;
            }
            for &d in self.traits.get(tr).map(|v| v.as_slice()).unwrap_or(&[]) {
                if self.sigs[d as usize].has_body && self.sigs[d as usize].has_self && self.names[d as usize].rsplit("::").next() == Some(name) {
                    return Some(d);
                }
            }
        }
        Some(id)
    }

    pub fn prim_method(&self, t: Ty, name: &str) -> Option<u32> {
        // (not a program's own `struct v128`, which is not the vector `tn` names)
        let direct = self.tn(t);
        if let Some(&id) = self.index.get(&format!("{direct}::{name}")).filter(|_| !(matches!(t, Ty::Vec(_)) && self.struct_index.contains_key(&direct))) {
            return Some(id);
        }
        // (a vector type's methods are under the name its impl wrote: `impl Vector for __m128i` has `__m128i::splat`)
        if let Ty::Vec(_) = t {
            // (not a name the program gives a struct of its own: `struct __m128i` is not the vector)
            if let Some(&id) = vector_names(t).iter().filter(|n| !self.struct_index.contains_key(**n)).find_map(|n| self.index.get(&format!("{n}::{name}"))) {
                return Some(id);
            }
        }
        // (the methods of `impl<T: Hash> Hash for [T]` / `for [T; N]` are `__slice::m` / `__array::m`)
        let base = match t {
            Ty::Slice(_) => Some("__slice"),
            Ty::Fn(_) => Some("__fnptr"),
            Ty::Ptr(..) => Some("__ptr"),
            Ty::Never => Some("__never"),
            Ty::Array(..) => Some("__array"),
            Ty::Box(_) => Some("Box"),
            // (`impl Tr for ()`: the methods of `#tuple0`)
            Ty::Unit => Some("#tuple0"),
            _ => None,
        };
        // (`impl<T> Tr for &[T; 3]` / `for &[T]`: also `__array::m` / `__slice::m`, the impl's self type a reference
        // that t fits)
        if let Ty::Ref(_, i @ (Ty::Array(..) | Ty::Slice(_))) = t {
            let g = if matches!(i, Ty::Array(..)) { "__array" } else { "__slice" };
            if let Some(&id) = self.index.get(&format!("{g}::__ref_{name}")).or_else(|| self.index.get(&format!("{g}::{name}"))) {
                let ids: Vec<u32> = self.overloads.get(&self.names[id as usize]).cloned().unwrap_or_else(|| vec![id]);
                let fit = ids.into_iter().find(|&i| match self.sigs[i as usize].self_ty {
                    Some(st @ Ty::Ref(..)) => self.match_ty(st, t, &mut vec![None; self.sigs[i as usize].generics.len()]),
                    _ => false,
                });
                if fit.is_some() {
                    return fit;
                }
            }
        }
        // (`impl dyn Tr { fn m(&self) }`: `__dyn_Tr::m`)
        if let Ty::Dyn(s) = t {
            let p = s.split('+').next().unwrap_or("").trim();
            // (`ObjectTrait < u8 >` as the object's text spaces it: the name alone)
            let p = p.split('<').next().unwrap_or(p).rsplit("::").next().unwrap_or(p).trim();
            if let Some(&id) = self.index.get(&format!("__dyn_{p}::{name}")) {
                return Some(id);
            }
        }
        if let Some(&id) = base.and_then(|g| self.index.get(&format!("{g}::{name}"))) {
            // (`impl Tr for [u8; 1]`, `.. for [u8; 2]` ..: among impls of one name, the one whose self type fits)
            if let Some(ids) = self.overloads.get(&self.names[id as usize]) {
                let fit = |i: u32| match self.sigs[i as usize].self_ty {
                    Some(st) => {
                        let mut sol = vec![None; self.sigs[i as usize].generics.len()];
                        self.match_ty(st, t, &mut sol)
                    }
                    None => false,
                };
                if let Some(&i) = ids.iter().find(|&&i| fit(i)) {
                    return Some(i);
                }
            }
            return Some(id);
        }
        let mut found = None;
        for (pt, tr) in &self.prim_impls {
            if *pt != t {
                continue;
            }
            for &d in self.traits.get(tr).map(|v| v.as_slice()).unwrap_or(&[]) {
                if self.sigs[d as usize].has_body
                    && self.names[d as usize].rsplit("::").next() == Some(name)
                {
                    found = Some(d);
                }
            }
        }
        found.or_else(|| self.blanket.get(name).copied())
    }

    /// When struct sid has no method named name, the default method of a trait it implements
    pub fn default_method(&self, sid: u32, name: &str) -> Option<u32> {
        let r = self.default_method_inner(sid, name, false);
        // (or else a blanket impl's method)
        r.or_else(|| self.blanket.get(name).copied())
    }

    /// default_method for a method call: one with `self`, as self_method. A trait's default associated function
    /// without it (`fn val() -> i32 { 5 }`) is no method candidate: `w.val()` goes on to what w derefs to
    pub fn self_default_method(&self, sid: u32, name: &str) -> Option<u32> {
        let r = self.default_method_inner(sid, name, true);
        r.or_else(|| self.blanket.get(name).copied().filter(|&b| self.sigs[b as usize].has_self))
    }

    fn default_method_inner(&self, sid: u32, name: &str, self_only: bool) -> Option<u32> {
        let sid = self.base_of(sid);
        let mut found = None;
        for (s, tr) in &self.impls {
            if *s != sid {
                continue;
            }
            let Some(decls) = self.traits.get(tr) else {
                continue;
            };
            for &d in decls {
                if self.sigs[d as usize].has_body
                    && (!self_only || self.sigs[d as usize].has_self)
                    && self.names[d as usize].rsplit("::").next() == Some(name)
                {
                    found = Some(d);
                }
            }
        }
        found
    }

    /// TySyn → Ty. Names in generics become type parameters
    pub fn resolve_g(&self, t: &TySyn, generics: &[&str]) -> R<Ty> {
        self.resolve_with(t, &|n| {
            generics
                .iter()
                .position(|g| *g == n)
                .map(|i| Ty::Param(i as u8))
        })
    }

    /// Asks names to resolve a name first, then looks up struct names
    fn resolve_with(&self, t: &TySyn, names: &dyn Fn(&str) -> Option<Ty>) -> R<Ty> {
        let r = |t: &TySyn| self.resolve_with(t, names);
        Ok(match t {
            TySyn::Prim(t) => *t,
            TySyn::Ref(m, inner) => Ty::Ref(*m, intern(self.bare_dyn(inner, names).unwrap_or_else(|| r(inner))?)),
            TySyn::Ptr(m, inner) => Ty::Ptr(*m, intern(self.bare_dyn(inner, names).unwrap_or_else(|| r(inner))?)),
            TySyn::Slice(inner) => Ty::Slice(intern(r(inner)?)),
            TySyn::Str => Ty::Str,
            // (its type parameters marked, the types it names as written: see dyn_canon)
            TySyn::Dyn(s) => Ty::Dyn(intern_str(self.dyn_canon(s, names).as_deref().unwrap_or(s))),
            TySyn::Infer => Ty::Param(HOLE),
            // (`M::S<u8>` of a module M reads as a generic associated type `#gat:S` of `(M, u8)`: when M is no type,
            // it is the type S<u8> as before)
            TySyn::Assoc(b, n) if n.starts_with(crate::ast_types::GAT_PREFIX) && matches!(&**b, TySyn::Tuple(p) if !p.is_empty() && r(&p[0]).is_err()) => {
                let TySyn::Tuple(parts) = &**b else { unreachable!() };
                let name = n[crate::ast_types::GAT_PREFIX.len()..].to_string();
                return r(&TySyn::App(name, parts[1..].to_vec()));
            }
            // (`<S as Trait<usize>>::Assoc`: the impl whose self type and trait arguments match, exactly)
            TySyn::Assoc(b, n) if n.starts_with(crate::ast_types::TARGS_PREFIX) && matches!(&**b, TySyn::Tuple(p) if !p.is_empty()) => {
                let TySyn::Tuple(parts) = &**b else { unreachable!() };
                let rest = &n[crate::ast_types::TARGS_PREFIX.len()..];
                let (tr, name) = rest.split_once('#').unwrap_or(("", rest));
                let bt = r(&parts[0])?;
                // (a const argument zustc cannot evaluate: one value for all the impls whose other arguments match,
                // or an error; never the first impl by self type alone)
                if parts[1..].iter().any(|p| matches!(p, TySyn::Infer)) {
                    let args = parts[1..].iter().map(|p| if matches!(p, TySyn::Infer) { Ok(None) } else { r(p).map(Some) }).collect::<R<Vec<Option<Ty>>>>()?;
                    let nm = name.strip_prefix(crate::ast_types::NO_STD_ASSOC).unwrap_or(name);
                    let trn = tr.rsplit("::").next().unwrap_or(tr);
                    let mut found: Option<Ty> = None;
                    let mut ambiguous = false;
                    for a in &self.assocs {
                        if a.name != nm || a.trait_name != trn || a.ngat != 0 {
                            continue;
                        }
                        let mut sol = vec![None; a.ngen];
                        // (an impl of the self type whose trait arguments are not resolved (none kept, or fewer than
                        // the trait has: `impl Pick<{ <u8 as K>::N }>`) may be the one: ambiguous)
                        let ta = match &a.trait_args {
                            Some(ta) if ta.len() == args.len() => ta,
                            _ => {
                                ambiguous |= crate::traits::mtch(self, a.self_ty, bt, &mut sol, false, 0);
                                continue;
                            }
                        };
                        if !crate::traits::mtch(self, a.self_ty, bt, &mut sol, false, 0)
                            || !ta.iter().zip(&args).all(|(&p, x)| x.is_none_or(|x| crate::traits::mtch(self, p, x, &mut sol, false, 0)))
                        {
                            continue;
                        }
                        let Some(full) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
                            ambiguous = true;
                            continue;
                        };
                        let v = self.subst(a.value, &full);
                        ambiguous |= found.is_some_and(|f| f != v);
                        found = Some(v);
                    }
                    return match (found, ambiguous) {
                        (Some(t), false) => Ok(t),
                        _ => Err(format!("unsupported: the const argument of `{trn}` that selects `{nm}` for `{}` cannot be evaluated", self.tn(bt))),
                    };
                }
                let args = parts[1..].iter().map(r).collect::<R<Vec<Ty>>>()?;
                if !self.has_param(bt) && !args.iter().any(|&a| self.has_param(a)) {
                    if let Some(t) = self.assoc_of_targs(bt, tr, &args, name.strip_prefix(crate::ast_types::NO_STD_ASSOC).unwrap_or(name)) {
                        return Ok(t);
                    }
                }
                // (as before the arguments were kept: by the self type alone)
                return r(&TySyn::Assoc(Box::new(parts[0].clone()), name.to_string()));
            }
            TySyn::Assoc(b, n) => {
                let bt = r(b)?;
                let (n, tr) = crate::ast_types::split_trait_assoc(n);
                let (n, std_impls) = match n.strip_prefix(crate::ast_types::NO_STD_ASSOC) {
                    Some(x) => (x, false),
                    None => (n, true),
                };
                if self.has_param(bt) {
                    // (with its trait, when another trait has an associated type of the name too: `<T as
                    // VectorFromWasmAbi>::Abi` is not T's IntoWasmAbi's once T is known)
                    let name = match tr {
                        Some(tr) if self.user_traits.contains(tr) && self.assoc_traits_of(n) > 1 => format!("{}{tr}:{n}", crate::ast_types::TRAIT_ASSOC),
                        _ => n.to_string(),
                    };
                    Ty::Assoc(intern_assoc(AssocTy { base: bt, name }))
                } else {
                    match tr.and_then(|tr| self.assoc_of_trait(bt, n, tr)).or_else(|| self.assoc_of_with(bt, n, std_impls)) {
                        Some(t) => t,
                        None => {
                            return Err(format!(
                                "cannot find associated type `{n}` for `{}`",
                                self.tn(bt)
                            ));
                        }
                    }
                }
            }
            TySyn::FnPtr(v) => {
                let mut ts = v.iter().map(r).collect::<R<Vec<Ty>>>()?;
                let ret = ts.pop().unwrap_or(Ty::Unit);
                Ty::Fn(intern_fn(FnTy { params: ts, ret }))
            }
            TySyn::Array(inner, len) => {
                let n = match len {
                    ArrLen::Lit(n) => *n,
                    // (a const generic N)
                    ArrLen::Const(c) => match names(c) {
                        Some(Ty::Param(i)) => (CONST_LEN_BASE + i as u32) as u64,
                        // (inside an instance: N is an array type carrying the length / a type argument `[(); N]` of length N)
                        Some(Ty::Array(Ty::Unit, m)) => m as u64,
                        _ => self.const_usize(c)?,
                    },
                };
                Ty::Array(intern(r(inner)?), n as u32)
            }
            TySyn::Named(n) => {
                if let Some(t) = names(n) {
                    return Ok(t);
                }
                // (the marks of a `dyn` type's text: dyn_canon. Not a generic parameter's name, nor a program's own
                // type's)
                if n.starts_with("__z") && !self.struct_index.contains_key(n) && !self.aliases.contains_key(n) {
                    if let Some((i, _)) = dyn_param_mark(n) {
                        return Ok(Ty::Param(i));
                    }
                    // (only a struct dyn_src writes so: a placeholder or a closure, which the program cannot name)
                    if let Some(id) = dyn_struct_mark(n).filter(|&id| (id as usize) < self.structs.len() && (self.is_placeholder(id) || self.closure_of_struct.contains_key(&self.base_of(id)))) {
                        return Ok(Ty::Struct(id));
                    }
                }
                if let Some((g, t)) = self.aliases.get(n) {
                    if g.is_empty() {
                        return self.resolve_with(t, names);
                    }
                    // (every parameter with a default: `Bytes` for `type Bytes<T = u8>`, as `Bytes<>`)
                    if self.alias_fills(n, 0) {
                        return self.resolve_with(&TySyn::App(n.clone(), vec![]), names);
                    }
                }
                match self.struct_index.get(n) {
                    Some(&id) if self.structs[id as usize].generics.is_empty() => Ty::Struct(id),
                    // (every parameter with a default: `W` for `struct W<T = u8>`, as `W<>`)
                    Some(&id) if self.struct_defaults.get(&id).is_some_and(|ds| ds.iter().all(Option::is_some)) => {
                        return self.resolve_with(&TySyn::App(n.clone(), vec![]), names);
                    }
                    Some(_) => return Err(format!("missing generics for `{n}`")),
                    // (`zeros::<K>()` / `Buf<K>`: a const's name is a const generic argument)
                    None if self.const_index.contains_key(n) => Ty::Array(intern(Ty::Unit), self.const_usize(n)? as u32),
                    // (core::arch's vector types, when the program has no type of the name: `v128`, `__m256`)
                    None => match crate::simd_sig::vector_name(n) {
                        Some(t) => t,
                        None => return Err(format!("cannot find type `{n}`")),
                    },
                }
            }
            // `Box<dyn Error>`: the prelude's __DynError (holds the Display and Debug strings). Also `+ Send + Sync`: it
            // holds two Strings, Send and Sync whatever the error was
            TySyn::App(n, args)
                if n == "Box"
                    && matches!(args.as_slice(), [TySyn::Dyn(d)] if crate::types::dyn_autos(d).0 == "Error")
                    && self.struct_index.contains_key("__DynError") =>
            {
                Ty::Struct(self.struct_index["__DynError"])
            }
            // `Cow<'a, [T]>` is the prelude's `Cow<T>` (a borrowed slice or a Vec), `Cow<'a, str>` is `__CowStr`
            TySyn::App(n, args) if n == "Cow" && args.len() == 1 && matches!(args[0], TySyn::Slice(_) | TySyn::Str) => {
                match &args[0] {
                    TySyn::Slice(e) => {
                        let Some(&id) = self.struct_index.get("Cow") else {
                            return Err("cannot find type `Cow`".into());
                        };
                        let e = r(e)?;
                        Ty::Struct(self.instantiate(id, vec![e])?)
                    }
                    _ => match self.struct_index.get("__CowStr") {
                        Some(&id) => Ty::Struct(id),
                        None => return Err("cannot find type `Cow`".into()),
                    },
                }
            }
            TySyn::App(n, args) if n == "Box" && args.len() == 1 && !self.struct_index.contains_key(n) => {
                Ty::Box(intern(self.bare_dyn(&args[0], names).unwrap_or_else(|| r(&args[0]))?))
            }
            // An alias with type parameters (`type R<T> = Result<T, String>;`), those left out taking their defaults
            // (`type Result<T, E = Error>` used as `Result<()>`)
            TySyn::App(n, args) if self.aliases.get(n).is_some_and(|a| a.0.len() == args.len() || self.alias_fills(n, args.len())) => {
                let (g, t) = self.aliases[n].clone();
                let mut vals = args.iter().map(r).collect::<R<Vec<Ty>>>()?;
                if vals.len() < g.len() {
                    self.fill_alias_defaults(n, &g, &mut vals)?;
                }
                return self.resolve_with(&t, &|x| match g.iter().position(|y| y == x) {
                    Some(i) => Some(vals[i]),
                    None => names(x),
                });
            }
            TySyn::App(n, args) => {
                let Some(&id) = self.struct_index.get(n) else {
                    return Err(format!("cannot find type `{n}`"));
                };
                // `HashMap<K, V, S>` / `HashSet<K, S>`: ignore the hasher type (BuildHasherDefault<..>), use the default hash
                let mut args = args.as_slice();
                if matches!(n.as_str(), "HashMap" | "HashSet") && args.len() == self.structs[id as usize].generics.len() + 1 {
                    args = &args[..args.len() - 1];
                }
                // (the parameters left out take their defaults, written in the earlier ones: `LazyCell<T>` is
                // `LazyCell<T, fn() -> T>`)
                let filled;
                if let Some(ds) = self.struct_defaults.get(&id) {
                    let ps = &self.structs[id as usize].generics;
                    if args.len() < ps.len() && ds[args.len()..].iter().all(Option::is_some) {
                        let mut v: Vec<TySyn> = args.to_vec();
                        for d in &ds[args.len()..] {
                            let t = subst_tysyn(d.as_ref().unwrap(), &ps[..v.len()], &v);
                            v.push(t);
                        }
                        filled = v;
                        args = filled.as_slice();
                    }
                }
                let args = args.iter().map(r).collect::<R<Vec<Ty>>>()?;
                Ty::Struct(self.instantiate(id, args)?)
            }
            TySyn::Tuple(es) => {
                let args = es.iter().map(r).collect::<R<Vec<Ty>>>()?;
                let base = self.tuple_base(args.len());
                Ty::Struct(self.instantiate(base, args)?)
            }
        })
    }

    /// A const used for an array length (evaluated as an integer constant expression from the summaries' consts)
    /// Whether an impl of trait tr for a type parameter (`impl<T> Tr for T { const N = .. }`) defines its const n: a
    /// type it covers takes that, not the trait's default (which the qualified-path fallback then must not read)
    fn blanket_defines(&self, tr: &str, n: &str) -> Option<String> {
        let suffix = format!(" as {tr}::{n}");
        self.const_index
            .keys()
            .find(|k| {
                k.strip_suffix(suffix.as_str()).is_some_and(|base| !self.struct_index.contains_key(base) && Ty::prim(base).is_none() && !self.aliases.contains_key(base))
            })
            .cloned()
    }

    pub fn const_usize(&self, name: &str) -> R<u64> {
        // (`<Foo as Bar>::SIZE`: the trait's under its qualified name, else the only `Foo::SIZE`; an expression with a
        // cast in it, `(1 as usize) + Foo::SIZE`, is not one)
        let word = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');
        if let Some((base, rest)) = name.split_once(" as ").filter(|(b, r)| word(b) && r.split("::").all(word) && r.contains("::")) {
            if !self.const_index.contains_key(name) {
                // (`type Alias = Foo;`: the impl's under `Foo`)
                let base = match self.aliases.get(base) {
                    Some((g, TySyn::Named(n))) if g.is_empty() => n.as_str(),
                    _ => base,
                };
                let (tr, last) = rest.rsplit_once("::").unwrap_or_default();
                let q = format!("{base} as {tr}::{last}");
                if self.const_index.contains_key(&q) {
                    return self.const_usize(&q);
                }
                // (the impl leaves it out: the trait's default, not an inherent `Foo::SIZE`)
                // (`impl<T> Tr for T { const N }`: the blanket impl's, which covers Foo)
                if let Some(k) = self.blanket_defines(tr, last) {
                    return self.const_usize(&k);
                }
                let d = format!("{tr}::{last}");
                if self.const_index.contains_key(&d) {
                    return self.const_usize(&d);
                }
                return self.const_usize(&format!("{base}::{last}"));
            }
        }
        // (a const generic argument's expression `{ A - 16 }` comes as the expression's text)
        let v = if !self.const_index.contains_key(name) && (name.starts_with(|c: char| c.is_ascii_digit()) || name.bytes().any(|c| !(c.is_ascii_alphanumeric() || c == b'_' || c == b':'))) {
            let e = crate::parse::parse_expr(name)?;
            self.eval_const(&e, 1)?
        } else {
            self.const_int(name, 0)?
        };
        if v < 0 || v > u64::MAX as i128 {
            return Err(format!("constant `{name}` is negative"));
        }
        Ok(v as u64)
    }

    /// Evaluates an integer const (literals, arithmetic, bit operations, shifts, casts, other consts)
    pub fn const_int(&self, name: &str, depth: u32) -> R<i128> {
        Ok(self.const_int_t(name, depth)?.0)
    }

    /// The part `pick` takes from the value of the constant expression b (through `&`, `*`, parentheses and other
    /// consts), and whether it is another constant's (parsed from that constant's text)
    fn const_part(&self, b: &crate::parse::Expr, depth: u32, pick: &dyn Fn(&crate::parse::Expr) -> Option<crate::parse::Expr>) -> R<(crate::parse::Expr, bool)> {
        use crate::parse::Expr;
        if depth > 64 {
            return Err("constant is too deeply nested".into());
        }
        match b {
            Expr::Paren(x) | Expr::Ref(_, x) => self.const_part(x, depth + 1, pick),
            Expr::Unary(op, x, _) if op == "*" => self.const_part(x, depth + 1, pick),
            Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => {
                let ci = *self.const_index.get(p.segs[0].name.as_str()).ok_or("unsupported expression in constant")?;
                let e = crate::parse::parse_expr(&self.consts[ci as usize].expr)?;
                Ok((self.const_part(&e, depth + 1, pick)?.0, true))
            }
            x => Ok((pick(x).ok_or("unsupported expression in constant")?, false)),
        }
    }

    /// A block of `let x = ..;` statements and a tail expression, with the locals in env (typed: a `let` keeps its
    /// value's type, the tail is evaluated as `want`)
    fn eval_block_env(&self, b: &crate::parse::Block, env: &mut Vec<(String, i128, Option<It>, Option<crate::parse::Expr>)>, depth: u32, want: Option<It>) -> R<(i128, Option<It>)> {
        // (the untyped locals this block registers are dropped when it is done)
        let start = LAZY.with(|l| l.borrow().0);
        let r = self.eval_block_in(b, env, depth, want);
        // (a local whose untyped value failed and that no use evaluated: an error if the block does not refer to it
        // (`{ let _x = 1 / 0; 1 }`), or if it fails at the block's type too (`let x = 1 / 0; if false { x } else { 1 }`);
        // `let x = 1 / ((!0 >> 7) + 1); if false { x } else { 0u8 }` is a u8's 0)
        let pending: Vec<(crate::parse::Expr, String, bool)> = LAZY.with(|l| {
            l.borrow().1.iter().filter(|(k, e)| **k >= start && e.1.is_empty()).filter_map(|(_, e)| e.2.clone().map(|er| (e.0.clone(), er, e.3))).collect()
        });
        let mut unused = None;
        for (x, er, referenced) in pending {
            // (a skipped use's type is not known here: the block's, or any integer type it succeeds at
            // (`let _y: u8 = if false { x } else { 0u8 };` in an i64 block))
            let at = |w: Option<It>| self.eval_t(&x, depth, w).is_ok();
            let ints = ["u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize"];
            let fails = !referenced || !(want.is_some_and(|w| at(Some(w))) || ints.iter().any(|n| at(It::of_name(n))));
            if fails && unused.is_none() {
                unused = Some(er);
            }
        }
        LAZY.with(|l| l.borrow_mut().1.retain(|&k, _| k < start));
        match (r, unused) {
            (Ok(_), Some(e)) => Err(e),
            (r, _) => r,
        }
    }

    /// An untyped local's expression as a marker (`__zust_lazy_N`) that a use evaluates at the type it wants
    fn lazy_marker(&self, x: crate::parse::Expr, failed: Option<String>) -> R<crate::parse::Expr> {
        let id = LAZY.with(|l| {
            let mut l = l.borrow_mut();
            let id = l.0;
            l.0 += 1;
            l.1.insert(id, (x, vec![], failed, false));
            id
        });
        crate::parse::parse_expr(&format!("__zust_lazy_{id}"))
    }

    /// An untyped local whose untyped value failed and that x refers to is used, even where x is not evaluated (an
    /// `if` arm not taken still gives it its type): its error is not the block's
    fn note_refs(&self, x: &crate::parse::Expr) {
        LAZY.with(|l| {
            let mut l = l.borrow_mut();
            if l.1.values().all(|e| e.2.is_none() || e.3) {
                return;
            }
            let t = format!("{x:?}");
            for (id, e) in l.1.iter_mut() {
                if e.2.is_some() && t.contains(&format!("\"__zust_lazy_{id}\"")) {
                    e.3 = true;
                }
            }
        });
    }

    fn eval_block_in(&self, b: &crate::parse::Block, env: &mut Vec<(String, i128, Option<It>, Option<crate::parse::Expr>)>, depth: u32, want: Option<It>) -> R<(i128, Option<It>)> {
        use crate::parse::{Pat, Stmt};
        let n = b.stmts.len();
        for (k, s) in b.stmts.iter().enumerate() {
            match s {
                Stmt::Let(Pat::Ident(false, _, name, None), ty, Some(init), None, ..) => {
                    // (`let x: u8 = ..`: the annotation is the value's type)
                    let x = subst_consts(init, env);
                    self.note_refs(&x);
                    let lt = ty.as_ref().and_then(|t| It::of_name(&self.cast_prim(t))).or_else(|| self.static_ty(&x));
                    // (with no type yet, its failing untyped value is not an error: `let x = 1 / ((!0 >> 7) + 1);` is
                    // 0 as a u8, whose use evaluates it again at that type)
                    let mut failed = None;
                    let (v, t) = match self.eval_t(&x, depth, lt) {
                        Err(e) if lt.is_none() => {
                            failed = Some(e);
                            (0, None)
                        }
                        r => r?,
                    };
                    let t = lt.or(t);
                    // (untyped (`let x = !0 >> 4;`): kept as its expression too, which takes the type of its use; as a
                    // marker, so a chain of them does not grow)
                    let lazy = if t.is_none() { Some(self.lazy_marker(x, failed)?) } else { None };
                    env.push((name.clone(), t.map_or(v, |it| it.wrap(v)), t, lazy));
                }
                Stmt::Expr(x, false) if k + 1 == n => {
                    let x = subst_consts(x, env);
                    self.note_refs(&x);
                    return self.eval_t(&x, depth, want);
                }
                // (`x = 3;` to a local: its new value)
                Stmt::Expr(crate::parse::Expr::Assign(l, r), _) => {
                    let crate::parse::Expr::Path(None, p) = &**l else { return Err("unsupported statement in a const fn".into()) };
                    let Some(name) = (p.segs.len() == 1).then(|| p.segs[0].name.clone()) else {
                        return Err("unsupported statement in a const fn".into());
                    };
                    let Some(slot) = env.iter().rposition(|(n, ..)| *n == name) else {
                        return Err("unsupported statement in a const fn".into());
                    };
                    let st = env[slot].2;
                    let x = subst_consts(r, env);
                    self.note_refs(&x);
                    let mut failed = None;
                    let (v, t) = match self.eval_t(&x, depth, st) {
                        Err(e) if st.is_none() => {
                            failed = Some(e);
                            (0, None)
                        }
                        r => r?,
                    };
                    env[slot].1 = v;
                    env[slot].2 = st.or(t);
                    env[slot].3 = if env[slot].2.is_none() { Some(self.lazy_marker(x, failed)?) } else { None };
                }
                _ => return Err("unsupported statement in a const fn".into()),
            }
        }
        Err("a const fn without a value".into())
    }

    /// A one-segment type name's primitive integer name, through aliases (`type Lane = i32` -> "i32"); "" for another
    pub fn prim_of_name(&self, n: &str) -> String {
        let mut prim = n.to_string();
        // (a primitive integer's name is that type, not an alias of it: the aliases are the whole program's, and a
        // program's `type u32 = u8;` must not change the prelude's `as u32`, whose cached code does not hash aliases)
        for _ in 0..16 {
            if matches!(prim.as_str(), "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize") {
                break;
            }
            match self.aliases.get(&prim) {
                Some((g, crate::types::TySyn::Named(n))) if g.is_empty() => prim = n.clone(),
                Some((g, crate::types::TySyn::Prim(p))) if g.is_empty() => prim = format!("{p:?}").to_lowercase(),
                _ => break,
            }
        }
        prim
    }

    /// The name of a cast's target type, through aliases of it (`type Lane = i32` -> "i32"); "" for another type
    fn cast_prim(&self, t: &crate::parse::Ty) -> String {
        match t {
            crate::parse::Ty::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => self.prim_of_name(&p.segs[0].name),
            // (`as core::primitive::u16`)
            crate::parse::Ty::Path(None, p)
                if p.segs.len() == 3 && matches!(p.segs[0].name.as_str(), "core" | "std") && p.segs[1].name == "primitive" && p.segs.iter().all(|s| s.args.is_empty()) =>
            {
                p.segs[2].name.clone()
            }
            _ => String::new(),
        }
    }

    /// Evaluates an integer constant expression of our own syntax tree (the value only: eval_t gives the type too)
    pub fn eval_const(&self, e: &crate::parse::Expr, depth: u32) -> R<i128> {
        Ok(self.eval_t(e, depth, None).map_err(unplaced)?.0)
    }

    /// e's value as type t (an integer type: unsuffixed literals in e are t's, the value is wrapped to t)
    pub fn eval_const_as(&self, e: &crate::parse::Expr, t: Ty) -> R<i128> {
        self.eval_const_placed(e, t).map_err(unplaced)
    }

    /// eval_const_as of e parsed at its place in the source: an E0080 of an operation in e has its place, the start
    /// of the operation and of its operator (`1:15:1:23: attempt to compute ..`); one in a constant e reads has none
    pub fn eval_const_placed(&self, e: &crate::parse::Expr, t: Ty) -> R<i128> {
        let it = It::of_ty(t);
        let (v, _) = self.eval_t(e, 0, it)?;
        Ok(it.map_or(v, |it| it.wrap(v)))
    }

    /// A constant's value and its integer type, wrapped to the type it is declared with (`const X: i8 = -1`)
    fn const_int_t(&self, name: &str, depth: u32) -> R<(i128, Option<It>)> {
        if depth > 64 {
            return Err(format!("cycle while evaluating constant `{name}`"));
        }
        let Some(&ci) = self.const_index.get(name) else {
            return Err(format!("cannot find constant `{name}`"));
        };
        let e = crate::parse::parse_expr(&self.consts[ci as usize].expr)?;
        // (evaluated as the declared type: an unsuffixed literal is one (`const M: u8 = !0;` is 255), and
        // `const A: u8 = 255 + 1;` overflows)
        let decl = It::of_ty(self.consts[ci as usize].ty);
        // (its places are in its initializer's text, not the source's)
        let (v, t) = self.eval_t(&e, depth + 1, decl).map_err(unplaced)?;
        Ok(match decl {
            Some(it) => (it.wrap(v), Some(it)),
            None => (v, t),
        })
    }

    /// A constant expression's value and its integer type, when it has one (a suffixed literal, a cast, a typed
    /// constant, `u8::MAX`; an unsuffixed literal has none and takes the other operand's). A typed value is held in
    /// its type's range (a u128 as its bit pattern), and arithmetic on it is checked as rustc's: past the type is an
    /// error, `!` and `<<` keep its bits (`!0u8` is 255, so `!0u8 as u16` is 255)
    fn eval_t(&self, e: &crate::parse::Expr, depth: u32, want: Option<It>) -> R<(i128, Option<It>)> {
        use crate::parse::Expr;
        let un = |v: i128| Ok((v, None));
        match e {
            Expr::Lit(t, _) => {
                let v = lit_int(t).ok_or("not an integer constant")?;
                // (a suffix that is a type's: `255u8`, `1_i64`; not a float's)
                let it = ["u128", "i128", "usize", "isize", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8"]
                    .iter()
                    .find(|x| t.ends_with(*x) && !t.starts_with('\'') && !t.starts_with('"') && !t.starts_with('b'))
                    .and_then(|x| It::of_name(x))
                    // (a byte literal is a u8: `b'\x01'`)
                    .or_else(|| t.starts_with("b'").then(|| It::U8));
                // (unsuffixed: the type the context wants, `const M: u8 = 255`)
                Ok(match it.or(want) {
                    Some(it) => (it.wrap(v), Some(it)),
                    None => (v, None),
                })
            }
            Expr::Paren(x) => self.eval_t(x, depth, want),
            // (an integer `as` wraps to its type, as rustc's: `4294967297u64 as i32` is 1; another type keeps the value)
            Expr::Cast(x, t) => {
                let (v, _) = self.eval_t(x, depth, None)?;
                Ok(match It::of_name(&self.cast_prim(t)) {
                    Some(it) => (it.wrap(v), Some(it)),
                    None => (v, None),
                })
            }
            // (`{ 3 }` / `{ let x = ..; }` is not: a block of one expression; `if c { a } else { b }`)
            Expr::Block(None, _, b) if b.stmts.len() == 1 => match &b.stmts[0] {
                crate::parse::Stmt::Expr(x, false) => self.eval_t(x, depth, want),
                _ => Err("unsupported expression in constant".into()),
            },
            // (`{ let x = 2; 5 }`)
            Expr::Block(None, _, b) if b.stmts.len() > 1 => self.eval_block_env(b, &mut vec![], depth + 1, want),
            Expr::Ref(false, x) => self.eval_t(x, depth, want),
            Expr::If(c, t, Some(els)) => {
                let tail = |b: &crate::parse::Block| match b.stmts.as_slice() {
                    [crate::parse::Stmt::Expr(x, false)] => Some(x.clone()),
                    _ => None,
                };
                // (of this text: eval_t, which keeps an E0080's place, not eval_const)
                let c = self.eval_t(c, depth, None)?.0;
                let x = tail(t).ok_or("unsupported expression in constant")?;
                // (both arms are one type: `if true { !0 } else { 1u8 }` is a u8's !0; the other arm is not evaluated)
                let want = want.or_else(|| self.static_ty(&x)).or_else(|| self.static_ty(els));
                if c != 0 { self.eval_t(&x, depth, want) } else { self.eval_t(els, depth, want) }
            }
            // (an untyped local of a block: its expression at the wanted type, once per type)
            Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].name.starts_with("__zust_lazy_") => {
                let id: u64 = p.segs[0].name["__zust_lazy_".len()..].parse().map_err(|_| "bad local")?;
                let (x, hit) = LAZY.with(|l| {
                    let l = l.borrow();
                    let (x, memo, ..) = l.1.get(&id).ok_or("a local out of its block")?;
                    R::Ok((x.clone(), memo.iter().find(|m| m.0 == want).map(|m| m.1.clone())))
                })?;
                if let Some(r) = hit {
                    return r;
                }
                // (an earlier local only, so no cycle: the same depth, or a long chain would hit the limit)
                let r = self.eval_t(&x, depth, want);
                LAZY.with(|l| {
                    if let Some(e) = l.borrow_mut().1.get_mut(&id) {
                        e.1.push((want, r.clone()));
                    }
                });
                r
            }
            Expr::Path(None, p) if p.segs.len() == 1 && (p.segs[0].name == "true" || p.segs[0].name == "false") => un((p.segs[0].name == "true") as i128),
            // (`usize::max_value()`: the type's `MAX`, getrandom's `const FD_UNINIT: usize = usize::max_value();` (#265))
            Expr::Call(f, args) if crate::parse::int_bound_call(f, args).is_some() => {
                let (t, k) = crate::parse::int_bound_call(f, args).unwrap();
                self.prim_const_t(t, k)
            }
            // (`size_of::<T>()` / `align_of::<T>()` of a primitive, a pointer or an alias of one: an array length in
            //  libc 0.2, `[u8; 128 - 2 * size_of::<c_int>()]`. Other types are not known here)
            Expr::Call(f, args)
                if args.is_empty()
                    && matches!(&**f, Expr::Path(None, p) if p.segs.last().is_some_and(|s| matches!(s.name.as_str(), "size_of" | "align_of") && s.args.len() == 1)
                        // (a program's own fn named size_of / align_of is not core's: a `const fn` without type parameters is
                        //  called as itself below; a generic one is not evaluated here, as before, rather than taken for core's)
                        && !(p.segs.len() == 1 && (self.const_fns.contains_key(p.segs[0].name.as_str()) || self.index.contains_key(p.segs[0].name.as_str())))) =>
            {
                let Expr::Path(None, p) = &**f else { unreachable!() };
                let seg = p.segs.last().unwrap();
                let crate::parse::GArg::Ty(t) = &seg.args[0] else { return Err("size_of needs a type".into()) };
                // (a pointer to a slice, a str or a trait object is two words; its alignment is one word's)
                // (and to a struct with an unsized tail (`*const Tail<[u8]>`), by its resolved type)
                let fat = |i: &crate::parse::Ty| {
                    matches!(i, crate::parse::Ty::Slice(_) | crate::parse::Ty::Dyn(_))
                        || matches!(i, crate::parse::Ty::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "str")
                        || (matches!(i, crate::parse::Ty::Path(None, _))
                            && crate::ast_types::tysyn(i, None).ok().and_then(|syn| self.resolve_g(&syn, &[]).ok()).is_some_and(|t| t.is_unsized()))
                };
                let size = match t {
                    crate::parse::Ty::Ptr(_, i) | crate::parse::Ty::Ref(_, _, i) if fat(i) => Some(if seg.name == "align_of" { 8 } else { 16 }),
                    crate::parse::Ty::Ptr(..) | crate::parse::Ty::Ref(..) => Some(8),
                    crate::parse::Ty::Tuple(v) if v.is_empty() => Some(0),
                    crate::parse::Ty::Path(None, tp) if tp.segs.len() == 1 && tp.segs[0].args.is_empty() => {
                        // (C's types of core::ffi, `c_int`: their primitive, as in a signature (libc 0.2's
                        //  `4 * size_of::<c_int>()` in glibc's `__PTHREAD_MUTEX_INITIALIZER`))
                        let prim = self.prim_of_name(&tp.segs[0].name);
                        let prim = match crate::ast_types::ffi_alias(&prim) {
                            Some(t) if !self.struct_index.contains_key(prim.as_str()) => format!("{t:?}").to_lowercase(),
                            _ => prim,
                        };
                        match prim.as_str() {
                            "u8" | "i8" | "bool" => Some(1),
                            "u16" | "i16" => Some(2),
                            "u32" | "i32" | "f32" | "char" => Some(4),
                            "u64" | "i64" | "f64" | "usize" | "isize" => Some(8),
                            "u128" | "i128" => Some(16),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                // (on x86-64 these types' alignment is their size, except the empty tuple's, which is 1)
                let v = match size {
                    Some(0) if seg.name == "align_of" => 1,
                    Some(n) => n,
                    None => return Err(format!("cannot evaluate `{}::<{}>()` in a constant", seg.name, crate::parse::ty_text(t))),
                };
                // (both return a usize: `size_of::<u64>().leading_zeros()` counts 64 bits)
                Ok((v, It::of_name("usize")))
            }
            // (a call of a `const fn` with integer arguments: its body with the parameters bound)
            Expr::Call(f, args) if matches!(&**f, Expr::Path(None, p) if p.segs.len() == 1 && self.const_fns.contains_key(p.segs[0].name.as_str())) => {
                let Expr::Path(None, p) = &**f else { unreachable!() };
                if depth > 48 {
                    return Err("constant evaluation recursed too deeply".into());
                }
                let (ps, body) = self.const_fns[p.segs[0].name.as_str()].clone();
                if ps.len() != args.len() {
                    return Err("wrong number of arguments in a constant call".into());
                }
                // (its parameters and result are its signature's integer types: `const fn flip(x: u8) -> u8 { !x >> 4 }`
                // shifts a u8; a parameter or result of another type is untyped)
                let (pts, rt) = self.const_fn_tys.get(p.segs[0].name.as_str()).cloned().unwrap_or_default();
                let int_of = |n: &str| It::of_name(&self.prim_of_name(n));
                let mut env: Vec<(String, i128, Option<It>, Option<crate::parse::Expr>)> = vec![];
                for (k, (n, a)) in ps.iter().zip(args).enumerate() {
                    let pt = pts.get(k).and_then(|t| int_of(t));
                    let (v, t) = self.eval_t(a, depth + 1, pt)?;
                    let t = pt.or(t);
                    env.push((n.clone(), t.map_or(v, |it| it.wrap(v)), t, None));
                }
                let b = crate::parse::parse_expr(&body)?;
                let Expr::Block(_, _, b) = b else { return Err("unsupported const fn".into()) };
                let rt = int_of(&rt);
                // (its places are in the body's text, not the source's: an E0080 there is at the call, as rustc's
                // `evaluation of `X` failed inside this call`)
                let (v, t) = self.eval_block_env(&b, &mut env, depth + 1, rt.or(want)).map_err(|e| placed_at(unplaced(e), p.pos))?;
                match rt.or(t).or(want) {
                    Some(it) => Ok((it.wrap(v), Some(it))),
                    None => un(v),
                }
            }
            // (`X[0]` / `T.1` of a const array, byte string, tuple or struct)
            Expr::Index(b, i, _) => {
                let i = self.eval_t(i, depth, None)?.0;
                let (part, other) = self.const_part(b, depth, &|x| match x {
                    Expr::Array(xs) => usize::try_from(i).ok().and_then(|i| xs.get(i).cloned()),
                    Expr::Repeat(x, _) => Some((**x).clone()),
                    Expr::Lit(t, p) => {
                        let (_, bs) = crate::ast_consteval::str_lit(t)?;
                        let b = *bs.get(usize::try_from(i).ok()?)?;
                        Some(Expr::Lit(b.to_string(), *p))
                    }
                    _ => None,
                })?;
                // (an element of a typed constant array is its element type: `const A: [u8; 1] = [!0]`)
                let et = match self.const_ty_of(b) {
                    Some(Ty::Array(e, _)) => It::of_ty(*e),
                    _ => self.early_part_ty(b, None),
                };
                // (a part of another constant has its places in that constant's text; one of an array written here,
                // `[255u8 + 1][0]`, in this one)
                let r = self.eval_t(&part, depth + 1, want.or(et));
                if other { r.map_err(unplaced) } else { r }
            }
            Expr::Field(b, f, _) => {
                let (part, other) = self.const_part(b, depth, &|x| match x {
                    Expr::Tuple(xs) => f.parse::<usize>().ok().and_then(|k| xs.get(k).cloned()),
                    Expr::Struct(_, fs, _) => fs.iter().find(|x| &x.0 == f).map(|x| x.1.clone()),
                    Expr::Call(_, xs) => f.parse::<usize>().ok().and_then(|k| xs.get(k).cloned()),
                    _ => None,
                })?;
                // (a field of a typed constant is its field's type: `const T: (u8,) = (!0,)`)
                let ft = match self.const_ty_of(b) {
                    Some(Ty::Struct(id)) => {
                        let sd = &self.structs[id as usize];
                        sd.fields.iter().find(|x| &x.0 == f).map(|x| x.1).and_then(It::of_ty)
                    }
                    _ => self.early_part_ty(b, f.parse::<usize>().ok()),
                };
                let r = self.eval_t(&part, depth + 1, want.or(ft));
                if other { r.map_err(unplaced) } else { r }
            }
            // (`-` of a literal is the negative literal: `-128i8` is i8::MIN, and so is `-(128i8)`; `-i8::MIN` overflows)
            Expr::Unary(op, x, _) if op == "-" && matches!(strip_parens(x), Expr::Lit(..)) => {
                let Expr::Lit(t, _) = strip_parens(x) else { unreachable!() };
                let raw = lit_int(t).ok_or("not an integer constant")?;
                let (_, it) = self.eval_t(x, depth, want)?;
                match it {
                    // (`-170141183460469231731687303715884105728i128` is i128::MIN: its magnitude wrapped to MIN)
                    Some(it) if it.signed && it.bits == 128 && raw == i128::MIN => Ok((i128::MIN, Some(it))),
                    Some(it) => Ok((it.check(raw.checked_neg().filter(|_| (it.signed || raw == 0) && raw >= 0))?, Some(it))),
                    None => un(raw.wrapping_neg()),
                }
            }
            // (`!` of a bool is the logical not: `(!true) as u8` is 0, not the complement of 1)
            Expr::Unary(op, x, _) if op == "!" && is_bool_expr(x) => {
                let (v, _) = self.eval_t(x, depth, None)?;
                un((v == 0) as i128)
            }
            Expr::Unary(op, x, sp) => {
                let (v, t) = self.eval_t(x, depth, want)?;
                match (op.as_str(), t) {
                    // (`-` of a typed value: checked (`-i8::MIN` is E0080); of an unsigned one, an error as rustc's)
                    ("-", Some(it)) if it.signed => match v.checked_neg().filter(|&n| it.fits(n)) {
                        Some(n) => Ok((n, t)),
                        None => Err(it.arith_err("neg", v, 0, None, *sp)),
                    },
                    ("-", Some(it)) => Ok((it.check(v.checked_neg().filter(|_| v == 0))?, t)),
                    ("-", None) => un(v.wrapping_neg()),
                    // (`!` of a typed value: its bits only)
                    ("!", Some(it)) => Ok((it.wrap(!v), t)),
                    ("!", None) => un(!v),
                    ("*", _) => Ok((v, t)),
                    _ => Err("not an integer constant".into()),
                }
            }
            Expr::Binary(op, l, r, sp) => {
                let op = op.as_str();
                let shift = op == "<<" || op == ">>";
                let cmp = matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||");
                // (the operands are the wanted type, but a comparison's are not its bool, and a shift's right one is
                // its own)
                let ow = if cmp { None } else { want };
                // (`&&` / `||` short-circuit: `true || (u8::MAX + 1 == 0)` is true)
                if matches!(op, "&&" | "||") {
                    let lv = self.eval_t(l, depth, None)?.0;
                    if (op == "&&") == (lv == 0) {
                        return un((op == "||") as i128);
                    }
                    return un((self.eval_t(r, depth, None)?.0 != 0) as i128);
                }
                // (the operands' common type, read before they are evaluated: in `(4 / ((!0 >> 7) + 1)) & 7u8` the
                // left one is a u8 too; a shift's right operand is its own)
                let common = if shift { ow.or_else(|| self.static_ty(l)) } else { ow.or_else(|| self.static_ty(l)).or_else(|| self.static_ty(r)) };
                let l0 = self.eval_t(l, depth, common)?;
                let r0 = self.eval_t(r, depth, if shift { None } else { common })?;
                // (an unsuffixed operand is the other's type, evaluated as it: in `!0 & 7u8` the `!0` is a u8's)
                let mutual = !shift && !matches!(op, "&&" | "||");
                let l1 = if mutual && l0.1.is_none() && r0.1.is_some() { self.eval_t(l, depth, r0.1)? } else { l0 };
                let r1 = if mutual && r0.1.is_none() && l1.1.is_some() { self.eval_t(r, depth, l1.1)? } else { r0 };
                let ((lv, lt), (rv, rt)) = (l1, r1);
                // (two operands of different integer types: `i8::MAX + 1u8` is rustc's E0308, not an overflow)
                if !shift {
                    if let (Some(a), Some(b)) = (lt, rt) {
                        if a != b {
                            return Err(format!("mismatched types: expected `{}`, found `{}`", a.name(), b.name()));
                        }
                    }
                }
                // (a shift has its left operand's type; another operator its operands' (one may be unsuffixed))
                let t = if op == "<<" || op == ">>" { lt } else { lt.or(rt) };
                let b = |x: bool| un(x as i128);
                match t {
                    // (u128: computed as u128, held as its bit pattern)
                    Some(it) if it.bits == 128 && !it.signed => {
                        let (x, y) = (lv as u128, rv as u128);
                        let v: Option<u128> = match op {
                            "+" => x.checked_add(y),
                            "-" => x.checked_sub(y),
                            "*" => x.checked_mul(y),
                            "/" => x.checked_div(y),
                            "%" => x.checked_rem(y),
                            "<<" => u32::try_from(rv).ok().and_then(|n| x.checked_shl(n)),
                            ">>" => u32::try_from(rv).ok().and_then(|n| x.checked_shr(n)),
                            "&" => Some(x & y),
                            "|" => Some(x | y),
                            "^" => Some(x ^ y),
                            "==" => return b(x == y),
                            "!=" => return b(x != y),
                            "<" => return b(x < y),
                            "<=" => return b(x <= y),
                            ">" => return b(x > y),
                            ">=" => return b(x >= y),
                            "&&" => return b(x != 0 && y != 0),
                            "||" => return b(x != 0 || y != 0),
                            _ => return Err("unsupported operator in constant".into()),
                        };
                        match v {
                            Some(v) => Ok((v as i128, t)),
                            None => Err(it.arith_err(op, lv, rv, rt, *sp)),
                        }
                    }
                    Some(it) => {
                        let v = match op {
                            "+" => lv.checked_add(rv),
                            "-" => lv.checked_sub(rv),
                            "*" => lv.checked_mul(rv),
                            "/" => lv.checked_div(rv),
                            // (`MIN % -1` overflows at the type's width, as `MIN / -1`)
                            "%" if it.signed && rv == -1 && lv == it.wrap(1i128 << (it.bits - 1)) => None,
                            "%" => lv.checked_rem(rv),
                            // (a shift past the width is an error; `<<` drops the bits shifted out)
                            "<<" if (0..it.bits as i128).contains(&rv) => return Ok((it.wrap(lv << rv), t)),
                            ">>" if (0..it.bits as i128).contains(&rv) => Some(lv >> rv),
                            "<<" | ">>" => None,
                            "&" => Some(lv & rv),
                            "|" => Some(lv | rv),
                            "^" => Some(lv ^ rv),
                            "==" => return b(lv == rv),
                            "!=" => return b(lv != rv),
                            "<" => return b(lv < rv),
                            "<=" => return b(lv <= rv),
                            ">" => return b(lv > rv),
                            ">=" => return b(lv >= rv),
                            "&&" => return b(lv != 0 && rv != 0),
                            "||" => return b(lv != 0 || rv != 0),
                            _ => return Err("unsupported operator in constant".into()),
                        };
                        // (an unsuffixed operand of a typed one is that type: it must fit in it too)
                        if op != "<<" && op != ">>" && !(it.fits(lv) && it.fits(rv)) {
                            return Err("overflow in constant".into());
                        }
                        match v.filter(|&v| it.fits(v)) {
                            Some(v) => Ok((v, t)),
                            None => Err(it.arith_err(op, lv, rv, rt, *sp)),
                        }
                    }
                    None => match op {
                        "+" => un(lv.wrapping_add(rv)),
                        "-" => un(lv.wrapping_sub(rv)),
                        "*" => un(lv.wrapping_mul(rv)),
                        "/" if rv != 0 => un(lv.wrapping_div(rv)),
                        "%" if rv != 0 => un(lv.wrapping_rem(rv)),
                        "<<" => un(lv.wrapping_shl(rv as u32)),
                        ">>" => un(lv.wrapping_shr(rv as u32)),
                        "&" => un(lv & rv),
                        "|" => un(lv | rv),
                        "^" => un(lv ^ rv),
                        "==" => b(lv == rv),
                        "!=" => b(lv != rv),
                        "<" => b(lv < rv),
                        "<=" => b(lv <= rv),
                        ">" => b(lv > rv),
                        ">=" => b(lv >= rv),
                        "&&" => b(lv != 0 && rv != 0),
                        "||" => b(lv != 0 || rv != 0),
                        _ => Err("unsupported operator in constant".into()),
                    },
                }
            }
            Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => self.const_int_t(&p.segs[0].name, depth),
            // `<u64 as Integer>::MAX_STR_LEN`
            Expr::Path(Some(q), p) if matches!(&**q, crate::parse::Ty::Path(None, tp) if tp.segs.len() == 1) && !p.segs.is_empty() => {
                let crate::parse::Ty::Path(None, tp) = &**q else { unreachable!() };
                // (the trait impl's under its qualified name first, where an inherent const took the plain one)
                if p.segs.len() == 2 {
                    let q = format!("{} as {}::{}", tp.segs[0].name, p.segs[0].name, p.segs[1].name);
                    if self.const_index.contains_key(&q) {
                        return self.const_int_t(&q, depth);
                    }
                    if let Some(k) = self.blanket_defines(&p.segs[0].name, &p.segs[1].name) {
                        return self.const_int_t(&k, depth);
                    }
                    let d = format!("{}::{}", p.segs[0].name, p.segs[1].name);
                    if self.const_index.contains_key(&d) {
                        return self.const_int_t(&d, depth);
                    }
                }
                let full = format!("{}::{}", tp.segs[0].name, p.segs.last().unwrap().name);
                if self.const_index.contains_key(&full) {
                    return self.const_int_t(&full, depth);
                }
                let t = Ty::prim(&tp.segs[0].name).ok_or("unsupported path in constant")?;
                self.prim_const_t(t, &p.segs.last().unwrap().name)
            }
            Expr::Path(None, p) if p.segs.len() == 2 => {
                // (an impl's associated const: `i128::MAX_STR_LEN`)
                let full = format!("{}::{}", p.segs[0].name, p.segs[1].name);
                if self.const_index.contains_key(&full) {
                    return self.const_int_t(&full, depth);
                }
                // (`Foo::Bar as usize`: a unit variant's discriminant)
                if let Some((sid, vi)) = self.variant_by_p(&PPath::of(p), None) {
                    // (as a value of the repr type: `#[repr(u64)]`'s 2^63 is not negative)
                    let d = self.structs[sid as usize].variants[vi].discr as i128;
                    let rt = self.reprs.get(&self.structs[sid as usize].name).copied().and_then(It::of_ty);
                    return Ok(match rt {
                        Some(it) => (it.wrap(d), Some(it)),
                        None => (d, None),
                    });
                }
                // (through an alias of a primitive: `type Mask = u16;` and `Mask::BITS`)
                let t = Ty::prim(&p.segs[0].name).or_else(|| Ty::prim(&self.prim_of_name(&p.segs[0].name))).ok_or("unsupported path in constant")?;
                self.prim_const_t(t, &p.segs[1].name)
            }
            // Integer methods (`u128::MAX.ilog10()`): by the receiver type's width and sign (`(-1i8).count_ones()` is 8)
            Expr::MethodCall(recv, m, _, args, sp) => {
                let (v, t) = self.eval_t(recv, depth, None)?;
                let bits = t.map_or(128, |it| it.bits);
                let unsigned = t.is_some_and(|it| !it.signed);
                let u = if bits < 128 { (v as u128) & ((1u128 << bits) - 1) } else { v as u128 };
                let u32t = Some(It::U32);
                // (an argument is its parameter's type: `pow`'s exponent a u32, `min` / `max`'s the receiver's)
                let aw = if m == "pow" { u32t } else { t };
                let a: Vec<i128> = args.iter().map(|x| Ok(self.eval_t(x, depth, aw)?.0)).collect::<R<Vec<i128>>>()?;
                Ok(match (m.as_str(), a.as_slice()) {
                    ("ilog10", []) if (unsigned && u != 0) || (!unsigned && v > 0) => {
                        let mut n = if unsigned { u } else { v as u128 };
                        let mut k = 0;
                        while n >= 10 {
                            n /= 10;
                            k += 1;
                        }
                        (k, u32t)
                    }
                    ("ilog2", []) if (unsigned && u != 0) || (!unsigned && v > 0) => ((127 - (if unsigned { u } else { v as u128 }).leading_zeros()) as i128, u32t),
                    ("count_ones", []) => (u.count_ones() as i128, u32t),
                    ("count_zeros", []) => ((bits - u.count_ones()) as i128, u32t),
                    ("trailing_zeros", []) => (u.trailing_zeros().min(bits) as i128, u32t),
                    ("leading_zeros", []) => ((u.leading_zeros() - (128 - bits)) as i128, u32t),
                    ("pow", [e]) => {
                        // (a u128 as u128: its bit pattern is not a positive i128 past i128::MAX)
                        let p = if bits == 128 && unsigned { (v as u128).checked_pow(*e as u32).map(|p| p as i128) } else { v.checked_pow(*e as u32) };
                        match t {
                            // (past the receiver's type: rustc's E0080 of the multiplication in core's pow that
                            // overflows, at the call)
                            Some(it) => match p.filter(|&p| it.fits(p)) {
                                Some(p) => (p, t),
                                None => return Err(it.pow_err(v, *e as u32, *sp)),
                            },
                            None => (p.ok_or("overflow in constant")?, t),
                        }
                    }
                    // (a u128 compares as unsigned: its bit pattern past i128::MAX is negative)
                    ("min", [b]) if bits == 128 && unsigned => ((v as u128).min(*b as u128) as i128, t),
                    ("max", [b]) if bits == 128 && unsigned => ((v as u128).max(*b as u128) as i128, t),
                    ("min", [b]) => (v.min(*b), t),
                    ("max", [b]) => (v.max(*b), t),
                    // (checked at the receiver's width: `i8::MIN.abs()` overflows, rustc's E0080 of the `-self` in core's
                    // abs, at the call)
                    ("abs", []) => match t {
                        Some(it) if it.signed => match v.checked_abs().filter(|&x| it.fits(x)) {
                            Some(x) => (x, t),
                            None => return Err(it.arith_err("neg", v, 0, None, *sp)),
                        },
                        Some(it) => (it.check(v.checked_abs())?, t),
                        None => (v.checked_abs().ok_or("overflow in constant")?, t),
                    },
                    _ => return Err(format!("unsupported method `{m}` in constant")),
                })
            }
            _ => Err("unsupported expression in constant".into()),
        }
    }

    /// The declared type of the constant e names (through parentheses and `&`)
    fn const_ty_of(&self, e: &crate::parse::Expr) -> Option<Ty> {
        use crate::parse::Expr;
        match e {
            Expr::Paren(x) | Expr::Ref(_, x) => self.const_ty_of(x),
            // (through references: `const R: &[u8; 1]` indexes as a `[u8; 1]`)
            Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => {
                let mut t = self.const_index.get(p.segs[0].name.as_str()).map(|&ci| self.consts[ci as usize].ty)?;
                while let Ty::Ref(_, x) = t {
                    t = *x;
                }
                Some(t)
            }
            _ => None,
        }
    }

    /// Before constant types are resolved (a layout's `[u8; N]`): the integer type of an element (k None) or tuple
    /// field k of the constant b, from its declared type as written
    fn early_part_ty(&self, b: &crate::parse::Expr, k: Option<usize>) -> Option<It> {
        use crate::parse::Expr;
        let b = strip_parens(b);
        let Expr::Path(None, p) = b else { return None };
        if p.global || p.segs.len() != 1 {
            return None;
        }
        let ci = *self.const_index.get(p.segs[0].name.as_str())?;
        if self.consts[ci as usize].ty != Ty::Unit {
            return None;
        }
        let mut t = self.early_syn.get(&ci)?;
        while let TySyn::Ref(_, x) = t {
            t = x;
        }
        let e = match (t, k) {
            (TySyn::Array(e, _), None) => &**e,
            (TySyn::Tuple(xs), Some(k)) => xs.get(k)?,
            _ => return None,
        };
        early_int_ty(e, &self.aliases).and_then(It::of_ty)
    }

    /// e's integer type as written, without evaluating it (a suffix, a cast, a constant's declared type, `u8::MAX`,
    /// an operator's operands'; None for an unsuffixed literal): what an unsuffixed literal beside it is
    fn static_ty(&self, e: &crate::parse::Expr) -> Option<It> {
        use crate::parse::Expr;
        match e {
            Expr::Lit(t, _) => {
                if t.starts_with("b'") {
                    return Some(It::U8);
                }
                if t.starts_with('\'') || t.starts_with('"') || t.starts_with('b') {
                    return None;
                }
                ["u128", "i128", "usize", "isize", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8"].iter().find(|x| t.ends_with(*x)).and_then(|x| It::of_name(x))
            }
            Expr::Paren(x) | Expr::Ref(_, x) => self.static_ty(x),
            Expr::Unary(op, x, _) if op == "-" || op == "!" => self.static_ty(x),
            Expr::Cast(_, t) => It::of_name(&self.cast_prim(t)),
            // (`u32::max_value()`, as `u32::MAX`)
            Expr::Call(f, args) if crate::parse::int_bound_call(f, args).is_some() => It::of_ty(crate::parse::int_bound_call(f, args)?.0),
            Expr::Binary(op, l, r, _) => match op.as_str() {
                "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||" => None,
                "<<" | ">>" => self.static_ty(l),
                _ => self.static_ty(l).or_else(|| self.static_ty(r)),
            },
            Expr::Block(None, _, b) => match b.stmts.last() {
                Some(crate::parse::Stmt::Expr(x, false)) => self.static_ty(x),
                _ => None,
            },
            Expr::If(_, t, Some(els)) => match t.stmts.last() {
                Some(crate::parse::Stmt::Expr(x, false)) => self.static_ty(x),
                _ => None,
            }
            .or_else(|| self.static_ty(els)),
            Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => self.const_index.get(p.segs[0].name.as_str()).and_then(|&ci| It::of_ty(self.consts[ci as usize].ty)),
            // (a declared associated constant first, as eval_t: `impl S { const BITS: u8 = 7; }`)
            Expr::Path(None, p) if p.segs.len() == 2 && self.const_index.contains_key(&format!("{}::{}", p.segs[0].name, p.segs[1].name)) => {
                let ci = self.const_index[&format!("{}::{}", p.segs[0].name, p.segs[1].name)];
                It::of_ty(self.consts[ci as usize].ty)
            }
            Expr::Path(None, p) if p.segs.len() == 2 => match (Ty::prim(&p.segs[0].name).or_else(|| Ty::prim(&self.prim_of_name(&p.segs[0].name))), p.segs[1].name.as_str()) {
                (Some(_), "BITS") => Some(It::U32),
                (Some(t), _) => It::of_ty(t),
                _ => None,
            },
            Expr::MethodCall(recv, m, ..) => match m.as_str() {
                "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" | "ilog2" | "ilog10" => Some(It::U32),
                "pow" | "min" | "max" | "abs" => self.static_ty(recv),
                _ => None,
            },
            Expr::Index(b, _, _) => match self.const_ty_of(b) {
                Some(Ty::Array(e, _)) => It::of_ty(*e),
                _ => self.early_part_ty(b, None),
            },
            _ => None,
        }
    }

    /// `u8::MAX` and the like, with their type (`BITS` is a u32)
    fn prim_const_t(&self, t: Ty, name: &str) -> R<(i128, Option<It>)> {
        let v = prim_assoc_const(t, name).ok_or("unsupported constant")?;
        Ok((v, if name == "BITS" { Some(It::U32) } else { It::of_ty(t) }))
    }

    /// The (built-in) generic definition of an n-element tuple
    /// The placeholder type for type parameter k (named `name`, bounded by the traits `bounds`) of fn id: a struct
    /// with no fields, named `name#` (no program's type has that name), that implements just those traits
    /// Whether t may be unsized: `[T]`, `str`, `dyn Tr`, a struct with such a tail, or the placeholder of a parameter
    /// declared `?Sized`
    pub fn maybe_unsized(&self, t: Ty) -> bool {
        t.is_unsized() || matches!(t, Ty::Struct(s) if self.structs[s as usize].maybe_unsized)
    }

    /// Whether t may be unsized when may says so of some types (a definition's `?Sized` parameters): those, or a
    /// struct whose unsized tail is one (`W<T>` with `struct W<T: ?Sized>(T)` of a `T: ?Sized`)
    pub fn maybe_unsized_with(&self, t: Ty, may: &dyn Fn(Ty) -> bool) -> bool {
        if may(t) || self.maybe_unsized(t) {
            return true;
        }
        match t {
            Ty::Struct(s) => {
                let sd = &self.structs[s as usize];
                sd.base.is_some_and(|b| !self.is_tuple(b) && self.instance_unsized_with(b, &sd.args, &|a| self.maybe_unsized_with(a, may)))
            }
            _ => false,
        }
    }

    /// The first type in t that may be unsized (maybe_unsized) where only a sized one may be: an argument of the
    /// program's struct or enum for a parameter not declared `?Sized`, of std's `Vec` / `Option` / `Result` and the
    /// collections, a tuple's element other than the last, a slice's or array's element (E0277)
    pub fn sized_wf(&self, t: Ty, depth: u32) -> Option<Ty> {
        self.sized_wf_with(t, &|t| self.maybe_unsized(t), depth)
    }

    /// sized_wf with may_unsized saying which types may be unsized (a definition's `?Sized` parameters too)
    pub fn sized_wf_with(&self, t: Ty, may_unsized: &dyn Fn(Ty) -> bool, depth: u32) -> Option<Ty> {
        if depth > 16 {
            return None;
        }
        match t {
            Ty::Struct(s) => {
                let sd = &self.structs[s as usize];
                let b = sd.base?;
                let tuple = self.is_tuple(b);
                let user = !tuple && self.struct_item.get(b as usize).is_some_and(|&ii| ii < self.prelude_from) && !self.is_closure(b);
                let std_sized = !user && !tuple && self.struct_item.get(b as usize).is_some_and(|&ii| ii >= self.prelude_from) && matches!(self.structs[b as usize].name.as_str(), "Vec" | "Option" | "Result" | "VecDeque" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "BinaryHeap" | "LinkedList");
                let n = sd.args.len();
                for (k, &a) in sd.args.iter().enumerate() {
                    let may = if tuple {
                        k + 1 == n
                    } else if user {
                        self.unsized_params.get(&b).is_some_and(|v| v.contains(&k))
                    } else {
                        !std_sized
                    };
                    if !may && may_unsized(a) {
                        return Some(a);
                    }
                    if let Some(x) = self.sized_wf_with(a, may_unsized, depth + 1) {
                        return Some(x);
                    }
                }
                None
            }
            Ty::Slice(e) | Ty::Array(e, _) => {
                if may_unsized(*e) {
                    return Some(*e);
                }
                self.sized_wf_with(*e, may_unsized, depth + 1)
            }
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) => self.sized_wf_with(*i, may_unsized, depth + 1),
            Ty::Fn(f) => f.params.iter().chain([&f.ret]).find_map(|&x| self.sized_wf_with(x, may_unsized, depth + 1)),
            _ => None,
        }
    }

    pub fn placeholder(&self, id: u32, k: u32, name: &str, bounds: &[String]) -> u32 {
        if let Some(&s) = self.structs.placeholders.lock().unwrap().get(&(id, k)) {
            return s;
        }
        let mut d = StructDef::blank(format!("{name}#"), false);
        d.placeholder = Some(bounds.to_vec());
        d.maybe_unsized = self.fn_unsized_gens.get(&id).is_some_and(|v| v.contains(&(k as usize)));
        let mut h = KeyHasher::new();
        h.add(&("placeholder", id, k));
        d.deep = h.finish();
        let mut g = self.structs.placeholders.lock().unwrap();
        if let Some(&s) = g.get(&(id, k)) {
            return s;
        }
        let s = self.structs.push(d);
        g.insert((id, k), s);
        self.structs.any_placeholder.store(true, std::sync::atomic::Ordering::Relaxed);
        s
    }

    /// A placeholder's associated type (`<T# as MyTrait>::Output` with `T: MyTrait` and `trait MyTrait { type Output;
    /// }`): a placeholder of its own, made once, bounded as the trait declares it. None where no bound's trait
    /// declares it, or the bound binds it (`T: Iterator<Item = u32>`)
    pub fn assoc_placeholder(&self, sid: u32, name: &str) -> Option<u32> {
        if let Some(&s) = self.structs.assoc_placeholders.lock().unwrap().0.get(&(sid, name.to_string())) {
            return Some(s);
        }
        let bs = self.structs[sid as usize].placeholder.as_ref()?;
        let (b, decl) = bs.iter().find_map(|b| {
            let tr = b.split('<').next().unwrap_or(b).trim();
            let short = tr.rsplit("::").next().unwrap_or(tr);
            self.assoc_bounds.get(&(short.to_string(), name.to_string())).map(|d| (b, d))
        })?;
        // (a binding of it in the bound: its type is the bound's, not a placeholder)
        if b.split(|c: char| c == '<' || c == ',').any(|a| a.split('=').next().is_some_and(|x| x.trim() == name) && a.contains('=')) {
            return None;
        }
        let base = self.structs[sid as usize].name.trim_end_matches('#').to_string();
        // (the bound as written, its tokens joined: `MyTrait < 'a >` shown `MyTrait<'a>`)
        let shown = b.replace(" <", "<").replace("< ", "<").replace(" >", ">").replace(" ,", ",");
        let mut d = StructDef::blank(format!("<{base} as {shown}>::{name}#"), false);
        d.placeholder = Some(decl.iter().filter(|x| !x.starts_with('\'')).cloned().collect());
        let mut h = KeyHasher::new();
        h.add(&("assoc placeholder", self.structs[sid as usize].deep, name));
        d.deep = h.finish();
        let mut g = self.structs.assoc_placeholders.lock().unwrap();
        if let Some(&s) = g.0.get(&(sid, name.to_string())) {
            return Some(s);
        }
        let s = self.structs.push(d);
        g.0.insert((sid, name.to_string()), s);
        g.1.insert(s, (sid, b.clone(), name.to_string()));
        Some(s)
    }

    /// t is a placeholder or has one in it (`&G#`, `Vec<T#>`)
    pub fn holds_placeholder(&self, t: Ty, depth: u32) -> bool {
        if depth > 16 {
            return false;
        }
        match t {
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Slice(i) | Ty::Array(i, _) => self.holds_placeholder(*i, depth + 1),
            Ty::Struct(s) => self.is_placeholder(s) || self.structs[s as usize].args.iter().any(|&a| self.holds_placeholder(a, depth + 1)),
            _ => false,
        }
    }

    /// t is a placeholder itself (its bounds are what it implements)
    pub fn is_placeholder_ty(&self, t: Ty) -> bool {
        matches!(t, Ty::Struct(s) if self.is_placeholder(s))
    }

    /// What a placeholder's associated type is of: (the placeholder, the bound whose trait declares it, its name)
    pub fn assoc_placeholder_of(&self, s: u32) -> Option<(u32, String, String)> {
        self.structs.assoc_placeholders.lock().unwrap().1.get(&s).cloned()
    }

    /// Whether the program has placeholders for type parameters (zustc check's last round) at all
    pub fn any_placeholder(&self) -> bool {
        self.structs.any_placeholder.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether t is surely not Clone (`r.clone()` of `&t` is then the reference's clone): a type of the program with no
    /// clone and no derive, or an instance with such an argument (`Option<NoClone>`), but for the shared pointers
    pub fn surely_not_clone(&self, t: Ty, depth: u32) -> bool {
        // (`&mut T` is not Clone: `Option<&mut T>` neither; an array or a Box is Clone when its element is)
        let sid = match t {
            Ty::Ref(true, _) => return true,
            Ty::Array(e, _) | Ty::Box(e) => return depth <= 4 && self.surely_not_clone(*e, depth + 1),
            Ty::Struct(sid) => sid,
            _ => return false,
        };
        // (a type parameter's placeholder: Clone when a bound is, or has Clone among its supertraits (`trait Dup:
        // Clone`, also through others); a trait with supertraits may imply it. `(&x).clone()` of `fn f<T>(x: T)` is the
        // reference's copy)
        if let Some(bs) = &self.structs[sid as usize].placeholder {
            return !bs.iter().any(|b| {
                let n = b.split('<').next().unwrap_or(b).trim();
                self.implies_clone(b, 0) || n.ends_with("Clone") || n.ends_with("Copy") || self.trait_supers.get(n).is_some_and(|s| !s.is_empty())
            });
        }
        let b = self.base_of(sid);
        let own = self.struct_item.get(b as usize).is_some_and(|&ii| ii < self.prelude_from);
        // (a clone of the program's own; std's (`Vec<T>`) is Clone when its arguments are)
        if depth > 4 || (own && self.method(sid, "clone").is_some()) {
            return false;
        }
        // (a closure is Clone when its captures are, whatever its signature: `|x: NotClone| -> NotClone {..}` is)
        if self.is_closure(b) {
            return self.structs[sid as usize].fields.iter().any(|f| self.surely_not_clone(f.1, depth + 1));
        }
        let derived = self.derived.get(&b).is_some_and(|d| d.iter().any(|x| x == "Clone"));
        if own && !derived && !self.implements(t, "Clone") {
            return true;
        }
        self.clone_needs_args(b, own) && self.structs[sid as usize].args.iter().any(|&a| self.surely_not_clone(a, depth + 1))
    }

    /// A bound b makes its type Clone: `Clone`, `Copy`, a trait with one of them among its supertraits, or a trait zustc
    /// does not know (which may have)
    fn implies_clone(&self, b: &str, depth: u32) -> bool {
        self.bound_implies(b, &["Clone", "Copy"], true, depth)
    }

    /// A bound b's trait has an associated-type equation among its supertraits (`trait View<U>: Deref<Target = U>`,
    /// the summary's `#Eq` mark), through any number of them
    pub fn bound_has_eq(&self, b: &str, _depth: u32) -> bool {
        // (every supertrait once, however long the chain: a visited set, no cutoff)
        let mut seen: Vec<&str> = vec![];
        let mut stack: Vec<&str> = vec![b];
        while let Some(b) = stack.pop() {
            let tr = b.split('<').next().unwrap_or(b).trim();
            let tr = tr.rsplit("::").next().unwrap_or(tr);
            if seen.contains(&tr) {
                continue;
            }
            seen.push(tr);
            if let Some(ss) = self.trait_supers.get(tr) {
                if ss.iter().any(|s| s == "#Eq") {
                    return true;
                }
                stack.extend(ss.iter().map(|s| s.as_str()));
            }
        }
        false
    }

    /// A bound b gives one of the traits `want`: itself, or through its supertraits. A trait zustc does not know (a
    /// std one's, `Fn(u8)`) gives them when `unknown` says so: as the caller's safe side is
    pub fn bound_implies(&self, b: &str, want: &[&str], unknown: bool, _depth: u32) -> bool {
        // (every supertrait once, however long the chain: a visited set, no cutoff)
        let mut seen: Vec<&str> = vec![];
        let mut stack: Vec<&str> = vec![b];
        while let Some(b) = stack.pop() {
            let tr = b.split('<').next().unwrap_or(b).trim();
            let tr = tr.rsplit("::").next().unwrap_or(tr);
            if want.contains(&tr) {
                return true;
            }
            if seen.contains(&tr) {
                continue;
            }
            seen.push(tr);
            if matches!(tr, "Sized" | "?Sized" | "Send" | "Sync" | "Unpin" | "Debug" | "Display" | "Default" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" | "Clone") {
                continue;
            }
            if self.trait_marker_supers.get(tr).is_some_and(|ms| ms.iter().any(|m| want.contains(&m.as_str()) || (m == "Copy" && want.contains(&"Clone")))) {
                return true;
            }
            match self.trait_supers.get(tr) {
                Some(ss) => stack.extend(ss.iter().map(|s| s.as_str())),
                None if unknown && !self.traits.contains_key(tr) => return true,
                None => {}
            }
        }
        false
    }

    /// Field i of the struct sid is a closure's capture held by reference: its type is a raw pointer, which the auto
    /// traits take as the reference it stands for. Some(true): a `&mut T` (the body assigns to it or borrows it
    /// mutably), Send when T is Send; Some(false): either kind (a `&mut self` method may be called on it), so Send when
    /// T is Sync and Send. Sync when T is Sync either way
    pub fn ref_capture(&self, sid: u32, i: usize) -> Option<bool> {
        let b = self.base_of(sid);
        let cd = self.closure_of_struct.get(&b).and_then(|f| self.closures.get(f))?;
        (!cd.by_move && cd.moved.get(i) == Some(&false)).then(|| cd.mutated.get(i) == Some(&true))
    }

    /// The base b is a closure's struct (`f::{closure#0}`)
    pub fn is_closure(&self, b: u32) -> bool {
        self.structs[b as usize].name.contains("{closure#")
    }

    /// Whether the base b is Clone only when its arguments are: a derive of the program's own, and std's containers
    /// (`Option<T>: Clone` needs `T: Clone`). Not the shared pointers, the senders or the iterators (`Iter<'_, T>` is
    /// Clone for every T)
    pub fn clone_needs_args(&self, b: u32, own: bool) -> bool {
        let name = self.structs[b as usize].name.as_str();
        own || name.starts_with("#tuple")
            || matches!(name, "Option" | "Result" | "Vec" | "VecDeque" | "BinaryHeap" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "RefCell" | "Reverse")
    }

    /// sid is a placeholder (Program::placeholder); cheap when none was made
    /// The fn and the index of the type parameter a placeholder stands for
    pub fn placeholder_of(&self, sid: u32) -> Option<(u32, u32)> {
        self.structs.placeholders.lock().unwrap().iter().find(|e| *e.1 == sid).map(|e| *e.0)
    }

    /// Whether t names a placeholder anywhere (Program::placeholder)
    pub fn names_placeholder(&self, t: Ty, depth: u32) -> bool {
        if !self.any_placeholder() || depth > 32 {
            return false;
        }
        match t {
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => self.names_placeholder(*i, depth + 1),
            Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&x| self.names_placeholder(x, depth + 1)),
            Ty::Struct(s) => {
                let sd = &self.structs[s as usize];
                sd.placeholder.is_some() || sd.args.iter().any(|&a| self.names_placeholder(a, depth + 1))
            }
            Ty::Assoc(a) => self.names_placeholder(a.base, depth + 1),
            _ => false,
        }
    }

    pub fn is_placeholder(&self, sid: u32) -> bool {
        self.structs.any_placeholder.load(std::sync::atomic::Ordering::Relaxed) && self.structs[sid as usize].placeholder.is_some()
    }

    pub fn tuple_base(&self, n: usize) -> u32 {
        if let Some(&id) = self.structs.tuples.lock().unwrap().get(&n) {
            return id;
        }
        let mut d = StructDef::blank(format!("#tuple{n}"), false);
        d.generics = (0..n).map(|i| format!("T{i}")).collect();
        d.fields = (0..n)
            .map(|i| (i.to_string(), Ty::Param(i as u8), 0))
            .collect();
        let mut h = KeyHasher::new();
        h.add(&("tuple", n));
        d.deep = h.finish();
        let mut g = self.structs.tuples.lock().unwrap();
        if let Some(&id) = g.get(&n) {
            return id;
        }
        let id = self.structs.push(d);
        g.insert(n, id);
        id
    }

    /// Whether a type consists only of prelude and primitive types (no struct / enum of the user or a dependency)
    pub fn is_prelude_ty(&self, t: Ty) -> bool {
        match t {
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => self.is_prelude_ty(*i),
            Ty::Struct(sid) => {
                let b = self.base_of(sid);
                let user = self.struct_item.get(b as usize).is_some_and(|&ii| ii < self.prelude_from);
                !user && self.structs[sid as usize].args.iter().all(|&a| self.is_prelude_ty(a))
            }
            Ty::Fn(f) => f.params.iter().all(|&p| self.is_prelude_ty(p)) && self.is_prelude_ty(f.ret),
            Ty::Dyn(_) | Ty::Assoc(_) | Ty::Param(_) => false,
            _ => true,
        }
    }

    pub fn is_tuple(&self, sid: u32) -> bool {
        self.structs[self.base_of(sid) as usize]
            .name
            .starts_with("#tuple")
    }

    /// For an instance, its base definition
    pub fn base_of(&self, sid: u32) -> u32 {
        self.structs[sid as usize].base.unwrap_or(sid)
    }

    /// rustc's method probe of a call `x.m(..)` of a method m of trait tr (one of ref_param_impls, `impl<T: Tr> Tr for
    /// &T`), whose declaration takes self as `kind` (`&self`: Some(false), `&mut self`: Some(true), `self`: None), on an
    /// x of type t: the Self it picks when that Self is a reference the impl covers, with the derefs of x and the
    /// autoref (Some(mutable)) that make the method's receiver of it (#144). Each autoderef step of t is tried by value,
    /// then borrowed (`&`, `&mut`), as rustc does, an inherent method of the step's struct first at each (one whose
    /// receiver is what that try makes); a step that `opaque` says is what a type parameter bounded by tr stands for
    /// (or `Self` of tr's default) is that parameter, whose structure the generic code does not see: it is Self itself.
    /// None where the Self found is not such a reference (the ordinary lookup's, through the deref), or where an
    /// inherent method comes first
    pub fn ref_param_probe(&self, tr: &str, m: &str, kind: Option<bool>, t: Ty, opaque: &dyn Fn(Ty) -> bool) -> Option<(Ty, usize, Option<bool>)> {
        let imp = |u: Ty| self.ref_param_implements(tr, u, 0);
        let covered = |st: Ty| matches!(st, Ty::Ref(m, _) if self.ref_param_impls.iter().any(|(t, rm)| t == tr && *rm == m));
        // (how an inherent method m of struct sid takes self, as trait_recv_kind says it)
        let inherent = |sid: u32| -> Option<Option<bool>> {
            let i = self.method(sid, m).filter(|&i| self.sigs[i as usize].impl_trait.is_none() && self.sigs[i as usize].has_self)?;
            Some(match self.sigs[i as usize].params.first()? {
                Ty::Ref(rm, _) => Some(*rm),
                _ => None,
            })
        };
        let (mut s, mut n) = (t, 0usize);
        loop {
            if opaque(s) {
                return covered(s).then_some((s, n, kind));
            }
            // (the tries at this step: by value (None), then `&` and `&mut` (Some(mutable)))
            for adj in [None, Some(false), Some(true)] {
                // (an inherent method whose receiver this try makes: `x.m()` of `S::m(&self)` with x a `&S` by value,
                // or an `S` borrowed)
                let inh = match (adj, s) {
                    (None, Ty::Struct(sid)) => inherent(sid) == Some(None),
                    (None, Ty::Ref(rm, &Ty::Struct(sid))) => inherent(sid) == Some(Some(rm)),
                    (Some(am), Ty::Struct(sid)) => inherent(sid) == Some(Some(am)),
                    _ => false,
                };
                if inh {
                    return None;
                }
                let found = match (adj, kind, s) {
                    (None, None, s) if imp(s) => Some(s),
                    (None, Some(km), Ty::Ref(xm, x)) if xm == km && imp(*x) => Some(*x),
                    (Some(am), Some(km), s) if am == km && imp(s) => Some(s),
                    _ => None,
                };
                if let Some(st) = found {
                    return covered(st).then_some((st, n, adj));
                }
            }
            match s {
                Ty::Ref(_, i) if n < 16 => {
                    s = *i;
                    n += 1;
                }
                _ => return None,
            }
        }
    }

    /// Whether u implements tr (one of ref_param_impls), through the program's impls for references to a type parameter
    /// too: `&&S` of `impl<T: Tr> Tr for &T` with `S: Tr`, where the impl's other bounds hold of S too (`T: Tr + Marker`)
    pub fn ref_param_implements(&self, tr: &str, u: Ty, depth: u32) -> bool {
        match u {
            Ty::Ref(m, i) if self.ref_param_impls.iter().any(|(t, rm)| t == tr && *rm == m) => {
                let others = self.ref_param_bounds.get(&(tr.to_string(), m)).into_iter().flatten();
                depth < 16
                    && self.ref_param_implements(tr, *i, depth + 1)
                    && others.filter(|b| b.split('<').next().unwrap_or(b).trim() != tr).all(|b| relaxed_bound(b) || self.has_param(*i) || self.implements(*i, b))
            }
            // (a reference of the other kind, `&mut &S` beside `impl<T: Tr> Tr for &T` alone: only an impl for it
            // (`impl Tr for &mut S`); the blanket impl for `&T` registered for tr is not one)
            Ty::Ref(..) => self.ref_impls.contains(&(u, tr.to_string())) || self.ref_impl_pat_holds(u, tr),
            Ty::Param(_) => false,
            u => self.implements(u, tr),
        }
    }

    /// The fn a call with Self st (a reference ref_param_probe found) of tr's method m runs: the impl's own (`T::__ref_m`,
    /// whose self type is the reference to its parameter), or tr's default where the impl leaves it out
    pub fn ref_param_fn(&self, st: Ty, m: &str, tr: &str) -> Option<u32> {
        let suffix = format!("::__ref_{m}");
        let own = (0..self.names.len() as u32).find(|&i| {
            let sig = &self.sigs[i as usize];
            matches!(sig.self_ty, Some(Ty::Ref(_, Ty::Param(_))))
                && self.names[i as usize].split('#').next().is_some_and(|b| b.ends_with(&suffix))
                && sig.impl_trait.as_deref().is_some_and(|it| it.split('<').next().unwrap_or(it).trim() == tr)
                && sig.self_ty.is_some_and(|t| self.match_ty_exact(t, st, &mut vec![None; sig.generics.len()]))
        });
        own.or_else(|| self.traits.get(tr)?.iter().copied().find(|&d| self.sigs[d as usize].has_body && self.names[d as usize].rsplit("::").next() == Some(m)))
    }

    /// How trait tr declares its method m to take self (`&self`: Some(Some(false)), `&mut self`: Some(Some(true)),
    /// `self`: Some(None)); None when tr declares no method m taking self
    pub fn trait_recv_kind(&self, tr: &str, m: &str) -> Option<Option<bool>> {
        let d = self.traits.get(tr)?.iter().copied().find(|&d| self.sigs[d as usize].has_self && self.names[d as usize].rsplit("::").next() == Some(m))?;
        Some(match self.sigs[d as usize].params.first()? {
            Ty::Ref(m, _) => Some(*m),
            _ => None,
        })
    }

    /// Whether a type parameter of fn root bounded by tr (or a supertrait of tr's) stands for t in an instance with
    /// env, or t is Self of tr's default root: ref_param_probe's `opaque`
    pub fn param_bounded_by(&self, root: u32, env: &TyEnv, tr: &str, t: Ty) -> bool {
        fn names(prog: &Program, b: &str, tr: &str, depth: u32) -> bool {
            let b = b.split('<').next().unwrap_or(b).trim();
            b == tr || depth < 8 && prog.trait_supers.get(b).is_some_and(|ss| ss.iter().any(|s| names(prog, s, tr, depth + 1)))
        }
        let Some(sig) = self.sigs.get(root as usize) else { return false };
        sig.generics.iter().any(|(g, bs)| bs.iter().any(|b| names(self, b, tr, 0)) && env.params.iter().any(|(n, it)| n == g && *it == t))
            || sig.trait_of.as_deref().is_some_and(|o| names(self, o, tr, 0)) && env.self_ty == Some(t)
    }

    /// A generic impl for references is for t (`impl<T: Marker> Flag for &W<T>`): t is its self type as it is, not
    /// through a coercion (`&mut W<u8>` is not `&W<T>`), and its parameters' bounds hold of what they matched
    pub fn ref_impl_pat_holds(&self, t: Ty, tr: &str) -> bool {
        self.ref_impl_pats.iter().any(|(pt, ptr, n, bs)| {
            let mut sol = vec![None; *n];
            ptr == tr
                && self.match_ty_exact(*pt, t, &mut sol)
                && bs.iter().zip(&sol).all(|(bs, a)| match a {
                    Some(a) if !self.has_param(*a) => bs.iter().all(|b| relaxed_bound(b) || self.implements(*a, b)),
                    _ => true,
                })
        })
    }


    fn relayout_instances(&mut self) {
        for _ in 0..8 {
            let mut ids: Vec<(u32, u32, Vec<Ty>)> =
                self.structs.inst.lock().unwrap().iter().map(|((b, a), &id)| (id, *b, a.clone())).collect();
            ids.sort_by_key(|x| x.0);
            let mut changed = false;
            for (id, base, args) in ids {
                if args.iter().any(|&t| self.has_param(t)) {
                    continue;
                }
                let b = &self.structs[base as usize];
                if b.is_enum {
                    let mut vs = vec![];
                    for v in &b.variants {
                        let fs = v.fields.iter().map(|(n, t, _)| (n.clone(), self.subst(*t, &args))).collect();
                        vs.push((v.name.clone(), v.kind, fs, v.discr));
                    }
                    let tag = b.tag;
                    let (vs, size, align) = self.enum_layout(&b.name, vs, tag);
                    let d = self.structs.get_mut(id as usize);
                    if d.size != size || d.align != align {
                        changed = true;
                    }
                    d.variants = vs;
                    d.size = size;
                    d.align = align;
                } else {
                    let fs: Vec<(String, Ty)> =
                        b.fields.iter().map(|(n, t, _)| (n.clone(), self.subst(*t, &args))).collect();
                    let (fs, size, align) = self.adt_fields_layout(&b.name, fs);
                    let d = self.structs.get_mut(id as usize);
                    if d.size != size || d.align != align || d.fields.len() != fs.len() {
                        changed = true;
                    }
                    d.fields = fs;
                    d.size = size;
                    d.align = align;
                }
            }
            // (a non-generic definition is redone too when the size of one of its fields' instances changed:
            //  `enum ItemKind { Const(String, Option<Ty>, ..) }` when Option<Ty> was made before Ty)
            for id in 0..self.structs.len() {
                let b = &self.structs[id];
                if b.base.is_some() || !b.generics.is_empty() || b.name.starts_with('#') {
                    continue;
                }
                if b.is_enum {
                    let vs: Vec<(String, VariantKind, Vec<(String, Ty)>, i64)> = b
                        .variants
                        .iter()
                        .map(|v| (v.name.clone(), v.kind, v.fields.iter().map(|(n, t, _)| (n.clone(), *t)).collect::<Vec<(String, Ty)>>(), v.discr))
                        .collect();
                    let tag = b.tag;
                    let (vs, size, align) = self.enum_layout(&b.name, vs, tag);
                    let d = self.structs.get_mut(id);
                    let moved = d.variants.iter().zip(&vs).any(|(a, b)| a.fields.iter().zip(&b.fields).any(|(x, y)| x.2 != y.2));
                    if d.size != size || d.align != align || moved {
                        changed = true;
                    }
                    d.variants = vs;
                    d.size = size;
                    d.align = align;
                } else {
                    let fs: Vec<(String, Ty)> = b.fields.iter().map(|(n, t, _)| (n.clone(), *t)).collect();
                    let (fs, size, align) = self.adt_fields_layout(&b.name, fs);
                    let d = self.structs.get_mut(id);
                    let moved = d.fields.iter().zip(&fs).any(|(x, y)| x.2 != y.2);
                    if d.size != size || d.align != align || moved {
                        changed = true;
                    }
                    d.fields = fs;
                    d.size = size;
                    d.align = align;
                }
            }
            if !changed {
                break;
            }
        }
    }

    pub fn instantiate(&self, base: u32, args: Vec<Ty>) -> R<u32> {
        if let Some(&id) = self.structs.inst.lock().unwrap().get(&(base, args.clone())) {
            return Ok(id);
        }
        // A type holding itself (in a Box or the like, `Node<T> { next: Option<Box<Node<T>>> }`): when an instance
        // being built is requested again, reserve its id and return it (the outer request fills it in when done)
        let key = (base, args.clone());
        // (recursion whose type arguments nest deeper (`struct A<T> { y: A<A<T>> }`) or a closure holding itself
        // never ends: as with rustc's recursion limit, too deep is an error)
        if INSTANTIATING.with(|v| v.borrow().len()) > 64 {
            return Err(format!("reached the recursion limit while instantiating `{}`", self.structs[base as usize].name));
        }
        // (known from the definitions before the instance exists: a placeholder handed out to a recursive request
        // carries the tag too)
        let dst = self.instance_unsized(base, &args);
        let tag = if dst { DST_TAG } else { 0 };
        let pending = INSTANTIATING.with(|v| {
            let mut v = v.borrow_mut();
            match v.iter_mut().find(|e| e.0 == key) {
                Some(e) => Some(*e.1.get_or_insert_with(|| {
                    let b = &self.structs[base as usize];
                    let mut d = StructDef::blank(self.inst_name(&b.name, &args), b.is_enum);
                    d.base = Some(base);
                    d.args = args.clone();
                    d.symbolic = args.iter().any(|&t| self.has_param(t));
                    self.structs.push(d) | tag
                })),
                None => {
                    v.push((key.clone(), None));
                    None
                }
            }
        });
        if let Some(id) = pending {
            return Ok(id);
        }
        let r = self.instantiate_inner(base, args);
        let placeholder = INSTANTIATING.with(|v| v.borrow_mut().pop().and_then(|e| e.1));
        let d = r?;
        let mut g = self.structs.inst.lock().unwrap();
        if let Some(&id) = g.get(&key) {
            return Ok(id);
        }
        // When the last field is unsized (`[T]` / `str` / such a struct), mark the id (`&S<[T]>` is a fat pointer)
        // (from the definition's last field: a symbolic instance (`Fat<[T]>`) has no laid-out fields)
        let id = match placeholder {
            Some(pid) => {
                self.structs.replace(pid, d);
                pid
            }
            None => self.structs.push(d) | tag,
        };
        g.insert(key, id);
        Ok(id)
    }

    /// Instance name `Name<A, B>` (`(A, B)` for tuples). Nested closure types (`dup(dup(..))`) double the name
    /// per level: one over 512 bytes becomes its head plus a hash of the whole (the hash keeps symbol names distinct)
    fn inst_name(&self, base_name: &str, args: &[Ty]) -> String {
        let names: Vec<String> = args.iter().map(|&t| self.tn(t)).collect();
        let full = if base_name.starts_with("#tuple") {
            if names.len() == 1 { format!("({},)", names[0]) } else { format!("({})", names.join(", ")) }
        } else {
            format!("{}<{}>", base_name, names.join(", "))
        };
        if full.len() <= 512 {
            return full;
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        full.hash(&mut h);
        let mut cut = 200;
        while !full.is_char_boundary(cut) {
            cut -= 1;
        }
        format!("{}…#{:016x}>", &full[..cut], h.finish())
    }

    /// Whether t, a definition's last field type with args for its parameters, is unsized (`[T]`, `str`, a struct
    /// with such a tail). Read from the definitions without instantiating (a recursive type's instance is not in the
    /// cache yet when this is asked)
    fn tail_unsized(&self, base: u32, t: Ty, param_unsized: &dyn Fn(u8) -> bool, depth: u32) -> bool {
        // (a parameter not declared `?Sized` is not unsized: `Bar<dyn Debug>` with `struct Bar<T>(T)` is an error
        // (E0277), not a struct with an unsized tail)
        let may = |b: u32, i: u8| self.unsized_params.get(&b).is_some_and(|v| v.contains(&(i as usize)));
        match t {
            Ty::Slice(_) | Ty::Str | Ty::Dyn(_) => true,
            Ty::Param(i) => may(base, i) && param_unsized(i),
            Ty::Struct(_) if t.is_dst_struct() => true,
            // (a struct application as the tail (`Middle<T>` ending in `Inner<T>`): its definition's tail, with its
            // parameters unsized when its arguments are; an indirection (`Box<Inner<T>>`) is not a struct here)
            Ty::Struct(s) if depth < 64 => {
                let sd = &self.structs[s as usize];
                let Some(b) = sd.base else { return false };
                let bd = &self.structs[b as usize];
                if bd.is_enum {
                    return false;
                }
                let args = &sd.args;
                let inner = |j: u8| args.get(j as usize).is_some_and(|&a| self.tail_unsized(base, a, param_unsized, depth + 1));
                match bd.fields.last() {
                    Some(&(_, lt, _)) => self.tail_unsized(b, lt, &inner, depth + 1),
                    // (a definition not resolved yet: from its last field as written)
                    None => self.tail_syn_unsized(b, &inner, depth + 1),
                }
            }
            _ => false,
        }
    }

    /// Whether the instance of base with args has an unsized tail (its id carries DST_TAG)
    fn instance_unsized(&self, base: u32, args: &[Ty]) -> bool {
        self.instance_unsized_with(base, args, &|a| self.maybe_unsized(a))
    }

    /// instance_unsized with may saying which arguments may be unsized (a definition's `?Sized` parameters too)
    fn instance_unsized_with(&self, base: u32, args: &[Ty], may: &dyn Fn(Ty) -> bool) -> bool {
        let b = &self.structs[base as usize];
        if b.is_enum {
            return false;
        }
        // (a placeholder of a `?Sized` parameter counts: `Bar<T>` with `struct Bar<T: ?Sized>` may be unsized)
        let oracle = |i: u8| args.get(i as usize).is_some_and(|&a| may(a));
        match b.fields.last() {
            Some(f) => self.tail_unsized(base, f.1, &oracle, 0),
            // (the definition's fields are not resolved yet: from its last field as written)
            None => self.tail_syn_unsized(base, &oracle, 0),
        }
    }

    /// tail_unsized from a definition's last field as written (tail_syn), for a definition not resolved yet
    fn tail_syn_unsized(&self, base: u32, param_unsized: &dyn Fn(u8) -> bool, depth: u32) -> bool {
        let Some((gens, last)) = self.tail_syn.get(&base) else { return false };
        let may = |i: usize| self.unsized_params.get(&base).is_some_and(|v| v.contains(&i));
        // (a type as written, in the parameters' terms: unsized when it is `[T]` / `str` / `dyn Tr`, an unsized
        // parameter, or a struct application whose tail is)
        fn syn_unsized(p: &Program, gens: &[String], t: &TySyn, may: &dyn Fn(usize) -> bool, pu: &dyn Fn(u8) -> bool, depth: u32) -> bool {
            match t {
                TySyn::Slice(_) | TySyn::Str | TySyn::Dyn(_) => true,
                TySyn::Named(n) if gens.iter().any(|g| g == n) => gens.iter().position(|g| g == n).is_some_and(|i| may(i) && pu(i as u8)),
                // (an alias (`type Alias<T: ?Sized> = Inner<T>`): its body, its parameters being the arguments)
                TySyn::Named(n) | TySyn::App(n, _) if depth < 64 && p.aliases.contains_key(n) && !p.struct_index.contains_key(n) => {
                    let (ag, body) = &p.aliases[n];
                    let args: &[TySyn] = match t {
                        TySyn::App(_, a) => a,
                        _ => &[],
                    };
                    if args.len() != ag.len() {
                        return false;
                    }
                    let inner = |k: u8| args.get(k as usize).is_some_and(|a| syn_unsized(p, gens, a, may, pu, depth + 1));
                    syn_unsized(p, ag, body, &|_| true, &inner, depth + 1)
                }
                TySyn::App(n, args) if depth < 64 => {
                    let Some(&sid) = p.struct_index.get(n) else { return false };
                    let inner = |k: u8| args.get(k as usize).is_some_and(|a| syn_unsized(p, gens, a, may, pu, depth + 1));
                    let bd = &p.structs[sid as usize];
                    match bd.fields.last() {
                        Some(&(_, lt, _)) if !bd.is_enum => p.tail_unsized(sid, lt, &inner, depth + 1),
                        _ => p.tail_syn_unsized(sid, &inner, depth + 1),
                    }
                }
                _ => false,
            }
        }
        syn_unsized(self, gens, last, &may, param_unsized, depth)
    }

    fn instantiate_inner(&self, base: u32, args: Vec<Ty>) -> R<StructDef> {
        let b = &self.structs[base as usize];
        if b.generics.len() != args.len() {
            return Err(format!(
                "`{}` takes {} type arguments, but {} were given",
                b.name,
                b.generics.len(),
                args.len()
            ));
        }
        let symbolic = args.iter().any(|&t| self.has_param(t));
        let name = self.inst_name(&b.name, &args);
        let mut d = StructDef::blank(name, b.is_enum);
        d.base = Some(base);
        d.args = args.clone();
        d.symbolic = symbolic;
        d.tag = b.tag;
        if !symbolic {
            if b.is_enum {
                let mut vs = vec![];
                for v in &b.variants {
                    let fs = v
                        .fields
                        .iter()
                        .map(|(n, t, _)| (n.clone(), self.subst(*t, &args)))
                        .collect();
                    vs.push((v.name.clone(), v.kind, fs, v.discr));
                }
                let (vs, size, align) = self.enum_layout(&b.name, vs, b.tag);
                d.variants = vs;
                d.size = size;
                d.align = align;
            } else {
                let fs: Vec<(String, Ty)> = b
                    .fields
                    .iter()
                    .map(|(n, t, _)| (n.clone(), self.subst(*t, &args)))
                    .collect();
                let (fs, size, align) = self.adt_fields_layout(&b.name, fs);
                d.fields = fs;
                d.size = size;
                d.align = align;
            }
        }
        Ok(d)
    }

    /// Lays out the fields in declaration order with natural alignment (like repr(C))
    /// The layout of a struct named name (a union: every field at offset 0)
    fn adt_fields_layout(&self, name: &str, fields: Vec<(String, Ty)>) -> (Vec<(String, Ty, u32)>, u32, u32) {
        // (`#[repr(packed(N))]`: each field aligned to at most N; `#[repr(align(N))]`: the whole to at least N)
        let pack = self.packed.get(name).copied().unwrap_or(u32::MAX);
        let min = self.min_align.get(name).copied().unwrap_or(1);
        if !self.unions.contains(name) {
            let (out, size, align) = self.struct_layout_packed(fields, pack);
            let align = align.max(min);
            return (out, size.next_multiple_of(align), align);
        }
        let (mut size, mut align) = (0u32, min);
        let mut out = vec![];
        for (n, t) in fields {
            let (sz, al) = self.size_align(t);
            size = size.max(sz);
            align = align.max(al.max(1).min(pack));
            out.push((n, t, 0));
        }
        (out, size.next_multiple_of(align), align)
    }

    fn struct_layout(&self, fields: Vec<(String, Ty)>) -> (Vec<(String, Ty, u32)>, u32, u32) {
        self.struct_layout_packed(fields, u32::MAX)
    }

    fn struct_layout_packed(&self, fields: Vec<(String, Ty)>, pack: u32) -> (Vec<(String, Ty, u32)>, u32, u32) {
        let mut out = vec![];
        let (mut off, mut align) = (0u32, 1u32);
        for (n, t) in fields {
            let (sz, al) = self.size_align(t);
            // (a type not laid out yet (a recursive type under construction) has alignment 0: alignment is at least 1)
            let al = al.max(1).min(pack);
            off = off.next_multiple_of(al);
            out.push((n, t, off));
            off += sz;
            align = align.max(al);
        }
        (out, off.next_multiple_of(align), align)
    }

    /// The tag (discriminant) goes first, and the payload in an area all variants share
    fn enum_layout(
        &self,
        name: &str,
        vs: Vec<(String, VariantKind, Vec<(String, Ty)>, i64)>,
        tag: Ty,
    ) -> (Vec<VariantDef>, u32, u32) {
        // (an enum of no variants has no values: no tag, size 0, as rustc lays it out)
        if vs.is_empty() {
            return (vec![], 0, self.min_align.get(name).copied().unwrap_or(1).max(1));
        }
        let tag_size = tag.bits() / 8;
        // (`#[repr(u8)]` and the like on an enum with fields (RFC 2195): each variant is laid out as a `#[repr(C)]`
        // struct of the tag and its fields, the enum as their union)
        if self.reprs.contains_key(name) && !self.repr_c.contains(name) && vs.iter().any(|v| !v.2.is_empty()) {
            let (mut size, mut align) = (tag_size, tag_size.max(1).max(self.min_align.get(name).copied().unwrap_or(1)));
            let mut out = vec![];
            for (vn, kind, fs, discr) in vs {
                let mut all = vec![("#tag".to_string(), tag)];
                all.extend(fs);
                let (f, sz, al) = self.struct_layout(all);
                size = size.max(sz);
                align = align.max(al);
                out.push(VariantDef { name: vn, kind, fields: f.into_iter().skip(1).collect(), discr });
            }
            return (out, size.next_multiple_of(align), align);
        }
        let mut laid = vec![];
        let (mut pmax, mut palign) = (0u32, 1u32);
        for (_, _, fs, _) in &vs {
            let (f, sz, al) = self.struct_layout(fs.clone());
            pmax = pmax.max(sz);
            palign = palign.max(al);
            laid.push(f);
        }
        let poff = tag_size.next_multiple_of(palign.max(1));
        let align = palign.max(tag_size).max(1).max(self.min_align.get(name).copied().unwrap_or(1));
        let out = vs
            .into_iter()
            .zip(laid)
            .map(|((name, kind, _, discr), f)| VariantDef {
                name,
                kind,
                fields: f.into_iter().map(|(n, t, o)| (n, t, o + poff)).collect(),
                discr,
            })
            .collect();
        (out, (poff + pmax).next_multiple_of(align), align)
    }

    /// Whether type parameters remain in the type
    pub fn has_param(&self, t: Ty) -> bool {
        match t {
            Ty::Param(_) => true,
            Ty::Array(_, n) if n >= CONST_LEN_BASE => true,
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => {
                self.has_param(*i)
            }
            Ty::Struct(sid) => self.structs[sid as usize].symbolic,
            Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&a| self.has_param(a)),
            // An associated type is not a concrete type (it only appears when type parameters are involved)
            Ty::Assoc(_) => true,
            // (`dyn A<T>`: its marks, dyn_canon)
            Ty::Dyn(s) => s.contains(PMARK) && dyn_marks(s).next().is_some(),
            _ => false,
        }
    }

    /// The normalizes-to goal `<t as tr>::name` (issue #2): the value the impls of tr give for t. Not by the name
    /// alone, as assoc_of: an impl of another trait with an associated type of the same name is not t's here. None
    /// (not known) when no impl of the program's says (std's the prelude does not write out), when the value still
    /// has a parameter in it, or when the impls that match t disagree (`impl Tr<u8> for X` and `impl Tr<u16> for X`)
    pub fn normalize(&self, t: Ty, tr: &str, name: &str) -> Option<Ty> {
        let key = (t, tr.to_string(), name.to_string());
        if let Some(&r) = self.normalized.lock().unwrap().get(&key) {
            return r;
        }
        let r = self.normalize0(t, tr, name);
        self.normalized.lock().unwrap().insert(key, r);
        r
    }

    fn normalize0(&self, t: Ty, tr: &str, name: &str) -> Option<Ty> {
        // (by name and the struct the self type is of: u32::MAX for a self type of another kind, a parameter's
        // too, which any t may match)
        let head = |p: &Program, t: Ty| match t {
            Ty::Struct(sid) => p.base_of(sid),
            _ => u32::MAX,
        };
        let index = self.assoc_index.get_or_init(|| {
            let mut m: FxMap<(String, u32), Vec<u32>> = FxMap::default();
            for (i, a) in self.assocs.iter().enumerate() {
                if !a.trait_name.is_empty() && a.ngat == 0 {
                    m.entry((a.name.clone(), head(self, a.self_ty))).or_default().push(i as u32);
                }
            }
            m
        });
        let mut found: Option<Ty> = None;
        let h = head(self, t);
        let own = index.get(&(name.to_string(), h)).map(|v| v.as_slice()).unwrap_or_default();
        let other = if h == u32::MAX { &[][..] } else { index.get(&(name.to_string(), u32::MAX)).map(|v| v.as_slice()).unwrap_or_default() };
        for &i in own.iter().chain(other) {
            let a = &self.assocs[i as usize];
            // (the impl's trait by its own name, as AssocDef keeps it: `ops::Add` is `Add`)
            if a.trait_name != tr.rsplit("::").next().unwrap_or(tr) {
                continue;
            }
            // (match_ty lets `&mut` meet `&` and a pointer meet a reference, as a call coerces: a self type does not)
            if !self.refs_agree(a.self_ty, t, 0) {
                continue;
            }
            let mut sol = vec![None; a.ngen];
            if !self.match_ty(a.self_ty, t, &mut sol) {
                continue;
            }
            let args: Vec<Ty> = sol.into_iter().collect::<Option<Vec<Ty>>>()?;
            // (a value with a projection in it, `type A = T::A;`, is not known: subst would look the projection up by
            // its name alone, whatever trait T's bound names)
            if self.mentions_assoc(a.value) {
                return None;
            }
            let v = self.subst(a.value, &args);
            if self.has_param(v) || found.is_some_and(|f| f != v) {
                return None;
            }
            found = Some(v);
        }
        found
    }

    /// The type a where clause on other than a parameter names for the arguments `args` of a fn's parameters
    /// `names` (Insts::request). A projection is a normalizes-to goal: `<T as Tr>::A` by Tr's impls, `T::A` by those of
    /// the traits T's bounds name, which must agree. None when it is not known (a parameter is left, nothing
    /// normalizes it, or a projection is inside another type, which subst would look up by its name alone)
    pub fn pred_subject(&self, syn: &crate::types::TySyn, qtr: &str, sig: &FnSig, names: &[&str], args: &[Ty]) -> Option<Ty> {
        // (a projection is taken apart before it is resolved: resolve_g would look `<u8 as Q>::A`, whose base is
        // concrete, up by the name A alone, whatever trait gives it)
        if let crate::types::TySyn::Assoc(bsyn, name) = syn {
            if name.starts_with(crate::ast_types::GAT_PREFIX) {
                return None;
            }
            // (a qualified path's trait is marked in the name, `#tr:Tr:A` of `<X as Tr>::A` (tysyn): the name without
            // it, and that trait when the caller has none)
            let (name, mtr) = crate::ast_types::split_trait_assoc(name);
            let qtr = if qtr.is_empty() { mtr.unwrap_or("") } else { qtr };
            // (`<i32 as Local>::Output` is marked as not std's: normalize asks the impls of the trait it names anyway)
            let name = name.strip_prefix(crate::ast_types::NO_STD_ASSOC).unwrap_or(name);
            let bt = self.resolve_g(bsyn, names).ok()?;
            let a = crate::types::AssocTy { base: bt, name: name.to_string() };
            return self.projection_subject(&a, qtr, sig, args);
        }
        let t = self.resolve_g(syn, names).ok()?;
        if self.mentions_assoc(t) {
            return None;
        }
        let t = self.subst(t, args);
        (!self.has_param(t)).then_some(t)
    }

    /// pred_subject's projection `<base as qtr>::name` (`base::name` when qtr is empty), normalized
    fn projection_subject(&self, a: &crate::types::AssocTy, qtr: &str, sig: &FnSig, args: &[Ty]) -> Option<Ty> {
        {
            let base = self.subst(a.base, args);
            if self.has_param(base) || self.mentions_assoc(base) {
                return None;
            }
            let mut traits: Vec<String> = if !qtr.is_empty() {
                vec![qtr.to_string()]
            } else if let Ty::Param(i) = a.base {
                sig.generics.get(i as usize)?.1.iter().map(|b| b.split('<').next().unwrap_or(b).trim().to_string()).collect()
            } else {
                // (`Self::A` in a method of `impl Tr for S`: by the impl's trait, and its supertraits below)
                let tr = sig.impl_trait.as_deref()?;
                vec![tr.split('<').next().unwrap_or(tr).trim().to_string()]
            };
            // (a qualified projection is its trait's own associated type when that trait gives one: `<T as Sub>::A`)
            if !qtr.is_empty() {
                if let Some(v) = self.normalize(base, qtr, &a.name) {
                    return Some(v);
                }
            }
            // (and their supertraits, which may declare it: `T::A` of `T: Sub` with `trait Sub: Super` and `Super::A`)
            let mut k = 0;
            while k < traits.len() && traits.len() < 64 {
                for sup in self.trait_supers.get(&traits[k]).into_iter().flatten() {
                    let sup = sup.split('<').next().unwrap_or(sup).trim().to_string();
                    if !traits.contains(&sup) {
                        traits.push(sup);
                    }
                }
                k += 1;
            }
            let mut found = None;
            for tr in &traits {
                if let Some(v) = self.normalize(base, tr, &a.name) {
                    if found.is_some_and(|f| f != v) {
                        return None;
                    }
                    found = Some(v);
                }
            }
            found
        }
    }

    /// t is a type whose impls of the program's implements does not see: a Box, an array or a slice, or one with a
    /// trait object in it (`dyn T2` and `dyn T2 + 'static` are told apart by their text here, where rustc sees one
    /// type with two lifetimes). A bound on one is not checked: a no would be a false error
    pub fn opaque_to_impls(&self, t: Ty) -> bool {
        matches!(t, Ty::Box(_) | Ty::Array(..) | Ty::Slice(_)) || self.tn(t).contains("dyn ")
    }

    /// t has a projection (`Ty::Assoc`) in it
    fn mentions_assoc(&self, t: Ty) -> bool {
        match t {
            Ty::Assoc(_) => true,
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => self.mentions_assoc(*i),
            Ty::Struct(sid) => self.structs[sid as usize].args.iter().any(|&a| self.mentions_assoc(a)),
            Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&a| self.mentions_assoc(a)),
            _ => false,
        }
    }

    /// A bound's bindings as normalizes-to goals (`T: Trait<AssociatedType = u32>`): with t for its type, each
    /// `<t as Trait>::AssociatedType` against its binding, written with the parameters `names` that `args` give.
    /// The first that normalizes to another type, with that type; a binding that does not normalize, or names a
    /// parameter not given, is not known and is taken to hold (E0271 is for a mismatch, not for not knowing)
    pub fn binding_mismatch(&self, t: Ty, bound: &str, names: &[&str], args: &[Ty]) -> Option<(String, Ty, Ty)> {
        if !bound.contains('=') || parse_fn_bound(bound).is_some() {
            return None;
        }
        let tr = bound.split('<').next().unwrap_or(bound).trim();
        // (the bindings of a bound's text, parsed once: an instance of each call asks again)
        thread_local! {
            static BINDINGS: std::cell::RefCell<FxMap<String, std::rc::Rc<Vec<(String, crate::types::TySyn)>>>> = Default::default();
        }
        let bs = BINDINGS.with(|m| m.borrow_mut().entry(bound.to_string()).or_insert_with(|| std::rc::Rc::new(crate::ast_summary::assoc_bindings(bound))).clone());
        for (name, syn) in bs.iter() {
            let name = name.as_str();
            let Ok(want) = self.resolve_g(syn, names) else { continue };
            let want = self.subst(want, args);
            if self.has_param(want) {
                continue;
            }
            let Some(got) = self.normalize(t, tr, name) else { continue };
            if got != want {
                return Some((format!("<{} as {tr}>::{name}", self.tn(t)), want, got));
            }
        }
        None
    }

    /// The associated type name of concrete type t (u32 for `impl Iterator for Counter { type Item = u32; }`)
    /// The associated type `name` of the impl of trait tr with arguments args for t: the impls whose self type and
    /// trait arguments match exactly (crate::traits::mtch), one definition or none
    pub fn assoc_of_targs(&self, t: Ty, tr: &str, args: &[Ty], name: &str) -> Option<Ty> {
        let tr = tr.rsplit("::").next().unwrap_or(tr);
        let mut found: Option<Ty> = None;
        for a in &self.assocs {
            if a.name != name || a.trait_name != tr || a.ngat != 0 {
                continue;
            }
            let Some(ta) = &a.trait_args else { return None };
            if ta.len() != args.len() {
                continue;
            }
            let mut sol = vec![None; a.ngen];
            if !crate::traits::mtch(self, a.self_ty, t, &mut sol, false, 0) || !ta.iter().zip(args).all(|(&p, &x)| crate::traits::mtch(self, p, x, &mut sol, false, 0)) {
                continue;
            }
            let full: Vec<Ty> = sol.iter().copied().collect::<Option<Vec<Ty>>>()?;
            let v = self.subst(a.value, &full);
            if found.is_some_and(|f| f != v) {
                return None;
            }
            found = Some(v);
        }
        found
    }

    /// The region codes of the associated type just pushed (the k-th of item ii's summary), if its value names a
    /// lifetime (Summary::assoc_regions), with its impl's self type's
    fn note_assoc_regions(&mut self, items: &[SrcItem], sums: &[Summary], ii: usize, k: usize) {
        let Some((_, codes)) = sums[ii].assoc_regions.iter().find(|x| x.0 as usize == k) else { return };
        let crate::lexer::ItemKind::ImplMember(h) = items[ii].kind else { return };
        // (an impl that names no lifetime has no header regions: no self slots)
        let (self_slots, outlives, trait_lts) = sums[h as usize].hdr_regions.as_ref().map(|r| (r.self_slots.clone(), r.outlives.clone(), r.trait_lts.clone())).unwrap_or_default();
        self.assoc_regions.insert(self.assocs.len() as u32 - 1, (codes.clone(), self_slots, outlives, trait_lts));
    }

    pub fn assoc_of(&self, t: Ty, name: &str) -> Option<Ty> {
        self.assoc_of_with(t, name, true)
    }

    /// The associated type `name` of t that an impl of trait tr gives (`<&J as BitAnd>::Output`), when the impls of
    /// two traits give one of that name for t; None when only one does (assoc_of answers then)
    pub fn assoc_of_trait(&self, t: Ty, name: &str, tr: &str) -> Option<Ty> {
        let fits = |a: &AssocDef| a.name == name && a.ngat == 0 && self.match_ty(a.self_ty, t, &mut vec![None; a.ngen]);
        if self.assocs.iter().filter(|a| fits(a)).nth(1).is_none() {
            return None;
        }
        let of_tr = |a: &&AssocDef| a.trait_name == tr && fits(a);
        let concrete = |a: &&AssocDef| !matches!(a.self_ty, Ty::Param(_));
        let a = self
            .assocs
            .iter()
            .filter(concrete)
            .find(|a| of_tr(a) && self.match_ty_exact(a.self_ty, t, &mut vec![None; a.ngen]))
            .or_else(|| self.assocs.iter().filter(|a| !concrete(a)).find(of_tr))
            .or_else(|| self.assocs.iter().find(of_tr))?;
        let mut sol = vec![None; a.ngen];
        self.match_ty(a.self_ty, t, &mut sol);
        let args = sol.iter().copied().collect::<Option<Vec<Ty>>>()?;
        Some(self.subst(a.value, &args))
    }

    /// How many traits' impls give an associated type of the name
    pub fn assoc_traits_of(&self, name: &str) -> usize {
        let mut trs: Vec<&str> = vec![];
        for a in self.assocs.iter().filter(|a| a.name == name) {
            let t = a.trait_name.as_str();
            if !trs.contains(&t) {
                trs.push(t);
                if trs.len() > 1 {
                    break;
                }
            }
        }
        trs.len()
    }

    /// assoc_of; with std_impls false, not std's impls the prelude does not write out (`<i32 as Local>::Output`)
    pub fn assoc_of_with(&self, t: Ty, name: &str, std_impls: bool) -> Option<Ty> {
        thread_local! {
            static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        }
        // (`impl<F: Future> IntoFuture for F { type Item = F::Item; }` refers to another trait's `Item` of the same
        // type: a cycle through the blanket impl. Bounded, and such a definition is not used to answer its own name.
        // A qualified one's too, `<T as B>::Out` in A's impl and `<T as A>::Out` in B's)
        if DEPTH.with(|d| d.get()) > 32 {
            return None;
        }
        DEPTH.with(|d| d.set(d.get() + 1));
        // (`#tr:Tr:Abi`, of a qualified path with a parameter that is known now: Tr's impl gives it)
        let (name, tr) = crate::ast_types::split_trait_assoc(name);
        let r = tr.and_then(|tr| self.assoc_of_trait(t, name, tr)).or_else(|| self.assoc_of0(t, name, std_impls));
        DEPTH.with(|d| d.set(d.get() - 1));
        r
    }

    fn assoc_of0(&self, t: Ty, name: &str, std_impls: bool) -> Option<Ty> {
        // (`P::Pointer<String>`: the associated type `#gat:Pointer` of `(P, String)`, the impl's for P with its own
        // parameters String)
        if let Some(gname) = name.strip_prefix(crate::ast_types::GAT_PREFIX) {
            let Ty::Struct(sid) = t else { return None };
            if !self.is_tuple(sid) {
                return None;
            }
            let parts = self.structs[sid as usize].args.clone();
            let (&base, gargs) = parts.split_first()?;
            for a in &self.assocs {
                if a.name != gname || a.ngat != gargs.len() {
                    continue;
                }
                // (match_ty lets `&mut` meet `&` (a coercion): an impl for `&T` and one for `&mut T` are two)
                if let (Ty::Ref(m1, _), Ty::Ref(m2, _)) = (a.self_ty, base) {
                    if m1 != m2 {
                        continue;
                    }
                }
                // (nor a pointer's impl a reference's: match_ty lets `*const T` meet `&x`, a coercion)
                if matches!((a.self_ty, base), (Ty::Ptr(..), Ty::Ref(..))) {
                    continue;
                }
                // (and `*const T`'s impl is not `*mut T`'s)
                if let (Ty::Ptr(m1, _), Ty::Ptr(m2, _)) = (a.self_ty, base) {
                    if m1 != m2 {
                        continue;
                    }
                }
                let mut sol = vec![None; a.ngen];
                if self.match_ty(a.self_ty, base, &mut sol) {
                    let Some(mut args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else { continue };
                    // (the mark's place, then the associated type's own arguments)
                    args.push(Ty::Unit);
                    args.extend_from_slice(gargs);
                    return Some(self.subst(a.value, &args));
                }
            }
            return None;
        }
        // (an impl for the type itself before a blanket one, `impl WasmAbi for WasmSlice` before `impl<T: WasmPrimitive>
        // WasmAbi for T`: when both are written, the blanket impl's bounds leave the type out, or they would overlap)
        // (and first one whose self type is the type with no coercion: `impl Tr for &J` before `impl<T> Tr for *const T`)
        // (one that matches only through a coercion last, `impl<T> Tr for *const T` for a `&J`: a blanket impl's before it)
        let blanket = |a: &&AssocDef| matches!(a.self_ty, Ty::Param(_));
        let exact = self.assocs.iter().filter(|a| !blanket(a)).map(|a| (a, true));
        let rest = self.assocs.iter().filter(blanket).chain(self.assocs.iter().filter(|a| !blanket(a))).map(|a| (a, false));
        for (a, ex) in exact.chain(rest) {
            // (a generic associated type without its arguments (`<() as Outer>::Ty` of a qualified path): its value
            // when that does not use its own parameters)
            if a.name != name || (a.ngat != 0 && self.uses_params_from(a.value, a.ngen as u8)) {
                continue;
            }
            if let Ty::Assoc(v) = a.value {
                if v.base == a.self_ty && v.name == name {
                    continue;
                }
            }
            // (match_ty lets `&mut` meet `&` (a coercion): an impl for `&T` and one for `&mut T` are two)
            if let (Ty::Ref(m1, _), Ty::Ref(m2, _)) = (a.self_ty, t) {
                if m1 != m2 {
                    continue;
                }
            }
            // (nor a pointer's impl a reference's: match_ty lets `*const T` meet `&x`, a coercion)
            if matches!((a.self_ty, t), (Ty::Ptr(..), Ty::Ref(..))) {
                continue;
            }
            // (and `*const T`'s impl is not `*mut T`'s)
            if let (Ty::Ptr(m1, _), Ty::Ptr(m2, _)) = (a.self_ty, t) {
                if m1 != m2 {
                    continue;
                }
            }
            let mut sol = vec![None; a.ngen];
            if if ex { self.match_ty_exact(a.self_ty, t, &mut sol) } else { self.match_ty(a.self_ty, t, &mut sol) } {
                let Some(mut args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
                    continue;
                };
                args.extend(std::iter::repeat_n(Ty::Unit, if a.ngat > 0 { a.ngat + 1 } else { 0 }));
                return Some(self.subst(a.value, &args));
            }
        }
        // (a placeholder's associated type its bounds' traits declare: a placeholder of its own)
        if let Ty::Struct(sid) = t {
            if self.is_placeholder(sid) && self.assoc_placeholder_of(sid).is_none() {
                return self.assoc_placeholder(sid, name).map(Ty::Struct);
            }
        }
        // (std's impls the prelude does not write out: `<i32 as Add<T>>::Output` is i32 (the operators of a number or
        // bool are Self's, through a reference too: `&i32 + i32`), `<&i32 as Deref>::Target` is i32)
        if !std_impls {
            return None;
        }
        let prim = |t: Ty| t.is_int() || t.is_float() || t == Ty::Bool;
        match (name, t) {
            ("Output", t) if prim(t) => Some(t),
            ("Output", Ty::Ref(_, i)) if prim(*i) => Some(*i),
            ("Target", Ty::Ref(_, i) | Ty::Box(i)) => Some(*i),
            _ => None,
        }
    }

    /// t names a type parameter numbered `from` or above
    fn uses_params_from(&self, t: Ty, from: u8) -> bool {
        match t {
            Ty::Param(i) => i >= from && i != HOLE,
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => self.uses_params_from(*i, from),
            Ty::Struct(sid) => self.structs[sid as usize].args.iter().any(|&a| self.uses_params_from(a, from)),
            Ty::Assoc(a) => self.uses_params_from(a.base, from),
            Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&p| self.uses_params_from(p, from)),
            Ty::Dyn(s) => s.contains(PMARK) && dyn_marks(s).any(|i| i >= from),
            _ => false,
        }
    }

    /// Substitutes type arguments for type parameters
    pub fn subst(&self, t: Ty, args: &[Ty]) -> Ty {
        match t {
            // (a type parameter with no argument (such as the `_` mark HOLE) stays: inference / checking handle it later)
            Ty::Param(i) => args.get(i as usize).copied().unwrap_or(t),
            Ty::Ref(m, i) => Ty::Ref(m, intern(self.subst(*i, args))),
            Ty::Ptr(m, i) => Ty::Ptr(m, intern(self.subst(*i, args))),
            Ty::Array(i, n) => {
                let n = match n.checked_sub(CONST_LEN_BASE).map(|k| args[k as usize]) {
                    Some(Ty::Array(_, m)) => m,
                    _ => n,
                };
                Ty::Array(intern(self.subst(*i, args)), n)
            }
            Ty::Slice(i) => Ty::Slice(intern(self.subst(*i, args))),
            Ty::Box(i) => Ty::Box(intern(self.subst(*i, args))),
            Ty::Assoc(a) => {
                let b = self.subst(a.base, args);
                if self.has_param(b) {
                    Ty::Assoc(intern_assoc(AssocTy {
                        base: b,
                        name: a.name.clone(),
                    }))
                } else {
                    self.assoc_of(b, &a.name).unwrap_or(t)
                }
            }
            Ty::Fn(f) if self.has_param(t) => Ty::Fn(intern_fn(FnTy {
                params: f.params.iter().map(|&p| self.subst(p, args)).collect(),
                ret: self.subst(f.ret, args),
            })),
            // (`dyn A<T>`: each parameter's mark becomes its argument's text, or the parameter's name where the
            // program cannot write it, as before the marks)
            Ty::Dyn(s) if s.contains(PMARK) => {
                let r = crate::parse::rewrite_tokens(s, &mut |k, toks| {
                    let (i, n) = dyn_param_mark(&toks[k].text)?;
                    Some(match *args.get(i as usize)? {
                        Ty::Param(HOLE) => "_".to_string(),
                        Ty::Param(j) if j == i => return None,
                        a => self.dyn_src(a, n).unwrap_or_else(|| n.to_string()),
                    })
                });
                r.map_or(t, |r| Ty::Dyn(intern_str(&r)))
            }
            Ty::Struct(sid) if self.has_param(t) => {
                let sd = &self.structs[sid as usize];
                let na: Vec<Ty> = sd.args.iter().map(|&a| self.subst(a, args)).collect();
                match self.instantiate(sd.base.unwrap(), na) {
                    Ok(id) => Ty::Struct(id),
                    Err(_) => t,
                }
            }
            t => t,
        }
    }

    /// Replaces only the known type parameters (unknown ones stay Param)
    pub fn partial_subst(&self, t: Ty, sol: &[Option<Ty>]) -> Ty {
        let args: Vec<Ty> = (0..sol.len())
            .map(|i| sol[i].unwrap_or(Ty::Param(i as u8)))
            .collect();
        if args.is_empty() {
            t
        } else {
            self.subst(t, &args)
        }
    }

    /// t is pattern with its holes (`_`, Param(HOLE)) filled: `Box<_>` and `Box<i32>`. Only when pattern has a hole
    pub fn fits_holes(&self, pattern: Ty, t: Ty, depth: u32) -> bool {
        if depth > 32 {
            return false;
        }
        let holes = |x: Ty| self.uses_hole(x, 0);
        if !holes(pattern) {
            return false;
        }
        let go = |a: Ty, b: Ty| a == b || self.fits_holes(a, b, depth + 1);
        match (pattern, t) {
            (Ty::Param(HOLE), _) => true,
            (Ty::Ref(m, a), Ty::Ref(n, b)) | (Ty::Ptr(m, a), Ty::Ptr(n, b)) => m == n && go(*a, *b),
            (Ty::Box(a), Ty::Box(b)) | (Ty::Slice(a), Ty::Slice(b)) => go(*a, *b),
            (Ty::Array(a, n), Ty::Array(b, m)) => n == m && go(*a, *b),
            (Ty::Struct(x), Ty::Struct(y)) => {
                let (sx, sy) = (&self.structs[x as usize], &self.structs[y as usize]);
                sx.base.is_some() && sx.base == sy.base && sx.args.len() == sy.args.len() && sx.args.iter().zip(&sy.args).all(|(&a, &b)| go(a, b))
            }
            (Ty::Fn(a), Ty::Fn(b)) => a.params.len() == b.params.len() && a.params.iter().zip(&b.params).chain([(&a.ret, &b.ret)]).all(|(&p, &q)| go(p, q)),
            _ => false,
        }
    }

    fn uses_hole(&self, t: Ty, depth: u32) -> bool {
        if depth > 32 {
            return false;
        }
        match t {
            Ty::Param(HOLE) => true,
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => self.uses_hole(*i, depth + 1),
            Ty::Struct(s) => self.structs[s as usize].args.iter().any(|&a| self.uses_hole(a, depth + 1)),
            Ty::Fn(f) => f.params.iter().any(|&p| self.uses_hole(p, depth + 1)) || self.uses_hole(f.ret, depth + 1),
            _ => false,
        }
    }

    /// Solves type parameters by matching parameter type pt (with type parameters) against argument type at
    pub fn match_ty(&self, pt: Ty, at: Ty, sol: &mut [Option<Ty>]) -> bool {
        self.match_ty_in(pt, at, sol, false)
    }

    /// match_ty with no coercion: a `&` is no `&mut`, a reference no raw pointer. For the inherent methods of one name
    /// in impls for disjoint instances (`W<&T>::copied` / `W<&mut T>::copied`): the receiver's own
    pub fn match_ty_exact(&self, pt: Ty, at: Ty, sol: &mut [Option<Ty>]) -> bool {
        self.match_ty_in(pt, at, sol, true)
    }

    fn match_ty_in(&self, pt: Ty, at: Ty, sol: &mut [Option<Ty>], exact: bool) -> bool {
        if exact {
            match (pt, at) {
                (Ty::Ref(m1, _), Ty::Ref(m2, _)) | (Ty::Ptr(m1, _), Ty::Ptr(m2, _)) if m1 != m2 => return false,
                (Ty::Ptr(..), Ty::Ref(..)) => return false,
                _ => {}
            }
        }
        match (pt, at) {
            // Not solvable from an associated type (it is replaced once the concrete type is known)
            (Ty::Assoc(_), _) => true,
            (Ty::Param(i), t) => match sol.get(i as usize).copied().flatten() {
                // (a turbofish's type with holes, `f::<Box<_>>(Box::new(100))`: the argument's type that fits it fills
                // them)
                Some(prev) if prev != t && self.fits_holes(prev, t, 0) => {
                    sol[i as usize] = Some(t);
                    true
                }
                Some(prev) => prev == t,
                None => {
                    if let Some(s) = sol.get_mut(i as usize) {
                        *s = Some(t);
                    }
                    true
                }
            },
            (Ty::Ref(_, a), Ty::Ref(_, b))
            | (Ty::Ptr(_, a), Ty::Ptr(_, b))
            // (`&mut x` passed for a `*mut T` coerces)
            | (Ty::Ptr(_, a), Ty::Ref(_, b))
            | (Ty::Slice(a), Ty::Slice(b))
            | (Ty::Box(a), Ty::Box(b)) => self.match_ty_in(*a, *b, sol, exact),
            // (a const generic length: solved with an array Ty::Array(Unit, m) carrying the length)
            (Ty::Array(a, n), Ty::Array(b, m)) if n >= CONST_LEN_BASE && m < CONST_LEN_BASE => {
                self.match_ty_in(Ty::Param((n - CONST_LEN_BASE) as u8), Ty::Array(intern(Ty::Unit), m), sol, exact) && self.match_ty_in(*a, *b, sol, exact)
            }
            (Ty::Array(a, n), Ty::Array(b, m)) => n == m && self.match_ty_in(*a, *b, sol, exact),
            (Ty::Fn(a), Ty::Fn(b)) if a != b => {
                a.params.len() == b.params.len()
                    && a.params
                        .iter()
                        .zip(&b.params)
                        .chain([(&a.ret, &b.ret)])
                        .all(|(&p, &q)| self.match_ty_in(p, q, sol, exact))
            }
            (Ty::Struct(a), Ty::Struct(b)) if a != b => {
                let (sa, sb) = (&self.structs[a as usize], &self.structs[b as usize]);
                match (sa.base, sb.base) {
                    (Some(x), Some(y)) if x == y => sa
                        .args
                        .iter()
                        .zip(&sb.args)
                        .all(|(&p, &q)| self.match_ty_in(p, q, sol, exact)),
                    _ => false,
                }
            }
            // (`dyn A<T>` against `dyn A<i32>`: their type arguments)
            // (or the same with its parameters' names, the ones known given: a value coerced to the type with them
            // (dyn_unmarked), `Fn(i32) -> U` for `Fn(T) -> U` with T = i32)
            (Ty::Dyn(p), Ty::Dyn(a)) if p != a && p.contains(PMARK) && self.dyn_named_as(p, a, sol) => true,
            (Ty::Dyn(p), Ty::Dyn(a)) if p != a && p.contains(PMARK) => match (self.dyn_parts(p), self.dyn_parts(a)) {
                // (a trait object's arguments are no coercion site: their references' mutabilities agree)
                (Some((n1, x)), Some((n2, y))) => n1 == n2 && x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| p.0 == q.0 && self.refs_agree(p.1, q.1, 0) && self.match_ty(p.1, q.1, sol)),
                _ => false,
            },
            (a, b) => a == b,
        }
    }

    /// Whether a is the `dyn` type's text p with its parameters by their names, those of them known then given (sol):
    /// a value coerced to it where only some were known (dyn_unmarked). Which were is not kept, so each subset of
    /// the ones known now is tried
    fn dyn_named_as(&self, p: &'static String, a: &str, sol: &[Option<Ty>]) -> bool {
        let mut known: Vec<u8> = dyn_marks(p).filter(|&i| sol.get(i as usize).copied().flatten().is_some()).collect();
        known.sort_unstable();
        known.dedup();
        if known.len() > 6 {
            return false;
        }
        (0u32..1 << known.len()).any(|mask| {
            let args: Vec<Ty> = (0..sol.len())
                .map(|i| match known.iter().position(|&k| k as usize == i) {
                    Some(b) if mask >> b & 1 == 1 => sol[i].unwrap(),
                    _ => Ty::Param(i as u8),
                })
                .collect();
            matches!(self.dyn_unmarked(self.subst(Ty::Dyn(p), &args)), Ty::Dyn(x) if x == a)
        })
    }

    /// t with each `dyn` type's parameters (dyn_canon's marks) by their names: the type as it was without the marks,
    /// for a value of a type parameter not known yet to be coerced to (a closure passed for `Box<dyn Fn(T) -> T>`)
    pub fn dyn_unmarked(&self, t: Ty) -> Ty {
        if !self.has_param(t) {
            return t;
        }
        match t {
            Ty::Dyn(s) => Ty::Dyn(intern_str(&self.dyn_shown(s, true))),
            Ty::Ref(m, i) => Ty::Ref(m, intern(self.dyn_unmarked(*i))),
            Ty::Ptr(m, i) => Ty::Ptr(m, intern(self.dyn_unmarked(*i))),
            Ty::Box(i) => Ty::Box(intern(self.dyn_unmarked(*i))),
            Ty::Slice(i) => Ty::Slice(intern(self.dyn_unmarked(*i))),
            Ty::Array(i, n) => Ty::Array(intern(self.dyn_unmarked(*i)), n),
            Ty::Fn(f) => Ty::Fn(intern_fn(FnTy { params: f.params.iter().map(|&p| self.dyn_unmarked(p)).collect(), ret: self.dyn_unmarked(f.ret) })),
            Ty::Struct(sid) => match self.structs[sid as usize].base {
                Some(b) => {
                    let args: Vec<Ty> = self.structs[sid as usize].args.iter().map(|&a| self.dyn_unmarked(a)).collect();
                    self.instantiate(b, args).map_or(t, Ty::Struct)
                }
                None => t,
            },
            t => t,
        }
    }

    /// A `dyn` type's text read: its trait's name, and its type arguments (a closure trait's parameters and result,
    /// as `Fn(A) -> R`'s), each with the name of the associated type it binds, if it does
    fn dyn_parts(&self, s: &str) -> Option<(String, Vec<(Option<String>, Ty)>)> {
        use crate::parse::{Bound, GArg, ItemKind, Ty as PTy};
        let f = crate::parse::parse_file(&format!("type __B = dyn {s};")).ok()?;
        let ItemKind::Type(_, _, _, Some(PTy::Dyn(bs))) = &f.items.first()?.kind else { return None };
        let Some(Bound::Trait(_, p)) = bs.first() else { return None };
        let seg = p.segs.last()?;
        let ty = |t: &PTy| self.resolve_with(&crate::ast_types::tysyn(t, None).ok()?, &|_| None).ok();
        let mut out = vec![];
        if let Some((ps, r)) = &seg.fn_args {
            for t in ps {
                out.push((None, ty(t)?));
            }
            out.push((None, match r {
                Some(r) => ty(r)?,
                None => Ty::Unit,
            }));
        }
        for a in &seg.args {
            match a {
                GArg::Ty(t) => out.push((None, ty(t)?)),
                GArg::Bind(n, t) => out.push((Some(n.clone()), ty(t)?)),
                GArg::Lifetime(_) => {}
                GArg::Const(_) => return None,
            }
        }
        Some((seg.name.clone(), out))
    }

    /// A trait's name written as a type behind a pointer before the 2021 edition (`Box<X>`, `&A<T>`): its trait object,
    /// as `dyn X` / `dyn A<T>`. None for any other type
    fn bare_dyn(&self, t: &TySyn, names: &dyn Fn(&str) -> Option<Ty>) -> Option<R<Ty>> {
        if self.edition >= 2021 {
            return None;
        }
        let (n, args): (&String, &[TySyn]) = match t {
            TySyn::Named(n) => (n, &[]),
            TySyn::App(n, a) => (n, a),
            _ => return None,
        };
        // (not a module's trait (`m__Bar` flattened), whose privacy is not checked here: a private one named through a
        // glob import is an error)
        if n.contains("__") || !(self.traits.contains_key(n) || self.trait_decls.contains(n)) || self.struct_index.contains_key(n) || self.aliases.contains_key(n) || names(n).is_some() {
            return None;
        }
        if self.deny_bare_trait {
            return Some(Err(format!("trait objects without an explicit `dyn` are deprecated: `dyn {n}` (bare_trait_objects)")));
        }
        if args.is_empty() {
            return Some(Ok(Ty::Dyn(intern_str(n))));
        }
        let mut src = vec![];
        for a in args {
            let t = match self.resolve_with(a, names) {
                Ok(t) => t,
                Err(e) => return Some(Err(e)),
            };
            src.push(self.dyn_src(t, if let TySyn::Named(x) = a { x } else { "" })?);
        }
        Some(Ok(Ty::Dyn(intern_str(&crate::parse::norm_tokens(&format!("{n}<{}>", src.join(", ")))?))))
    }

    /// The text of a `dyn` type written with names (a type parameter's, or `Self`), each of them that names gives
    /// a type replaced: a type parameter by its mark `__zp{i}_{name}`, another type by its text (dyn_src). So the
    /// type is the same where it is written in a generic fn's or type's declaration and where it is used, its
    /// parameters given (Program::subst) or named in the env of an instance. None where nothing is replaced
    fn dyn_canon(&self, s: &str, names: &dyn Fn(&str) -> Option<Ty>) -> Option<String> {
        if !s.contains(['<', '(']) {
            return None;
        }
        crate::parse::rewrite_tokens(s, &mut |k, toks| {
            let t = &toks[k];
            // (not the trait's name, a path's segment, an associated type's binding nor a keyword)
            if k == 0 || toks[k - 1].text == ":" || toks.get(k + 1).is_some_and(|n| n.text == ":" || n.text == "=") {
                return None;
            }
            if matches!(t.text.as_str(), "dyn" | "mut" | "const" | "fn" | "for" | "impl" | "as" | "where" | "unsafe" | "extern") {
                return None;
            }
            // (a name the env gives, before a mark: a generic parameter named like one is that parameter; a mark that
            // names nothing here stays as it is)
            match names(&t.text)? {
                Ty::Param(HOLE) => None,
                Ty::Param(i) => Some(format!("{PMARK}{i}_{}", t.text)),
                ty => self.dyn_src(ty, &t.text),
            }
        })
    }

    /// t as text that resolves to t again, in a `dyn` type's text: a type parameter as its mark (named `name`, or
    /// `T{i}` where empty), a struct the program cannot name (a placeholder) as `__zs{id}`. None for an associated type
    fn dyn_src(&self, t: Ty, name: &str) -> Option<String> {
        let r = |t: Ty| self.dyn_src(t, "");
        Some(match t {
            Ty::Param(HOLE) => "_".into(),
            Ty::Param(i) if name.is_empty() => format!("{PMARK}{i}_T{i}"),
            Ty::Param(i) => format!("{PMARK}{i}_{name}"),
            Ty::Ref(m, i) => format!("&{}{}", if m { "mut " } else { "" }, r(*i)?),
            Ty::Ptr(m, i) => format!("*{} {}", if m { "mut" } else { "const" }, r(*i)?),
            Ty::Slice(i) => format!("[{}]", r(*i)?),
            Ty::Array(i, n) if n < CONST_LEN_BASE => format!("[{}; {n}]", r(*i)?),
            Ty::Array(..) | Ty::Assoc(_) => return None,
            Ty::Str => "str".into(),
            Ty::Unit => "()".into(),
            Ty::Never => "!".into(),
            Ty::Box(i) => format!("Box<{}>", r(*i)?),
            Ty::Dyn(s) => format!("dyn {s}"),
            Ty::Fn(f) => {
                let ps: Vec<String> = f.params.iter().map(|&p| r(p)).collect::<Option<_>>()?;
                match f.ret {
                    Ty::Unit => format!("fn({})", ps.join(", ")),
                    rt => format!("fn({}) -> {}", ps.join(", "), r(rt)?),
                }
            }
            Ty::Struct(sid) => {
                let sd = &self.structs[sid as usize];
                let named = |b: u32| Some(&self.structs[b as usize].name).filter(|n| self.struct_index.get(*n) == Some(&b));
                let args = || sd.args.iter().map(|&a| r(a)).collect::<Option<Vec<String>>>();
                let text = match sd.base {
                    Some(b) if self.is_tuple(b) => args().map(|a| if a.len() == 1 { format!("({},)", a[0]) } else { format!("({})", a.join(", ")) }),
                    Some(b) => named(b).and_then(|n| Some(format!("{n}<{}>", args()?.join(", ")))),
                    None => named(sid).cloned(),
                };
                // (a placeholder or a closure as its mark; another struct the program does not name, not at all)
                match text {
                    Some(t) => t,
                    None if self.is_placeholder(sid) || self.closure_of_struct.contains_key(&self.base_of(sid)) => format!("{SMARK}{sid}"),
                    None => return None,
                }
            }
            t => {
                let n = self.tn(t);
                n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_').then_some(n)?
            }
        })
    }

    /// Ty → TySyn (to write instances by name in the cache)
    pub fn ty_syn(&self, t: Ty) -> TySyn {
        match t {
            Ty::Ref(m, i) => TySyn::Ref(m, Box::new(self.ty_syn(*i))),
            Ty::Ptr(m, i) => TySyn::Ptr(m, Box::new(self.ty_syn(*i))),
            Ty::Slice(i) => TySyn::Slice(Box::new(self.ty_syn(*i))),
            Ty::Array(i, n) => TySyn::Array(Box::new(self.ty_syn(*i)), ArrLen::Lit(n as u64)),
            Ty::Str => TySyn::Str,
            // (a struct's mark names it: a placeholder, which no cache keeps)
            Ty::Dyn(s) => TySyn::Dyn(self.dyn_shown(s, false)),
            Ty::Box(i) => TySyn::App("Box".into(), vec![self.ty_syn(*i)]),
            Ty::Assoc(a) => TySyn::Assoc(Box::new(self.ty_syn(a.base)), a.name.clone()),
            Ty::Fn(f) => TySyn::FnPtr(
                f.params.iter().chain([&f.ret]).map(|&t| self.ty_syn(t)).collect(),
            ),
            Ty::Struct(id) => {
                let sd = &self.structs[id as usize];
                match sd.base {
                    Some(b) if self.is_tuple(b) => {
                        TySyn::Tuple(sd.args.iter().map(|&a| self.ty_syn(a)).collect())
                    }
                    Some(b) => TySyn::App(
                        self.structs[b as usize].name.clone(),
                        sd.args.iter().map(|&a| self.ty_syn(a)).collect(),
                    ),
                    None => TySyn::Named(sd.name.clone()),
                }
            }
            t => TySyn::Prim(t),
        }
    }

    fn layout(
        &mut self,
        id: usize,
        sdefs: &[(usize, String, AdtShape, Vec<String>)],
        items: &[SrcItem],
        state: &mut [u8],
    ) -> R<()> {
        match state[id] {
            2 => return Ok(()),
            1 => {
                // (at the item's first token past its attributes, as rustc: name_line reads the mark)
                return Err(format!("{}:1: \u{5}\u{5}recursive type `{}` has infinite size", items[sdefs[id].0].line, sdefs[id].1));
            }
            _ => {}
        }
        state[id] = 1;
        let (ii, _, shape, generics) = &sdefs[id];
        let line = items[*ii].line;
        let gnames: Vec<&str> = generics.iter().map(|s| s.as_str()).collect();
        // Lay out first the types held by value (not what references / pointers point to)
        let fields_of = |sh: &AdtShape| -> Vec<TySyn> {
            match sh {
                AdtShape::Struct(f) => f.iter().map(|x| x.1.clone()).collect(),
                AdtShape::Enum(vs) => vs
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|x| x.1.clone()))
                    .collect(),
            }
        };
        let mut todo: Vec<(TySyn, Vec<String>)> = fields_of(shape)
            .into_iter()
            .map(|t| (t, generics.clone()))
            .collect();
        let mut visited_bases = HashSet::default();
        while let Some((t, gs)) = todo.pop() {
            match t {
                TySyn::Named(n) if !gs.contains(&n) => {
                    if let Some(&sid) = self.struct_index.get(&n) {
                        if sdefs[sid as usize].3.is_empty() {
                            self.layout(sid as usize, sdefs, items, state)?;
                        }
                    }
                }
                // `Box<T>` is a pointer, so T's layout is not needed
                TySyn::App(n, _) if n == "Box" && !self.struct_index.contains_key(&n) => {}
                // (`C::Assoc<Self>` / `type MaybeBox<T> = <T as A<T, Box<T>>>::B;`: a projection, whose type is the
                // impl's: whether it holds its arguments by value is not known here)
                // (`A::Allocated<Self>` arrives as `Allocated<..>`: a name that is no struct and no alias is such an
                // associated type)
                TySyn::App(n, _) if n.contains("::") || (!self.struct_index.contains_key(&n) && !self.aliases.contains_key(&n)) => {}
                // (`type Alias<T> = (T, <() as Tr>::A);`: its value with the arguments, whose projections are skipped
                // and whose other parts are held as written)
                TySyn::App(n, args) if self.aliases.get(&n).is_some_and(|a| syn_has_assoc(&a.1)) => {
                    let (ps, body) = self.aliases[&n].clone();
                    if ps.len() == args.len() {
                        todo.push((subst_tysyn(&body, &ps, &args), gs.clone()));
                    }
                }
                TySyn::App(n, args) => {
                    // The type arguments the type holds by value (`Vec<T>`'s T is behind a pointer, so it is not needed)
                    let mut by_value = vec![true; args.len()];
                    // (a generic type holding itself by value: `struct Foo<T> { f: Option<Foo<T>> }`)
                    if self.struct_index.get(&n) == Some(&(id as u32)) {
                        return Err(format!("{line}:1: recursive type `{}` has infinite size", sdefs[id].1));
                    }
                    if let Some(&sid) = self.struct_index.get(&n) {
                        let bg = sdefs[sid as usize].3.clone();
                        let fs = fields_of(&sdefs[sid as usize].2);
                        if bg.len() == args.len() {
                            for (k, g) in bg.iter().enumerate() {
                                by_value[k] = fs.iter().any(|f| syn_holds_by_value(f, g));
                            }
                        }
                        if visited_bases.insert(sid) {
                            todo.extend(fs.into_iter().map(|t| (t, bg.clone())));
                        }
                    }
                    todo.extend(args.into_iter().zip(by_value).filter(|(_, v)| *v).map(|(a, _)| (a, gs.clone())));
                }
                TySyn::Tuple(es) => todo.extend(es.into_iter().map(|a| (a, gs.clone()))),
                TySyn::Array(e, _) => todo.push((*e, gs)),
                _ => {}
            }
        }
        let res = |this: &Program, fs: &[(String, TySyn)]| -> R<Vec<(String, Ty)>> {
            fs.iter()
                .map(|(n, t)| {
                    Ok((
                        n.clone(),
                        this.resolve_g(t, &gnames)
                            .map_err(|e| format!("{line}:1: {e}"))?,
                    ))
                })
                .collect()
        };
        let generic = !generics.is_empty();
        match shape {
            AdtShape::Struct(fields) => {
                let fs = res(self, fields)?;
                if generic {
                    self.structs.get_mut(id).fields =
                        fs.into_iter().map(|(n, t)| (n, t, 0)).collect();
                } else {
                    let (out, size, align) = self.adt_fields_layout(&sdefs[id].1, fs);
                    let sd = self.structs.get_mut(id);
                    sd.fields = out;
                    sd.size = size;
                    sd.align = align;
                }
            }
            AdtShape::Enum(vs) => {
                let mut discrs = vec![];
                // (the implicit successor, as a value of the repr type: None past its range)
                let mut next: Option<i128> = Some(0);
                // (a discriminant is a value of the repr type, isize without one: `(1 << 63) >> 63` is -1)
                let dt = self.reprs.get(&self.structs[id as u32 as usize].name).copied().unwrap_or(Ty::Isize);
                let dit = It::of_ty(dt);
                let mut env: Vec<(String, i128, Option<It>, Option<crate::parse::Expr>)> = vec![];
                for v in vs {
                    let d = if v.discr_src.is_empty() {
                        match v.discr {
                            Some(x) => x,
                            // (an implicit one past the repr type's range is E0370: `#[repr(u8)]`'s `A = 255, B`)
                            None => match next.filter(|&n| dit.is_none_or(|it| it.fits(n))) {
                                Some(n) => n as i64,
                                None => return Err(format!("{line}:1: enum discriminant overflowed (E0370): `{}`", v.name)),
                            },
                        }
                    } else {
                        // (other constants, and the variants before it: `Y = Foo::X as isize - 3`)
                        let e = crate::parse::parse_expr(&v.discr_src).map_err(|e| format!("{line}:1: {e}"))?;
                        let x = self.eval_const_as(&subst_consts(&e, &env), dt).map_err(|e| format!("{line}:1: {e}"))?;
                        i64::try_from(x).or_else(|_| u64::try_from(x).map(|u| u as i64)).map_err(|_| format!("{line}:1: discriminant out of range"))?
                    };
                    if !v.discr_src.is_empty() || vs.iter().any(|w| !w.discr_src.is_empty()) {
                        // (as a value of the repr type: `#[repr(u64)]`'s `1 << 63` is 2^63, not i64::MIN)
                        let it = It::of_ty(dt);
                        let val = it.map_or(d as i128, |it| it.wrap(d as i128));
                        env.push((format!("{}::{}", sdefs[id].1, v.name), val, it, None));
                        env.push((format!("Self::{}", v.name), val, it, None));
                    }
                    if discrs.contains(&d) {
                        return Err(format!(
                            "{line}:1: discriminant value `{d}` assigned more than once"
                        ));
                    }
                    discrs.push(d);
                    next = dit.map_or(d as i128, |it| it.wrap(d as i128)).checked_add(1);
                }
                let repr = self.reprs.get(&self.structs[id as u32 as usize].name).copied();
                let tag = if let Some(t) = repr {
                    // (`#[repr(u8)]`: the discriminants are values of that type, `!0 as u8` is 255)
                    let bits = t.bits();
                    if bits < 64 {
                        for d in &mut discrs {
                            let m = (1i64 << bits) - 1;
                            let v = *d & m;
                            *d = if t.signed() && v >> (bits - 1) == 1 { v - (1i64 << bits) } else { v };
                        }
                    }
                    t
                } else {
                    // (the smallest integer type that holds every discriminant, as rustc picks)
                    let (lo, hi) = (discrs.iter().copied().min().unwrap_or(0), discrs.iter().copied().max().unwrap_or(0));
                    if lo >= 0 {
                        if hi <= 0xff { Ty::U8 } else if hi <= 0xffff { Ty::U16 } else if hi <= 0xffff_ffff { Ty::U32 } else { Ty::U64 }
                    } else if lo >= -0x80 && hi <= 0x7f {
                        Ty::I8
                    } else if lo >= -0x8000 && hi <= 0x7fff {
                        Ty::I16
                    } else if lo >= -0x8000_0000 && hi <= 0x7fff_ffff {
                        Ty::I32
                    } else {
                        Ty::I64
                    }
                };
                let mut rv = vec![];
                for (v, d) in vs.iter().zip(discrs) {
                    rv.push((v.name.clone(), v.kind, res(self, &v.fields)?, d));
                }
                if generic {
                    let sd = self.structs.get_mut(id);
                    sd.tag = tag;
                    sd.variants = rv
                        .into_iter()
                        .map(|(name, kind, fs, discr)| VariantDef {
                            name,
                            kind,
                            fields: fs.into_iter().map(|(n, t)| (n, t, 0)).collect(),
                            discr,
                        })
                        .collect();
                } else {
                    let (vs, size, align) = self.enum_layout(&sdefs[id].1, rv, tag);
                    let sd = self.structs.get_mut(id);
                    sd.tag = tag;
                    sd.variants = vs;
                    sd.size = size;
                    sd.align = align;
                }
            }
        }
        state[id] = 2;
        Ok(())
    }

    pub fn resolve(&self, t: &TySyn) -> R<Ty> {
        self.resolve_with(t, &|_| None)
    }

    pub fn resolve_env(&self, t: &TySyn, env: &TyEnv) -> R<Ty> {
        self.resolve_with(t, &|n| {
            if n == "Self" {
                return env.self_ty.or(env.self_sid.map(Ty::Struct));
            }
            env.params.iter().find(|p| p.0 == n).map(|p| p.1)
        })
    }

    /// Method `name` of struct `sid`
    /// Whether the references in two types' struct arguments agree, `&` and `&mut` apart (match_ty does not tell them
    /// apart), past any references around the two: `Either<&L, &R>` is not `Either<&mut i32, &mut u8>`'s self type, and
    /// `&W<&T>` (a `&self` receiver) is not `W<&mut i32>`'s
    /// Whether two impls' self types could be one type: they unify, each impl's type and const parameters its own
    /// (`Cake<T>` and `Cake<Box<U>>` could, `[u8; N]` and `[u8; 1]` at N = 1, `fn(T)` and `fn(i32)`; `W<i32>` and
    /// `W<u8>` could not, nor `Either<&L, &R>` and `Either<&mut L, &mut R>`, nor `W<T, T>` and `W<u8, u16>`)
    pub fn self_tys_overlap(&self, a: Ty, b: Ty) -> bool {
        use crate::types::CONST_LEN_BASE;
        // (a binding: of impl s's parameter (a type parameter, or a const one), to a type of impl s2's. A const value is
        // `Array(Unit, n)`, as match_ty has it, and an array length `CONST_LEN_BASE + i` is const parameter i: one
        // binding for a const parameter, whether it stands as a type argument or as a length)
        struct U<'a> {
            p: &'a Program,
            tys: FxMap<(u8, u8), (u8, Ty)>,
        }
        fn len_ty(n: u32) -> Ty {
            if n >= CONST_LEN_BASE { Ty::Param((n - CONST_LEN_BASE) as u8) } else { Ty::Array(intern(Ty::Unit), n) }
        }
        impl U<'_> {
            /// Whether impl s's parameter x occurs in t (of impl st), through the bindings so far
            fn occurs(&self, s: u8, x: u8, st: u8, t: Ty, depth: u32) -> bool {
                if depth > 32 {
                    return true;
                }
                match t {
                    Ty::Param(y) if (st, y) == (s, x) => true,
                    Ty::Param(y) => self.tys.get(&(st, y)).is_some_and(|&(s2, t2)| self.occurs(s, x, s2, t2, depth + 1)),
                    Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) | Ty::Slice(i) => self.occurs(s, x, st, *i, depth + 1),
                    Ty::Array(i, n) => (n >= CONST_LEN_BASE && self.occurs(s, x, st, len_ty(n), depth + 1)) || self.occurs(s, x, st, *i, depth + 1),
                    Ty::Fn(f) => f.params.iter().any(|&u| self.occurs(s, x, st, u, depth + 1)) || self.occurs(s, x, st, f.ret, depth + 1),
                    Ty::Struct(sid) => self.p.structs[sid as usize].args.iter().any(|&u| self.occurs(s, x, st, u, depth + 1)),
                    _ => false,
                }
            }
            fn bind(&mut self, s: u8, x: u8, st: u8, t: Ty) -> bool {
                // (`T = Box<T>` has no finite solution: no overlap)
                if self.occurs(s, x, st, t, 0) {
                    return false;
                }
                self.tys.insert((s, x), (st, t));
                true
            }
            fn ty(&mut self, sa: u8, a: Ty, sb: u8, b: Ty, depth: u32) -> bool {
                if depth > 32 {
                    return true;
                }
                if let Ty::Param(x) = a {
                    if let Some(&(s2, t2)) = self.tys.get(&(sa, x)) {
                        return self.ty(s2, t2, sb, b, depth + 1);
                    }
                }
                if let Ty::Param(y) = b {
                    if let Some(&(s2, t2)) = self.tys.get(&(sb, y)) {
                        return self.ty(sa, a, s2, t2, depth + 1);
                    }
                }
                match (a, b) {
                    (Ty::Param(x), Ty::Param(y)) if sa == sb && x == y => true,
                    (Ty::Param(x), _) => self.bind(sa, x, sb, b),
                    (_, Ty::Param(y)) => self.bind(sb, y, sa, a),
                    // (an associated type, `T::Item`, may be any type)
                    (Ty::Assoc(_), _) | (_, Ty::Assoc(_)) => true,
                    (Ty::Ref(m, x), Ty::Ref(n, y)) | (Ty::Ptr(m, x), Ty::Ptr(n, y)) => m == n && self.ty(sa, *x, sb, *y, depth + 1),
                    (Ty::Box(x), Ty::Box(y)) | (Ty::Slice(x), Ty::Slice(y)) => self.ty(sa, *x, sb, *y, depth + 1),
                    (Ty::Array(x, n), Ty::Array(y, k)) => {
                        let lens = if n < CONST_LEN_BASE && k < CONST_LEN_BASE { n == k } else { self.ty(sa, len_ty(n), sb, len_ty(k), depth + 1) };
                        lens && self.ty(sa, *x, sb, *y, depth + 1)
                    }
                    (Ty::Fn(f), Ty::Fn(g)) => {
                        f.params.len() == g.params.len()
                            && f.params.iter().zip(&g.params).all(|(&u, &v)| self.ty(sa, u, sb, v, depth + 1))
                            && self.ty(sa, f.ret, sb, g.ret, depth + 1)
                    }
                    (Ty::Struct(x), Ty::Struct(y)) => {
                        let p = self.p;
                        if p.base_of(x) != p.base_of(y) {
                            return false;
                        }
                        let (xa, ya) = (p.structs[x as usize].args.clone(), p.structs[y as usize].args.clone());
                        xa.len() != ya.len() || xa.iter().zip(&ya).all(|(&u, &v)| self.ty(sa, u, sb, v, depth + 1))
                    }
                    _ => a == b,
                }
            }
        }
        U { p: self, tys: FxMap::default() }.ty(0, a, 1, b, 0)
    }

    /// The impls of the consts of const_groups, once types resolve: their self types and trait arguments. Two inherent
    /// impls' consts whose self types may be one type are E0592's duplicate after all (`impl<T> W<T> { const C }` and
    /// `impl W<u8> { const C }`), at the first one's line as rustc has it
    fn const_group_impls(&mut self, items: &[SrcItem], sums: &[Summary]) -> R<()> {
        for g in 0..self.const_groups.len() {
            for k in 0..self.const_groups[g].len() {
                let ci = self.const_groups[g][k];
                let ii = self.consts[ci as usize].item as usize;
                let crate::lexer::ItemKind::ImplMember(h) = items[ii].kind else { continue };
                let sm = &sums[h as usize];
                let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                let self_ty = sm.impl_self_ty.as_ref().and_then(|t| self.resolve_g(t, &gn).ok());
                let (trait_name, trait_args) = match sm.impl_trait {
                    Some(_) => {
                        let (n, a) = impl_trait_of(self, items, sums, ii, &gn);
                        (Some(n), a)
                    }
                    None => (None, None),
                };
                let ngen = gn.len();
                self.const_impls.insert(ci, ConstImpl { self_ty, trait_name, trait_args, ngen });
            }
            let cs = &self.const_groups[g];
            for (k, &b) in cs.iter().enumerate() {
                for &a in &cs[..k] {
                    let (ia, ib) = (&self.const_impls[&a], &self.const_impls[&b]);
                    if ia.trait_name.is_some() || ib.trait_name.is_some() {
                        continue;
                    }
                    let apart = matches!((ia.self_ty, ib.self_ty), (Some(x), Some(y)) if !self.self_tys_overlap(x, y));
                    if !apart {
                        let c = &self.consts[a as usize];
                        return Err(format!("{}:1: duplicate definition of `{}`", items[c.item as usize].line, c.name));
                    }
                }
            }
        }
        Ok(())
    }

    /// Of the consts of ci's group (const_groups), the one of the impl a use names: by the self type t (an exact match
    /// of the impl's), the trait tr and its arguments args, and else by the type the use expects (want, typenum's
    /// `let x: i16 = Z0::INT`). ci itself when ci has no group, or when none fits
    pub fn pick_const(&self, ci: u32, t: Option<Ty>, tr: Option<&str>, args: Option<&[Ty]>, want: Option<Ty>) -> u32 {
        let Some(&g) = self.const_group.get(&ci) else { return ci };
        let group = &self.const_groups[g as usize];
        let tr = tr.map(|tr| tr.rsplit("::").next().unwrap_or(tr));
        let fits = |c: u32| -> bool {
            let Some(im) = self.const_impls.get(&c) else { return false };
            let mut sol = vec![None; im.ngen];
            if let Some(t) = t {
                if !im.self_ty.is_some_and(|st| crate::traits::mtch(self, st, t, &mut sol, false, 0)) {
                    return false;
                }
            }
            if tr.is_some_and(|tr| im.trait_name.as_deref() != Some(tr)) {
                return false;
            }
            match (args, &im.trait_args) {
                (None, _) => true,
                (Some(a), Some(ta)) => a.len() == ta.len() && ta.iter().zip(a).all(|(&p, &x)| crate::traits::mtch(self, p, x, &mut sol, false, 0)),
                (Some(_), None) => false,
            }
        };
        let fit: Vec<u32> = group.iter().copied().filter(|&c| fits(c)).collect();
        if let [c] = fit[..] {
            return c;
        }
        // (several: the one whose declared type is the one wanted)
        if let (Some(w), true) = (want.filter(|&w| !self.has_param(w)), fit.len() > 1) {
            let by_ty: Vec<u32> = fit.iter().copied().filter(|&c| self.consts[c as usize].ty == w).collect();
            if let [c] = by_ty[..] {
                return c;
            }
        }
        if fit.contains(&ci) { ci } else { fit.first().copied().unwrap_or(ci) }
    }

    /// The self type a path to an associated item names by its segment before the last (`W::<u16>::new`, `Alias::C`
    /// of `type Alias = W<u16>`, `Self::C`, `T::C` of a type parameter), modules before it stripped; None when that is
    /// no type (or a type parameter still)
    pub fn path_self(&self, p: &crate::parse::Path, env: &TyEnv) -> Option<Ty> {
        let n = p.segs.len();
        if n < 2 || (n > 2 && self.strip_mods_p(&PPath::of(p)).is_none_or(|q| q.segs.len() != 2)) {
            return None;
        }
        let ty = crate::parse::Ty::Path(None, crate::parse::Path { global: false, segs: vec![p.segs[n - 2].clone()], pos: Default::default() });
        let ts = crate::ast_types::tysyn(&ty, Some(&TySyn::Named("Self".into()))).ok()?;
        self.resolve_env(&ts, env).ok().filter(|t| !matches!(t, Ty::Param(_)))
    }

    /// Of the functions of one name in impls for other instances of a type (self_ty_groups: `impl W<u8> { fn new() }`
    /// beside `impl W<u16> { fn new() }`, `impl From<u8> for W<u8>` beside `impl From<u16> for W<u16>`), one of the impl
    /// for t, the self type the path to it names (path_self): id when it is one, else the first such inherent
    /// function, then the first trait impl's; id when none is
    pub fn fn_for_self_ty(&self, id: u32, t: Ty) -> u32 {
        let name = &self.names[id as usize];
        let Some(group) = self.overloads.get(name.split('#').next().unwrap_or(name)) else { return id };
        let fits = |c: u32| {
            let sig = &self.sigs[c as usize];
            sig.self_ty.is_some_and(|st| self.match_ty_exact(st, t, &mut vec![None; sig.generics.len()]))
        };
        if fits(id) {
            return id;
        }
        let inherent = |c: &u32| self.sigs[*c as usize].impl_trait.is_none();
        group.iter().copied().filter(inherent).find(|&c| fits(c)).or_else(|| group.iter().copied().find(|&c| fits(c))).unwrap_or(id)
    }

    /// fn_for_self_ty of the fn id a path p names (fn_by_p), by the self type the path names in env
    pub fn fn_of_path_self(&self, id: u32, p: &crate::parse::Path, env: &TyEnv) -> u32 {
        if self.self_ty_groups.is_empty() {
            return id;
        }
        let name = &self.names[id as usize];
        if !self.self_ty_groups.contains(name.split('#').next().unwrap_or(name)) {
            return id;
        }
        match self.path_self(p, env) {
            Some(t) => self.fn_for_self_ty(id, t),
            None => id,
        }
    }

    /// Of the inherent methods with `self` overloading `id` (in impls for other instances of the type), the one whose
    /// self type `t` is (match_ty and arg_refs_agree); `id` when there is not exactly one, or when `id` is not inherent
    pub fn inherent_overload_for(&self, id: u32, t: Ty) -> u32 {
        // (only from an inherent method: a trait impl's (`impl Foo for AtomicUsize { fn store(self, ..) }` beside the
        // inherent `store(&self, ..)`) is chosen by the receiver later, as rustc's by-value step before the autoref)
        if self.sigs[id as usize].impl_trait.is_some() || !self.sigs[id as usize].has_self {
            return id;
        }
        // (by the group's name: self_method may give an overload, `W::f#1`)
        let name = &self.names[id as usize];
        let Some(group) = self.overloads.get(name.split('#').next().unwrap_or(name)) else { return id };
        let fit: Vec<u32> = group
            .iter()
            .copied()
            .filter(|&c| {
                let sig = &self.sigs[c as usize];
                let mut sol = vec![None; sig.generics.len()];
                sig.has_self && sig.impl_trait.is_none() && sig.self_ty.is_some_and(|st| self.match_ty(st, t, &mut sol) && self.arg_refs_agree(st, t))
            })
            .collect();
        match fit.as_slice() {
            [c] => *c,
            _ => id,
        }
    }

    pub fn arg_refs_agree(&self, a: Ty, b: Ty) -> bool {
        fn strip(mut t: Ty) -> Ty {
            while let Ty::Ref(_, i) = t {
                t = *i;
            }
            t
        }
        fn deep(p: &Program, a: Ty, b: Ty, depth: u32) -> bool {
            if depth > 16 {
                return true;
            }
            match (a, b) {
                (Ty::Ref(m, x), Ty::Ref(n, y)) | (Ty::Ptr(m, x), Ty::Ptr(n, y)) => m == n && deep(p, *x, *y, depth + 1),
                (Ty::Ref(..), _) => false,
                // (a raw pointer's self type is not a reference's: `W<*const i32>` is not `W<&i32>`'s, which match_ty lets
                // through as a coercion)
                (Ty::Ptr(..), _) => false,
                // (and beneath arrays, slices and boxes: `W<[&i32; 1]>` is not `W<[&mut i32; 1]>`)
                (Ty::Array(x, _), Ty::Array(y, _)) | (Ty::Slice(x), Ty::Slice(y)) | (Ty::Box(x), Ty::Box(y)) => deep(p, *x, *y, depth + 1),
                // (and in a function pointer's parameters and result: `W<fn(&i32)>` is not `W<fn(&mut i32)>`)
                (Ty::Fn(f), Ty::Fn(g)) => {
                    f.params.len() == g.params.len()
                        && f.params.iter().zip(&g.params).all(|(&u, &v)| deep(p, u, v, depth + 1))
                        && deep(p, f.ret, g.ret, depth + 1)
                }
                (Ty::Struct(x), Ty::Struct(y)) if x != y => {
                    let (xa, ya) = (&p.structs[x as usize].args, &p.structs[y as usize].args);
                    xa.len() != ya.len() || xa.iter().zip(ya).all(|(&u, &v)| deep(p, u, v, depth + 1))
                }
                _ => true,
            }
        }
        match (strip(a), strip(b)) {
            (Ty::Struct(x), Ty::Struct(y)) if x != y => {
                let (xa, ya) = (&self.structs[x as usize].args, &self.structs[y as usize].args);
                xa.len() != ya.len() || xa.iter().zip(ya).all(|(&u, &v)| deep(self, u, v, 0))
            }
            _ => true,
        }
    }

    pub fn method(&self, sid: u32, name: &str) -> Option<u32> {
        self.index
            .get(&format!(
                "{}::{name}",
                self.structs[self.base_of(sid) as usize].name
            ))
            .copied()
    }

    /// The method named name a method call on sid's type finds among its impls: one with `self`. An associated
    /// function without it is no method candidate (rustc's probe): `impl Wire for Vec<T> { fn get(r: &[u8]) -> Self }`
    /// does not hide the inherent `Vec::get(&self, i)` from `v.get(i)` (#207). None when every fn of the name lacks it.
    /// For a receiver of the type by value (self_method_at for one reached through a reference)
    pub fn self_method(&self, sid: u32, name: &str) -> Option<u32> {
        self.self_method_at(sid, name, None)
    }

    /// self_method for a receiver that reaches sid's type last through `wrap`: a `&` (Some(false)) or a `&mut`
    /// (Some(true)), or None by value (also through a Box or a deref)
    pub fn self_method_at(&self, sid: u32, name: &str, wrap: Option<bool>) -> Option<u32> {
        let key = format!("{}::{name}", self.structs[self.base_of(sid) as usize].name);
        let &id = self.index.get(&key)?;
        if self.sigs[id as usize].has_self {
            return Some(id);
        }
        // (one with `self` among the fns of the name in other impls of the type, as rustc's probe picks it: the receiver
        // steps in order, the type as the receiver reaches it, then `&` and `&mut` of it (by value: `self`, `&self`,
        // `&mut self`; through a `&`: `&self`, `self`, `&mut self`; through a `&mut`: `&mut self`, `self`, `&self`), and
        // at one step an inherent method before the traits'. So `S(0).f()` with `impl Take for S { fn f(self) }` beside
        // `impl S { fn f(&self) }` is Take's and `(&s).f()` the inherent one; with `impl Other for S { unsafe fn
        // f(&self) }` instead, `S(0).f()` is the inherent one. One for sid's instance first)
        let group = self.overloads.get(&key)?;
        let rank = |c: u32| -> (u8, bool) {
            let sig = &self.sigs[c as usize];
            let step = match (sig.params.first(), wrap) {
                (Some(Ty::Ref(false, _)), Some(false)) | (Some(Ty::Ref(true, _)), Some(true)) => 0,
                (Some(Ty::Struct(_) | Ty::Param(_)), None) => 0,
                (Some(Ty::Struct(_) | Ty::Param(_)), Some(_)) | (Some(Ty::Ref(false, _)), None) => 1,
                (Some(Ty::Ref(..)), _) => 2,
                _ => 3,
            };
            (step, sig.impl_trait.is_some())
        };
        let t = Ty::Struct(sid);
        let fits = |c: u32| {
            let sig = &self.sigs[c as usize];
            sig.self_ty.is_some_and(|st| self.match_ty(st, t, &mut vec![None; sig.generics.len()]) && self.arg_refs_agree(st, t))
        };
        let with_self = || group.iter().copied().filter(|&c| self.sigs[c as usize].has_self);
        with_self().filter(|&c| fits(c)).min_by_key(|&c| rank(c)).or_else(|| with_self().min_by_key(|&c| rank(c)))
    }

    /// self_method resolved for a receiver of sid's own instance: of inherent methods of the name in impls for other
    /// instances (`impl Target<u8> { fn edit(&self) }`, `impl Target<u16> { fn edit(&mut self) }`), the one for sid's
    /// (inherent_overload_for), for what reads how the call takes its receiver
    pub fn self_method_of(&self, sid: u32, name: &str) -> Option<u32> {
        self.self_method(sid, name).map(|id| self.inherent_overload_for(id, Ty::Struct(sid)))
    }

    pub fn size_align(&self, t: Ty) -> (u32, u32) {
        match t {
            Ty::Struct(id) => {
                let s = &self.structs[id as usize];
                (s.size, s.align)
            }
            Ty::Array(e, n) => {
                // (the provisional length of a const generic (CONST_LEN_BASE + i) is large, so wrap around: it is never used)
                let (s, a) = self.size_align(*e);
                (s.wrapping_mul(n), a)
            }
            t if t.is_fat() => (16, 8),
            // (an unsized tail has size 0 and the element's alignment: the tail of `S<[T]>` sits where `S<[T; N]>`'s does)
            Ty::Slice(e) => (0, self.size_align(*e).1),
            Ty::Unit | Ty::Never | Ty::Str | Ty::Dyn(_) | Ty::Param(_) => (0, 1),
            t => (t.bits() / 8, t.bits() / 8),
        }
    }

    pub fn field(&self, sid: u32, name: &str) -> Option<(Ty, u32)> {
        self.structs[sid as usize]
            .fields
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|&(_, t, o)| (t, o))
    }

    /// The type name for error messages
    /// A `dyn` type's text with its marks (dyn_canon) as the program writes them: a struct's by its name, and (params)
    /// a type parameter's by the parameter's name
    fn dyn_shown(&self, s: &str, params: bool) -> String {
        if !s.contains(PMARK) && !s.contains(SMARK) {
            return s.to_string();
        }
        crate::parse::rewrite_tokens(s, &mut |k, toks| {
            let w = &toks[k].text;
            if let Some(id) = dyn_struct_mark(w).filter(|&id| (id as usize) < self.structs.len()) {
                return Some(self.tn(Ty::Struct(id)));
            }
            Some(dyn_param_mark(w).filter(|_| params)?.1.to_string())
        })
        .unwrap_or_else(|| s.to_string())
    }

    pub fn tn(&self, t: Ty) -> String {
        match t {
            Ty::Ref(false, i) => format!("&{}", self.tn(*i)),
            Ty::Ref(true, i) => format!("&mut {}", self.tn(*i)),
            Ty::Struct(id) => self.structs[id as usize].name.clone(),
            Ty::Unit => "()".into(),
            Ty::Never => "!".into(),
            Ty::Vec(128) => "v128".into(),
            Ty::Vec(b) => format!("__m{b}"),
            // (a hole of an annotation (`Foo<_>`), as written)
            Ty::Param(HOLE) => "_".into(),
            Ty::Param(i) => format!("T{i}"),
            Ty::Ptr(false, i) => format!("*const {}", self.tn(*i)),
            Ty::Ptr(true, i) => format!("*mut {}", self.tn(*i)),
            Ty::Array(i, n) => format!("[{}; {n}]", self.tn(*i)),
            Ty::Slice(i) => format!("[{}]", self.tn(*i)),
            Ty::Str => "str".into(),
            Ty::Dyn(s) => format!("dyn {}", self.dyn_shown(s, true).replace(" (", "(").replace("( ", "(").replace(" )", ")")),
            Ty::Box(i) => format!("Box<{}>", self.tn(*i)),
            Ty::Assoc(a) => format!("<{}>::{}", self.tn(a.base), crate::ast_types::split_trait_assoc(&a.name).0),
            Ty::Fn(f) => {
                let ps: Vec<String> = f.params.iter().map(|&p| self.tn(p)).collect();
                match f.ret {
                    Ty::Unit => format!("fn({})", ps.join(", ")),
                    r => format!("fn({}) -> {}", ps.join(", "), self.tn(r)),
                }
            }
            t => format!("{t:?}").to_lowercase(),
        }
    }

    /// A type's identity as a key (`TypeId::of`): its structural hash (`hash_ty`, which memoizes instances), not
    /// its display name: a program's `struct v128` is not `__m128i`, at any depth (`&v128`, `(v128,)`,
    /// `Option<v128>`), and a deeply nested instance's key stays fixed-size
    pub fn type_key(&self, t: Ty) -> String {
        let mut h = KeyHasher::new();
        self.hash_ty(t, &mut h);
        format!("{:032x}", h.finish())
    }

    /// For cache keys: hashes a type (a struct by its deep hash)
    pub fn hash_ty(&self, t: Ty, h: &mut KeyHasher) {
        match t {
            Ty::Ref(m, i) => {
                h.add(&(100u8, m));
                self.hash_ty(*i, h);
            }
            Ty::Struct(id) => {
                let sd = &self.structs[id as usize];
                match sd.base {
                    // An instance: the base definition's deep hash + the type arguments
                    // (one item has several closure structs, so the name goes in too)
                    Some(b) => {
                        let cached = self.inst_keys.lock().unwrap().get(&id).copied();
                        let k = match cached {
                            Some(k) => k,
                            None => {
                                let mut hi = KeyHasher::new();
                                hi.add(&(103u8, self.structs[b as usize].deep, sd.args.len()));
                                if self.closure_of_struct.contains_key(&b) {
                                    hi.add(&self.structs[b as usize].name);
                                }
                                for &a in &sd.args {
                                    self.hash_ty(a, &mut hi);
                                }
                                let k = hi.finish();
                                self.inst_keys.lock().unwrap().insert(id, k);
                                k
                            }
                        };
                        h.add(&(103u8, k));
                    }
                    None => h.add(&(101u8, sd.deep)),
                }
            }
            Ty::Param(i) => h.add(&(102u8, i)),
            Ty::Ptr(m, i) => {
                h.add(&(104u8, m));
                self.hash_ty(*i, h);
            }
            Ty::Array(i, n) => {
                h.add(&(105u8, n));
                self.hash_ty(*i, h);
            }
            Ty::Slice(i) => {
                h.add(&106u8);
                self.hash_ty(*i, h);
            }
            Ty::Str => h.add(&107u8),
            // The vtable order (the trait's method order) is embedded in the code too
            Ty::Dyn(s) => {
                h.add(&(108u8, s));
                // (the trait's methods: of `Tr` for a `dyn Tr + Send`)
                for &f in self.traits.get(crate::types::dyn_autos(s).0).map(|v| &v[..]).unwrap_or(&[]) {
                    h.add(&self.names[f as usize]);
                }
            }
            Ty::Box(i) => {
                h.add(&110u8);
                self.hash_ty(*i, h);
            }
            Ty::Assoc(a) => {
                h.add(&(111u8, a.name.as_str()));
                self.hash_ty(a.base, h);
            }
            Ty::Fn(f) => {
                h.add(&(109u8, f.params.len()));
                for &p in f.params.iter().chain([&f.ret]) {
                    self.hash_ty(p, h);
                }
            }
            Ty::Vec(b) => h.add(&(112u8, b)),
            t => h.add(&ty_code(t)),
        }
    }

    pub fn hash_sig(&self, id: u32, h: &mut KeyHasher) {
        let s = &self.sigs[id as usize];
        h.add(self.names[id as usize].as_str());
        h.add(&(
            s.symbol.as_str(),
            s.is_extern,
            s.is_main,
            s.has_self,
            s.params.len(),
        ));
        // Fields empty for most fns go in only when non-empty (a no-op build hashes every fn)
        if !s.generics.is_empty() || s.trait_of.is_some() || !s.has_body || s.impl_trait.is_some() {
            h.add(&(&s.generics, &s.trait_of, s.has_body, &s.impl_trait));
        }
        // (which parameters must be sized, which may not: a call's E0277 depends on them)
        if let Some(v) = self.fn_sized_gens.get(&id) {
            h.add(&("sized", v));
        }
        if let Some(v) = self.fn_unsized_gens.get(&id) {
            h.add(&("unsized", v));
        }
        if let Some(t) = s.self_ty {
            self.hash_ty(t, h);
        }
        for &p in &s.params {
            self.hash_ty(p, h);
        }
        self.hash_ty(s.ret, h);
        // (what the borrow and region checkers read of the signature: a caller is checked again when it changes)
        // (most fns return no reference: ret_from is "known, none", not hashed)
        if s.ret_from != 1 << 63 {
            h.add(&s.ret_from);
        }
        if let Some(r) = &s.regions {
            h.add(&**r);
        }
    }

    /// The next fn (names.len()) into the group of the last segment `last` (by_last, ref_groups)
    fn add_to_group(&mut self, last: String) {
        let id = self.names.len() as u32;
        let groups = &mut self.ref_groups;
        let g = *self.by_last.entry(last).or_insert_with(|| {
            groups.push(vec![]);
            groups.len() as u32 - 1
        });
        let v = &mut self.ref_groups[g as usize];
        if v.last() != Some(&id) {
            v.push(id);
        }
    }

    /// The fns whose last name segment is `last` (`len` → `A::len`, `B::len`), ascending
    pub fn fns_named(&self, last: &str) -> Option<&[u32]> {
        self.by_last.get(last).map(|&g| self.ref_groups[g as usize].as_slice())
    }

    /// The set of names refs looks up (names in any table). Most identifiers (variables, keywords) need one lookup
    /// A fingerprint of what refs_in reads of the program: the names of its fns (with their groups), structs / enums,
    /// consts and aliases, and what each leads to. Two programs of one fingerprint give an item the same refs. Each
    /// entry is hashed on its own and the hashes are added, so no map's order matters
    pub fn ref_tables_fp(&self) -> Key {
        fn sum<'a, K: std::hash::Hash + ?Sized + 'a, V: std::hash::Hash + 'a>(tag: u8, it: impl Iterator<Item = (&'a K, &'a V)>) -> Key {
            let (mut s, mut n): (Key, u64) = (0, 0);
            for (k, v) in it {
                let mut h = KeyHasher::new();
                h.add(&tag);
                h.add(k);
                h.add(v);
                s = s.wrapping_add(h.finish());
                n += 1;
            }
            let mut h = KeyHasher::new();
            h.add(&tag);
            h.add(&n);
            h.add(&s);
            h.finish()
        }
        let mut h = KeyHasher::new();
        h.add(&sum(0, self.by_last.iter()));
        h.add(&sum(1, self.struct_index.iter()));
        h.add(&sum(2, self.const_index.iter()));
        h.add(&sum(3, self.const_by_last.iter()));
        h.add(&sum(4, self.aliases.iter()));
        h.finish()
    }

    pub fn ref_names(&self) -> FxSet<&str> {
        let mut s: FxSet<&str> = FxSet::default();
        s.extend(self.by_last.keys().map(|k| k.as_str()));
        s.extend(self.struct_index.keys().map(|k| k.as_str()));
        s.extend(self.const_index.keys().map(|k| k.as_str()));
        s.extend(self.const_by_last.keys().map(|k| k.as_str()));
        // (`A::V` through `type A = E;`: the alias leads to E)
        s.extend(self.aliases.keys().map(|k| k.as_str()));
        s.insert("for");
        s
    }

    /// The names in the item that name fns (their groups, ref_groups: sorted), and the structs / enums and the consts
    /// / statics it names. An over-approximation that ignores shadowing by local variables, which errs on the safe
    /// side for both cache keys and reachability. Scans the identifiers only once.
    /// names: ref_names (when given, identifiers not in it are skipped)
    pub fn refs_in(&self, src: &str, it: &SrcItem, names: Option<&FxSet<&str>>) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        let mut fns = vec![];
        let mut sts = vec![];
        let mut cs = vec![];
        let has_consts = !self.const_index.is_empty();
        let has_assoc = !self.const_by_last.is_empty();
        for &(a, b) in &it.idents {
            let s = &src[a as usize..b as usize];
            if names.is_some_and(|n| !n.contains(s)) {
                continue;
            }
            // `for` calls next on an iterator without writing the name
            let s = if s == "for" { "next" } else { s };
            let mut hit = false;
            if let Some(&g) = self.by_last.get(s) {
                fns.push(g);
                hit = true;
            }
            if let Some(&i) = self.struct_index.get(s) {
                sts.push(i);
                hit = true;
            }
            if has_consts {
                if let Some(&i) = self.const_index.get(s) {
                    cs.push(i);
                    hit = true;
                }
                if has_assoc {
                    if let Some(ids) = self.const_by_last.get(s) {
                        cs.extend_from_slice(ids);
                        hit = true;
                    }
                }
            }
            // (a name no table has: an alias, whose structs / enums a fn using `A::V` depends on, so a change to `E`
            // reaches it)
            if !hit {
                if let Some((_, syn)) = self.aliases.get(s) {
                    alias_structs(syn, &self.struct_index, &mut sts);
                }
            }
        }
        for v in [&mut fns, &mut sts, &mut cs] {
            v.sort_unstable();
            v.dedup();
        }
        (fns, sts, cs)
    }

    /// The fn IDs reachable from main (refs: each fn's groups (ref_groups)).
    /// Unreachable fns are not parsed, type-checked or compiled (relax/blaze's demand-driven approach, done AOT).
    pub fn reachable(&self, refs: &[Vec<u32>]) -> Vec<bool> {
        let groups = &self.ref_groups;
        let mut seen = vec![false; self.names.len()];
        // (a group's fns are pushed once: each name is followed once, however many fns name it)
        let mut gseen = vec![false; groups.len()];
        // Roots: main, exported fns, the panic handler (a panic can happen anywhere)
        let mut stack: Vec<u32> = self.index.get("main").copied().into_iter().collect();
        stack.extend(self.index.get(TEST_ENTRY).copied());
        stack.extend((0..self.names.len() as u32).filter(|&i| self.sigs[i as usize].export));
        stack.extend(self.panic_handler);
        // The allocator `Box::new` calls and the free drop calls (the prelude has them only when Box / Drop are used)
        stack.extend(self.index.get(crate::prelude::BOX_ALLOC).copied());
        stack.extend(self.index.get(crate::prelude::BOX_FREE).copied());
        // `vec![..]` / `"..".to_string()` use Vec's realloc / String::from without writing their names
        stack.extend(self.index.get("__exr_realloc").copied());
        stack.extend(self.index.get("String::from").copied());
        // (`process::exit` is called without its name: a root when present; the summaries leave it out when unused)
        stack.extend(self.index.get("__core_process_exit").copied());
        // (`fs::read_to_string` / `env::args` and the like are looked up by path, so names do not reach them)
        for n in [
            "__exr_fs_read", "__exr_fs_read_to_string", "__exr_fs_write", "__exr_env_args", "__exr_env_var",
            "__exr_env_var_os", "__exr_env_split_paths", "__exr_fs_remove_file", "__exr_path_new", "__exr_ptr_eq",
            "__core_ptr_swap",
        ] {
            stack.extend(self.index.get(n).copied());
        }
        // Fns called from const expressions (`const N: u32 = sq(4);`: a const is expanded as an expression where used)
        for c in &self.consts {
            if c.expr.contains('(') {
                for w in c.expr.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')) {
                    stack.extend(self.index.get(w).copied());
                }
            }
        }

        // `impl Drop`'s drop is called from drop glue, and method `eq` from `==` (without writing their names)
        stack.extend((0..self.names.len() as u32).filter(|&i| {
            let s = &self.sigs[i as usize];
            // (codegen also calls the formatting parts and Display / Debug impls without writing their names)
            matches!(s.impl_trait.as_deref(), Some("Drop"))
                || (s.has_self && self.names[i as usize].ends_with("::eq"))
                // (for `write!(w, ..)` codegen calls w.write_str(..))
                || (s.has_self && self.names[i as usize].ends_with("::write_str"))
                // (codegen calls the prelude's externs such as libm without their names: only declarations, so no extra code)
                || (s.is_extern && self.names[i as usize].starts_with("__exr_"))
                // (codegen turns the operator `a + b` into `a.add(b)`)
                || (s.has_self
                    && matches!(
                        s.impl_trait.as_deref(),
                        Some(
                            "Add" | "Sub" | "Mul" | "Div" | "Rem" | "Neg" | "Not" | "BitAnd" | "BitOr" | "BitXor"
                                | "Shl" | "Shr" | "AddAssign" | "SubAssign" | "MulAssign" | "DivAssign"
                                | "RemAssign" | "BitAndAssign" | "BitOrAssign" | "BitXorAssign" | "ShlAssign"
                                | "ShrAssign" | "Index" | "IndexMut" | "IntoIterator" | "Deref" | "DerefMut"
                        )
                    ))
                // (`<` and sort call partial_cmp / cmp from the comparison glue)
                || (s.has_self
                    && matches!(s.impl_trait.as_deref(), Some("PartialOrd" | "Ord"))
                    && (self.names[i as usize].ends_with("::cmp")
                        || self.names[i as usize].ends_with("::partial_cmp")))
                || self.names[i as usize] == "String::new"
        }));
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i as usize], true) {
                continue;
            }
            for &g in &refs[i as usize] {
                if !std::mem::replace(&mut gseen[g as usize], true) {
                    stack.extend(groups[g as usize].iter().copied());
                }
            }
        }
        seen
    }
}

/// Integer, bool, char and byte literals (`0x1F_u8` / `true` / `'a'` / `b'a'`)
pub fn lit_int(t: &str) -> Option<i128> {
    if t == "true" {
        return Some(1);
    }
    if t == "false" {
        return Some(0);
    }
    if t.starts_with('\'') || t.starts_with("b'") {
        let s = t.trim_start_matches('b');
        let inner = &s[1..s.len() - 1];
        return char_lit(inner).map(|c| c as i128);
    }
    if !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let s: String = t.chars().filter(|&c| c != '_').collect();
    let (radix, body) = if let Some(h) = s.strip_prefix("0x") {
        (16, h)
    } else if let Some(b) = s.strip_prefix("0b") {
        (2, b)
    } else if let Some(o) = s.strip_prefix("0o") {
        (8, o)
    } else {
        (10, s.as_str())
    };
    let digits: String = body.chars().take_while(|c| c.is_digit(radix)).collect();
    if digits.len() != body.len() && body[digits.len()..].starts_with(|c: char| c == '.' || c == 'e' || c == 'E') && radix == 10 {
        return None;
    }
    if digits.is_empty() {
        return None;
    }
    let mut v: u128 = 0;
    for c in digits.chars() {
        v = v.checked_mul(radix as u128)?.checked_add(c.to_digit(radix)? as u128)?;
    }
    Some(v as i128)
}

pub fn char_lit(s: &str) -> Option<char> {
    let mut cs = s.chars();
    let c = cs.next()?;
    if c != '\\' {
        return Some(c);
    }
    Some(match cs.next()? {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        '0' => '\0',
        '\\' => '\\',
        '\'' => '\'',
        '"' => '"',
        'x' => {
            let h: String = cs.take(2).collect();
            u8::from_str_radix(&h, 16).ok()? as char
        }
        'u' => {
            let h: String = cs.filter(|c| c.is_ascii_hexdigit()).collect();
            char::from_u32(u32::from_str_radix(&h, 16).ok()?)?
        }
        _ => return None,
    })
}

/// The impl of header sm overlaps an earlier impl of trait tr for sn: their self types and trait arguments unify
/// (`impl<T> Tr for (T, T)` and `impl Tr for (usize, isize)` do not; nor `impl Tr<u8> for S` and `impl Tr<u16> for S`)
fn impl_overlaps<'a>(seen: &mut FxMap<(&'a str, &'a str), Vec<usize>>, sums: &[Summary], user: usize, ii: usize, sn: &'a str, tr: &'a str, dup: bool) -> bool {
    let list = seen.entry((sn, tr)).or_default();
    // (only compared when the type already implements the trait: most impls are the first of their pair)
    let key = |sm: &Summary| {
        let mut v = vec![sm.impl_self_ty.clone().unwrap_or(TySyn::Named(sn.to_string()))];
        v.extend(sm.impl_trait_args.iter().cloned());
        let gs: Vec<String> = sm.impl_generics.iter().map(|g| g.0.clone()).collect();
        (TySyn::Tuple(v), gs)
    };
    let r = dup && {
        let (t, gs) = key(&sums[ii]);
        list.iter().any(|&k| {
            let (u, g) = key(&sums[k]);
            let mut sub = vec![];
            // (not when a parameter meets a type its bound cannot hold for: `impl<T: Into<J>, E> Tr for Result<T, E>`
            // beside `impl<E> Tr for Result<(), E>`, with no `impl From<()> for J`)
            syn_unify(&u, &g, &t, &gs, &mut sub)
                && !sub.iter().any(|((side, n), (s2, c))| {
                    let vars = if *s2 { &gs } else { &g };
                    let concrete = !matches!(c, TySyn::Named(m) if vars.iter().any(|x| x.strip_prefix('?').unwrap_or(x) == m));
                    let sm = if *side { &sums[ii] } else { &sums[k] };
                    concrete && sm.impl_generics.iter().find(|x| x.0 == *n).is_some_and(|x| x.1.iter().any(|b| bound_unmet_syn(b, c, sums, user)))
                })
        })
    };
    list.push(ii);
    r
}

/// The bound b cannot hold for the type c (one with no parameters of the impls'): b is `Into<X>` of a type X the
/// program defines with no `impl From<c> for X` (nor an `Into<X>` that could be c's), or a trait the program
/// declares with no impl that could be c's. Only for c `()`, a number, a tuple or a named type
fn bound_unmet_syn(b: &str, c: &TySyn, sums: &[Summary], user: usize) -> bool {
    // (a type alias is the type it names, `impl Tr for W<Alias>` with `type Alias = S;`: S's impls decide. A generic
    // one, or a chain that does not end, decides nothing)
    let mut c = c;
    for _ in 0..16 {
        let TySyn::Named(n) = c else { break };
        match sums.iter().flat_map(|s| s.aliases.iter()).find(|a| a.0 == *n) {
            Some((_, g, t)) if g.is_empty() => c = t,
            Some(_) => return false,
            None => break,
        }
    }
    if matches!(c, TySyn::Named(n) if sums.iter().flat_map(|s| s.aliases.iter()).any(|a| a.0 == *n)) {
        return false;
    }
    if !matches!(c, TySyn::Prim(_) | TySyn::Tuple(_) | TySyn::Named(_)) {
        return false;
    }
    let user = user.min(sums.len());
    let applies = |s: &Summary, st: &TySyn| {
        let gs: Vec<String> = s.impl_generics.iter().map(|g| g.0.clone()).collect();
        syn_unify(st, &gs, c, &[], &mut vec![])
    };
    // (the bound as written, `Into < JsValue >`)
    let b: String = b.chars().filter(|c| !c.is_whitespace()).collect();
    let b = b.as_str();
    if let Some(x) = b.strip_prefix("Into<").and_then(|r| r.strip_suffix('>')).map(str::trim) {
        let local = sums[..user].iter().any(|s| s.structs.iter().any(|d| d.0 == x) || s.enums.iter().any(|d| d.0 == x));
        if !local || matches!(c, TySyn::Named(n) if n == x) {
            return false;
        }
        return !sums.iter().any(|s| match s.impl_trait.as_deref() {
            Some("From") => s.impl_self.as_deref() == Some(x) && s.impl_trait_args.first().is_none_or(|a| applies(s, a)),
            Some("Into") => s.impl_self_ty.as_ref().is_none_or(|st| applies(s, st)),
            _ => false,
        });
    }
    if b.chars().all(|ch| ch.is_alphanumeric() || ch == '_') && sums[..user].iter().any(|s| s.trait_decl.as_deref() == Some(b)) {
        return !sums.iter().any(|s| s.impl_trait.as_deref() == Some(b) && s.impl_self_ty.as_ref().is_none_or(|st| applies(s, st)));
    }
    false
}

/// The self types of two impls can be the same type: a and b unify, with the names in ga / gb (the impls' parameters)
/// as variables (bound in `sub`, keyed by side)
fn syn_unify(a: &TySyn, ga: &[String], b: &TySyn, gb: &[String], sub: &mut Vec<((bool, String), (bool, TySyn))>) -> bool {
    fn go(a: &TySyn, sa: bool, b: &TySyn, sb: bool, g: [&[String]; 2], sub: &mut Vec<((bool, String), (bool, TySyn))>, d: u32) -> bool {
        if d > 32 {
            return true;
        }
        let var = |t: &TySyn, s: bool| match t {
            TySyn::Named(n) if g[s as usize].iter().any(|x| x.strip_prefix('?').unwrap_or(x) == n) => Some(n.clone()),
            _ => None,
        };
        if let Some(n) = var(a, sa) {
            let sized = !g[sa as usize].iter().any(|x| x.strip_prefix('?') == Some(n.as_str()));
            // (and std's unsized types, `impl<T> AsMut<T>` beside `impl AsMut<::std::path::Path>`: `Path`, `OsStr`,
            // `CStr`, by their last segment)
            let std_unsized = |t: &TySyn| {
                matches!(t, TySyn::Named(m) if matches!(m.rsplit("::").next().unwrap_or(m), "Path" | "OsStr" | "CStr"))
            };
            // (not the other impl's own parameter of the name, `impl<Path> Pick<Path> for S`)
            if sized && (matches!(b, TySyn::Slice(_) | TySyn::Str | TySyn::Dyn(_)) || (var(b, sb).is_none() && std_unsized(b))) {
                return false;
            }
            if let Some((_, (s2, t2))) = sub.iter().find(|x| x.0 == (sa, n.clone())).cloned() {
                return go(&t2, s2, b, sb, g, sub, d + 1);
            }
            if var(b, sb).as_ref() == Some(&n) && sa == sb {
                return true;
            }
            sub.push(((sa, n), (sb, b.clone())));
            return true;
        }
        if var(b, sb).is_some() {
            return go(b, sb, a, sa, g, sub, d + 1);
        }
        match (a, b) {
            (TySyn::Infer, _) | (_, TySyn::Infer) | (TySyn::Assoc(..), _) | (_, TySyn::Assoc(..)) => true,
            (TySyn::Ref(m, x), TySyn::Ref(n, y)) | (TySyn::Ptr(m, x), TySyn::Ptr(n, y)) => m == n && go(x, sa, y, sb, g, sub, d + 1),
            (TySyn::Slice(x), TySyn::Slice(y)) => go(x, sa, y, sb, g, sub, d + 1),
            (TySyn::Array(x, l), TySyn::Array(y, k)) => go(x, sa, y, sb, g, sub, d + 1) && (l == k || matches!((l, k), (ArrLen::Const(_), _) | (_, ArrLen::Const(_)))),
            (TySyn::App(n, xs), TySyn::App(m, ys)) => n == m && xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| go(x, sa, y, sb, g, sub, d + 1)),
            (TySyn::Tuple(xs), TySyn::Tuple(ys)) | (TySyn::FnPtr(xs), TySyn::FnPtr(ys)) => {
                xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| go(x, sa, y, sb, g, sub, d + 1))
            }
            (x, y) => x == y,
        }
    }
    go(a, false, b, true, [ga, gb], sub, 0)
}

/// The prefix of a type parameter's mark in a `dyn` type's text (`__zp{i}_{name}`), and of a struct's (`__zs{id}`):
/// names of the compiler's own, as the prelude's `__core_*` are
const PMARK: &str = "__zp";
const SMARK: &str = "__zs";

/// A type parameter's mark in a `dyn` type's text (Program::dyn_canon): `__zp{i}_{name}`, the parameter i written name
fn dyn_param_mark(n: &str) -> Option<(u8, &str)> {
    let (i, name) = n.strip_prefix(PMARK)?.split_once('_')?;
    Some((i.parse().ok()?, name))
}

/// A struct's mark in a `dyn` type's text (Program::dyn_src): `__zs{id}`, struct id
fn dyn_struct_mark(n: &str) -> Option<u32> {
    n.strip_prefix(SMARK)?.parse().ok()
}

/// The type parameters a `dyn` type's text marks
fn dyn_marks(s: &str) -> impl Iterator<Item = u8> + '_ {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter_map(|w| dyn_param_mark(w).map(|x| x.0))
}

/// t names a generic type (`Vec<u8>`) or an alias (one may stand for one)
/// t with each parameter of ps replaced by its argument (a type alias's value with the arguments it is given)
fn subst_tysyn(t: &TySyn, ps: &[String], args: &[TySyn]) -> TySyn {
    let r = |t: &TySyn| subst_tysyn(t, ps, args);
    match t {
        TySyn::Named(n) => ps.iter().position(|p| p == n).map_or_else(|| t.clone(), |k| args[k].clone()),
        TySyn::App(n, a) => TySyn::App(n.clone(), a.iter().map(r).collect()),
        TySyn::Ref(m, i) => TySyn::Ref(*m, Box::new(r(i))),
        TySyn::Ptr(m, i) => TySyn::Ptr(*m, Box::new(r(i))),
        TySyn::Slice(i) => TySyn::Slice(Box::new(r(i))),
        TySyn::Array(i, n) => TySyn::Array(Box::new(r(i)), n.clone()),
        TySyn::Tuple(v) => TySyn::Tuple(v.iter().map(r).collect()),
        TySyn::FnPtr(v) => TySyn::FnPtr(v.iter().map(r).collect()),
        TySyn::Assoc(b, n) => TySyn::Assoc(Box::new(r(b)), n.clone()),
        t => t.clone(),
    }
}

/// t names an associated type (`T::Item`, `<T as A>::B`) somewhere
fn syn_has_assoc(t: &TySyn) -> bool {
    match t {
        TySyn::Assoc(..) => true,
        TySyn::App(n, a) => n.contains("::") || a.iter().any(syn_has_assoc),
        TySyn::Ref(_, i) | TySyn::Ptr(_, i) | TySyn::Slice(i) | TySyn::Array(i, _) => syn_has_assoc(i),
        TySyn::Tuple(a) | TySyn::FnPtr(a) => a.iter().any(syn_has_assoc),
        _ => false,
    }
}

/// A method's own early-bound lifetimes: those its bounds name (`'a: 'b`, `T: Tr<'a>`), in their order. Returns
/// whether each of its own lifetimes is early-bound, and its bounds `'a: 'b` among the early-bound ones, to
/// `'static` (-1) and to the impl's or trait's lifetimes (-100 - k, k their name's place in `outer_names`), each own
/// lifetime by its place among the early-bound ones
fn early_bound(r: Option<&crate::types::RSigSyn>, g: &[(String, Vec<String>)], outer_names: &mut Vec<String>) -> (Vec<bool>, Vec<(i32, i32)>) {
    let Some(r) = r else { return (vec![], vec![]) };
    let in_bounds = |l: &str| g.iter().flat_map(|x| x.1.iter()).any(|b| b.match_indices(l).any(|(k, _)| !b[k + l.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')));
    // (also a lifetime a where clause bounds a type by, of the trait's parameters too: `where T: 'a`)
    let idx: Vec<usize> = (r.outer as usize..r.lts.len())
        .filter(|&i| in_bounds(&r.lts[i]) || r.outlives.iter().any(|&(a, b)| a as usize == i || b as usize == i) || r.ty_outlives.iter().any(|x| x.1 as usize == i))
        .collect();
    let mut ord = |c: u8| match idx.iter().position(|&k| k == c as usize) {
        Some(p) => Some(p as i32),
        None if c == crate::types::R_STATIC => Some(-1),
        None if c < r.outer => {
            let n = &r.lts[c as usize];
            let k = outer_names.iter().position(|x| x == n).unwrap_or_else(|| {
                outer_names.push(n.clone());
                outer_names.len() - 1
            });
            Some(-100 - k as i32)
        }
        None => None,
    };
    let rel = r.outlives.iter().filter_map(|&(a, b)| Some((ord(a)?, ord(b)?))).collect();
    ((r.outer as usize..r.lts.len()).map(|i| idx.contains(&i)).collect(), rel)
}

/// E0053 beyond the receiver: `unsafe` on one side only, or a parameter's or the result's type other than the
/// trait's, where both are concrete (no type parameter, `Self` or projection) and neither the trait nor the methods
/// have parameters of their own. The part rustc points at, for name_line: `f` the signature, `p{i}` the `i`th
/// parameter's type, `r` the result's
fn sig_incompatible(p: &Program, ds: &FnSig, ms: &FnSig, tr: &str) -> Option<String> {
    if ds.has_self != ms.has_self || ds.params.len() != ms.params.len() {
        return None;
    }
    if ds.is_unsafe != ms.is_unsafe {
        return Some("f".into());
    }
    let own = |g: &Vec<(String, Vec<String>)>| g.iter().all(|(n, _)| n == "Self");
    if !own(&ds.generics) || !own(&ms.generics) || tr.contains('<') {
        return None;
    }
    let concrete = |t: Ty| !p.has_param(t) && !p.tn(t).contains("Self");
    let differ = |a: Ty, b: Ty| a != b && concrete(a) && concrete(b);
    let k0 = usize::from(ds.has_self);
    if let Some(i) = (k0..ds.params.len()).find(|&i| differ(ds.params[i], ms.params[i])) {
        return Some(format!("p{i}"));
    }
    // (the result: not when the declaration's is `-> impl Trait` (kept as `()`, a placeholder) or the declaration is an
    // `async fn` (kept as its output): the impl may return any type of those bounds, a future for an `async fn`)
    (ds.ret != Ty::Unit && differ(ds.ret, ms.ret) && !p.implements(ms.ret, "Future")).then(|| "r".into())
}

/// An impl's method against the trait's declaration: E0195 when they have a different number of early-bound
/// lifetimes, or the same lifetimes with one early-bound where the other's is late-bound; E0276 when the impl's
/// bounds among them are not implied by the declaration's (rustc's order of the checks). The impl's and the trait's
/// own lifetimes are told apart by name: a bound with one of the impl's that the trait has no lifetime of that name for
/// is not compared (the header's trait arguments, which relate them, are not kept)
fn early_mismatch(decl: &FnSig, imp: &FnSig, m: &str) -> Option<String> {
    let mut names = vec![];
    let (de, drel) = early_bound(decl.regions.as_deref(), &decl.generics, &mut names);
    let n_decl = names.len();
    let (me, mrel) = early_bound(imp.regions.as_deref(), &imp.generics, &mut names);
    let count = |v: &[bool]| v.iter().filter(|&&x| x).count();
    if count(&de) != count(&me) || de.len() == me.len() && de != me {
        return Some(format!("lifetime parameters or bounds on method `{m}` do not match the trait declaration (E0195)"));
    }
    // (the trait's lifetimes: the declaration's own ones, and the impl's of the same names)
    let decl_outer = |x: i32| x > -100 || ((-100 - x) as usize) < n_decl || decl.regions.as_deref().is_some_and(|r| r.lts[..r.outer as usize].iter().any(|l| *l == names[(-100 - x) as usize]));
    // ('a: 'b holds when the declaration's bounds lead from 'a to 'b, or 'a is 'static)
    let implied = |a: i32, b: i32| {
        if a == b || a == -1 {
            return true;
        }
        let (mut seen, mut work) = (vec![a], vec![a]);
        while let Some(x) = work.pop() {
            for &(p, q) in &drel {
                if p == x && !seen.contains(&q) {
                    if q == b || q == -1 {
                        return true;
                    }
                    seen.push(q);
                    work.push(q);
                }
            }
        }
        false
    };
    // (a bound with none of the method's own lifetimes may be the impl header's (`impl<'a: 'b, 'b>`): not compared)
    if mrel.iter().any(|&(a, b)| (a >= 0 || b >= 0) && decl_outer(a) && decl_outer(b) && !implied(a, b)) {
        return Some(format!("impl has stricter requirements than trait: method `{m}` (E0276)"));
    }
    None
}

fn syn_has_app(t: &TySyn, aliases: &FxMap<String, (Vec<String>, TySyn)>) -> bool {
    match t {
        TySyn::App(..) | TySyn::Dyn(_) => true,
        TySyn::Named(n) => aliases.contains_key(n),
        TySyn::Ref(_, i) | TySyn::Ptr(_, i) | TySyn::Slice(i) | TySyn::Array(i, _) | TySyn::Assoc(i, _) => syn_has_app(i, aliases),
        TySyn::Tuple(v) | TySyn::FnPtr(v) => v.iter().any(|x| syn_has_app(x, aliases)),
        _ => false,
    }
}

/// Whether t holds type parameter g by value (not behind a reference, pointer or Box; assumed in other types' args)
fn syn_holds_by_value(t: &TySyn, g: &str) -> bool {
    match t {
        TySyn::Named(n) => n == g,
        TySyn::App(n, _) if n == "Box" => false,
        TySyn::App(_, args) => args.iter().any(|a| syn_holds_by_value(a, g)),
        TySyn::Tuple(es) => es.iter().any(|a| syn_holds_by_value(a, g)),
        TySyn::Array(e, _) => syn_holds_by_value(e, g),
        _ => false,
    }
}

pub use crate::types::TYS;

pub fn ty_code(t: Ty) -> u8 {
    TYS.iter().position(|&x| x == t).unwrap() as u8
}

/// A path segment (name, and whether it has `<..>` / `(..)` args): lookups convert syn and own-tree paths to it

/// std's traits that the lowering takes in a path call without a declaration of theirs (`Clone::clone(&x)`,
/// `Add::add(a, b)`, `Ord::cmp(&a, &b)`): the same names its two-segment `Trait::method(x, ..)` rule knows
pub const STD_PATH_TRAITS: &[&str] = &[
    "Clone", "ToString", "Add", "Sub", "Mul", "Div", "Rem", "Neg", "Not", "BitAnd", "BitOr", "BitXor", "Shl", "Shr", "PartialEq",
    "PartialOrd", "Ord", "Eq", "Iterator", "IntoIterator", "Into", "AsRef", "AsMut", "Hash", "Deref", "DerefMut", "Borrow",
    "ToOwned", "AddAssign", "SubAssign", "MulAssign", "DivAssign", "Index", "Fn", "FnMut", "FnOnce", "Debug", "Display",
];

#[derive(Clone, Debug, PartialEq)]
pub struct PSeg {
    pub name: String,
    pub args: bool,
}

/// A path of names (the leading `::` of `::a::b`, and the segments)
#[derive(Clone, Debug, PartialEq)]
pub struct PPath {
    pub global: bool,
    pub segs: Vec<PSeg>,
}

impl PPath {
    /// A path of our own syntax tree
    pub fn of(p: &crate::parse::Path) -> PPath {
        PPath {
            global: p.global,
            segs: p.segs.iter().map(|s| PSeg { name: s.name.clone(), args: !s.args.is_empty() || s.fn_args.is_some() }).collect(),
        }
    }
    fn tail(&self, k: usize) -> PPath {
        PPath { global: false, segs: self.segs[k..].to_vec() }
    }
    fn ident(&self) -> Option<&str> {
        if !self.global && self.segs.len() == 1 && !self.segs[0].args {
            Some(&self.segs[0].name)
        } else {
            None
        }
    }
}

// --- Looking up struct / enum variants / fns by path (front.rs's syn version calls these) ---
impl Program {
    /// The path without its leading module part (`crate::` / `self::` / `super::` / a lowercase module name)
    /// `std::fmt::Write::write_str` / `core::clone::Clone::clone`: a path through std's modules to a fn of a trait, which
    /// its last two segments `Trait::m` name as a two-segment path does (#153). The trait is one the program knows (the
    /// prelude's) or one of std's the lowering takes without a declaration (STD_PATH_TRAITS). strip_mods_p
    /// leaves std paths alone, so that `std::mem::swap` is not a `swap` of the program's: here the segment before the
    /// fn must be such a trait. The number of leading segments to leave out
    pub fn std_trait_path_skip(&self, p: &crate::parse::Path) -> Option<usize> {
        let n = p.segs.len();
        // (most paths stop here: one or two segments, or not std's)
        if n < 4 || !matches!(p.segs[0].name.as_str(), "std" | "core" | "alloc") {
            return None;
        }
        let plain = |s: &crate::parse::Seg| s.args.is_empty() && s.fn_args.is_none();
        // (`std` / `core` / `alloc`, then modules, then the trait: no generic arguments before the fn's own)
        if !plain(&p.segs[0]) || !p.segs[1..n - 2].iter().all(|s| plain(s) && s.name.starts_with(|c: char| c.is_ascii_lowercase())) {
            return None;
        }
        let tr = &p.segs[n - 2];
        // (`std::io::Error::from_raw_os_error(e)` / `std::fmt::Error`: std's error types, not the Error trait (#256))
        if tr.name == "Error" && matches!(p.segs[n - 3].name.as_str(), "io" | "fmt") {
            return None;
        }
        // (not a name the program declares a trait of: `Trait::m` would be the program's trait, where the std path names
        // std's (`std::string::ToString::to_string` beside a program's `trait ToString`). That call stays unread)
        let known = self.traits.contains_key(&tr.name) || STD_PATH_TRAITS.contains(&tr.name.as_str());
        (plain(tr) && known && !self.user_traits.contains(&tr.name)).then_some(n - 2)
    }

    pub fn strip_mods_p(&self, p: &PPath) -> Option<PPath> {
        let mut k = 0;
        let n = p.segs.len();
        while k + 1 < n {
            let s = p.segs[k].name.as_str();
            // (also one with leading underscores, `__rt::f` (wasm-bindgen's): not zustc's own heads, `__slice::m`)
            let internal = matches!(s, "__array" | "__slice" | "__fnptr" | "__ptr" | "__never") || s.starts_with("__dyn_") || s.starts_with("__exr");
            let module = matches!(s, "crate" | "self" | "super")
                || (s.trim_start_matches('_').starts_with(|c: char| c.is_ascii_lowercase())
                    && !internal
                    && !matches!(s, "std" | "core" | "alloc" | "str" | "char" | "bool")
                    && Ty::prim(s).is_none()
                    && !self.struct_index.contains_key(s)
                    && !self.aliases.contains_key(s)
                    && !p.segs[k].args);
            if !module {
                break;
            }
            k += 1;
        }
        if k == 0 {
            return None;
        }
        Some(p.tail(k))
    }

    pub fn variant_by_p(&self, p: &PPath, self_sid: Option<u32>) -> Option<(u32, usize)> {
        if let Some(r) = self.variant_by_p_inner(p, self_sid) {
            return Some(r);
        }
        let q = self.strip_mods_p(p)?;
        self.variant_by_p_inner(&q, self_sid)
    }

    fn variant_by_p_inner(&self, p: &PPath, self_sid: Option<u32>) -> Option<(u32, usize)> {
        // `core::option::Option::Some` / `alloc::borrow::Cow::Borrowed` and the like are matched by the last two
        let segs: &[PSeg] = if p.segs.len() > 2 && matches!(p.segs[0].name.as_str(), "core" | "crate" | "std" | "alloc") {
            &p.segs[p.segs.len() - 2..]
        } else {
            &p.segs
        };
        if segs.len() == 1 {
            let n = segs[0].name.as_str();
            let en = match n {
                "Some" | "None" => "Option",
                "Ok" | "Err" => "Result",
                // A variant brought in by `use Expr::*;` (it starts uppercase, and no other enum has the name)
                // (a struct of the same name wins: `TokenTree::Punct(Punct { .. })`)
                _ if n.starts_with(|c: char| c.is_ascii_uppercase()) => {
                    // (but not a struct of the prelude with named fields (its iterator `Skip { .. }`), which a program
                    // never writes as a value: `use Ord::*;`'s unit variant `Skip` is the variant)
                    let v = self.variant_names.get(n).copied().flatten();
                    let unit = v.is_some_and(|(sid, vi)| self.structs[sid as usize].variants[vi].fields.is_empty());
                    let prelude_named = |id: u32| {
                        let sd = &self.structs[id as usize];
                        !sd.fields.is_empty() && sd.fields[0].0 != "0" && self.struct_item.get(id as usize).is_some_and(|&ii| ii >= self.prelude_from)
                    };
                    if self.struct_index.get(n).is_some_and(|&id| !self.structs[id as usize].is_enum && !(unit && prelude_named(id))) {
                        return None;
                    }
                    return v;
                }
                _ => return None,
            };
            let sid = *self.struct_index.get(en)?;
            let vi = self.structs[sid as usize].variants.iter().position(|v| v.name == n)?;
            return Some((sid, vi));
        }
        if segs.len() != 2 {
            return None;
        }
        let tn = segs[0].name.as_str();
        // (`E2::A` of `type E2 = E;`; an alias with arguments, `Lt::<'static>::None`, is not looked through yet)
        let sid = if tn == "Self" { self_sid? } else { self.struct_index.get(tn).copied().or_else(|| self.alias_adt(tn).filter(|_| !segs[0].args))? };
        let sd = &self.structs[sid as usize];
        if !sd.is_enum {
            return None;
        }
        let vn = &segs[1].name;
        sd.variants.iter().position(|v| &v.name == vn).map(|vi| (sid, vi))
    }

    /// a const parameter's declared type name, through an alias (`type Switch = bool;` with `const B: Switch`: bool)
    pub fn const_decl<'a>(&self, n: &'a str) -> std::borrow::Cow<'a, str> {
        match self.aliases.get(n) {
            Some((_, TySyn::Prim(t))) if Ty::prim(n).is_none() => std::borrow::Cow::Owned(self.tn(*t)),
            _ => std::borrow::Cow::Borrowed(n),
        }
    }

    /// `type E2 = E;` / `type Opt<T> = Option<T>;`: the struct / enum (its base) an alias names directly
    pub fn alias_adt(&self, name: &str) -> Option<u32> {
        let (_, syn) = self.aliases.get(name)?;
        // (`type A = E<u8>;`: its base; a pattern through it is checked against the instance by variant_on)
        let n = match syn {
            TySyn::Named(n) | TySyn::App(n, _) => n.as_str(),
            _ => return None,
        };
        self.struct_index.get(n).copied()
    }

    pub fn struct_by_p(&self, p: &PPath, self_sid: Option<u32>) -> Option<u32> {
        // `std::cmp::Reverse` / `core::cmp::Ordering` and the like by the last segment (a prelude type)
        if p.segs.len() > 1 && matches!(p.segs[0].name.as_str(), "std" | "core" | "alloc") {
            return self.struct_index.get(&p.segs.last().unwrap().name).copied();
        }
        if p.segs.len() > 1 {
            return self.struct_by_p(&self.strip_mods_p(p)?, self_sid);
        }
        let id = p.ident()?;
        if id == "Self" {
            return self_sid;
        }
        self.struct_index.get(id).copied()
    }

    /// The methods a `dyn tr` vtable holds, in order: the trait's own, then those of its supertraits (each name once)
    pub fn dyn_methods(&self, tr: &str) -> Vec<u32> {
        let tr = crate::types::dyn_autos(tr).0;
        let mut out: Vec<u32> = self.traits.get(tr).cloned().unwrap_or_default();
        let mut todo: Vec<String> = self.trait_supers.get(tr).cloned().unwrap_or_default();
        let mut seen = vec![tr.to_string()];
        while let Some(s) = todo.pop() {
            if seen.contains(&s) {
                continue;
            }
            seen.push(s.clone());
            for &d in self.traits.get(&s).map(|v| v.as_slice()).unwrap_or(&[]) {
                let m = self.names[d as usize].rsplit("::").next();
                if !out.iter().any(|&o| self.names[o as usize].rsplit("::").next() == m) {
                    out.push(d);
                }
            }
            todo.extend(self.trait_supers.get(&s).cloned().unwrap_or_default());
        }
        out
    }

    pub fn fn_by_p(&self, p: &PPath, self_sid: Option<u32>) -> Option<u32> {
        if let Some(r) = self.fn_by_p_inner(p, self_sid) {
            return Some(r);
        }
        // (`str::from_utf8` with `use core::str;`: the module's fn, as `std::str::from_utf8`)
        if p.segs.len() == 2 && !p.global && matches!(p.segs[0].name.as_str(), "str" | "char") && !p.segs[0].args {
            let mut q = p.clone();
            q.segs.insert(0, q.segs[0].clone());
            q.segs[0].name = "core".into();
            if let Some(r) = self.fn_by_p_inner(&q, self_sid).or_else(|| self.strip_mods_p(&q).and_then(|q| self.fn_by_p_inner(&q, self_sid))) {
                return Some(r);
            }
        }
        let q = self.strip_mods_p(p)?;
        self.fn_by_p_inner(&q, self_sid)
    }

    fn fn_by_p_inner(&self, p: &PPath, self_sid: Option<u32>) -> Option<u32> {
        // The caller reads the last segment's turbofish (`f::<T>`)
        let n = p.segs.len();
        // (`Buf::<8>::new`: the caller reads a type segment's type arguments too)
        if n == 2 && p.segs[0].args && !p.global {
            let mut q = p.clone();
            q.segs[0].args = false;
            return self.fn_by_p_inner(&q, self_sid);
        }
        // (`::std::mem::swap` is `std::mem::swap`)
        if p.global && p.segs.first().is_some_and(|s| matches!(s.name.as_str(), "std" | "core" | "alloc")) {
            let mut q = p.clone();
            q.global = false;
            return self.fn_by_p_inner(&q, self_sid);
        }
        if p.global || p.segs.iter().take(n.saturating_sub(1)).any(|s| s.args) {
            return None;
        }
        let name = |k: usize| p.segs[k].name.as_str();
        // (`io::Error::from_raw_os_error(e)` / `std::io::Error::last_os_error()`: the prelude's IoError's, as the type
        // `io::Error` is IoError)
        // (std's io only: `std::io::` / `core::io::`, or `io::` where the program has no `io::Error` of its own (a
        // `mod io { pub struct Error; .. }` flattens to `io__Error`), which the ordinary lookup finds)
        let std_io = match n {
            3 => !self.struct_index.contains_key("io__Error"),
            4 => matches!(name(0), "std" | "core"),
            _ => false,
        };
        if std_io && name(n - 3) == "io" && name(n - 2) == "Error" {
            if let Some(&i) = self.index.get(&format!("IoError::{}", name(n - 1))) {
                return Some(i);
            }
        }
        // `std::collections::HashMap::new` and the like are `HashMap::new`
        if n >= 3 && matches!(name(0), "core" | "std" | "alloc") {
            let ty = name(n - 2);
            // (`std::cell::Ref::map`: below, as `Ref::map`; `std::vec::Vec::from_raw_parts` as well, when the prelude's
            // helper stands for it)
            let helper = matches!((ty, name(n - 1)), ("Vec", "from_raw_parts") | ("String", "from_utf8_unchecked"));
            if self.struct_index.contains_key(ty) && !matches!(ty, "Ref" | "RefMut") && !(helper && !self.index.contains_key(&format!("{ty}::{}", name(n - 1)))) {
                return self.index.get(&format!("{ty}::{}", name(n - 1))).copied();
            }
        }
        // `std::process::exit` / `process::exit` (under another name, so it does not clash with the user's `exit`)
        // `fs::read_to_string` / `fs::write` / `env::args` and the like are the prelude's `__core_*`
        if n >= 2 {
            let core = match (name(n - 2), name(n - 1)) {
                ("fs", "read_to_string") => Some("__exr_fs_read_to_string"),
                ("fs", "read") => Some("__exr_fs_read"),
                ("fs", "write") => Some("__exr_fs_write"),
                ("env", "args") => Some("__exr_env_args"),
                ("env", "var") => Some("__exr_env_var"),
                ("env", "var_os") => Some("__exr_env_var_os"),
                ("env", "split_paths") => Some("__exr_env_split_paths"),
                ("fs", "remove_file") => Some("__exr_fs_remove_file"),
                ("Path", "new") => Some("__exr_path_new"),
                // (not a program's own `ptr` type's or module's)
                ("ptr", "eq") if !self.index.contains_key("ptr::eq") => Some("__exr_ptr_eq"),
                ("ptr", "swap") if !self.index.contains_key("ptr::swap") => Some("__core_ptr_swap"),
                ("array", "from_fn") => Some("__exr_array_from_fn"),
                // (RefCell's guards: associated functions of the prelude's Ref / RefMut, unless the program has its own)
                ("Ref", "map") if !self.index.contains_key("Ref::map") => Some("__exr_ref_map"),
                ("RefMut", "map") if !self.index.contains_key("RefMut::map") => Some("__exr_refmut_map"),
                ("Ref", "clone") if !self.index.contains_key("Ref::clone") => Some("__exr_ref_clone"),
                ("Vec", "from_raw_parts") if !self.index.contains_key("Vec::from_raw_parts") => Some("__exr_vec_from_raw_parts"),
                ("String", "from_utf8_unchecked") if !self.index.contains_key("String::from_utf8_unchecked") => Some("__exr_string_from_utf8_unchecked"),
                _ => None,
            };
            if let Some(c) = core {
                return self.index.get(c).copied();
            }
        }
        if n >= 2 && name(n - 2) == "process" && name(n - 1) == "exit" {
            return self.index.get("__core_process_exit").copied();
        }
        // (`alloc::alloc(layout)` after `use std::alloc;`, and the crate's `alloc::alloc::alloc(layout)`: std::alloc's
        // functions, the prelude's `__core_*`. `alloc` is kept as a path's head above, as it names the crate too; not
        // a program's own `alloc::f`)
        if n >= 2
            && name(0) == "alloc"
            && (n == 2 || (n == 3 && name(1) == "alloc"))
            && matches!(name(n - 1), "alloc" | "alloc_zeroed" | "dealloc" | "realloc" | "handle_alloc_error")
            && !self.index.contains_key(&format!("alloc::{}", name(n - 1)))
        {
            return self.index.get(&format!("__core_{}", name(n - 1))).copied();
        }
        // `core::cmp::max` / `core::mem::swap` and the like go to the prelude's `__core_*`
        if n >= 2 && matches!(name(0), "core" | "std") {
            return self.index.get(&format!("__core_{}", name(n - 1))).copied();
        }
        match n {
            1 => {
                let f = name(0);
                // `max(..)` after `use core::cmp::max;`: core's unless the user has the fn
                self.index.get(f).or_else(|| self.index.get(&format!("__core_{f}"))).copied()
            }
            2 => {
                let (ty, m) = (name(0), name(1));
                // `f64::max` / `f64::sqrt` (as fns): the prelude's `__exr_f64_*`; `i32::max` is core::cmp::max
                // (or a trait's method of the program's, `impl From<J> for f64`: `f64::from`)
                if ty == "f64" || ty == "f32" {
                    return self.index.get(&format!("__exr_{ty}_{m}")).or_else(|| self.index.get(&format!("{ty}::{m}"))).copied();
                }
                if Ty::from_suffix(ty).is_some() && matches!(m, "max" | "min") {
                    return self.index.get(&format!("__core_{m}")).copied();
                }
                // `char::from_digit(d, 10)` / `char::from_u32(x)` / `char::from(b)`: the prelude's `__exr_char_*`
                if ty == "char" {
                    let m = match m {
                        "from_u32" => "from_u32_checked",
                        "from" => "from_u8",
                        _ => m,
                    };
                    return self
                        .index
                        .get(&format!("__exr_char_{m}"))
                        .or_else(|| self.index.get(&format!("__exr_charfn_{m}")))
                        .copied();
                }
                let ty = if ty == "Self" { self.structs[self.base_of(self_sid?) as usize].name.clone() } else { ty.to_string() };
                if let Some(&id) = self.index.get(&format!("{ty}::{m}")) {
                    return Some(id);
                }
                // (`type Ham = foo::Qux;` then `Ham::new()`: the function of the type the alias names)
                match self.aliases.get(&ty).map(|a| &a.1) {
                    Some(TySyn::Named(t) | TySyn::App(t, _)) if *t != ty => self.index.get(&format!("{t}::{m}")).copied(),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// When name is a const generic argument (`[(); n]`) in the type environment, its value
pub fn const_generic_value(env: &TyEnv, name: &str) -> Option<u32> {
    env.params.iter().find(|(n, _)| n == name).and_then(|(_, t)| match t {
        Ty::Array(Ty::Unit, n) if *n < CONST_LEN_BASE => Some(*n),
        _ => None,
    })
}

/// e without the parentheses around it
/// e is a bool by its form: `true` / `false`, a comparison, `&&` / `||`, or `!` of one
fn is_bool_expr(e: &crate::parse::Expr) -> bool {
    use crate::parse::Expr;
    match strip_parens(e) {
        Expr::Path(None, p) => p.segs.len() == 1 && matches!(p.segs[0].name.as_str(), "true" | "false"),
        Expr::Lit(t, _) => t == "true" || t == "false",
        Expr::Binary(op, ..) => matches!(op.as_str(), "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||"),
        Expr::Unary(op, x, _) => op == "!" && is_bool_expr(x),
        _ => false,
    }
}

fn strip_parens(e: &crate::parse::Expr) -> &crate::parse::Expr {
    match e {
        crate::parse::Expr::Paren(x) => strip_parens(x),
        e => e,
    }
}

/// A value as a literal: with its type's suffix when it has one (`255u8`), so that it keeps the type
fn typed_lit(v: i128, t: Option<It>, pos: crate::parse::Pos) -> crate::parse::Expr {
    use crate::parse::Expr;
    let sfx = t.map(|it| it.name()).unwrap_or("");
    // (a u128 past i128::MAX is held as a negative bit pattern: its literal is the unsigned value)
    if t.is_some_and(|it| it.bits == 128 && !it.signed) {
        return Expr::Lit(format!("{}{sfx}", v as u128), pos);
    }
    // (i128::MIN has no positive literal to negate)
    if v == i128::MIN {
        let seg = |n: &str| crate::parse::Seg { name: n.to_string(), args: vec![], fn_args: None, text: n.to_string() };
        return Expr::Path(None, crate::parse::Path { global: false, segs: vec![seg("i128"), seg("MIN")], pos });
    }
    if v < 0 {
        Expr::Paren(Box::new(Expr::Unary("-".into(), Box::new(Expr::Lit(format!("{}{sfx}", -v), pos)), Default::default())))
    } else {
        Expr::Lit(format!("{v}{sfx}"), pos)
    }
}

/// e with the names bound in env (the latest binding of each) replaced by their values (typed literals)
fn subst_consts(e: &crate::parse::Expr, env: &[(String, i128, Option<It>, Option<crate::parse::Expr>)]) -> crate::parse::Expr {
    use crate::parse::Expr;
    let r = |x: &Expr| Box::new(subst_consts(x, env));
    match e {
        Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => {
            match env.iter().rev().find(|(n, ..)| *n == p.segs[0].name) {
                // (an untyped `let`: its expression, which takes the type of where it is used)
                Some((_, _, None, Some(x))) => Expr::Paren(Box::new(x.clone())),
                Some((_, v, t, _)) => typed_lit(*v, *t, p.pos),
                None => e.clone(),
            }
        }
        // (`Foo::X`: an earlier variant's discriminant, for an enum's discriminant)
        Expr::Path(None, p) if p.segs.len() == 2 && p.segs.iter().all(|s| s.args.is_empty()) && !env.is_empty() => {
            let n = format!("{}::{}", p.segs[0].name, p.segs[1].name);
            match env.iter().rev().find(|(x, ..)| *x == n) {
                Some((_, v, t, _)) => typed_lit(*v, *t, p.pos),
                None => e.clone(),
            }
        }
        Expr::Paren(x) => Expr::Paren(r(x)),
        Expr::Unary(op, x, s) => Expr::Unary(op.clone(), r(x), *s),
        Expr::Binary(op, a, b, s) => Expr::Binary(op.clone(), r(a), r(b), *s),
        Expr::Cast(x, t) => Expr::Cast(r(x), t.clone()),
        Expr::Call(f, args) => Expr::Call(f.clone(), args.iter().map(|a| subst_consts(a, env)).collect()),
        Expr::MethodCall(x, m, g, args, s) => Expr::MethodCall(r(x), m.clone(), g.clone(), args.iter().map(|a| subst_consts(a, env)).collect(), *s),
        Expr::Index(a, b, s) => Expr::Index(r(a), r(b), *s),
        Expr::If(c, t, els) => {
            let mut t = t.clone();
            for st in &mut t.stmts {
                if let crate::parse::Stmt::Expr(x, semi) = st {
                    *st = crate::parse::Stmt::Expr(subst_consts(x, env), *semi);
                }
            }
            Expr::If(r(c), t, els.as_ref().map(|x| r(x)))
        }
        // (a nested block's own `let x` shadows an outer x from there on: `{ let x = 1u8; { let x = 255u8; x } }`)
        Expr::Block(l, u, b) => {
            let mut b = b.clone();
            let mut env: Vec<(String, i128, Option<It>, Option<Expr>)> = env.to_vec();
            for st in &mut b.stmts {
                match st {
                    crate::parse::Stmt::Expr(x, semi) => *st = crate::parse::Stmt::Expr(subst_consts(x, &env), *semi),
                    crate::parse::Stmt::Let(pat, ty, init, els, pos) => {
                        let init = init.as_ref().map(|x| subst_consts(x, &env));
                        if let crate::parse::Pat::Ident(_, _, n, _) = &*pat {
                            env.retain(|(x, ..)| x != n);
                        }
                        *st = crate::parse::Stmt::Let(pat.clone(), ty.clone(), init, els.clone(), *pos);
                    }
                    _ => {}
                }
            }
            Expr::Block(l.clone(), *u, b)
        }
        e => e.clone(),
    }
}

/// std has `impl From<a> for b` (b losslessly holds every a; `T: From<T>`)
pub fn prim_from(a: Ty, b: Ty) -> bool {
    use Ty::*;
    if a == b {
        return true;
    }
    match a {
        Bool => b.is_int(),
        Char => matches!(b, U32 | U64 | U128),
        U8 => matches!(b, U16 | U32 | U64 | U128 | Usize | I16 | I32 | I64 | I128 | Isize | F32 | F64 | Char),
        U16 => matches!(b, U32 | U64 | U128 | Usize | I32 | I64 | I128 | F32 | F64),
        U32 => matches!(b, U64 | U128 | I64 | I128 | F64),
        U64 => matches!(b, U128 | I128),
        I8 => matches!(b, I16 | I32 | I64 | I128 | Isize | F32 | F64),
        I16 => matches!(b, I32 | I64 | I128 | Isize | F32 | F64),
        I32 => matches!(b, I64 | I128 | F64),
        I64 => matches!(b, I128),
        F32 => matches!(b, F64),
        _ => false,
    }
}

/// the structs / enums a type alias's value names (by the program's index)
fn alias_structs(t: &TySyn, index: &FxMap<String, u32>, out: &mut Vec<u32>) {
    match t {
        TySyn::Named(n) => out.extend(index.get(n).copied()),
        TySyn::App(n, args) => {
            out.extend(index.get(n).copied());
            args.iter().for_each(|a| alias_structs(a, index, out));
        }
        TySyn::Ref(_, i) | TySyn::Ptr(_, i) | TySyn::Slice(i) | TySyn::Array(i, _) | TySyn::Assoc(i, _) => alias_structs(i, index, out),
        TySyn::Tuple(ts) | TySyn::FnPtr(ts) => ts.iter().for_each(|a| alias_structs(a, index, out)),
        _ => {}
    }
}

/// A trait object's first trait as written, without spaces or a path (`Obj<u8>` of `dyn m::Obj< u8 >`). Its auto
/// traits are not in it: a type of zustc's has none (`dyn Obj + Send` is `dyn Obj`, ast_types::tysyn)
fn dyn_first(d: &str) -> String {
    let first: String = d.split('+').next().unwrap_or(d).chars().filter(|c| !c.is_whitespace()).collect();
    let cut = first.find(|c: char| c == '<' || c == '(').unwrap_or(first.len());
    match first[..cut].rfind("::") {
        Some(i) => first[i + 2..].to_string(),
        None => first,
    }
}

impl Program {
    /// Whether the program's impl of trait t for the object `dyn tr` (its text) writes the object's lifetime (`impl X
    /// for dyn Obj<u8> + '_`): by the object as written for an impl with no parameters, by its trait's name for a
    /// generic one. Else it is for `dyn Obj + 'static`
    pub fn dyn_impl_writes_lt(&self, t: &str, tr: &str) -> bool {
        let first = dyn_first(tr);
        let name = first.split(|c: char| c == '(' || c == '<').next().unwrap_or(&first).trim().to_string();
        let key = |o: String| (t.to_string(), o);
        (self.dyn_impls_exact.contains(&key(first.clone())) && self.dyn_impls_lt.contains(&key(first))) || (self.dyn_impls.contains(&key(name.clone())) && self.dyn_impls_lt.contains(&key(name)))
    }

    /// The traits the program implements for the trait object `dyn tr` (its text) by an impl of its own (`impl
    /// MyTrait for dyn Tr {}`, generic or not): the candidates for a default method called through the object
    pub fn dyn_impl_traits(&self, tr: &str) -> Vec<&String> {
        if self.dyn_impls_exact.is_empty() && self.dyn_impls.is_empty() {
            return vec![];
        }
        let first = dyn_first(tr);
        let name = first.split(|c: char| c == '(' || c == '<').next().unwrap_or(&first).trim().to_string();
        self.dyn_impls_exact.iter().filter(|(_, o)| *o == first).chain(self.dyn_impls.iter().filter(|(_, o)| *o == name)).map(|(t, _)| t).collect()
    }
}

/// A bound that asks nothing of the type: `?Sized`, a lifetime (`'a`)
fn relaxed_bound(b: &str) -> bool {
    b.starts_with('?') || b.starts_with('\'')
}

/// E0185 / E0186: `fn foo(&self)` in the impl for the trait's `fn foo()`, or the other way round. s: the signature
/// with the receiver
#[cold]
fn receiver_mismatch(line: &str, m: &str, s: &FnSig, in_impl: bool) -> String {
    let recv = match s.params.first() {
        Some(Ty::Ref(false, _)) => "&self",
        Some(Ty::Ref(true, _)) => "&mut self",
        _ => "self",
    };
    if in_impl {
        format!("{line}method `{m}` has a `{recv}` declaration in the impl, but not in the trait (E0185)")
    } else {
        format!("{line}method `{m}` has a `{recv}` declaration in the trait, but not in the impl (E0186)")
    }
}
