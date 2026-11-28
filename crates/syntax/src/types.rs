//! The representation of types (the part split out of front.rs that does not depend on syn): `Ty` (resolved types), `TySyn` (syntactic types) and interning.
//! Written so that zustc itself can compile it (a self-hosting target).

// (the interning tables: Fx hashing, deterministic and cheaper than the std maps' SipHash)
type HashSet<K> = crate::fxhash::FxSet<K>;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A resolved type. To make it Copy, what it refers to is held as an interned `&'static Ty`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Ty {
    I8,
    I16,
    I32,
    I64,
    Isize,
    U8,
    U16,
    U32,
    U64,
    Usize,
    Bool,
    Unit,
    Never,
    /// 128-bit integers
    I128,
    U128,
    /// A Unicode scalar value (32 bit)
    Char,
    /// Floating point (IEEE 754)
    F32,
    F64,
    /// A SIMD vector of this many bits: one value, whose lanes are views given by the operations (issue #30).
    /// `Vec(128)` is `core::arch::wasm32::v128`; 256 and 512 are for wider targets (AVX's `__m256` / `__m512`), which
    /// no backend emits yet
    Vec(u16),
    /// `&T` (false) / `&mut T` (true)
    Ref(bool, &'static Ty),
    /// An index into Program::structs
    Struct(u32),
    /// A type parameter of a generic fn (an index into FnSig::generics). Replaced by a concrete type on instantiation
    Param(u8),
    /// `[T; N]` (an aggregate type handled as a value)
    Array(&'static Ty, u32),
    /// `[T]` (unsized; appears only as the pointee of `&[T]`)
    Slice(&'static Ty),
    /// `str` (unsized; appears only as the pointee of `&str`)
    Str,
    /// `*const T` (false) / `*mut T` (true)
    Ptr(bool, &'static Ty),
    /// `dyn Trait` (unsized; appears only as the pointee of `&dyn Trait` = (data, vtable)).
    /// Its content is the string of the trait bound (`Shape` / `Fn(i32) -> i32`)
    Dyn(&'static String),
    /// A fn pointer `fn(A) -> R` (an fn item used as a value is this too)
    Fn(&'static FnTy),
    /// `Box<T>`: an owning pointer to the heap (`Box<dyn Trait>` is fat). On drop, drops the contents and frees
    Box(&'static Ty),
    /// An associated type `T::Item` (appears only when T is a type parameter; once it becomes a concrete type, it is replaced by the impl's definition)
    Assoc(&'static AssocTy),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssocTy {
    pub base: Ty,
    pub name: String,
}

pub fn intern_assoc(a: AssocTy) -> &'static AssocTy {
    static TABLE: OnceLock<Mutex<HashSet<&'static AssocTy>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::default())).lock().unwrap();
    if let Some(&r) = tab.get(&a) {
        return r;
    }
    let r: &'static AssocTy = Box::leak(Box::new(a));
    tab.insert(r);
    r
}

impl Ty {
    /// A type that is placed in memory and handled by its address as a value (struct / enum / array / the fat pointers `&[T]` and `&str`)
    pub fn is_agg(self) -> bool {
        matches!(self, Ty::Struct(_) | Ty::Array(..)) || self.is_fat()
    }
    /// A reference represented by the 2 words (pointer, length)
    /// (raw pointers to them too: `*const [T]` / `*const dyn Tr` are the same two words)
    #[inline]
    pub fn is_fat(self) -> bool {
        match self {
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Box(i) => i.is_unsized(),
            _ => false,
        }
    }
    /// A type without a size known statically: `[T]`, `str`, `dyn Tr`, a struct with such a tail (what a pointer to
    /// is a pair)
    #[inline]
    pub fn is_unsized(self) -> bool {
        matches!(self, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) || self.is_dst_struct()
    }
    /// An instance of a struct whose last field is unsized (`Foo<[u8]>`): its number carries `DST_TAG`
    #[inline]
    pub fn is_dst_struct(self) -> bool {
        matches!(self, Ty::Struct(s) if s & DST_TAG != 0)
    }
}

/// The mark put on the number of an instance of a struct whose last field is unsized (`&Foo<[u8]>` is a fat pointer too)
pub const DST_TAG: u32 = 1 << 31;

/// The intern table of strings (the trait bounds of `dyn`)
/// (holds a `&String` rather than a fat `&str`, to keep `Ty` at 16 bytes)
/// A trait object's text without the auto traits it keeps (`Fn() + Send + Sync` is `Fn()`), and whether it is Send,
/// and Sync (ast_types::tysyn writes them last, in that order)
pub fn dyn_autos(s: &str) -> (&str, bool, bool) {
    let (s, sync) = match s.strip_suffix(" + Sync") {
        Some(r) => (r, true),
        None => (s, false),
    };
    let (s, send) = match s.strip_suffix(" + Send") {
        Some(r) => (r, true),
        None => (s, false),
    };
    (s, send, sync)
}

pub fn intern_str(s: &str) -> &'static String {
    static TABLE: OnceLock<Mutex<HashSet<&'static String>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::default())).lock().unwrap();
    if let Some(&r) = tab.get(&s.to_string()) {
        return r;
    }
    let r: &'static String = Box::leak(Box::new(s.to_string()));
    tab.insert(r);
    r
}

/// The contents of a fn pointer type `fn(A, B) -> R`
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnTy {
    pub params: Vec<Ty>,
    pub ret: Ty,
}

pub fn intern_fn(f: FnTy) -> &'static FnTy {
    static TABLE: OnceLock<Mutex<HashSet<&'static FnTy>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::default())).lock().unwrap();
    if let Some(&r) = tab.get(&f) {
        return r;
    }
    let r: &'static FnTy = Box::leak(Box::new(f));
    tab.insert(r);
    r
}

/// Turns a trait bound into a string (only the last segment of the path: `core::ops::Fn(u8) -> u8` → `Fn (u8) -> u8`)
pub fn intern(t: Ty) -> &'static Ty {
    static TABLE: OnceLock<Mutex<HashSet<&'static Ty>>> = OnceLock::new();
    let mut tab = TABLE.get_or_init(|| Mutex::new(HashSet::default())).lock().unwrap();
    if let Some(&r) = tab.get(&t) {
        return r;
    }
    let r: &'static Ty = Box::leak(Box::new(t));
    tab.insert(r);
    r
}

impl Ty {
    /// `v128`
    pub const V128: Ty = Ty::Vec(128);

    pub fn from_suffix(s: &str) -> Option<Ty> {
        Some(match s {
            "i8" => Ty::I8,
            "i16" => Ty::I16,
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "isize" => Ty::Isize,
            "u8" => Ty::U8,
            "u16" => Ty::U16,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "usize" => Ty::Usize,
            "i128" => Ty::I128,
            "u128" => Ty::U128,
            _ => return None,
        })
    }

    pub fn prim(s: &str) -> Option<Ty> {
        Ty::from_suffix(s).or(match s {
            "bool" => Some(Ty::Bool),
            "char" => Some(Ty::Char),
            "f32" => Some(Ty::F32),
            "f64" => Some(Ty::F64),
            _ => None,
        })
    }

    pub fn is_float(self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }

    pub fn is_int(self) -> bool {
        matches!(
            self,
            Ty::I8
                | Ty::I16
                | Ty::I32
                | Ty::I64
                | Ty::Isize
                | Ty::U8
                | Ty::U16
                | Ty::U32
                | Ty::U64
                | Ty::Usize
                | Ty::I128
                | Ty::U128
        )
    }

    pub fn signed(self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize | Ty::I128)
    }

    /// The bit width of a scalar (integer / bool / reference). 0 for struct / unit / never.
    pub fn bits(self) -> u32 {
        match self {
            Ty::I8 | Ty::U8 | Ty::Bool => 8,
            Ty::I16 | Ty::U16 => 16,
            Ty::I32 | Ty::U32 | Ty::Char | Ty::F32 => 32,
            Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::Ptr(..) | Ty::Fn(_) | Ty::F64 => 64,
            Ty::I128 | Ty::U128 => 128,
            Ty::Vec(b) => b as u32,
            Ty::Ref(..) | Ty::Box(_) if self.is_fat() => 0,
            Ty::Ref(..) | Ty::Box(_) => 64,
            Ty::Unit
            | Ty::Never
            | Ty::Struct(_)
            | Ty::Param(_)
            | Ty::Array(..)
            | Ty::Slice(_)
            | Ty::Str
            | Ty::Dyn(_)
            | Ty::Assoc(_) => 0,
        }
    }
}

/// A type whose names are not yet resolved (the form saved in summaries). Structs are held by name, so
/// the cache does not break even when the order of the items changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TySyn {
    Prim(Ty),
    Ref(bool, Box<TySyn>),
    Named(String),
    /// An application of a generic struct / enum, such as `Option<T>`
    App(String, Vec<TySyn>),
    /// `(A, B)`
    Tuple(Vec<TySyn>),
    /// `[T; N]` (N is a literal or the name of a const)
    Array(Box<TySyn>, ArrLen),
    Slice(Box<TySyn>),
    Str,
    Ptr(bool, Box<TySyn>),
    /// `dyn Trait` (the string of the bound)
    Dyn(String),
    /// `fn(A, B) -> R` (the last is the return value)
    FnPtr(Vec<TySyn>),
    /// An associated type `T::Item` / `Self::Item`
    Assoc(Box<TySyn>, String),
    /// `_`, left to type inference (`collect::<Vec<_>>()`)
    Infer,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArrLen {
    Lit(u64),
    Const(String),
}


// ---- Item summaries (the part split out of front.rs that does not depend on syn) ----

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct SigSyn {
    pub params: Vec<TySyn>,
    pub ret: TySyn,
    pub symbol: String,
    pub is_extern: bool,
    pub is_main: bool,
    /// For a fn inside an impl, the name of the self type
    pub impl_of: Option<String>,
    /// The first parameter is self (callable with method call syntax)
    pub has_self: bool,
    /// The type parameters and the trait bounds of each. A fn inside a trait has `Self` first
    pub generics: Vec<(String, Vec<String>)>,
    /// For a fn declared inside a trait, the trait's name
    pub trait_of: Option<String>,
    /// Whether it has a body (false for a fn that is only a declaration in a trait)
    pub has_body: bool,
    /// For a fn inside `impl Trait for S`, the trait's name
    pub impl_trait: Option<String>,
    /// For a fn inside an impl, the type of `Self` (`Option<T>` for `impl<T> Option<T>`)
    pub self_ty: Option<TySyn>,
    /// `#[no_mangle]` / `#[unsafe(no_mangle)]`: exposed externally under its name as is
    pub export: bool,
    /// `#[panic_handler]`
    pub panic_handler: bool,
    /// `unsafe fn`, or a fn of an `extern` block (calling it needs `unsafe`, E0133)
    pub is_unsafe: bool,
    /// Which reference arguments the return value may borrow from (borrow checking): bit i for argument i (self
    /// first), bit 63 set when known. 0: not known (any of them). With bit 62 instead (OPAQUE_FROM), a result
    /// `impl Trait`: bits 0..31 the arguments it may borrow from by edition 2024's rules (every lifetime of the
    /// arguments, or those of its `use<..>`), bits 31..62 by 2021's (the lifetimes its bounds name)
    pub ret_from: u64,
    /// The regions of the signature (None: it writes no reference and no lifetime, so every region in it is elided)
    pub regions: Option<Box<RSigSyn>>,
    /// The fn's own where clauses on other than a parameter (`where T::A: Bar`, `where <T as Tr>::A: Bar`, `where
    /// W<T>: Bar`): the type, the trait of a qualified path (`Tr`; empty otherwise), the bounds. The trait solver
    /// proves them as goals once the parameters are known (issue #2)
    pub preds: Vec<(TySyn, String, Vec<String>)>,
    /// The fn's own type parameters (not its impl's or trait's) declared `?Sized`; its other own ones must be sized at
    /// a call (E0277, Program::fn_sized_gens)
    pub own_unsized: Vec<String>,
}

// ---- regions of signatures (region checking; `Ty` itself carries none) ----
//
// The regions of a type are its *region slots*, in preorder: a reference's region, then those of its pointee; a
// struct / enum's lifetime parameters, then the slots of its type arguments; a tuple's elements in order; the element
// of a box, array, slice or raw pointer. A slot is written as a code: an index into the lifetimes in scope, or one of
// the markers below.

/// `'static`
pub const R_STATIC: u8 = 0xF0;
/// An elided region (`&T`, `'_`)
pub const R_ELIDED: u8 = 0xF1;
/// A path type written without lifetime arguments (`Parser` for `Parser<'a>`, `Vec<T>`): as many elided regions as the
/// type has lifetime parameters (often none)
pub const R_ELIDED_ADT: u8 = 0xF2;
/// A type the checker does not follow (a projection, `dyn`, `impl`, a fn pointer, a macro): the whole parameter is
/// not checked
pub const R_UNKNOWN: u8 = 0xF3;
/// A trait object's lifetime not written (`&'a dyn Tr`, `Box<dyn Tr>`): the default (the reference's region directly
/// around it, else 'static in a signature; inferred in a body)
pub const R_DYN_DEFAULT: u8 = 0xF4;
/// A region a fn pointer's type binds (written `for<'r>`, or elided in it): a variable of its own
pub const R_BOUND: u8 = 0xF5;
/// The codes of a fn pointer's type written as one follow (its parameters', then its result's); without it (a type
/// alias naming one) a fn pointer's regions are its own
pub const R_FN: u8 = 0xF6;
/// After a trait object's lifetime: the lifetimes in scope its trait's arguments name follow (their number, then
/// their codes). The object has no slots for them; they make a definition holding it invariant in them
pub const R_DYN_ARGS: u8 = 0xF7;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct RSigSyn {
    /// The lifetime parameters in scope (the impl's or trait's, then the fn's)
    pub lts: Vec<String>,
    /// The region slots of each parameter (self first), then of the return type
    pub slots: Vec<Vec<u8>>,
    /// Declared `'a: 'b` (codes)
    pub outlives: Vec<(u8, u8)>,
    /// Declared `T: 'a` (type parameter, code)
    pub ty_outlives: Vec<(String, u8)>,
    /// How many of lts are the impl's or trait's (the rest are the fn's own)
    pub outer: u8,
    /// An impl's method: the region slots of the impl's self type (it is well formed: implied in the body, required
    /// of a call)
    pub self_slots: Vec<u8>,
    /// The outlives that a bound on a type implies of its parts (`D<'a>: 'b`: 'a: 'b), with the names of the paths
    /// the part is in (`D`, comma-separated): the program adds them where none of those is a type alias, whose
    /// lifetimes may not be in what it stands for (`type Erased<'a> = ();`)
    pub derived: Vec<(String, u8, u8)>,
    /// The same for type parameters (`E<'a, T>: 'b`: T: 'b): (paths, parameter, code)
    pub derived_ty: Vec<(String, String, u8)>,
}

/// The regions of an impl header of a trait, as a trait obligation selecting the impl relates them to its goal's
/// (the head: its self type and its trait's arguments; the where clause's outlives). Codes are of `lts`; an elided
/// region of the header is one of its own (a fresh one), a region a fn pointer type binds is R_BOUND
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct RHeadSyn {
    /// The impl's lifetime parameters, then one `'_k` for each `'_` of its self type
    pub lts: Vec<String>,
    /// The region slots of the self type
    pub self_slots: Vec<u8>,
    /// The codes of the lifetimes the trait path writes as its arguments, in order
    pub trait_lts: Vec<u8>,
    /// The region slots of each type argument the trait path writes, in order
    pub trait_args: Vec<Vec<u8>>,
    /// Declared `'a: 'b`, inline or in the where clause
    pub outlives: Vec<(u8, u8)>,
}

/// The regions of a struct / enum definition
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct RAdtSyn {
    /// Its lifetime parameters
    pub lts: Vec<String>,
    /// The region slots of each field (an enum's: of each variant's fields, in order)
    pub fields: Vec<Vec<u8>>,
    pub outlives: Vec<(u8, u8)>,
    pub ty_outlives: Vec<(String, u8)>,
}

/// The regions of an impl / trait header, for the signatures of its members
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct RImplSyn {
    pub lts: Vec<String>,
    /// The region slots of `Self` (an impl's self type; empty in a trait)
    pub self_slots: Vec<u8>,
    pub outlives: Vec<(u8, u8)>,
    pub ty_outlives: Vec<(String, u8)>,
    /// An impl's lifetime parameters neither its self type nor its trait names (an associated type must not name
    /// them, E0207)
    pub free: Vec<String>,
    /// What the header knows of outlives by name (declared, or implied by its types: crate::wf::Known), and the
    /// names another item may relate
    pub wf_pairs: Vec<(String, String)>,
    pub wf_vague: Vec<String>,
    /// The codes of the lifetimes an impl's trait path writes as its arguments (`impl<'a, 'd: 'a> Visitor<'d> for
    /// &'a ()`: 'd's), in order: a projection that selects the impl gives them those it writes there
    pub trait_lts: Vec<u8>,
    /// For each region of the self type, the names of the types around it (comma separated, outermost first):
    /// `Self: 'b` in a method implies what `'a: 'b` it may only where none of them is a type alias, which may drop
    /// the lifetime (`impl<'a> Tr for Erased<'a>` with `type Erased<'a> = ();`)
    pub self_paths: Vec<(String, u8)>,
}

/// An enum variant (the form saved in summaries)
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct VariantSyn {
    pub name: String,
    pub kind: VariantKind,
    pub fields: Vec<(String, TySyn)>,
    pub discr: Option<i64>,
    /// A discriminant that names constants (`Bar = QUUX`): the text of the expression, for Program to evaluate
    pub discr_src: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum VariantKind {
    #[default]
    Unit,
    /// `V(a, b)`. The field names are "0", "1", ...
    Tuple,
    /// `V { x, y }`
    Struct,
}

/// The summary of one item (cached). The information here alone is enough
/// to generate code for other fns.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    /// (name, signature). One for an fn item, several for an extern block
    pub fns: Vec<(String, SigSyn)>,
    /// (name, fields). One for a struct item
    pub structs: Vec<(String, Vec<(String, TySyn)>)>,
    /// The names of the structs above that are unions (fields all at offset 0)
    pub unions: Vec<String>,
    /// (enum, integer type) for an enum with `#[repr(u8)]` and the like
    pub reprs: Vec<(String, String)>,
    /// (name, parameter names, body text) of a free `const fn`: constants may call it
    pub const_fns: Vec<(String, Vec<String>, String)>,
    /// (name, parameter type names, result type name) of those const fns: a type's name is a one-segment path's
    /// (`u8`, an alias), "" for another type
    pub const_fn_tys: Vec<(String, Vec<String>, String)>,
    /// An error in a struct / enum / union definition (unused parameter ..), reported for the program's own items only
    /// (the prelude's types may break these rules)
    pub adt_err: Option<String>,
    /// For an impl header, the name of the self type
    pub impl_self: Option<String>,
    /// (name, variants). One for an enum item
    pub enums: Vec<(String, Vec<VariantSyn>)>,
    /// For a trait header, its name
    pub trait_decl: Option<String>,
    /// The type parameters of a trait header (the T of `trait From<T>`) and their bounds. The supertraits (the Foo of `trait Bar: Foo`; except Send / Sync / Sized
    /// and the like) are the entry named `#supers` (kept here so as not to make Summary bigger)
    pub trait_generics: Vec<(String, Vec<String>)>,
    /// For an `impl Trait for S` header, the trait's name
    pub impl_trait: Option<String>,
    /// The (type name, trait names) given by `#[derive(..)]`. A derived trait is treated as implemented
    pub derives: Vec<(String, Vec<String>)>,
    /// The type parameters of this item's struct / enum (["T"] for `struct W<T>`)
    pub adt_generics: Vec<String>,
    /// The plain trait bounds on those parameters (`struct S<T: Tr>`: ("T", ["Tr"])), only the parameters that have
    /// some: a static's type is checked against them
    pub adt_bounds: Vec<(String, Vec<String>)>,
    /// Those parameters declared `?Sized` (`struct Foo<T: ?Sized>`): only they may be an unsized type, in the last
    /// field
    pub adt_unsized: Vec<String>,
    /// The defaults of those parameters, by place (`struct W<T, F = fn() -> T>`: [None, Some(fn() -> T)])
    pub adt_defaults: Vec<Option<TySyn>>,
    /// The regions of an impl / trait header (None: it names no lifetime)
    pub hdr_regions: Option<Box<RImplSyn>>,
    /// The regions of a trait impl's head, where it names a lifetime (`'static` too), binds one in a fn pointer type
    /// or declares an outlives (None otherwise: its regions are all its own, any goal's)
    pub impl_head: Option<Box<RHeadSyn>>,
    /// The regions of the struct / enum defined (None: it names no lifetime, so it has no region of its own)
    pub adt_regions: Option<Box<RAdtSyn>>,
    /// The type parameters and self type of an impl header (`impl<T> Option<T>`)
    pub impl_generics: Vec<(String, Vec<String>)>,
    pub impl_self_ty: Option<TySyn>,
    /// The self type is written with a path, `impl From<TokenStream> for proc_macro::TokenStream`: another type than
    /// a same-named one of this module, though the summary's name may be the last segment
    pub impl_self_path: bool,
    /// The type arguments of the trait of an `impl Trait<A, B> for S` header
    pub impl_trait_args: Vec<TySyn>,
    /// The header's trait path writes a lifetime (`impl Tr<&'a u8>`, `impl Tr<&'static u8>`): its regions are not
    /// all elided ones (regionck's impl_sig_mismatch)
    pub impl_trait_lts: bool,
    /// The (name, type, source of the expression) of `const N: T = e;`. Embedded as an expression at the use site
    pub consts: Vec<(String, TySyn, String)>,
    /// The (name, type, source of the initial value, mut) of `static [mut] N: T = e;`
    pub statics: Vec<(String, TySyn, String, bool)>,
    /// The closures in the fn body (preorder). See [`crate::closure`]
    pub closures: Vec<ClosureSyn>,
    /// The associated types of an impl (`type Item = u32;`)
    pub assocs: Vec<AssocSyn>,
    /// a trait's associated type declaration with bounds (`type Y: Clone;`): (trait, name, bounds)
    pub assoc_bounds: Vec<(String, String, Vec<String>)>,
    /// The (name, type parameters, body) of `type Key = u128;` / `type R<T> = Result<T, E>;`
    pub aliases: Vec<(String, Vec<String>, TySyn)>,
    /// The aliases whose definitions name a lifetime that is not their own (`type S = &'static u8;`): TySyn keeps no
    /// lifetimes, so a use of one written without lifetimes cannot be told from an elided one
    pub alias_fixed_lts: Vec<String>,
    /// An impl's constants whose types name its lifetimes (`impl<'a> Foo<'a> { const C: &'a u32 }`): the region codes of
    /// the type against the impl's lifetimes (a use instantiates them with those its path writes: `<Foo<'a>>::C`)
    pub const_regions: Vec<(String, Vec<u8>)>,
    /// An impl's associated types whose values name its lifetimes (`impl<'a> Foo for &'a () { type Out = &'a u32; }`,
    /// `type Out = &'static u32`): by the index in assocs, the region codes of the value against the impl's lifetimes
    /// (a projection written in a body instantiates them with those its self type writes: `<&'static () as Foo>::Out`)
    pub assoc_regions: Vec<(u64, Vec<u8>)>,
    /// The concrete (non-parameter, non-primitive) types bounded in the item's where clauses (`where Local: Any`):
    /// Program checks that they exist
    pub where_types: Vec<String>,
    /// A member of `impl<T> Tr for &T` / `&mut T`: the method it defines (the impl's methods are not called: a call on
    /// the reference reaches T's through the deref)
    pub ref_impl_fns: Vec<String>,
    /// That method, when T is bounded by Tr and it does more than forward to T's method (ast_summary::forwards_to_referent)
    pub ref_impl_bad: Vec<String>,
    /// A forwarding method's `_` parameter given the path of its type as a value, (method, the path's last segment):
    /// a forward only when that is a unit struct, not a const or static of the same name (the driver checks)
    pub ref_impl_unit_args: Vec<(String, String)>,
}

/// A trait's declaration: the lifetimes Self must outlive (`trait Wedding<'t>: 't`) are the item `#outlives` of
/// Summary::trait_generics, as the indices of its lifetime parameters (`static`: 'static)
pub const SELF_OUTLIVES: &str = "#outlives";

/// A type that must outlive what a trait's declaration requires of Self: the self type of `impl Tr<'x> for S<'a, T>`,
/// or a trait object `dyn Tr<'x> + 'r`
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WfObl {
    pub tr: String,
    /// The trait's lifetime arguments as written
    pub args: Vec<String>,
    /// The lifetimes and type parameters that must outlive them
    pub sub: Vec<String>,
    /// What the item knows (crate::wf::Known): (longer, shorter), and the names it may not
    pub known: Vec<(String, String)>,
    pub vague: Vec<String>,
    /// The line in the item (from 1)
    pub line: u64,
}

#[derive(Clone, Debug, Default)]
pub struct AssocSyn {
    pub name: String,
    pub self_ty: TySyn,
    pub generics: Vec<String>,
    pub value: TySyn,
}

/// One lifted closure (a struct and an fn)
#[derive(Clone, Debug, Default)]
pub struct ClosureSyn {
    /// The struct's name (`parent::{closure#k}`). The fn is `<name>::call`
    pub name: String,
    /// The type parameters of the struct and fn: the parent's type parameters, __C.., __P.., __R
    pub generics: Vec<String>,
    /// Captures (name, `__Ci`)
    pub fields: Vec<(String, TySyn)>,
    pub sig: SigSyn,
    pub by_move: bool,
    /// Per capture, whether it is held by value (`move`, or the body passes / returns that capture by value). The rest are held by reference
    pub moved: Vec<bool>,
    /// The return type annotation (`|x| -> u8 { .. }`)
    pub ret_ann: Option<TySyn>,
    /// The number of the parent's type parameters
    pub n_parent: u64,
    /// The variables the body reads only by `let _ = place;` (ast_closure::fake_reads): not captured
    pub fake_reads: Vec<String>,
    /// Per capture: a parameter of the fn that is a shared reference (`h: &T`, `&self`), used in the body only through
    /// a deref of it (ast_closure::deref_uses_only). From edition 2021 it is held by value: the closure captures `*h`
    pub deref: Vec<bool>,
    /// Per capture: the body assigns to it, through it or borrows it mutably (`x = ..`, `x.f += ..`, `*x = ..`,
    /// `&mut x`; ast_closure::mutates_capture). Held by reference, it is then a `&mut T` (or a unique borrow): Send when
    /// T is. The others held by reference may be either kind (a method's `&mut self` is not seen here)
    pub mutated: Vec<bool>,
}

/// The owner of the members of an impl / trait
pub struct Owner {
    /// The name put before method names (the self type's name for an impl, the trait's name for a trait)
    pub base: String,
    /// What `Self` is
    pub self_ty: TySyn,
    /// The trait of `impl Trait for S`
    pub impl_trait: Option<String>,
    /// The impl's type parameters (`Self` for a trait)
    pub generics: Vec<(String, Vec<String>)>,
    pub is_trait: bool,
    /// The header's regions
    pub regions: Option<Box<RImplSyn>>,
}

/// The mark of a const generic (put in the place of a bound's name)
pub const CONST_GENERIC: &str = "__const";
/// When the N of `[T; N]` is type parameter i, the length of the Ty::Array is CONST_LEN_BASE + i
pub const CONST_LEN_BASE: u32 = 0xFFFF_FF00;
/// The placeholder type name of an `impl Trait` in return position
pub const IMPL_RET: &str = "impl Trait";
/// A return value of `impl Trait` that is not a closure (`-> impl Iterator<Item = u32>`): decided by the body's type inference
/// SigSyn::ret_from's mark for a result `impl Trait`
pub const OPAQUE_FROM: u64 = 1 << 62;
pub const OPAQUE_RET: &str = "impl Trait (opaque)";
/// `-> Option<impl Debug>`: `App(OPAQUE_NESTED, [the declared type with each impl Trait as _])`. The whole type comes
/// from the body, as for OPAQUE_RET; the declared type is resolved only for the names it does not find
pub const OPAQUE_NESTED: &str = "impl Trait (nested)";

impl Default for TySyn {
    fn default() -> Self {
        TySyn::Prim(Ty::Unit)
    }
}

/// The built-in types (in the order of their numbers in the cache serialization)
pub const TYS: [Ty; 18] = [
    Ty::I8,
    Ty::I16,
    Ty::I32,
    Ty::I64,
    Ty::Isize,
    Ty::U8,
    Ty::U16,
    Ty::U32,
    Ty::U64,
    Ty::Usize,
    Ty::Bool,
    Ty::Unit,
    Ty::Never,
    Ty::Char,
    Ty::F32,
    Ty::F64,
    Ty::I128,
    Ty::U128,
];

/// The prelude's name of a std unit struct a value path names by another (`fmt::Error`, std's unit struct, is the
/// prelude's `__FmtError`), else the path's last segment
pub fn std_unit_name<'a>(segs: &[&'a str]) -> &'a str {
    match segs {
        [.., "fmt", "Error"] => "__FmtError",
        [.., last] => last,
        [] => "",
    }
}
