//! 型の表現 (front.rs から分けた、syn に依存しない部分)。`Ty` (解決済みの型) と `TySyn` (構文上の型) と intern。
//! zustc 自身でもコンパイルできる書き方にしてある (セルフホストの対象)。

// (the interning tables: Fx hashing, deterministic and cheaper than the std maps' SipHash)
type HashSet<K> = crate::fxhash::FxSet<K>;
use std::sync::{Mutex, OnceLock};

// ---------------------------------------------------------------------------
// 型
// ---------------------------------------------------------------------------

/// 解決済みの型。Copy にするため参照先は intern した `&'static Ty` で持つ。
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
    /// 128 bit 整数
    I128,
    U128,
    /// Unicode のスカラー値 (32 bit)
    Char,
    /// 浮動小数点 (IEEE 754)
    F32,
    F64,
    /// `&T` (false) / `&mut T` (true)
    Ref(bool, &'static Ty),
    /// Program::structs の index
    Struct(u32),
    /// ジェネリック関数の型パラメータ (FnSig::generics の index)。インスタンス化で具体型に置き換わる
    Param(u8),
    /// `[T; N]` (値として扱う集約型)
    Array(&'static Ty, u32),
    /// `[T]` (サイズ不定。`&[T]` の中身としてだけ現れる)
    Slice(&'static Ty),
    /// `str` (サイズ不定。`&str` の中身としてだけ現れる)
    Str,
    /// `*const T` (false) / `*mut T` (true)
    Ptr(bool, &'static Ty),
    /// `dyn Trait` (サイズ不定。`&dyn Trait` = (データ, vtable) の中身としてだけ現れる)。
    /// 中身は trait 境界の文字列 (`Shape` / `Fn(i32) -> i32`)
    Dyn(&'static String),
    /// 関数ポインタ `fn(A) -> R` (関数アイテムを値として使ったものもこれ)
    Fn(&'static FnTy),
    /// `Box<T>`: ヒープへの所有ポインタ (`Box<dyn Trait>` は fat)。drop で中身を drop して free する
    Box(&'static Ty),
    /// 関連型 `T::Item` (T が型パラメータのときだけ現れる。具体型になったら impl の定義に置き換わる)
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
    /// メモリ上に置き、値としてはアドレスで扱う型 (struct / enum / 配列 / `&[T]`・`&str` の fat pointer)
    pub fn is_agg(self) -> bool {
        matches!(self, Ty::Struct(_) | Ty::Array(..)) || self.is_fat()
    }
    /// (ポインタ, 長さ) の 2 語で表す参照
    pub fn is_fat(self) -> bool {
        matches!(
            self,
            Ty::Ref(_, Ty::Slice(_) | Ty::Str | Ty::Dyn(_)) | Ty::Box(Ty::Slice(_) | Ty::Str | Ty::Dyn(_))
        )
    }
}

/// 文字列の intern 表 (`dyn` の trait 境界)
/// (`Ty` を 16 byte に保つため、fat な `&str` ではなく `&String` を持つ)
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

/// 関数ポインタ型 `fn(A, B) -> R` の中身
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

/// trait 境界を文字列にする (パスの最後のセグメントだけ。`core::ops::Fn(u8) -> u8` → `Fn (u8) -> u8`)
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

    /// スカラー (整数・bool・参照) のビット幅。struct / unit / never は 0。
    pub fn bits(self) -> u32 {
        match self {
            Ty::I8 | Ty::U8 | Ty::Bool => 8,
            Ty::I16 | Ty::U16 => 16,
            Ty::I32 | Ty::U32 | Ty::Char | Ty::F32 => 32,
            Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize | Ty::Ptr(..) | Ty::Fn(_) | Ty::F64 => 64,
            Ty::I128 | Ty::U128 => 128,
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

/// 名前未解決の型 (要約に保存する形)。struct は名前で持つので、
/// アイテムの並び順が変わってもキャッシュが壊れない。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TySyn {
    Prim(Ty),
    Ref(bool, Box<TySyn>),
    Named(String),
    /// `Option<T>` など、ジェネリックな struct / enum の適用
    App(String, Vec<TySyn>),
    /// `(A, B)`
    Tuple(Vec<TySyn>),
    /// `[T; N]` (N はリテラルか const の名前)
    Array(Box<TySyn>, ArrLen),
    Slice(Box<TySyn>),
    Str,
    Ptr(bool, Box<TySyn>),
    /// `dyn Trait` (境界の文字列)
    Dyn(String),
    /// `fn(A, B) -> R` (最後が戻り値)
    FnPtr(Vec<TySyn>),
    /// 関連型 `T::Item` / `Self::Item`
    Assoc(Box<TySyn>, String),
    /// 型推論に任せる `_` (`collect::<Vec<_>>()`)
    Infer,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArrLen {
    Lit(u64),
    Const(String),
}


// ---- アイテムの要約 (front.rs から分けた、syn に依存しない部分) ----

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct SigSyn {
    pub params: Vec<TySyn>,
    pub ret: TySyn,
    pub symbol: String,
    pub is_extern: bool,
    pub is_main: bool,
    /// impl の中の関数なら self 型の名前
    pub impl_of: Option<String>,
    /// 第 1 引数が self (メソッド呼び出し構文で呼べる)
    pub has_self: bool,
    /// 型パラメータと、それぞれのトレイト境界。trait の中の関数は先頭に `Self` を持つ
    pub generics: Vec<(String, Vec<String>)>,
    /// trait の中で宣言された関数ならその trait 名
    pub trait_of: Option<String>,
    /// 本体があるか (trait の宣言だけの関数は false)
    pub has_body: bool,
    /// `impl Trait for S` の中の関数ならその trait 名
    pub impl_trait: Option<String>,
    /// impl の中の関数なら `Self` の型 (`impl<T> Option<T>` なら `Option<T>`)
    pub self_ty: Option<TySyn>,
    /// `#[no_mangle]` / `#[unsafe(no_mangle)]`: 名前そのままで外に見せる
    pub export: bool,
    /// `#[panic_handler]`
    pub panic_handler: bool,
    /// `unsafe fn`, or a fn of an `extern` block (calling it needs `unsafe`, E0133)
    pub is_unsafe: bool,
    /// Which reference arguments the return value may borrow from (borrow checking): bit i for argument i (self
    /// first), bit 63 set when known. 0: not known (any of them)
    pub ret_from: u64,
    /// The regions of the signature (None: it writes no reference and no lifetime, so every region in it is elided)
    pub regions: Option<Box<RSigSyn>>,
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
}

/// enum のバリアント (要約に保存する形)
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
    /// `V(a, b)`。フィールド名は "0", "1", ...
    Tuple,
    /// `V { x, y }`
    Struct,
}

/// アイテム 1 個分の要約 (キャッシュされる)。ここに入っている情報だけで
/// 他の関数のコード生成ができる。
#[derive(Clone, Debug, Default)]
pub struct Summary {
    /// (名前, シグネチャ)。fn アイテムなら 1 個、extern ブロックなら複数
    pub fns: Vec<(String, SigSyn)>,
    /// (名前, フィールド)。struct アイテムなら 1 個
    pub structs: Vec<(String, Vec<(String, TySyn)>)>,
    /// The names of the structs above that are unions (fields all at offset 0)
    pub unions: Vec<String>,
    /// (enum, integer type) for an enum with `#[repr(u8)]` and the like
    pub reprs: Vec<(String, String)>,
    /// (name, parameter names, body text) of a free `const fn`: constants may call it
    pub const_fns: Vec<(String, Vec<String>, String)>,
    /// An error in a struct / enum / union definition (unused parameter ..), reported for the program's own items only
    /// (the prelude's types may break these rules)
    pub adt_err: Option<String>,
    /// impl ヘッダなら self 型の名前
    pub impl_self: Option<String>,
    /// (名前, バリアント)。enum アイテムなら 1 個
    pub enums: Vec<(String, Vec<VariantSyn>)>,
    /// trait ヘッダならその名前
    pub trait_decl: Option<String>,
    /// trait ヘッダの型パラメータ (`trait From<T>` の T) と境界。親トレイト (`trait Bar: Foo` の Foo; Send / Sync / Sized
    /// などは除く) は名前 `#supers` の項目 (Summary を大きくしないために、ここに持つ)
    pub trait_generics: Vec<(String, Vec<String>)>,
    /// `impl Trait for S` のヘッダなら trait 名
    pub impl_trait: Option<String>,
    /// `#[derive(..)]` された (型名, trait 名)。derive した trait は実装扱いにする
    pub derives: Vec<(String, Vec<String>)>,
    /// このアイテムの struct / enum の型パラメータ (`struct W<T>` なら ["T"])
    pub adt_generics: Vec<String>,
    /// The plain trait bounds on those parameters (`struct S<T: Tr>`: ("T", ["Tr"])), only the parameters that have
    /// some: a static's type is checked against them
    pub adt_bounds: Vec<(String, Vec<String>)>,
    /// The regions of an impl / trait header (None: it names no lifetime)
    pub hdr_regions: Option<Box<RImplSyn>>,
    /// The regions of the struct / enum defined (None: it names no lifetime, so it has no region of its own)
    pub adt_regions: Option<Box<RAdtSyn>>,
    /// impl ヘッダの型パラメータと self 型 (`impl<T> Option<T>`)
    pub impl_generics: Vec<(String, Vec<String>)>,
    pub impl_self_ty: Option<TySyn>,
    /// The type arguments of the trait of an `impl Trait<A, B> for S` header
    pub impl_trait_args: Vec<TySyn>,
    /// `const N: T = e;` の (名前, 型, 式のソース)。使う側で式として埋め込む
    pub consts: Vec<(String, TySyn, String)>,
    /// `static [mut] N: T = e;` の (名前, 型, 初期値のソース, mut)
    pub statics: Vec<(String, TySyn, String, bool)>,
    /// fn 本体の中のクロージャ (前順)。[`crate::closure`] を参照
    pub closures: Vec<ClosureSyn>,
    /// impl の関連型 (`type Item = u32;`)
    pub assocs: Vec<AssocSyn>,
    /// a trait's associated type declaration with bounds (`type Y: Clone;`): (trait, name, bounds)
    pub assoc_bounds: Vec<(String, String, Vec<String>)>,
    /// `type Key = u128;` / `type R<T> = Result<T, E>;` の (名前, 型パラメータ, 中身)
    pub aliases: Vec<(String, Vec<String>, TySyn)>,
    /// The concrete (non-parameter, non-primitive) types bounded in the item's where clauses (`where Local: Any`):
    /// Program checks that they exist
    pub where_types: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AssocSyn {
    pub name: String,
    pub self_ty: TySyn,
    pub generics: Vec<String>,
    pub value: TySyn,
}

/// 持ち上げたクロージャ 1 個分 (struct と fn)
#[derive(Clone, Debug, Default)]
pub struct ClosureSyn {
    /// struct の名前 (`親::{closure#k}`)。fn は `<name>::call`
    pub name: String,
    /// struct と fn の型パラメータ: 親の型パラメータ, __C.., __P.., __R
    pub generics: Vec<String>,
    /// キャプチャ (名前, `__Ci`)
    pub fields: Vec<(String, TySyn)>,
    pub sig: SigSyn,
    pub by_move: bool,
    /// キャプチャごとに値で持つか (`move` か、本体がそのキャプチャを値として渡す・返す)。残りは参照で持つ
    pub moved: Vec<bool>,
    /// 戻り値型の注釈 (`|x| -> u8 { .. }`)
    pub ret_ann: Option<TySyn>,
    /// 親の型パラメータの数
    pub n_parent: u64,
}

/// impl / trait のメンバの所有者
pub struct Owner {
    /// メソッド名の前に付ける名前 (impl なら self 型の名前、trait なら trait 名)
    pub base: String,
    /// `Self` の中身
    pub self_ty: TySyn,
    /// `impl Trait for S` の trait
    pub impl_trait: Option<String>,
    /// impl の型パラメータ (trait なら `Self`)
    pub generics: Vec<(String, Vec<String>)>,
    pub is_trait: bool,
    /// The header's regions
    pub regions: Option<Box<RImplSyn>>,
}

/// const ジェネリクスの印 (境界の名前の位置に入れる)
pub const CONST_GENERIC: &str = "__const";
/// `[T; N]` の N が型パラメータ i のとき、Ty::Array の長さは CONST_LEN_BASE + i
pub const CONST_LEN_BASE: u32 = 0xFFFF_FF00;
/// 戻り値位置の `impl Trait` の仮の型名
pub const IMPL_RET: &str = "impl Trait";
/// クロージャでない `impl Trait` の戻り値 (`-> impl Iterator<Item = u32>`): 本体の型推論で決める
pub const OPAQUE_RET: &str = "impl Trait (opaque)";
/// `-> Option<impl Debug>`: `App(OPAQUE_NESTED, [the declared type with each impl Trait as _])`. The whole type comes
/// from the body, as for OPAQUE_RET; the declared type is resolved only for the names it does not find
pub const OPAQUE_NESTED: &str = "impl Trait (nested)";

impl Default for TySyn {
    fn default() -> Self {
        TySyn::Prim(Ty::Unit)
    }
}

/// 組み込みの型 (キャッシュの直列化での番号の順)
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
