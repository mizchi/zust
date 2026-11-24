//! プログラム全体の表 (関数シグネチャ・struct / enum の定義とレイアウト・trait 実装など)。
//! アイテム要約 (`Summary`) から作る。syn に依存しない (zustc 自身でもコンパイルできる) 部分で、
//! syn の構文木を引く検索 (`fn_by_path` など) は front.rs の別の `impl Program` にある。

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
    /// オブジェクトファイル上のシンボル名
    pub symbol: String,
    pub is_extern: bool,
    pub is_main: bool,
    /// impl の中の関数なら self 型 (struct id)
    pub impl_of: Option<u32>,
    pub has_self: bool,
    pub generics: Vec<(String, Vec<String>)>,
    pub trait_of: Option<String>,
    pub has_body: bool,
    pub impl_trait: Option<String>,
    /// impl の中の関数の `Self` (型パラメータを含みうる)
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
    /// 単相化が必要か (直接はコンパイルしない)
    pub fn is_generic(&self) -> bool {
        !self.generics.is_empty()
    }
}

/// 関数本体の中で型名を解決するための環境 (`Self` と型パラメータ)
#[derive(Clone, Debug, Default)]
pub struct TyEnv {
    pub self_sid: Option<u32>,
    pub params: Vec<(String, Ty)>,
    /// `Self` の型 (struct なら Struct(self_sid)、プリミティブへの trait 実装なら u32 など)
    pub self_ty: Option<Ty>,
}

pub struct VariantDef {
    pub name: String,
    pub kind: VariantKind,
    /// (名前, 型, enum 先頭からのオフセット)
    pub fields: Vec<(String, Ty, u32)>,
    pub discr: i64,
}

/// struct と enum (名前付きの集約型) の定義。enum も同じ表に入れることで、
/// コピー・引数渡し・impl・deep hash などの処理を struct と共有する。
pub struct StructDef {
    pub name: String,
    /// (名前, 型, オフセット)
    pub fields: Vec<(String, Ty, u32)>,
    pub size: u32,
    pub align: u32,
    /// 定義と、フィールドから推移的に届く struct の定義すべてのハッシュ (レイアウトが変わったら変わる)
    pub deep: Key,
    /// enum ならバリアント (struct なら空)
    pub variants: Vec<VariantDef>,
    pub is_enum: bool,
    /// enum のタグの型 (U8 か I32)。オフセット 0 に置く
    pub tag: Ty,
    /// ジェネリックな定義 (`Option<T>`) なら型パラメータ名。フィールドの型は Param を含み、レイアウトはない
    pub generics: Vec<String>,
    /// インスタンス (`Option<i64>`) なら元の定義と型引数
    pub base: Option<u32>,
    pub args: Vec<Ty>,
    /// 型引数に型パラメータが残っているインスタンスか (has_param が引数を辿り直さないように: 入れ子のクロージャの
    /// 型は共有された部分を持つので、辿り直すと段の数の指数になる)
    pub symbolic: bool,
    /// A placeholder for a type parameter of a generic fn `zustc check` instantiates though nothing calls it: the
    /// traits the parameter's bounds name (Program::placeholder)
    pub placeholder: Option<Vec<String>>,
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
    /// 作っている途中のインスタンス (基の型, 型引数) と、途中で取った番号
    static INSTANTIATING: std::cell::RefCell<Vec<((u32, Vec<Ty>), Option<u32>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// struct / enum の表。後からインスタンス (`Option<i64>` など) を足せる (並列のコード生成中にも)。
/// 要素は Box で持ち、決して削除・移動しないので、`&self` から要素への参照を返せる
pub struct AdtTable {
    v: Mutex<Vec<Box<StructDef>>>,
    inst: Mutex<HashMap<(u32, Vec<Ty>), u32>>,
    tuples: Mutex<HashMap<usize, u32>>,
    /// The placeholders made, by (fn, type parameter)
    placeholders: Mutex<HashMap<(u32, u32), u32>>,
    /// Whether any was made (read without the lock, on hot paths)
    any_placeholder: std::sync::atomic::AtomicBool,
}

impl AdtTable {
    fn new() -> Self {
        AdtTable {
            v: Mutex::new(vec![]),
            inst: Mutex::new(HashMap::default()),
            tuples: Mutex::new(HashMap::default()),
            placeholders: Mutex::new(HashMap::default()),
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
        &mut self.v.get_mut().unwrap()[i]
    }
    /// 先に番号だけ取ったインスタンスの中身を入れる (まだ誰にも公開していない番号だけに使う)
    fn replace(&self, i: u32, d: StructDef) {
        let mut g = self.v.lock().unwrap();
        *g[i as usize] = d;
    }
}

impl std::ops::Index<usize> for AdtTable {
    type Output = StructDef;
    fn index(&self, i: usize) -> &StructDef {
        let g = self.v.lock().unwrap();
        let p: *const StructDef = &*g[i];
        drop(g);
        // SAFETY: 要素は Box に入っていて削除されない (Vec の再確保で動くのは Box のポインタだけ)。
        // 公開後の要素を変更するのは &mut self を取る get_mut (build 中) だけ
        unsafe { &*p }
    }
}

/// レイアウト計算の入力
#[derive(Clone)]
enum AdtShape {
    Struct(Vec<(String, TySyn)>),
    Enum(Vec<VariantSyn>),
}

/// const / static の定義
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

pub struct ConstDef {
    pub name: String,
    pub ty: Ty,
    /// 初期値の式のソース (使う側・初期化で parse し直す)
    pub expr: String,
    /// 定義しているアイテム (キャッシュキー用)
    pub item: u32,
    /// static なら Some(mut か)
    pub stat: Option<bool>,
}

/// `u32::MAX` などの関連定数
/// `const _: () = { .. };` (名前のない const: コンパイル時の検査だけなので読み飛ばす。モジュールを平らにした `m___` も)
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

/// An integer type in constant evaluation: its width and sign (usize / isize are 64 bits)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct It {
    pub bits: u32,
    pub signed: bool,
}

impl It {
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
        Some(It { bits, signed })
    }
    pub fn of_ty(t: Ty) -> Option<It> {
        match t {
            Ty::U8 => It::of_name("u8"),
            Ty::U16 => It::of_name("u16"),
            Ty::U32 => It::of_name("u32"),
            Ty::U64 | Ty::Usize => It::of_name("u64"),
            Ty::U128 => It::of_name("u128"),
            Ty::I8 => It::of_name("i8"),
            Ty::I16 => It::of_name("i16"),
            Ty::I32 => It::of_name("i32"),
            Ty::I64 | Ty::Isize => It::of_name("i64"),
            Ty::I128 => It::of_name("i128"),
            _ => None,
        }
    }
    /// The type's name, as a literal's suffix (usize / isize as u64 / i64: the same width)
    pub fn name(self) -> &'static str {
        match (self.bits, self.signed) {
            (8, false) => "u8",
            (16, false) => "u16",
            (32, false) => "u32",
            (64, false) => "u64",
            (128, false) => "u128",
            (8, true) => "i8",
            (16, true) => "i16",
            (32, true) => "i32",
            (64, true) => "i64",
            _ => "i128",
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
}

pub fn prim_assoc_const(t: Ty, name: &str) -> Option<i128> {
    let bits = t.bits();
    Some(match (name, t.signed()) {
        ("BITS", _) => bits as i128,
        // (u128::MAX はビット列として持つ: -1)
        ("MAX", true) => (u128::MAX >> (129 - bits)) as i128,
        ("MIN", true) => -((u128::MAX >> (129 - bits)) as i128) - 1,
        ("MAX", false) if t == Ty::Char => 0x10FFFF,
        ("MAX", false) => (u128::MAX >> (128 - bits)) as i128,
        ("MIN", false) => 0,
        _ => return None,
    })
}

pub struct Program {
    /// 全関数 (ユーザ定義 + extern)。index が u32 の関数 ID になる
    pub names: Vec<String>,
    pub index: FxMap<String, u32>,
    pub sigs: Vec<FnSig>,
    /// ユーザ定義関数を定義しているアイテム番号 (extern は None)
    pub item_of: Vec<Option<u32>>,
    /// The associated types of trait impls by name (indices into assocs), made once for normalize: every name is
    /// looked for again by each instance's goals
    pub assoc_index: std::sync::OnceLock<FxMap<(String, u32), Vec<u32>>>,
    /// normalize's answers so far, by (type, trait, associated type)
    pub normalized: std::sync::Mutex<FxMap<(Ty, String, String), Option<Ty>>>,
    /// A fn's where clauses on other than a parameter (SigSyn::preds), by fn id: goals of each instance (issue #2)
    pub fn_preds: FxMap<u32, Vec<(crate::types::TySyn, String, Vec<String>)>>,
    pub structs: AdtTable,
    pub struct_index: FxMap<String, u32>,
    /// バリアント名 → (enum, 番号)。`use E::*;` で取り込んだバリアントを短い名前で引く (同名が複数なら引かない)
    pub variant_names: FxMap<String, Option<(u32, usize)>>,
    /// 戻り値が本体から決まる `impl Trait` の関数
    pub opaque: Vec<u32>,
    /// これ以降のアイテムは prelude (オーバーフロー検査をしない)
    pub prelude_from: u32,
    /// 算術のオーバーフロー検査をするか (-O でなければ)
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
    /// the program's source declares modules (`mod m { .. }`), whose scopes are flattened away: a name's item may then
    /// be out of scope where the name is written
    pub has_mods: bool,
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
    /// `#![deny(trivial_casts)]` / `#![deny(trivial_numeric_casts)]`: a cast a coercion or nothing would do
    pub deny_trivial_casts: (bool, bool),
    /// the program's `#[must_use]` fns (by name), for unused_must_use
    pub must_use_fns: Vec<String>,
    /// the names of the methods of numbers / bool / char that take `&mut self` (`impl Inc for i32 { fn inc(&mut
    /// self) }`): a local that calls one is kept in memory, and the method gets its address
    pub prim_mut_methods: HashSet<String>,
    /// -O (最適化ビルド)
    pub opt: bool,
    /// ブランケット impl のメソッド (名前 → 関数 id) と、その self 型の型パラメータ名
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
    /// 同じ型への別々の trait impl が同じ名前の関数を持つもの (`impl From<&str> for P` と `impl From<String> for P`):
    /// 最初の名前 → 候補の関数 (2 つ目以降は `P::from#1` のような名前)。呼び出し側が引数の型で選ぶ
    pub overloads: FxMap<String, Vec<u32>>,
    /// zust_syntax::lexer::trait_scopes of the program's own source: where two scopes define traits of one name, the
    /// traits in scope on each line (pick_overload chooses the impl of the one in scope at the call)
    pub trait_scopes: Vec<(u32, u32, Vec<String>)>,
    /// The source line (1-based) of each item, for trait_scopes: the diagnostic line base of a function is not it (a
    /// dependency's is tagged, and it is the line before). Empty when trait_scopes is
    pub item_lines: Vec<u32>,
    /// 名前の最後のセグメント → 関数 ID 群 (`len` → `A::len`, `B::len`)。参照の過大近似用
    pub by_last: FxMap<String, Vec<u32>>,
    /// trait 名 → その trait で宣言された関数
    pub traits: FxMap<String, Vec<u32>>,
    /// impls whose self type is a reference to a concrete type (`impl Tr for &i32` / `for &S`): (that type, trait)
    pub ref_impls: HashSet<(Ty, String)>,
    /// generic impls for references: (self type with the impl's parameters, trait, number of parameters)
    pub ref_impl_pats: Vec<(Ty, String, usize)>,
    /// traits implemented for fn pointers (`impl Tr for fn(A) -> B`; generic ones count for every fn pointer)
    pub fnptr_impls: HashSet<(Option<Ty>, String)>,
    /// 親トレイト (`trait Bar: Foo`)
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
    /// (型, trait) の実装
    pub impls: HashSet<(u32, String)>,
    /// the self types of the impls of a trait for a struct's instances (`impl A for W<u8>`), by (struct, trait), with
    /// the impl's parameters as `Param(i)` and their number
    pub impl_selfs: FxMap<(u32, String), Vec<(Ty, usize, Vec<(usize, String)>)>>,
    /// the traits the program declares (not the prelude's): a bound of one is checked by its impls
    pub user_traits: FxSet<String>,
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
    /// `#[panic_handler]` の関数 (no_std)。panic (範囲外アクセス・unwrap など) はここを呼ぶ
    pub panic_handler: Option<u32>,
    /// const と static
    pub consts: Vec<ConstDef>,
    pub const_index: FxMap<String, u32>,
    /// 関連定数 (`S::N`) を最後の名前 (`N`) で引く (参照関係の過大近似用)
    pub const_by_last: FxMap<String, Vec<u32>>,
    /// プリミティブ型への trait 実装 (型, trait)
    pub prim_impls: HashSet<(Ty, String)>,
    /// prim_impls that only an impl for a reference to the type makes (`impl Tr for &mut i32`): the type itself does not
    /// implement the trait (implements_std)
    pub prim_ref_only: HashSet<(Ty, String)>,
    /// `&[T]` に実装された trait (`impl BufRead for &[u8]`)
    pub ref_slice_traits: HashSet<String>,
    /// クロージャの fn id → 定義
    pub closures: FxMap<u32, ClosureDef>,
    /// (親の関数 id, 前順の番号) → クロージャの fn id
    pub closure_by: FxMap<(u32, u32), u32>,
    /// クロージャの (ジェネリックな) struct id → fn id
    pub closure_of_struct: FxMap<u32, u32>,
    /// hash_ty のインスタンスごとの記憶 (入れ子のクロージャの型は同じ部分を共有するので、辿り直すと指数になる)
    pub inst_keys: std::sync::Mutex<FxMap<u32, Key>>,
    /// `impl Drop for S` のある struct / enum (ジェネリックなら元の定義)
    pub drop_impls: crate::fxhash::FxSet<u32>,
    /// The structs (base ids) whose `deref` returns a reference to an array, slice or str: `t.len()` on one of them is
    /// that target's method
    pub deref_to_seq: crate::fxhash::FxSet<u32>,
    /// The program's structs (base ids) with a `deref_mut`: a method call's receiver of another one reaches no
    /// program's `DerefMut`
    pub deref_mut_of: crate::fxhash::FxSet<u32>,
    /// drop が要る型がありうるか (prelude に drop glue があるか。Box も Drop もなければ false)
    pub has_drop: bool,
    /// impl の関連型の定義
    pub assocs: Vec<AssocDef>,
    /// (trait, associated type) -> the bounds of its declaration (`type Y: Clone;`)
    pub assoc_bounds: HashMap<(String, String), Vec<String>>,
    /// generic fns returning `impl Trait`: (parameters, body, has self), to type each instance's body
    pub opaque_generic: HashMap<u32, std::sync::Arc<(Vec<(crate::parse::Pat, crate::parse::Ty)>, crate::parse::Block, bool)>>,
    /// 右辺の型を持つ比較の impl の eq (`Vec::<PartialEq<[T;N]>>::eq` など)。`a == b` の型が違うとき探す
    pub peq: Vec<u32>,
    /// 型の別名: 名前 → (型パラメータ, 中身)
    pub aliases: FxMap<String, (Vec<String>, TySyn)>,
    /// 同じく `S::<PartialOrd<R>>::partial_cmp`。`a < b` の型が違うとき探す
    pub pord: Vec<u32>,
    /// struct / enum の定義 (番号の若いもの) を書いたアイテムの番号
    pub struct_item: Vec<u32>,
    /// The regions of the struct / enum definitions that name lifetimes (by definition id): region checking
    pub adt_regions: FxMap<u32, std::sync::Arc<RAdtSyn>>,
}

/// Program::assoc_regions' entry: (value codes, impl self type codes, header outlives, trait path lifetime codes)
pub type AssocRegions = (Vec<u8>, Vec<u8>, Vec<(u8, u8)>, Vec<u8>);

/// `impl<..> Trait for S<..> { type Name = Value; }`
pub struct AssocDef {
    pub name: String,
    /// impl の self 型と値 (impl の型パラメータを Param で含む)
    pub self_ty: Ty,
    pub value: Ty,
    /// the impl's parameters
    pub ngen: usize,
    /// a generic associated type's own parameters (`type Pointer<T>`: 1), Params after the impl's and a mark
    pub ngat: usize,
    /// the name of the trait the impl is of (`Iterator`), without its arguments; None for an inherent impl's
    pub trait_name: Option<String>,
}

/// The name of the trait of the impl an item is a member of (`impl<T> Iterator<Item = T> for S`: Iterator)
fn impl_trait_name(kind: &crate::lexer::ItemKind, sums: &[Summary]) -> Option<String> {
    let crate::lexer::ItemKind::ImplMember(h) = *kind else { return None };
    let tr = sums.get(h as usize)?.impl_trait.as_deref()?;
    Some(tr.split('<').next().unwrap_or(tr).trim().to_string())
}

/// (the impl's parameter count, the generic associated type's) of an associated type's parameter names: those after
/// the mark `#gat` are its own
fn gat_split(gn: &[&str]) -> (usize, usize) {
    match gn.iter().position(|g| *g == "#gat") {
        Some(k) => (k, gn.len() - k - 1),
        None => (gn.len(), 0),
    }
}

/// 型推論に任せる `_` の印 (型パラメータの番号としては使わない値)
pub const HOLE: u8 = 250;
/// the key mark of a closure's fn-pointer instance in the instance table (not a type parameter)
pub const FNPTR_MARK: u8 = 251;

/// 持ち上げたクロージャ ([`crate::closure`])
pub struct ClosureDef {
    /// 親 (クロージャを書いた関数) の id と、その本体での前順の番号
    pub parent: u32,
    pub k: u32,
    /// ジェネリックな struct
    pub sid: u32,
    /// キャプチャの名前 (struct のフィールド順)
    pub captures: Vec<String>,
    pub by_move: bool,
    /// キャプチャごとに値で持つか (残りは参照で持つ)
    pub moved: Vec<bool>,
    /// 親の型パラメータの数 (型引数の先頭)
    pub n_parent: usize,
    /// 戻り値の注釈 (親の型パラメータを Param で含みうる)
    pub ret_ann: Option<Ty>,
    /// ClosureSyn::fake_reads
    pub fake_reads: Vec<String>,
}

/// 単相化したインスタンス (ジェネリック関数 base に型引数 args を入れたもの)
pub struct Inst {
    pub base: u32,
    pub args: Vec<Ty>,
    pub sig: FnSig,
    /// a closure with no captures called through a fn pointer: no environment parameter (`let f: fn(i32) -> i32 = |x| x;`)
    pub fnptr: bool,
}

/// インスタンス表。関数 ID は Program の関数の後ろに続けて振る。
/// コード生成中に (並列に) 要求されたものを pending に積み、ドライバが不動点まで処理する
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
    /// Of those, the instances of fns that instances were made of with `Copy` types only (driver2's
    /// placeholder_wave): checked for what moving a T does, nothing else
    pub recheck: FxSet<u32>,
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
            kind_free: FxSet::default(),
            fx_log: vec![],
        }
    }

    pub fn get(&self, id: u32) -> Option<&Inst> {
        id.checked_sub(self.n_fns)
            .and_then(|i| self.list.get(i as usize))
    }

    /// インスタンスを要求する (なければ作り、pending に積む)。トレイト境界もここで確かめる
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
        for ((pn, bounds), &t) in b.generics.iter().zip(&args) {
            for tr in bounds {
                // (a trait's default method for an impl for a reference to a primitive (`impl Third for &&u32`) is an
                // instance whose Self is the primitive: the impl is for it there (prim_ref_only))
                if pn == "Self" && prog.prim_impls.contains(&(t, tr.clone())) {
                    continue;
                }
                match prog.evaluate(t, tr) {
                    Certainty::No => {
                        return Err(format!("the trait bound `{}: {tr}` is not satisfied (for `{pn}`)", prog.tn(t)));
                    }
                    // (the bound needs a goal that needs itself: inductive, so it does not hold. Overflow stays
                    // accepted: the limit is not rustc's)
                    Certainty::Maybe(MaybeCause::Cycle) => {
                        return Err(format!("overflow evaluating the requirement `{}: {tr}` (E0275)", prog.tn(t)));
                    }
                    _ => {}
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
                for tr in bounds {
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
            // Self: trait のデフォルトメソッドなら型引数の先頭、impl のメソッドなら self 型に型引数を入れたもの
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

    /// 関数 (またはインスタンス) id のシグネチャ
    pub fn sig<'a>(&'a self, prog: &'a Program, id: u32) -> &'a FnSig {
        match self.get(id) {
            Some(i) => &i.sig,
            None => &prog.sigs[id as usize],
        }
    }

    /// 本体を持つ関数 (インスタンスなら元のジェネリック関数) の id
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

    /// 本体を型検査するときの型環境
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


/// The summary with the items of `impl Alias { .. }` moved to the aliased type (`Alias::f` -> `Foo::f`)
fn rebase_aliased(sm: &Summary, map: &FxMap<String, String>) -> Summary {
    let mut sm = sm.clone();
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
    sm
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

/// struct / enum 定義の中での `Self`
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

    pub fn build(items: &[SrcItem], sums: &[Summary]) -> R<Program> {
        Program::build_with(items, sums, true, items.len())
    }

    /// need_main: false ならライブラリ (main がなくてよい)
    /// `user`: how many of the items are the program's (the rest are the prelude's)
    pub fn build_with(items: &[SrcItem], sums: &[Summary], need_main: bool, user: usize) -> R<Program> {
        // (the crate's `main`, summed up as an ordinary function because it cannot be the entry (ast_summary): an error
        //  when it would be, so not under `--test`. First, as it was when the summary reported it)
        if need_main && !sums.iter().any(|s| s.fns.iter().any(|(n, _)| n == TEST_ENTRY)) {
            for (i, s) in sums[..user.min(sums.len())].iter().enumerate() {
                if let Some((_, sig)) = s.fns.iter().find(|(n, sig)| n == "main" && !sig.is_main && !sig.export && !sig.is_extern && sig.symbol == "_xmain") {
                    let msg = if sig.generics.is_empty() { "`main` must be `fn main()`" } else { "`main` cannot be generic" };
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
            opaque: vec![],
            prelude_from: u32::MAX,
            fn_preds: FxMap::default(),
            assoc_index: std::sync::OnceLock::new(),
            normalized: std::sync::Mutex::new(FxMap::default()),
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
            impl_targs_lts: HashSet::default(),
            ref_impls: Default::default(),
            ref_impl_pats: vec![],
            fnptr_impls: Default::default(),
            deny_unreachable: false,
            deny_must_use: false,
            edition: 2021,
            path_lts: false,
            deny_unused_alloc: false,
            deny_unused_mut: false,
            allow_overflowing_literals: false,
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
            trait_scopes: vec![],
            item_lines: vec![],
            by_last: FxMap::default(),
            traits: FxMap::default(),
            impls: HashSet::default(),
            panic_handler: None,
            consts: vec![],
            const_index: FxMap::default(),
            derived_peq: FxSet::default(),
            derived: FxMap::default(),
            const_param_tys: FxMap::default(),
            struct_defaults: FxMap::default(),
            copy_params: FxMap::default(),
            has_mods: false,
            trait_clone_fns: false,
            multi_trait_methods: FxSet::default(),
            ambiguous_methods: FxMap::default(),
            unbounded_blankets: FxSet::default(),
            use_names: FxSet::default(),
            impl_selfs: FxMap::default(),
            user_traits: FxSet::default(),
            lower_consts: false,
            const_by_last: FxMap::default(),
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
        // 0. 型の別名 (struct のフィールドの型より先に)
        // (同じ名前の struct / enum があればそちら: ユーザの `struct Path` と prelude の `type Path = str`)
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
        let owned_sums;
        let sums: &[Summary] = if alias_owner.is_empty() {
            sums
        } else {
            owned_sums = sums.iter().map(|sm| rebase_aliased(sm, &alias_owner)).collect::<Vec<_>>();
            &owned_sums
        };
        // (a closure is named after the fn it is in, `Vec::m::{closure#0}`, which leaves out the trait: two traits of
        // one name (in two modules) implemented for Vec, each with a closure in `m`, would share the name. A later
        // one also gets its item's number. It was an error before, so no program that built is renamed)
        let owned_unique;
        let sums: &[Summary] = {
            let mut seen: FxMap<&str, usize> = FxMap::default();
            let dup = sums.iter().enumerate().any(|(ii, sm)| sm.closures.iter().any(|c| *seen.entry(c.name.as_str()).or_insert(ii) != ii));
            if dup {
                let mut first: FxMap<String, usize> = FxMap::default();
                owned_unique = sums
                    .iter()
                    .enumerate()
                    .map(|(ii, sm)| {
                        let mut sm = sm.clone();
                        let mut renames: Vec<(String, String)> = vec![];
                        for c in &mut sm.closures {
                            if *first.entry(c.name.clone()).or_insert(ii) != ii {
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
                        sm
                    })
                    .collect::<Vec<_>>();
                &owned_unique
            } else {
                sums
            }
        };
        // 0. const / static の名前 (配列の長さで使うので型より先に)
        for (ii, sm) in sums.iter().enumerate() {
            let cs = sm
                .consts
                .iter()
                .map(|(n, t, e)| (n, t, e, None))
                .chain(sm.statics.iter().map(|(n, t, e, m)| (n, t, e, Some(*m))));
            for (n, t, e, stat) in cs {
                // (`const _: () = { assert!(..) };` はコンパイル時の検査だけ: 読み飛ばす)
                if anon_const(n) {
                    continue;
                }
                // (`impl Foo { const C }` beside `impl Tr for Foo { const C }`: rustc's `Foo::C` and `<Foo as Tr>::C`.
                // The trait impl's goes by `Foo as Tr::C`, which a qualified path reads)
                let trait_of = |k: usize| match items[k].kind {
                    crate::lexer::ItemKind::ImplMember(h) => sums[h as usize].impl_trait.clone(),
                    _ => None,
                };
                let mut n = n.clone();
                if let Some(&prev) = p.const_index.get(&n) {
                    let pk = p.consts[prev as usize].item as usize;
                    match (trait_of(ii), trait_of(pk)) {
                        // (two of one trait impl's: a real duplicate)
                        (Some(a), Some(b)) if a == b => {
                            return Err(format!("{}:1: duplicate definition of `{n}`", items[ii].line));
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
                        (None, None) => {
                            return Err(format!("{}:1: duplicate definition of `{n}`", items[ii].line));
                        }
                    }
                    if p.const_index.contains_key(&n) {
                        return Err(format!("{}:1: duplicate definition of `{n}`", items[ii].line));
                    }
                }
                let n = &n;
                if let Some((_, last)) = n.rsplit_once("::") {
                    p.const_by_last.entry(last.to_string()).or_default().push(p.consts.len() as u32);
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
        // 1. struct / enum 名を登録
        let mut sdefs: Vec<(usize, String, AdtShape, Vec<String>)> = vec![];
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            let adts = sm
                .structs
                .iter()
                .map(|(n, f)| (n, AdtShape::Struct(f.clone())))
                .chain(sm.enums.iter().map(|(n, v)| (n, AdtShape::Enum(v.clone()))));
            for (name, shape) in adts {
                if p.struct_index.contains_key(name) {
                    return Err(format!("{}:1: duplicate definition of `{name}`", it.line));
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
                if !sm.adt_defaults.is_empty() {
                    p.struct_defaults.insert(sdefs.len() as u32, sm.adt_defaults.clone());
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
        // 2. フィールド型を解決し、依存順にレイアウトを決める
        for (_, n, sh, g) in &sdefs {
            let mut d = StructDef::blank(n.clone(), matches!(sh, AdtShape::Enum(_)));
            d.generics = g.clone();
            p.structs.push(d);
        }
        // (associated types without generic structs, `impl Tr for i32 { type Ty = i32; }`, before the fields: a field
        // or a const may be `<i32 as Tr>::Ty`. The others below, once the definitions are laid out)
        let mut early = vec![];
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
                    let trait_name = impl_trait_name(&items[ii].kind, sums);
                    p.assocs.push(AssocDef { name: a.name.clone(), self_ty, value, ngen, ngat, trait_name });
                    p.note_assoc_regions(items, sums, ii, k);
                    p.assoc_items.push(ii as u32);
                    early.push((ii, k));
                }
            }
        }
        // ジェネリックな定義を先に (フィールドの型を Param で解決するだけ。レイアウトはインスタンスごと)
        let mut state = vec![0u8; sdefs.len()]; // 0 未処理, 1 処理中, 2 済み
        for id in 0..sdefs.len() {
            if !sdefs[id].3.is_empty() {
                p.layout(id, &sdefs, items, &mut state)?;
            }
        }
        // (and those of generic types, `impl<R> Generator for Core<R> { type Output = [u32; 64]; }`, once the generic
        // definitions are there: a field `BlockRng<Core<R8>>` of a non-generic struct, laid out next, has a field of type
        // `G::Output` (rand_core 0.10 with chacha20). Those that do not resolve yet are taken below, as before)
        for (ii, sm) in sums.iter().enumerate() {
            let of_trait = matches!(items[ii].kind, crate::lexer::ItemKind::ImplMember(h) if sums[h as usize].impl_trait.is_some());
            if sm.assocs.is_empty() || !of_trait {
                continue;
            }
            for (k, a) in sm.assocs.iter().enumerate() {
                if early.contains(&(ii, k)) || !syn_has_app(&a.self_ty, &p.aliases) || syn_has_app(&a.value, &p.aliases) {
                    continue;
                }
                let gn: Vec<&str> = a.generics.iter().map(|g| g.as_str()).collect();
                if let (Ok(self_ty), Ok(value)) = (p.resolve_g(&a.self_ty, &gn), p.resolve_g(&a.value, &gn)) {
                    let (ngen, ngat) = gat_split(&gn);
                    let trait_name = impl_trait_name(&items[ii].kind, sums);
                    p.assocs.push(AssocDef { name: a.name.clone(), self_ty, value, ngen, ngat, trait_name });
                    p.note_assoc_regions(items, sums, ii, k);
                    p.assoc_items.push(ii as u32);
                    early.push((ii, k));
                }
            }
        }
        let early: HashSet<(usize, usize)> = early.into_iter().collect();
        for id in 0..sdefs.len() {
            p.layout(id, &sdefs, items, &mut state)?;
        }
        // レイアウトの途中で作ったインスタンス (`Node` の中の `RefCell<Node>`) は、まだ大きさの決まっていない
        // 型で計算されているので、全部決まってから取り直す
        p.relayout_instances();
        for sid in 0..p.structs.len() {
            let sd = &p.structs[sid];
            if !sd.is_enum || sd.base.is_some() || sd.name.starts_with('#') {
                continue;
            }
            for (vi, v) in sd.variants.iter().enumerate() {
                p.variant_names
                    .entry(v.name.clone())
                    .and_modify(|e| *e = None)
                    .or_insert(Some((sid as u32, vi)));
            }
        }
        // deep hash = 値・参照を問わずフィールドから推移的に届く全 struct の定義ハッシュ。
        // 参照経由 (`&'a Other`) でもフィールドオフセットはコードに埋まるので含める。
        let n_defs = sdefs.len();
        // `impl Drop` の有無も、その型を使う関数のコード (drop の呼び出し) を変えるので deep hash に入れる
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
                                // クロージャの struct: レイアウトは型引数 (キャプチャの型) で決まり、本体は
                                // クロージャの fn の側のキーに入るので、親アイテムのハッシュは入れない
                                // (クロージャを 1 つ編集しても、それを受け取る map などのインスタンスは作り直さない)
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
                // (相互再帰の型 (`Ty` ↔ `FnTy`) は届く定義の集合が同じなので、自分の名前も入れて区別する)
                if id < n_defs {
                    h.add(&sdefs[id].1);
                }
                h.finish()
            })
            .collect();
        for (i, d) in deeps.into_iter().enumerate() {
            p.structs.get_mut(i).deep = d;
        }
        // const / static の型
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
        // 関連型 (関数のシグネチャの `Self::Item` を解決するので先に)
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            for (k, a) in sm.assocs.iter().enumerate() {
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
                p.assocs.push(AssocDef {
                    name: a.name.clone(),
                    self_ty,
                    value,
                    ngen,
                    ngat,
                    trait_name: impl_trait_name(&it.kind, sums),
                });
                p.note_assoc_regions(items, sums, ii, k);
                p.assoc_items.push(ii as u32);
            }
        }
        // (the impls of each trait for each type so far: their self types and trait arguments, and parameters)
        let mut self_tys: FxMap<(&str, &str), Vec<usize>> = FxMap::default();
        // 3. 関数
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
                        if ss.impl_trait.is_none() && p.sigs[prev as usize].impl_trait.is_none() {
                            // (in two impl blocks, E0592 is reported at the first definition)
                            let line = match p.item_of.get(prev as usize).copied().flatten().map(|k| &items[k as usize]) {
                                Some(pit) if matches!((pit.kind, it.kind), (crate::lexer::ItemKind::ImplMember(a), crate::lexer::ItemKind::ImplMember(b)) if a != b) => pit.line,
                                _ => it.line,
                            };
                            return Err(format!("{line}:1: duplicate definition of `{name}`"));
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
                let mut params = vec![];
                for t in &ss.params {
                    params.push(
                        p.resolve_g(t, &gnames)
                            .map_err(|e| format!("{}:1: {e}", it.line))?,
                    );
                }
                // (本体から決める `impl Trait` はいったん () にして、後で resolve_opaque が入れる)
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
                        // ブランケット impl (`impl<T: Display> Describe for T`): どの型のメソッドとしても探す
                        // (その中のクロージャを持ち上げた関数は self を持たない: メソッドとしては探さない)
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
                // (同名の trait impl の関数 `P::from#1` も `from` で引く)
                let last = name.rsplit("::").next().unwrap().split('#').next().unwrap().to_string();
                // (`io::stdin()` / `mem::swap(..)` は prelude の `__core_*`: 名前の後ろで引けるようにする)
                if let Some(short) = last.strip_prefix("__core_") {
                    p.by_last.entry(short.to_string()).or_default().push(p.names.len() as u32);
                }
                p.by_last
                    .entry(last)
                    .or_default()
                    .push(p.names.len() as u32);
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
                    regions: ss.regions.as_deref().map(|r| std::sync::Arc::new(r.clone())),
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
                        by_move: c.by_move,
                        moved: c.moved.clone(),
                        n_parent: c.n_parent as usize,
                        ret_ann,
                        fake_reads: c.fake_reads.clone(),
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
        // 4. trait の実装 (impl ヘッダの要約から)
        // (traits with an impl for `&T` of a type parameter T)
        // (the traits of `impl<T> Tr for &T` / `&mut T` (whether mut), and of `impl<T> Tr for T`)
        let mut ref_blankets: HashSet<(String, bool)> = Default::default();
        let mut bare_blankets: HashSet<String> = Default::default();
        // (impls for Box: (trait, line), checked against blanket impls of the same trait after the loop)
        let mut box_impls: Vec<(String, usize)> = vec![];
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
            // (右辺の型を持つ比較の impl と、配列・スライスへの impl は、`==` が関数を直接選ぶので数えない)
            if tr.starts_with("PartialEq<") || tr.starts_with("PartialOrd<") || sn == "__array" || sn == "__slice" || sn.starts_with("__dyn_") {
                // (`impl BufRead for &[u8]`: 境界を満たすかのために覚える)
                if sn == "__slice" && matches!(sm.impl_self_ty, Some(TySyn::Ref(false, _))) && !tr.contains('<') {
                    p.ref_slice_traits.insert(tr.clone());
                }
                // (`impl<'a, T> Tr for &'a [T; 3]` of a trait of the program's: the self type with the impl's
                // parameters, matched against a type for bounds)
                if (sn == "__array" || sn == "__slice") && matches!(sm.impl_self_ty, Some(TySyn::Ref(..))) && !tr.contains('<') && p.traits.contains_key(tr) {
                    let gn: Vec<&str> = sm.impl_generics.iter().map(|g| g.0.as_str()).collect();
                    if let Some(t) = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &gn).ok()) {
                        p.ref_impl_pats.push((t, tr.clone(), gn.len()));
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
                        if sm.impl_trait_lts {
                            p.impl_targs_lts.insert((tr.clone(), st));
                        }
                        p.impl_targs.entry(tr.clone()).or_default().push((st, args, gn.len()));
                    }
                }
            }
            // (`impl From<S> for S` / `impl TryFrom<S> for S`: std's `impl<T> From<T> for T`, and the TryFrom it gives,
            //  already cover it (E0119))
            // (not `impl From<TokenStream> for proc_macro::TokenStream`: a path names another crate's type, proc-macro2)
            if (tr == "From" || tr == "TryFrom") && !sm.impl_self_path && sm.impl_trait_args.len() == 1 && sm.impl_self_ty.as_ref() == Some(&sm.impl_trait_args[0]) {
                return Err(format!("{}:1: conflicting implementations of trait `{tr}<{sn}>` for type `{sn}` (E0119)", it.line));
            }
            // プリミティブ型への trait 実装 (`impl Tr for u32`)
            // (`impl Hash for str`: str も組み込みの型として持つ)
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
                if impl_overlaps(&mut self_tys, sums, ii, sn, tr, dup) {
                    return Err(format!(
                        "{}:1: conflicting implementations of trait `{tr}` for `{sn}`",
                        it.line
                    ));
                }
                continue;
            }
            // (`impl Tr for Box<S>`: a trait of that Box type, as for primitives; `impl<T> Tr for Box<T>` has no single type)
            if sn == "Box" && !p.struct_index.contains_key("Box") {
                box_impls.push((tr.clone(), it.line as usize));
                if let Some(t) = sm.impl_self_ty.as_ref().and_then(|t| p.resolve_g(t, &[]).ok()) {
                    p.prim_impls.insert((t, tr.clone()));
                }
                continue;
            }
            // (`impl<T> Tr for T`, with or without members)
            // (`impl<T: Tr> Tr for &T` / `&mut T` with no functions (only types and consts), and a trait with no default
            // methods: nothing to call, so it counts as a blanket impl. With functions, calls on `&T` would reach T's
            // methods instead: not supported)
            let ref_of_param = matches!(&sm.impl_self_ty, Some(TySyn::Ref(_, i)) if matches!(&**i, TySyn::Named(n) if n == sn))
                && sm.impl_generics.iter().any(|g| g.0 == *sn);
            if ref_of_param {
                let defaults = p.traits.get(tr).is_some_and(|ids| ids.iter().any(|&id| p.sigs[id as usize].has_body));
                if !sm.fns.is_empty() || defaults {
                    return Err(format!("{}:1: impls for references to a type parameter (`impl<T> {tr} for &T`) are not supported", it.line));
                }
                // (`impl<T> Foo for T` covers `&U` too; `&T` and `&mut T` are two types)
                let m = matches!(&sm.impl_self_ty, Some(TySyn::Ref(true, _)));
                if bare_blankets.contains(tr) || !ref_blankets.insert((tr.clone(), m)) {
                    return Err(format!("{}:1: conflicting implementations of trait `{tr}` for type `&{}_` (E0119)", it.line, if m { "mut " } else { "" }));
                }
                p.blanket_selfs.insert(sn.clone());
                p.blanket_traits.insert(tr.clone());
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
            // (参照への impl は trait の実装としては数えない: 演算子だけが `__ref_*` を呼ぶ)
            if matches!(sm.impl_self_ty, Some(TySyn::Ref(..))) {
                continue;
            }
            // (型引数を取る trait は別々の引数で何度も実装できる: `impl From<&str> for P` と `impl From<String> for P`)
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

            if impl_overlaps(&mut self_tys, sums, ii, sn, tr, dup)
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
        // derive は実装扱い (Clone / Copy など。中身は常にビットコピーなので本体は要らない)
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
        // 必須メソッドが実装されているか (derive したものは本体を codegen が作る)
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
            }
        }
        for (sid, tr) in &p.impls {
            if derived.contains(&(*sid, tr.clone())) {
                continue;
            }
            let Some(decls) = p.traits.get(tr) else {
                continue;
            }; // derive した組み込み trait
            for &d in decls {
                let m = p.names[d as usize].rsplit("::").next().unwrap();
                // (Display / Debug のメソッドは `S::<Display>::fmt`)
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
                        if ds.has_self && ms.has_self && kind(&p, ds.params[0]) != kind(&p, ms.params[0]) {
                            return Err(format!("{line}method `{m}` has an incompatible type for trait `{tr}` (E0053)"));
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

    /// 型 t の値を捨てるときに drop glue が要るか (Box / `impl Drop` / それらを含む型)
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

    /// 型 t が trait tr を実装しているか
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

    /// Whether t: tr holds, for a static's bound. For a trait the program declares, one of its impls in the program must
    /// apply: its self type matches t exactly (`impl Tr for G<u8>` is not one for G<u16>, nor `&mut u8`'s for `&u8`;
    /// a blanket `impl<P: B> Tr for P` matches any t) and the impl's bounds on its parameters hold for what they matched
    /// (checked the same way). An impl this cannot read (a where clause, a self type that does not resolve) counts as
    /// applying. Other traits (std's) are asked of `implements`
    pub fn bound_holds(&self, sums: &[Summary], user: usize, t: Ty, tr: &str, depth: u32) -> bool {
        self.bound_holds_in(sums, user, t, tr, depth, &mut vec![])
    }

    /// bound_holds, with the obligations being proved (active): one that comes back around (`impl<T: B> A for T` with
    /// `impl<T: A> B for T`) is no proof, and that candidate fails
    fn bound_holds_in(&self, sums: &[Summary], user: usize, t: Ty, tr: &str, depth: u32, active: &mut Vec<(Ty, String)>) -> bool {
        // (the program's own traits: the first `user` summaries are its items, the rest the prelude's, whose traits
        // (Default ..) builtin types implement without an impl to find)
        if matches!(t, Ty::Param(_)) || self.has_param(t) || !sums[..user.min(sums.len())].iter().any(|s| s.trait_decl.as_deref() == Some(tr)) {
            return self.implements(t, tr);
        }
        // (a trait object is its own trait's: `dyn Tr: Tr`, and the other traits written in it (`dyn Tr + Other`);
        // not any blanket trait `implements` would also answer for it)
        if let Ty::Dyn(d) = t {
            if d.split('+').any(|p| {
                let p = p.trim();
                let name = p.split('<').next().unwrap_or(p).trim();
                name.rsplit("::").next().unwrap_or(name) == tr
            }) {
                return true;
            }
        }
        if active.iter().any(|(at, atr)| *at == t && atr == tr) {
            return false;
        }
        // (a proof deeper than this (types that grow at each step) is not followed: it counts as holding)
        if depth > 16 {
            return true;
        }
        active.push((t, tr.to_string()));
        let r = self.bound_impls(sums, user, t, tr, depth, active);
        active.pop();
        r
    }

    fn bound_impls(&self, sums: &[Summary], user: usize, t: Ty, tr: &str, depth: u32, active: &mut Vec<(Ty, String)>) -> bool {
        let readable = |b: &str| b.chars().all(|c| c.is_alphanumeric() || c == '_');
        for s in sums.iter().filter(|s| s.impl_trait.as_deref() == Some(tr)) {
            let gn: Vec<&str> = s.impl_generics.iter().map(|g| g.0.as_str()).collect();
            let Some(pt) = s.impl_self_ty.as_ref().and_then(|st| self.resolve_g(st, &gn).ok()) else {
                return true;
            };
            let mut sol = vec![None; gn.len()];
            if !self.match_ty(pt, t, &mut sol) {
                continue;
            }
            // (match_ty also lets `&mut` stand for `&` and the like: the impl applies only to its self type itself)
            if let Some(args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() {
                if self.subst(pt, &args) != t {
                    continue;
                }
            }
            // (a where clause this does not read: the impl applies, once its self type matches)
            if !s.where_types.is_empty() {
                return true;
            }
            let bounds_hold = s.impl_generics.iter().zip(&sol).all(|((_, bs), a)| match a {
                Some(a) => bs.iter().all(|b| {
                    let b = b.trim();
                    !readable(b) || self.bound_holds_in(sums, user, *a, b, depth + 1, active)
                }),
                None => true,
            });
            if bounds_hold {
                return true;
            }
        }
        false
    }

    /// The primitive t implements tr by an impl of the program's (prim_impls), not one for a reference to t only
    fn prim_has(&self, t: Ty, tr: &str) -> bool {
        let k = (t, tr.to_string());
        self.prim_impls.contains(&k) && !self.prim_ref_only.contains(&k)
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
        if matches!(t, Ty::Ref(..)) && !self.ref_impls.is_empty() && self.ref_impls.contains(&(t, tr.to_string())) {
            return true;
        }
        if matches!(t, Ty::Ref(..)) && self.ref_impl_pats.iter().any(|(pt, ptr, n)| ptr == tr && self.match_ty(*pt, t, &mut vec![None; *n])) {
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
        // (const ジェネリクスの引数は長さを持つ配列型)
        if tr == CONST_GENERIC || tr.strip_prefix(CONST_GENERIC).is_some_and(|r| r.starts_with(':')) {
            return matches!(t, Ty::Array(Ty::Unit, _));
        }
        // 組み込みの AsRef<str> / AsRef<[T]> (文字列・Vec・配列・スライスとその参照)
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
        // collect の先 (組み込み): String / Vec / HashMap など
        if tr.starts_with("FromIterator") {
            if let Ty::Struct(sid) = t {
                let n = self.structs[self.base_of(sid) as usize].name.as_str();
                if matches!(n, "String" | "Vec" | "HashMap" | "HashSet" | "BTreeMap" | "BTreeSet" | "VecDeque" | "BinaryHeap" | "Option" | "Result") {
                    return true;
                }
            }
        }
        // `Iterator<Item = T>` は trait の名前で見る (関連型の制約は呼び出し側で解く)
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
                // クロージャ: Fn 系と、キャプチャがすべて Copy なら Copy / Clone
                parse_fn_bound(tr).is_some()
                    || matches!(tr, "Fn" | "FnMut" | "FnOnce" | "Sized")
                    || (matches!(tr, "Copy" | "Clone")
                        && self.structs[sid as usize]
                            .fields
                            .iter()
                            .all(|f| self.implements(f.1, tr)))
            }
            // flat_map / flatten の中身: イテレータもよい
            Ty::Struct(_) if tr == "__Flat" && self.implements(t, "Iterator") => true,
            // タプル: 要素がすべて実装していれば (core の impl と同じ)
            Ty::Struct(sid) if self.is_tuple(sid) => {
                if matches!(
                    tr,
                    "Copy" | "Clone" | "Sized" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash" | "Debug" | "Default"
                ) {
                    self.structs[sid as usize].fields.iter().all(|f| self.implements(f.1, tr))
                } else {
                    // ユーザの `impl<A, B> Tr for (A, B)`: its self type matches t, and its bounds hold (as a struct's)
                    self.impls.contains(&(self.base_of(sid), tr.to_string())) && self.impl_applies_noted(t, tr)
                }
            }
            // prelude のコレクションと Option は Default (空 / None。中身が 0 のビット列)
            Ty::Struct(sid)
                if tr == "Default"
                    && matches!(
                        self.structs[self.base_of(sid) as usize].name.as_str(),
                        "Vec" | "String" | "HashMap" | "BinaryHeap" | "Option"
                    ) =>
            {
                true
            }
            // eq / clone メソッドを持つ型 (prelude の String / Vec など)
            Ty::Struct(sid)
                if (tr == "PartialEq" && self.method(sid, "eq").is_some())
                    || (tr == "Clone" && self.method(sid, "clone").is_some()) =>
            {
                true
            }
            // Vec / String は要素しだいで比較できる
            Ty::Struct(sid)
                if matches!(tr, "PartialOrd" | "Ord" | "Eq" | "Hash" | "PartialEq" | "Clone")
                    && matches!(self.structs[self.base_of(sid) as usize].name.as_str(), "Vec" | "String") =>
            {
                self.structs[sid as usize].args.iter().all(|&a| self.implements(a, tr))
            }
            // (an impl for the struct, whose self type t matches and whose bounds t's arguments meet: `impl<T: Bar> Foo
            // for W<T>` is not for W<u8> without `impl Bar for u8`, `impl Foo for W<i32>` is not for W<u8> (issue #2))
            Ty::Struct(sid) => self.impls.contains(&(self.base_of(sid), tr.to_string())) && self.impl_applies_noted(t, tr),
            // (配列・スライス: 比較・Debug・Hash は要素が実装していれば)
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
            }),
            // 関数ポインタ: Fn 系 (引数の数が合うもの) と Copy
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
            // 組み込み型: マーカー trait と比較・既定値など (core が実装しているもの)
            Ty::Ref(true, _) => tr == "Sized",
            Ty::Box(_) => matches!(tr, "Sized" | "Drop"),
            // `&T: Display` / `Debug` / `Hash` は T しだい
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
            bs.iter().all(|b| self.implements(ty, b))
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
            Ty::Ref(_, i) | Ty::Box(i) | Ty::Array(i, _) | Ty::Slice(i) => self.auto_trait(*i, sync, d + 1),
            Ty::Struct(sid) => {
                let b = self.base_of(sid);
                let name = self.structs[b as usize].name.as_str();
                // (a user's `unsafe impl Send for T`)
                if self.impls.contains(&(b, if sync { "Sync" } else { "Send" }.to_string())) {
                    return true;
                }
                match name {
                    "Rc" => return false,
                    "Cell" | "RefCell" | "LazyCell" if sync => return false,
                    // (the prelude's own types hold raw pointers: what they hold decides)
                    _ => {}
                }
                let sd = &self.structs[sid as usize];
                let prelude = self.struct_item.get(b as usize).is_some_and(|&i| i >= self.prelude_from);
                if prelude {
                    return sd.args.iter().all(|&a| self.auto_trait(a, sync, d + 1));
                }
                sd.fields.iter().all(|f| self.auto_trait(f.1, sync, d + 1))
                    && sd.variants.iter().all(|v| v.fields.iter().all(|f| self.auto_trait(f.1, sync, d + 1)))
            }
            _ => true,
        }
    }

    /// プリミティブ型 t のメソッド (trait 実装のもの → なければ trait のデフォルトメソッド)
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
        // (`impl<T: Hash> Hash for [T]` / `for [T; N]` のメソッドは `__slice::m` / `__array::m`)
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
            let p = p.split('<').next().unwrap_or(p).rsplit("::").next().unwrap_or(p);
            if let Some(&id) = self.index.get(&format!("__dyn_{p}::{name}")) {
                return Some(id);
            }
        }
        if let Some(&id) = base.and_then(|g| self.index.get(&format!("{g}::{name}"))) {
            // (`impl Tr for [u8; 1]`, `.. for [u8; 2]` ..: 同じ名前の impl が並ぶなら self の型が合うもの)
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

    /// struct sid に名前 name のメソッドが見つからないとき、実装している trait のデフォルトメソッド
    pub fn default_method(&self, sid: u32, name: &str) -> Option<u32> {
        let r = self.default_method_inner(sid, name);
        // (なければブランケット impl のメソッド)
        r.or_else(|| self.blanket.get(name).copied())
    }

    fn default_method_inner(&self, sid: u32, name: &str) -> Option<u32> {
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
                    && self.names[d as usize].rsplit("::").next() == Some(name)
                {
                    found = Some(d);
                }
            }
        }
        found
    }

    /// TySyn → Ty。generics に含まれる名前は型パラメータになる
    pub fn resolve_g(&self, t: &TySyn, generics: &[&str]) -> R<Ty> {
        self.resolve_with(t, &|n| {
            generics
                .iter()
                .position(|g| *g == n)
                .map(|i| Ty::Param(i as u8))
        })
    }

    /// 名前の解決を names に先に聞いてから struct 名を引く
    fn resolve_with(&self, t: &TySyn, names: &dyn Fn(&str) -> Option<Ty>) -> R<Ty> {
        let r = |t: &TySyn| self.resolve_with(t, names);
        Ok(match t {
            TySyn::Prim(t) => *t,
            TySyn::Ref(m, inner) => Ty::Ref(*m, intern(r(inner)?)),
            TySyn::Ptr(m, inner) => Ty::Ptr(*m, intern(r(inner)?)),
            TySyn::Slice(inner) => Ty::Slice(intern(r(inner)?)),
            TySyn::Str => Ty::Str,
            TySyn::Dyn(s) => Ty::Dyn(intern_str(s)),
            TySyn::Infer => Ty::Param(HOLE),
            // (`M::S<u8>` of a module M reads as a generic associated type `#gat:S` of `(M, u8)`: when M is no type,
            // it is the type S<u8> as before)
            TySyn::Assoc(b, n) if n.starts_with(crate::ast_types::GAT_PREFIX) && matches!(&**b, TySyn::Tuple(p) if !p.is_empty() && r(&p[0]).is_err()) => {
                let TySyn::Tuple(parts) = &**b else { unreachable!() };
                let name = n[crate::ast_types::GAT_PREFIX.len()..].to_string();
                return r(&TySyn::App(name, parts[1..].to_vec()));
            }
            TySyn::Assoc(b, n) => {
                let bt = r(b)?;
                let (n, std_impls) = match n.strip_prefix(crate::ast_types::NO_STD_ASSOC) {
                    Some(x) => (x, false),
                    None => (n.as_str(), true),
                };
                if self.has_param(bt) {
                    Ty::Assoc(intern_assoc(AssocTy {
                        base: bt,
                        name: n.to_string(),
                    }))
                } else {
                    match self.assoc_of_with(bt, n, std_impls) {
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
                    // (const ジェネリクスの N)
                    ArrLen::Const(c) => match names(c) {
                        Some(Ty::Param(i)) => (CONST_LEN_BASE + i as u32) as u64,
                        // (インスタンスの中: N は長さを持つ配列型 / 長さが N の型引数 `[(); N]`)
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
                if let Some((g, t)) = self.aliases.get(n) {
                    if g.is_empty() {
                        return self.resolve_with(t, names);
                    }
                }
                match self.struct_index.get(n) {
                    Some(&id) if self.structs[id as usize].generics.is_empty() => Ty::Struct(id),
                    // (every parameter with a default: `W` for `struct W<T = u8>`, as `W<>`)
                    Some(&id) if self.struct_defaults.get(&id).is_some_and(|ds| ds.iter().all(Option::is_some)) => {
                        return self.resolve_with(&TySyn::App(n.clone(), vec![]), names);
                    }
                    Some(_) => return Err(format!("missing generics for `{n}`")),
                    // (`zeros::<K>()` / `Buf<K>`: const の名前は const ジェネリクスの引数)
                    None if self.const_index.contains_key(n) => Ty::Array(intern(Ty::Unit), self.const_usize(n)? as u32),
                    // (core::arch's vector types, when the program has no type of the name: `v128`, `__m256`)
                    None => match crate::simd_sig::vector_name(n) {
                        Some(t) => t,
                        None => return Err(format!("cannot find type `{n}`")),
                    },
                }
            }
            // `Box<dyn Error>`: prelude の __DynError (表示と Debug の文字列を持つ)
            TySyn::App(n, args)
                if n == "Box"
                    && matches!(args.as_slice(), [TySyn::Dyn(d)] if d == "Error")
                    && self.struct_index.contains_key("__DynError") =>
            {
                Ty::Struct(self.struct_index["__DynError"])
            }
            // `Cow<'a, [T]>` は prelude の `Cow<T>` (借りたスライスか Vec)、`Cow<'a, str>` は `__CowStr`
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
                Ty::Box(intern(r(&args[0])?))
            }
            // 型パラメータを持つ別名 (`type R<T> = Result<T, String>;`)
            TySyn::App(n, args) if self.aliases.get(n).is_some_and(|a| a.0.len() == args.len()) => {
                let (g, t) = self.aliases[n].clone();
                let vals = args.iter().map(r).collect::<R<Vec<Ty>>>()?;
                return self.resolve_with(&t, &|x| match g.iter().position(|y| y == x) {
                    Some(i) => Some(vals[i]),
                    None => names(x),
                });
            }
            TySyn::App(n, args) => {
                let Some(&id) = self.struct_index.get(n) else {
                    return Err(format!("cannot find type `{n}`"));
                };
                // `HashMap<K, V, S>` / `HashSet<K, S>`: ハッシャの型 (BuildHasherDefault<..>) は無視して既定のハッシュで
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

    /// 配列の長さに使う const (要約の const 表から整数の定数式として評価する)
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
        // (const ジェネリクスの引数の式 `{ A - 16 }` は式のテキストで来る)
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

    /// 整数の const を評価する (リテラル・四則演算・ビット演算・シフト・キャスト・他の const)
    pub fn const_int(&self, name: &str, depth: u32) -> R<i128> {
        Ok(self.const_int_t(name, depth)?.0)
    }

    /// The part `pick` takes from the value of the constant expression b (through `&`, `*`, parentheses and other
    /// consts)
    fn const_part(&self, b: &crate::parse::Expr, depth: u32, pick: &dyn Fn(&crate::parse::Expr) -> Option<crate::parse::Expr>) -> R<crate::parse::Expr> {
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
                self.const_part(&e, depth + 1, pick)
            }
            x => pick(x).ok_or_else(|| "unsupported expression in constant".into()),
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

    /// 自前の構文木の整数の定数式を評価する (値だけ: 型は eval_t)
    pub fn eval_const(&self, e: &crate::parse::Expr, depth: u32) -> R<i128> {
        Ok(self.eval_t(e, depth, None)?.0)
    }

    /// e's value as type t (an integer type: unsuffixed literals in e are t's, the value is wrapped to t)
    pub fn eval_const_as(&self, e: &crate::parse::Expr, t: Ty) -> R<i128> {
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
        let (v, t) = self.eval_t(&e, depth + 1, decl)?;
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
                    .or_else(|| t.starts_with("b'").then(|| It { bits: 8, signed: false }));
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
                let c = self.eval_const(c, depth)?;
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
                let fat = |i: &crate::parse::Ty| {
                    matches!(i, crate::parse::Ty::Slice(_) | crate::parse::Ty::Dyn(_))
                        || matches!(i, crate::parse::Ty::Path(None, p) if p.segs.len() == 1 && p.segs[0].name == "str")
                };
                let size = match t {
                    crate::parse::Ty::Ptr(_, i) | crate::parse::Ty::Ref(_, _, i) if fat(i) => Some(if seg.name == "align_of" { 8 } else { 16 }),
                    crate::parse::Ty::Ptr(..) | crate::parse::Ty::Ref(..) => Some(8),
                    crate::parse::Ty::Tuple(v) if v.is_empty() => Some(0),
                    crate::parse::Ty::Path(None, tp) if tp.segs.len() == 1 && tp.segs[0].args.is_empty() => {
                        match self.prim_of_name(&tp.segs[0].name).as_str() {
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
                    None => return Err(format!("cannot evaluate `{}` of this type in a constant", seg.name)),
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
                let (v, t) = self.eval_block_env(&b, &mut env, depth + 1, rt.or(want))?;
                match rt.or(t).or(want) {
                    Some(it) => Ok((it.wrap(v), Some(it))),
                    None => un(v),
                }
            }
            // (`X[0]` / `T.1` of a const array, byte string, tuple or struct)
            Expr::Index(b, i, _) => {
                let i = self.eval_const(i, depth)?;
                let part = self.const_part(b, depth, &|x| match x {
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
                self.eval_t(&part, depth + 1, want.or(et))
            }
            Expr::Field(b, f, _) => {
                let part = self.const_part(b, depth, &|x| match x {
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
                self.eval_t(&part, depth + 1, want.or(ft))
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
            Expr::Unary(op, x, _) => {
                let (v, t) = self.eval_t(x, depth, want)?;
                match (op.as_str(), t) {
                    // (`-` of a typed value: checked; of an unsigned one, an error as rustc's)
                    ("-", Some(it)) => Ok((it.check(v.checked_neg().filter(|_| it.signed || v == 0))?, t)),
                    ("-", None) => un(v.wrapping_neg()),
                    // (`!` of a typed value: its bits only)
                    ("!", Some(it)) => Ok((it.wrap(!v), t)),
                    ("!", None) => un(!v),
                    ("*", _) => Ok((v, t)),
                    _ => Err("not an integer constant".into()),
                }
            }
            Expr::Binary(op, l, r, _) => {
                let op = op.as_str();
                let shift = op == "<<" || op == ">>";
                let cmp = matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||");
                // (the operands are the wanted type, but a comparison's are not its bool, and a shift's right one is
                // its own)
                let ow = if cmp { None } else { want };
                // (`&&` / `||` short-circuit: `true || (u8::MAX + 1 == 0)` is true)
                if matches!(op, "&&" | "||") {
                    let lv = self.eval_const(l, depth)?;
                    if (op == "&&") == (lv == 0) {
                        return un((op == "||") as i128);
                    }
                    return un((self.eval_const(r, depth)? != 0) as i128);
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
                        Ok((v.ok_or("overflow in constant")? as i128, t))
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
                        Ok((it.check(v)?, t))
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
                // (impl の関連 const: `i128::MAX_STR_LEN`)
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
            // 整数のメソッド (`u128::MAX.ilog10()`): 受け手の型の幅・符号で (`(-1i8).count_ones()` は 8)
            Expr::MethodCall(recv, m, _, args, _) => {
                let (v, t) = self.eval_t(recv, depth, None)?;
                let bits = t.map_or(128, |it| it.bits);
                let unsigned = t.is_some_and(|it| !it.signed);
                let u = if bits < 128 { (v as u128) & ((1u128 << bits) - 1) } else { v as u128 };
                let u32t = Some(It { bits: 32, signed: false });
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
                        let p = if bits == 128 && unsigned {
                            (v as u128).checked_pow(*e as u32).ok_or("overflow in constant")? as i128
                        } else {
                            v.checked_pow(*e as u32).ok_or("overflow in constant")?
                        };
                        match t {
                            Some(it) if it.bits < 128 => (it.check(Some(p))?, t),
                            _ => (p, t),
                        }
                    }
                    // (a u128 compares as unsigned: its bit pattern past i128::MAX is negative)
                    ("min", [b]) if bits == 128 && unsigned => ((v as u128).min(*b as u128) as i128, t),
                    ("max", [b]) if bits == 128 && unsigned => ((v as u128).max(*b as u128) as i128, t),
                    ("min", [b]) => (v.min(*b), t),
                    ("max", [b]) => (v.max(*b), t),
                    // (checked at the receiver's width: `i8::MIN.abs()` overflows)
                    ("abs", []) => match t {
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
                    return Some(It { bits: 8, signed: false });
                }
                if t.starts_with('\'') || t.starts_with('"') || t.starts_with('b') {
                    return None;
                }
                ["u128", "i128", "usize", "isize", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8"].iter().find(|x| t.ends_with(*x)).and_then(|x| It::of_name(x))
            }
            Expr::Paren(x) | Expr::Ref(_, x) => self.static_ty(x),
            Expr::Unary(op, x, _) if op == "-" || op == "!" => self.static_ty(x),
            Expr::Cast(_, t) => It::of_name(&self.cast_prim(t)),
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
                (Some(_), "BITS") => Some(It { bits: 32, signed: false }),
                (Some(t), _) => It::of_ty(t),
                _ => None,
            },
            Expr::MethodCall(recv, m, ..) => match m.as_str() {
                "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" | "ilog2" | "ilog10" => Some(It { bits: 32, signed: false }),
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
        Ok((v, if name == "BITS" { Some(It { bits: 32, signed: false }) } else { It::of_ty(t) }))
    }

    /// n 要素のタプルの (組み込みの) ジェネリック定義
    /// The placeholder type for type parameter k (named `name`, bounded by the traits `bounds`) of fn id: a struct
    /// with no fields, named `name#` (no program's type has that name), that implements just those traits
    pub fn placeholder(&self, id: u32, k: u32, name: &str, bounds: &[String]) -> u32 {
        if let Some(&s) = self.structs.placeholders.lock().unwrap().get(&(id, k)) {
            return s;
        }
        let mut d = StructDef::blank(format!("{name}#"), false);
        d.placeholder = Some(bounds.to_vec());
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

    /// 型が prelude とプリミティブの型だけでできているか (ユーザ・依存するクレートの struct / enum を含まない)
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

    /// インスタンスなら元の定義
    pub fn base_of(&self, sid: u32) -> u32 {
        self.structs[sid as usize].base.unwrap_or(sid)
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
            // (ジェネリックでない定義も、フィールドのインスタンスの大きさが変わっていれば取り直す:
            //  `enum ItemKind { Const(String, Option<Ty>, ..) }` の Option<Ty> が Ty より先に作られた場合)
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
        // 自分を (Box などの中に) 含む型 (`Node<T> { next: Option<Box<Node<T>>> }`): 作っている途中の
        // インスタンスがまた要求されたら、先に番号だけ取って返す (中身は外側が作り終えてから入れる)
        let key = (base, args.clone());
        // (型引数が入れ子に増えていく再帰 (`struct A<T> { y: A<A<T>> }`) や、自分を含むクロージャは止まらない:
        // rustc の再帰の上限と同じく、深すぎればエラー)
        if INSTANTIATING.with(|v| v.borrow().len()) > 64 {
            return Err(format!("reached the recursion limit while instantiating `{}`", self.structs[base as usize].name));
        }
        let pending = INSTANTIATING.with(|v| {
            let mut v = v.borrow_mut();
            match v.iter_mut().find(|e| e.0 == key) {
                Some(e) => Some(*e.1.get_or_insert_with(|| {
                    let b = &self.structs[base as usize];
                    let mut d = StructDef::blank(self.inst_name(&b.name, &args), b.is_enum);
                    d.base = Some(base);
                    d.args = args.clone();
                    d.symbolic = args.iter().any(|&t| self.has_param(t));
                    self.structs.push(d)
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
        let id = match placeholder {
            Some(pid) => {
                self.structs.replace(pid, d);
                pid
            }
            None => self.structs.push(d),
        };
        g.insert(key, id);
        Ok(id)
    }

    /// インスタンスの名前 `Name<A, B>` (タプルは `(A, B)`)。入れ子のクロージャの型 (`dup(dup(..))`) は名前が段ごとに倍に
    /// なるので、長いもの (512 byte を超える) は先頭と全体のハッシュにする (名前はシンボルにも使うので、ハッシュで区別を残す)
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

    /// フィールドを宣言順に自然なアラインメントで並べる (repr(C) 相当)
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
            // (まだレイアウトの決まっていない型 (作っている途中の再帰する型) は揃えが 0: 揃えは 1 以上)
            let al = al.max(1).min(pack);
            off = off.next_multiple_of(al);
            out.push((n, t, off));
            off += sz;
            align = align.max(al);
        }
        (out, off.next_multiple_of(align), align)
    }

    /// タグ (判別値) は先頭、ペイロードは全バリアントで共有する領域に置く
    fn enum_layout(
        &self,
        name: &str,
        vs: Vec<(String, VariantKind, Vec<(String, Ty)>, i64)>,
        tag: Ty,
    ) -> (Vec<VariantDef>, u32, u32) {
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

    /// 型の中に型パラメータが残っているか
    pub fn has_param(&self, t: Ty) -> bool {
        match t {
            Ty::Param(_) => true,
            Ty::Array(_, n) if n >= CONST_LEN_BASE => true,
            Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => {
                self.has_param(*i)
            }
            Ty::Struct(sid) => self.structs[sid as usize].symbolic,
            Ty::Fn(f) => f.params.iter().chain([&f.ret]).any(|&a| self.has_param(a)),
            // 関連型は具体型ではない (型パラメータを含むときだけ現れる)
            Ty::Assoc(_) => true,
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
                if a.trait_name.is_some() && a.ngat == 0 {
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
            if a.trait_name.as_deref() != Some(tr) {
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
        let t = self.resolve_g(syn, names).ok()?;
        if let Ty::Assoc(a) = t {
            let base = self.subst(a.base, args);
            if self.has_param(base) {
                return None;
            }
            let mut traits: Vec<String> = if !qtr.is_empty() {
                vec![qtr.to_string()]
            } else {
                let Ty::Param(i) = a.base else { return None };
                sig.generics.get(i as usize)?.1.iter().map(|b| b.split('<').next().unwrap_or(b).trim().to_string()).collect()
            };
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
            return found;
        }
        if self.mentions_assoc(t) {
            return None;
        }
        let t = self.subst(t, args);
        (!self.has_param(t)).then_some(t)
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

    /// 具体型 t の関連型 name (`impl Iterator for Counter { type Item = u32; }` なら u32)
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

    /// assoc_of; with std_impls false, not std's impls the prelude does not write out (`<i32 as Local>::Output`)
    pub fn assoc_of_with(&self, t: Ty, name: &str, std_impls: bool) -> Option<Ty> {
        thread_local! {
            static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        }
        // (`impl<F: Future> IntoFuture for F { type Item = F::Item; }` refers to another trait's `Item` of the same
        // type: a cycle through the blanket impl. Bounded, and such a definition is not used to answer its own name)
        if DEPTH.with(|d| d.get()) > 32 {
            return None;
        }
        DEPTH.with(|d| d.set(d.get() + 1));
        let r = self.assoc_of0(t, name, std_impls);
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
        for a in &self.assocs {
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
            if self.match_ty(a.self_ty, t, &mut sol) {
                let Some(mut args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
                    continue;
                };
                args.extend(std::iter::repeat_n(Ty::Unit, if a.ngat > 0 { a.ngat + 1 } else { 0 }));
                return Some(self.subst(a.value, &args));
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
            _ => false,
        }
    }

    /// 型パラメータに型引数を入れる
    pub fn subst(&self, t: Ty, args: &[Ty]) -> Ty {
        match t {
            // (引数のない型パラメータ (`_` の印 HOLE など) はそのまま: 型推論・検査が後で扱う)
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

    /// 分かっている型パラメータだけ置き換える (分からないものは Param のまま)
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

    /// 仮引数型 pt (型パラメータを含む) と実引数型 at を突き合わせて型パラメータを解く
    pub fn match_ty(&self, pt: Ty, at: Ty, sol: &mut [Option<Ty>]) -> bool {
        match (pt, at) {
            // 関連型からは解けない (具体型が分かってから置き換わる)
            (Ty::Assoc(_), _) => true,
            (Ty::Param(i), t) => match sol.get(i as usize).copied().flatten() {
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
            | (Ty::Box(a), Ty::Box(b)) => self.match_ty(*a, *b, sol),
            // (const ジェネリクスの長さ: 長さを持つ Ty::Array(Unit, m) で解く)
            (Ty::Array(a, n), Ty::Array(b, m)) if n >= CONST_LEN_BASE && m < CONST_LEN_BASE => {
                self.match_ty(Ty::Param((n - CONST_LEN_BASE) as u8), Ty::Array(intern(Ty::Unit), m), sol) && self.match_ty(*a, *b, sol)
            }
            (Ty::Array(a, n), Ty::Array(b, m)) => n == m && self.match_ty(*a, *b, sol),
            (Ty::Fn(a), Ty::Fn(b)) if a != b => {
                a.params.len() == b.params.len()
                    && a.params
                        .iter()
                        .zip(&b.params)
                        .chain([(&a.ret, &b.ret)])
                        .all(|(&p, &q)| self.match_ty(p, q, sol))
            }
            (Ty::Struct(a), Ty::Struct(b)) if a != b => {
                let (sa, sb) = (&self.structs[a as usize], &self.structs[b as usize]);
                match (sa.base, sb.base) {
                    (Some(x), Some(y)) if x == y => sa
                        .args
                        .iter()
                        .zip(&sb.args)
                        .all(|(&p, &q)| self.match_ty(p, q, sol)),
                    _ => false,
                }
            }
            (a, b) => a == b,
        }
    }

    /// Ty → TySyn (キャッシュにインスタンスを名前で書くため)
    pub fn ty_syn(&self, t: Ty) -> TySyn {
        match t {
            Ty::Ref(m, i) => TySyn::Ref(m, Box::new(self.ty_syn(*i))),
            Ty::Ptr(m, i) => TySyn::Ptr(m, Box::new(self.ty_syn(*i))),
            Ty::Slice(i) => TySyn::Slice(Box::new(self.ty_syn(*i))),
            Ty::Array(i, n) => TySyn::Array(Box::new(self.ty_syn(*i)), ArrLen::Lit(n as u64)),
            Ty::Str => TySyn::Str,
            Ty::Dyn(s) => TySyn::Dyn(s.to_string()),
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
                return Err(format!("{}:1: recursive type `{}` has infinite size", items[sdefs[id].0].line, sdefs[id].1));
            }
            _ => {}
        }
        state[id] = 1;
        let (ii, _, shape, generics) = &sdefs[id];
        let line = items[*ii].line;
        let gnames: Vec<&str> = generics.iter().map(|s| s.as_str()).collect();
        // 値として含む型 (参照・ポインタの先は除く) を先にレイアウトしておく
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
                // `Box<T>` はポインタなので T のレイアウトは要らない
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
                    // 型引数のうち、その型が値として持つもの (`Vec<T>` の T はポインタの先なので要らない)
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

    /// struct `sid` のメソッド `name`
    pub fn method(&self, sid: u32, name: &str) -> Option<u32> {
        self.index
            .get(&format!(
                "{}::{name}",
                self.structs[self.base_of(sid) as usize].name
            ))
            .copied()
    }

    pub fn size_align(&self, t: Ty) -> (u32, u32) {
        match t {
            Ty::Struct(id) => {
                let s = &self.structs[id as usize];
                (s.size, s.align)
            }
            Ty::Array(e, n) => {
                // (const ジェネリクスの長さの仮の値 (CONST_LEN_BASE + i) は大きいので折り返す: 使われない)
                let (s, a) = self.size_align(*e);
                (s.wrapping_mul(n), a)
            }
            t if t.is_fat() => (16, 8),
            Ty::Unit | Ty::Never | Ty::Slice(_) | Ty::Str | Ty::Dyn(_) | Ty::Param(_) => (0, 1),
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

    /// エラーメッセージ用の型名
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
            Ty::Dyn(s) => format!("dyn {}", s.replace(" (", "(").replace("( ", "(").replace(" )", ")")),
            Ty::Box(i) => format!("Box<{}>", self.tn(*i)),
            Ty::Assoc(a) => format!("<{}>::{}", self.tn(a.base), a.name),
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

    /// キャッシュキー用: 型を (struct は deep hash で) ハッシュする
    pub fn hash_ty(&self, t: Ty, h: &mut KeyHasher) {
        match t {
            Ty::Ref(m, i) => {
                h.add(&(100u8, m));
                self.hash_ty(*i, h);
            }
            Ty::Struct(id) => {
                let sd = &self.structs[id as usize];
                match sd.base {
                    // インスタンス: 元の定義の deep hash + 型引数
                    // (クロージャの struct は同じアイテムに複数あるので名前も入れる)
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
            // vtable の並び (trait のメソッドの順) もコードに埋まる
            Ty::Dyn(s) => {
                h.add(&(108u8, s));
                for &f in self.traits.get(s).map(|v| &v[..]).unwrap_or(&[]) {
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
        // 大半の関数では空のフィールドは、空でないときだけ入れる (no-op ビルドで全関数分ハッシュするため)
        if !s.generics.is_empty() || s.trait_of.is_some() || !s.has_body || s.impl_trait.is_some() {
            h.add(&(&s.generics, &s.trait_of, s.has_body, &s.impl_trait));
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

    /// アイテム内の識別子のうち関数名に一致するもの (ソート済み) と struct 名に一致するもの。
    /// ローカル変数による shadowing は無視した過大近似だが、
    /// キャッシュキーと到達可能性の両方にとって安全側である。
    /// (関数, struct / enum, const / static) の参照。識別子を 1 回だけ走査する
    pub fn refs(&self, src: &str, it: &SrcItem) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        self.refs_in(src, it, None)
    }

    /// refs で引く名前 (どれかの表にある名前) の集合。多くの識別子 (変数・キーワード) は 1 回引くだけで済む
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

    /// names: ref_names (あれば、そこにない識別子は飛ばす)
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
            // `for` はイテレータなら名前を書かずに next を呼ぶ
            let s = if s == "for" { "next" } else { s };
            let mut hit = false;
            if let Some(ids) = self.by_last.get(s) {
                fns.extend_from_slice(ids);
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

    /// main から到達可能な関数 ID。
    /// 到達しない関数は parse も型検査もコード生成もしない (relax/blaze の demand-driven の AOT 版)。
    pub fn reachable(&self, refs: &[Vec<u32>]) -> Vec<bool> {
        let mut seen = vec![false; self.names.len()];
        // 根: main・外に見せる関数・panic handler (panic はどこからでも起こりうる)
        let mut stack: Vec<u32> = self.index.get("main").copied().into_iter().collect();
        stack.extend(self.index.get(TEST_ENTRY).copied());
        stack.extend((0..self.names.len() as u32).filter(|&i| self.sigs[i as usize].export));
        stack.extend(self.panic_handler);
        // `Box::new` が呼ぶアロケータと drop が呼ぶ free (prelude にあるのは Box / Drop が使われたときだけ)
        stack.extend(self.index.get(crate::prelude::BOX_ALLOC).copied());
        stack.extend(self.index.get(crate::prelude::BOX_FREE).copied());
        // `vec![..]` / `"..".to_string()` は名前を書かずに Vec の realloc / String::from を使う
        stack.extend(self.index.get("__exr_realloc").copied());
        stack.extend(self.index.get("String::from").copied());
        // (`process::exit` は名前を書かずに呼ばれる: あれば根にする。使われなければ要約で選ばれない)
        stack.extend(self.index.get("__core_process_exit").copied());
        // (`fs::read_to_string` / `env::args` などはパスで引くので、名前からは届かない)
        for n in [
            "__exr_fs_read", "__exr_fs_read_to_string", "__exr_fs_write", "__exr_env_args", "__exr_env_var",
            "__exr_env_var_os", "__exr_env_split_paths", "__exr_fs_remove_file", "__exr_path_new", "__exr_ptr_eq",
        ] {
            stack.extend(self.index.get(n).copied());
        }
        // const の式から呼ぶ関数 (`const N: u32 = sq(4);`。const は使う場所で式として展開される)
        for c in &self.consts {
            if c.expr.contains('(') {
                for w in c.expr.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')) {
                    stack.extend(self.index.get(w).copied());
                }
            }
        }

        // `impl Drop` の drop は drop glue から、メソッド `eq` は `==` から (名前を書かずに) 呼ばれる
        stack.extend((0..self.names.len() as u32).filter(|&i| {
            let s = &self.sigs[i as usize];
            // (書式化の部品と Display / Debug の impl も、codegen が名前を書かずに呼ぶ)
            matches!(s.impl_trait.as_deref(), Some("Drop"))
                || (s.has_self && self.names[i as usize].ends_with("::eq"))
                // (`write!(w, ..)` は codegen が w.write_str(..) を呼ぶ)
                || (s.has_self && self.names[i as usize].ends_with("::write_str"))
                // (libm などの prelude の extern は codegen が名前を書かずに呼ぶ。宣言だけなのでコードは増えない)
                || (s.is_extern && self.names[i as usize].starts_with("__exr_"))
                // (演算子 `a + b` は codegen が `a.add(b)` にする)
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
                // (`<` と sort は比較の glue から partial_cmp / cmp を呼ぶ)
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
            stack.extend(refs[i as usize].iter().copied());
        }
        seen
    }
}

/// 整数・bool・文字・バイトのリテラル (`0x1F_u8` / `true` / `'a'` / `b'a'`)
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
fn impl_overlaps<'a>(seen: &mut FxMap<(&'a str, &'a str), Vec<usize>>, sums: &[Summary], ii: usize, sn: &'a str, tr: &'a str, dup: bool) -> bool {
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
            syn_unify(&u, &g, &t, &gs, &mut vec![])
        })
    };
    list.push(ii);
    r
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
            if sized && matches!(b, TySyn::Slice(_) | TySyn::Str | TySyn::Dyn(_)) {
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
    let idx: Vec<usize> = (r.outer as usize..r.lts.len()).filter(|&i| in_bounds(&r.lts[i]) || r.outlives.iter().any(|&(a, b)| a as usize == i || b as usize == i)).collect();
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

/// 型 t が型パラメータ g を値として含むか (参照・ポインタ・Box の先は含まない。ほかの型の引数は含むとみなす)
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

/// パスの 1 セグメント (名前と、型引数 `<..>` / `(..)` があるか)。syn のパスも自前の構文木のパスもこれにして引く
#[derive(Clone, Debug, PartialEq)]
pub struct PSeg {
    pub name: String,
    pub args: bool,
}

/// 名前のパス (`::a::b` の先頭の `::` と、セグメント)
#[derive(Clone, Debug, PartialEq)]
pub struct PPath {
    pub global: bool,
    pub segs: Vec<PSeg>,
}

impl PPath {
    /// 自前の構文木のパス
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

// --- パスで struct / enum のバリアント / 関数を引く (front.rs の syn 版はこれを呼ぶ) ---
impl Program {
    /// 先頭のモジュールの部分 (`crate::` / `self::` / `super::` / 小文字のモジュール名) を外したパス
    pub fn strip_mods_p(&self, p: &PPath) -> Option<PPath> {
        let mut k = 0;
        let n = p.segs.len();
        while k + 1 < n {
            let s = p.segs[k].name.as_str();
            let module = matches!(s, "crate" | "self" | "super")
                || (s.starts_with(|c: char| c.is_ascii_lowercase())
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
        // `core::option::Option::Some` / `alloc::borrow::Cow::Borrowed` などは最後の 2 つで見る
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
                // `use Expr::*;` したバリアント (大文字で始まり、ほかの enum と名前が重ならないもの)
                // (同じ名前の struct があればそちら: `TokenTree::Punct(Punct { .. })`)
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
        // `std::cmp::Reverse` / `core::cmp::Ordering` などは最後のセグメント (prelude の型)
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
        // 最後のセグメントの turbofish (`f::<T>`) は呼び出し側で読む
        let n = p.segs.len();
        // (`Buf::<8>::new`: 型のセグメントの型引数も呼び出し側で読む)
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
        // `std::collections::HashMap::new` などは `HashMap::new`
        if n >= 3 && matches!(name(0), "core" | "std" | "alloc") {
            let ty = name(n - 2);
            // (`std::cell::Ref::map`: below, as `Ref::map`)
            if self.struct_index.contains_key(ty) && !matches!(ty, "Ref" | "RefMut") {
                return self.index.get(&format!("{ty}::{}", name(n - 1))).copied();
            }
        }
        // `std::process::exit` / `process::exit` (ユーザの `exit` と名前がぶつからないように別の名前)
        // `fs::read_to_string` / `fs::write` / `env::args` などは prelude の `__core_*`
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
                ("ptr", "eq") => Some("__exr_ptr_eq"),
                ("array", "from_fn") => Some("__exr_array_from_fn"),
                // (RefCell's guards: associated functions of the prelude's Ref / RefMut, unless the program has its own)
                ("Ref", "map") if !self.index.contains_key("Ref::map") => Some("__exr_ref_map"),
                ("RefMut", "map") if !self.index.contains_key("RefMut::map") => Some("__exr_refmut_map"),
                ("Ref", "clone") if !self.index.contains_key("Ref::clone") => Some("__exr_ref_clone"),
                _ => None,
            };
            if let Some(c) = core {
                return self.index.get(c).copied();
            }
        }
        if n >= 2 && name(n - 2) == "process" && name(n - 1) == "exit" {
            return self.index.get("__core_process_exit").copied();
        }
        // `core::cmp::max` / `core::mem::swap` などは prelude の `__core_*` へ
        if n >= 2 && matches!(name(0), "core" | "std") {
            return self.index.get(&format!("__core_{}", name(n - 1))).copied();
        }
        match n {
            1 => {
                let f = name(0);
                // `use core::cmp::max;` してからの `max(..)`: ユーザの関数がなければ core のもの
                self.index.get(f).or_else(|| self.index.get(&format!("__core_{f}"))).copied()
            }
            2 => {
                let (ty, m) = (name(0), name(1));
                // `f64::max` / `f64::sqrt` (関数として): prelude の `__exr_f64_*`、`i32::max` は core::cmp::max
                if ty == "f64" || ty == "f32" {
                    return self.index.get(&format!("__exr_{ty}_{m}")).copied();
                }
                if Ty::from_suffix(ty).is_some() && matches!(m, "max" | "min") {
                    return self.index.get(&format!("__core_{m}")).copied();
                }
                // `char::from_digit(d, 10)` / `char::from_u32(x)` / `char::from(b)`: prelude の `__exr_char_*`
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

/// 型の環境で name が const ジェネリクスの引数 (`[(); n]`) なら、その値
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
