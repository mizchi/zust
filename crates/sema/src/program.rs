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

thread_local! {
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
    /// the program's source declares modules (`mod m { .. }`), whose scopes are flattened away: a name's item may then
    /// be out of scope where the name is written
    pub has_mods: bool,
    /// a constant with a lowercase name exists (`const or: usize`): an identifier pattern of any case may name one;
    /// without, only an uppercase one is looked up
    pub lower_consts: bool,
    /// `#![deny(unused_must_use)]`: a statement whose value is a `Result` is an error
    pub deny_must_use: bool,
    /// `#![deny(unused_allocation)]`: `Box::new(x).m()` with m of `&self` on what the box holds is an error
    pub deny_unused_alloc: bool,
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
    /// the generics a method of the trait starts with: Self and the trait's own parameters
    pub trait_ngen: FxMap<String, usize>,
    /// a generic trait's impls: trait → (self type, the trait's arguments, both with the impl's parameters as
    /// `Param(i)`, and their number) (`impl Getter<isize> for isize`: a default method's T for an isize receiver)
    pub impl_targs: FxMap<String, Vec<(Ty, Vec<Ty>, usize)>>,
    /// (型, trait) の実装
    pub impls: HashSet<(u32, String)>,
    /// `#[panic_handler]` の関数 (no_std)。panic (範囲外アクセス・unwrap など) はここを呼ぶ
    pub panic_handler: Option<u32>,
    /// const と static
    pub consts: Vec<ConstDef>,
    pub const_index: FxMap<String, u32>,
    /// 関連定数 (`S::N`) を最後の名前 (`N`) で引く (参照関係の過大近似用)
    pub const_by_last: FxMap<String, Vec<u32>>,
    /// プリミティブ型への trait 実装 (型, trait)
    pub prim_impls: HashSet<(Ty, String)>,
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
                if !prog.implements(t, tr) {
                    return Err(format!(
                        "the trait bound `{}: {tr}` is not satisfied (for `{pn}`)",
                        prog.tn(t)
                    ));
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
            overflow_checks: false,
            trait_supers: FxMap::default(),
            trait_ngen: FxMap::default(),
            impl_targs: FxMap::default(),
            ref_impls: Default::default(),
            ref_impl_pats: vec![],
            fnptr_impls: Default::default(),
            deny_unreachable: false,
            deny_must_use: false,
            deny_unused_alloc: false,
            deny_trivial_casts: (false, false),
            must_use_fns: vec![],
            prim_mut_methods: HashSet::default(),
            opt: false,
            blanket: FxMap::default(),
            blanket_selfs: Default::default(),
            blanket_traits: Default::default(),
            unions: Default::default(),
            reprs: Default::default(),
            packed: Default::default(),
            min_align: Default::default(),
            repr_c: Default::default(),
            const_fns: Default::default(),
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
            has_mods: false,
            lower_consts: false,
            const_by_last: FxMap::default(),
            prim_impls: HashSet::default(),
            ref_slice_traits: HashSet::default(),
            closures: FxMap::default(),
            closure_by: FxMap::default(),
            closure_of_struct: FxMap::default(),
            inst_keys: Default::default(),
            drop_impls: Default::default(),
            deref_to_seq: Default::default(),
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
            for (n, _, e, stat) in cs {
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
                p.consts.push(ConstDef {
                    name: n.clone(),
                    ty: Ty::Unit,
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
                let builtin = Ty::prim(sn).is_some() || sn == "str" || sn == "Box" || sn.starts_with("#tuple") || sn.starts_with("__");
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
                    p.assocs.push(AssocDef { name: a.name.clone(), self_ty, value, ngen, ngat });
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
        let mut next_early = 0;
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            for (k, a) in sm.assocs.iter().enumerate() {
                // (`early` is in the same order as this loop)
                if early.get(next_early) == Some(&(ii, k)) {
                    next_early += 1;
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
                });
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
                    },
                );
                p.closure_by.insert((first, k as u32), fid);
                p.closure_of_struct.insert(sid, fid);
            }
            if let Some(tr) = &sm.trait_decl {
                p.traits.entry(tr.clone()).or_default();
                if let Some((_, sup)) = sm.trait_generics.iter().find(|g| g.0 == "#supers") {
                    p.trait_supers.insert(tr.clone(), sup.clone());
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
        let mut ref_blankets: HashSet<String> = Default::default();
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
                        p.impl_targs.entry(tr.clone()).or_default().push((st, args, gn.len()));
                    }
                }
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
            if let Some(pt) = Ty::prim(sn).or((sn == "str").then_some(Ty::Str)).or((sn == "#tuple0").then_some(Ty::Unit)) {
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
                // (`impl<T> Foo for T` covers `&U` too)
                if p.blanket_traits.contains(tr) {
                    return Err(format!("{}:1: conflicting implementations of trait `{tr}` for type `&_` (E0119)", it.line));
                }
                ref_blankets.insert(tr.clone());
                p.blanket_selfs.insert(sn.clone());
                p.blanket_traits.insert(tr.clone());
                continue;
            }
            if sm.impl_generics.iter().any(|g| g.0 == *sn) && matches!(sm.impl_self_ty, Some(TySyn::Named(_))) {
                // (`impl<T> Tr for T` with no bound and no where clause: every other impl of Tr overlaps it, E0119)
                if ii < user && sm.impl_generics.iter().any(|g| g.0 == *sn && g.1.is_empty()) && sm.where_types.is_empty() && sm.impl_trait_args.is_empty() {
                    unbounded_blankets.push((tr.clone(), it.line));
                }
                if ref_blankets.contains(tr) {
                    return Err(format!("{}:1: conflicting implementations of trait `{tr}` for type `&_` (E0119)", it.line));
                }
                p.blanket_selfs.insert(sn.clone());
                p.blanket_traits.insert(tr.clone());
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
        if let Some((tr, line)) = box_impls.iter().find(|(tr, _)| p.blanket_traits.contains(tr)) {
            return Err(format!("{line}:1: conflicting implementations of trait `{tr}` for type `Box<_>`"));
        }
        // derive は実装扱い (Clone / Copy など。中身は常にビットコピーなので本体は要らない)
        for sm in sums {
            for (tn, ds) in &sm.derives {
                if let Some(&sid) = p.struct_index.get(tn) {
                    for d in ds {
                        p.impls.insert((sid, d.clone()));
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
                        {
                            // (a lifetime a type parameter's bound names (`T: Test<'p>`) is early-bound too)
                            let early = |r: Option<&crate::types::RSigSyn>, g: &[(String, Vec<String>)]| -> Vec<usize> {
                                let Some(r) = r else { return vec![] };
                                let in_bounds = |l: &str| {
                                    g.iter().flat_map(|x| x.1.iter()).any(|b| b.match_indices(l).any(|(k, _)| !b[k + l.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')))
                                };
                                (r.outer as usize..r.lts.len())
                                    .map(|i| r.outlives.iter().filter(|&&(a, b)| a as usize == i || b as usize == i).count() + in_bounds(&r.lts[i]) as usize)
                                    .filter(|&n| n > 0)
                                    .collect()
                            };
                            let (de, me) = (early(ds.regions.as_deref(), &ds.generics), early(ms.regions.as_deref(), &ms.generics));
                            if de != me {
                                return Err(format!("{line}lifetime parameters or bounds on method `{m}` do not match the trait declaration (E0195)"));
                            }
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
                let bare = m.strip_prefix("__ref_").unwrap_or(m);
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
        if need_main && !p.index.contains_key("main") {
            return Err("no `main` function".into());
        }
        p.deref_to_seq = p
            .names
            .iter()
            .enumerate()
            .filter(|(i, n)| n.ends_with("::deref") && matches!(p.sigs[*i].ret, Ty::Ref(_, Ty::Array(..) | Ty::Slice(_) | Ty::Str)))
            .filter_map(|(_, n)| p.struct_index.get(n.trim_end_matches("::deref")).copied())
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

    pub fn implements(&self, t: Ty, tr: &str) -> bool {
        if self.blanket_traits.contains(tr) {
            return true;
        }
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
        // (`Box<T>` is Clone / PartialEq / Eq / PartialOrd / Ord / Debug / Display / Hash / Default when T is)
        if let Ty::Box(i) = t {
            if matches!(tr, "Clone" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Display" | "Hash" | "Default") {
                return self.implements(*i, tr);
            }
        }
        // (const ジェネリクスの引数は長さを持つ配列型)
        if tr == CONST_GENERIC {
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
                return self.implements(t, tr[..i].trim());
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
                    // ユーザの `impl<A, B> Tr for (A, B)`
                    self.impls.contains(&(self.base_of(sid), tr.to_string()))
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
            Ty::Struct(sid) => self.impls.contains(&(self.base_of(sid), tr.to_string())),
            // (配列・スライス: 比較・Debug・Hash は要素が実装していれば)
            Ty::Array(e, _) => {
                tr == "Sized"
                    || (matches!(tr, "Copy" | "Clone" | "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Debug" | "Hash" | "Default")
                        && self.implements(*e, tr))
            }
            Ty::Str => {
                matches!(tr, "Display" | "Debug" | "PartialEq" | "Eq" | "Hash" | "PartialOrd" | "Ord")
                    || self.prim_impls.contains(&(Ty::Str, tr.to_string()))
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
            ) || NUM_OPS.contains(&tr) || matches!(tr, "FromStr" | "ToString") || self.prim_impls.contains(&(t, tr.to_string())),
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
                ) || self.prim_impls.contains(&(t, tr.to_string()))
            }
            _ => matches!(tr, "Copy" | "Clone" | "Sized") || self.prim_impls.contains(&(t, tr.to_string())),
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
                    "Cell" | "RefCell" if sync => return false,
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
        if let Some(&id) = self.index.get(&format!("{}::{name}", self.tn(t))) {
            return Some(id);
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
                if self.has_param(bt) {
                    Ty::Assoc(intern_assoc(AssocTy {
                        base: bt,
                        name: n.clone(),
                    }))
                } else {
                    match self.assoc_of(bt, n) {
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
        if depth > 64 {
            return Err(format!("cycle while evaluating constant `{name}`"));
        }
        let Some(&ci) = self.const_index.get(name) else {
            return Err(format!("cannot find constant `{name}`"));
        };
        let e = crate::parse::parse_expr(&self.consts[ci as usize].expr)?;
        self.eval_const(&e, depth + 1)
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

    /// A `const fn` body of `let x = ..;` statements and a tail expression, with the locals in env
    fn eval_block_env(&self, b: &crate::parse::Block, env: &mut Vec<(String, i128)>, depth: u32) -> R<i128> {
        use crate::parse::{Pat, Stmt};
        let n = b.stmts.len();
        for (k, s) in b.stmts.iter().enumerate() {
            match s {
                Stmt::Let(Pat::Ident(false, _, name, None), _, Some(init), None, ..) => {
                    let v = self.eval_const(&subst_consts(init, env), depth)?;
                    env.push((name.clone(), v));
                }
                Stmt::Expr(x, false) if k + 1 == n => return self.eval_const(&subst_consts(x, env), depth),
                // (`x = 3;` to a local: its new value)
                Stmt::Expr(crate::parse::Expr::Assign(l, r), _) => {
                    let crate::parse::Expr::Path(None, p) = &**l else { return Err("unsupported statement in a const fn".into()) };
                    let Some(name) = (p.segs.len() == 1).then(|| p.segs[0].name.clone()) else {
                        return Err("unsupported statement in a const fn".into());
                    };
                    let v = self.eval_const(&subst_consts(r, env), depth)?;
                    match env.iter_mut().rev().find(|(n, _)| *n == name) {
                        Some(slot) => slot.1 = v,
                        None => return Err("unsupported statement in a const fn".into()),
                    }
                }
                _ => return Err("unsupported statement in a const fn".into()),
            }
        }
        Err("a const fn without a value".into())
    }

    /// The name of a cast's target type, through aliases of it (`type Lane = i32` -> "i32"); "" for another type
    fn cast_prim(&self, t: &crate::parse::Ty) -> String {
        let mut prim = match t {
            crate::parse::Ty::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => p.segs[0].name.clone(),
            _ => String::new(),
        };
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

    /// The width in bits of a constant expression's integer type when it is written (a cast, a suffixed literal,
    /// `-` of one): `count_ones` of `-1i8` counts 8 bits, not the 128 of the evaluator's i128
    fn const_width(&self, e: &crate::parse::Expr) -> Option<u32> {
        use crate::parse::Expr;
        let of = |n: &str| match n {
            "u8" | "i8" => Some(8),
            "u16" | "i16" => Some(16),
            "u32" | "i32" => Some(32),
            "u64" | "i64" | "usize" | "isize" => Some(64),
            "u128" | "i128" => Some(128),
            _ => None,
        };
        match e {
            Expr::Cast(_, t) => of(&self.cast_prim(t)),
            Expr::Paren(x) => self.const_width(x),
            Expr::Unary(op, x, _) if op == "-" || op == "!" => self.const_width(x),
            Expr::Lit(t, _) => ["u128", "i128", "usize", "isize", "u64", "i64", "u32", "i32", "u16", "i16", "u8", "i8"]
                .iter()
                .find(|s| t.ends_with(*s))
                .and_then(|s| of(s)),
            _ => None,
        }
    }

    /// 自前の構文木の整数の定数式を評価する
    pub fn eval_const(&self, e: &crate::parse::Expr, depth: u32) -> R<i128> {
        use crate::parse::Expr;
        Ok(match e {
            Expr::Lit(t, _) => lit_int(t).ok_or("not an integer constant")?,
            Expr::Paren(x) => self.eval_const(x, depth)?,
            // (an integer `as` wraps to its type, as rustc's: `4294967297u64 as i32` is 1)
            Expr::Cast(x, t) => {
                let v = self.eval_const(x, depth)?;
                let prim = self.cast_prim(t);
                match prim.as_str() {
                    "u8" => v as u8 as i128,
                    "u16" => v as u16 as i128,
                    "u32" => v as u32 as i128,
                    "u64" | "usize" => v as u64 as i128,
                    "i8" => v as i8 as i128,
                    "i16" => v as i16 as i128,
                    "i32" => v as i32 as i128,
                    "i64" | "isize" => v as i64 as i128,
                    _ => v,
                }
            }
            // (`{ 3 }` / `{ let x = ..; }` is not: a block of one expression; `if c { a } else { b }`)
            Expr::Block(None, false, b) if b.stmts.len() == 1 => match &b.stmts[0] {
                crate::parse::Stmt::Expr(x, false) => self.eval_const(x, depth)?,
                _ => return Err("unsupported expression in constant".into()),
            },
            // (`{ let x = 2; 5 }`)
            Expr::Block(None, false, b) if b.stmts.len() > 1 => self.eval_block_env(b, &mut vec![], depth + 1)?,
            Expr::Ref(false, x) => self.eval_const(x, depth)?,
            Expr::If(c, t, Some(els)) => {
                let tail = |b: &crate::parse::Block| match b.stmts.as_slice() {
                    [crate::parse::Stmt::Expr(x, false)] => Some(x.clone()),
                    _ => None,
                };
                let c = self.eval_const(c, depth)?;
                if c != 0 {
                    let x = tail(t).ok_or("unsupported expression in constant")?;
                    self.eval_const(&x, depth)?
                } else {
                    self.eval_const(els, depth)?
                }
            }
            Expr::Path(None, p) if p.segs.len() == 1 && (p.segs[0].name == "true" || p.segs[0].name == "false") => (p.segs[0].name == "true") as i128,
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
                let mut env: Vec<(String, i128)> = vec![];
                for (n, a) in ps.iter().zip(args) {
                    env.push((n.clone(), self.eval_const(a, depth + 1)?));
                }
                let b = crate::parse::parse_expr(&body)?;
                let Expr::Block(_, _, b) = b else { return Err("unsupported const fn".into()) };
                self.eval_block_env(&b, &mut env, depth + 1)?
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
                self.eval_const(&part, depth + 1)?
            }
            Expr::Field(b, f, _) => {
                let part = self.const_part(b, depth, &|x| match x {
                    Expr::Tuple(xs) => f.parse::<usize>().ok().and_then(|k| xs.get(k).cloned()),
                    Expr::Struct(_, fs, _) => fs.iter().find(|x| &x.0 == f).map(|x| x.1.clone()),
                    Expr::Call(_, xs) => f.parse::<usize>().ok().and_then(|k| xs.get(k).cloned()),
                    _ => None,
                })?;
                self.eval_const(&part, depth + 1)?
            }
            Expr::Unary(op, x, _) => {
                let v = self.eval_const(x, depth)?;
                match op.as_str() {
                    "-" => -v,
                    "!" => !v,
                    "*" => v,
                    _ => return Err("not an integer constant".into()),
                }
            }
            Expr::Binary(op, l, r, _) => {
                let (l, r) = (self.eval_const(l, depth)?, self.eval_const(r, depth)?);
                match op.as_str() {
                    "+" => l + r,
                    "-" => l - r,
                    "*" => l * r,
                    "/" if r != 0 => l / r,
                    "%" if r != 0 => l % r,
                    "<<" => l << r,
                    ">>" => l >> r,
                    "&" => l & r,
                    "|" => l | r,
                    "^" => l ^ r,
                    "==" => (l == r) as i128,
                    "!=" => (l != r) as i128,
                    "<" => (l < r) as i128,
                    "<=" => (l <= r) as i128,
                    ">" => (l > r) as i128,
                    ">=" => (l >= r) as i128,
                    "&&" => (l != 0 && r != 0) as i128,
                    "||" => (l != 0 || r != 0) as i128,
                    _ => return Err("unsupported operator in constant".into()),
                }
            }
            Expr::Path(None, p) if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() => {
                self.const_int(&p.segs[0].name, depth)?
            }
            // `<u64 as Integer>::MAX_STR_LEN`
            Expr::Path(Some(q), p) if matches!(&**q, crate::parse::Ty::Path(None, tp) if tp.segs.len() == 1) && !p.segs.is_empty() => {
                let crate::parse::Ty::Path(None, tp) = &**q else { unreachable!() };
                // (the trait impl's under its qualified name first, where an inherent const took the plain one)
                if p.segs.len() == 2 {
                    let q = format!("{} as {}::{}", tp.segs[0].name, p.segs[0].name, p.segs[1].name);
                    if self.const_index.contains_key(&q) {
                        return self.const_int(&q, depth);
                    }
                    if let Some(k) = self.blanket_defines(&p.segs[0].name, &p.segs[1].name) {
                        return self.const_int(&k, depth);
                    }
                    let d = format!("{}::{}", p.segs[0].name, p.segs[1].name);
                    if self.const_index.contains_key(&d) {
                        return self.const_int(&d, depth);
                    }
                }
                let full = format!("{}::{}", tp.segs[0].name, p.segs.last().unwrap().name);
                if self.const_index.contains_key(&full) {
                    return self.const_int(&full, depth);
                }
                let t = Ty::prim(&tp.segs[0].name).ok_or("unsupported path in constant")?;
                prim_assoc_const(t, &p.segs.last().unwrap().name).ok_or("unsupported constant")?
            }
            Expr::Path(None, p) if p.segs.len() == 2 => {
                // (impl の関連 const: `i128::MAX_STR_LEN`)
                let full = format!("{}::{}", p.segs[0].name, p.segs[1].name);
                if self.const_index.contains_key(&full) {
                    return self.const_int(&full, depth);
                }
                // (`Foo::Bar as usize`: a unit variant's discriminant)
                if let Some((sid, vi)) = self.variant_by_p(&PPath::of(p), None) {
                    return Ok(self.structs[sid as usize].variants[vi].discr as i128);
                }
                let t = Ty::prim(&p.segs[0].name).ok_or("unsupported path in constant")?;
                prim_assoc_const(t, &p.segs[1].name).ok_or("unsupported constant")?
            }
            // 整数のメソッド (`u128::MAX.ilog10()`)。u128 の値はビット列で持つので、受け手が符号なしの型の
            // 関連 const なら符号なしとして見る
            Expr::MethodCall(recv, m, _, args, _) => {
                let v = self.eval_const(recv, depth)?;
                let unsigned = matches!(&**recv, Expr::Path(None, p) if p.segs.len() == 2 && Ty::prim(&p.segs[0].name).is_some_and(|t| !t.signed()));
                // (the bits of the receiver's type only: a negative i8 is 8 bits, not 128)
                let bits = self.const_width(recv).unwrap_or(128);
                let u = if bits < 128 { (v as u128) & ((1u128 << bits) - 1) } else { v as u128 };
                let a: Vec<i128> = args.iter().map(|x| self.eval_const(x, depth)).collect::<R<Vec<i128>>>()?;
                match (m.as_str(), a.as_slice()) {
                    ("ilog10", []) if unsigned || v > 0 => {
                        let mut n = if unsigned { u } else { v as u128 };
                        let mut k = 0;
                        while n >= 10 {
                            n /= 10;
                            k += 1;
                        }
                        k
                    }
                    ("ilog2", []) if unsigned || v > 0 => (127 - (if unsigned { u } else { v as u128 }).leading_zeros()) as i128,
                    ("count_ones", []) => u.count_ones() as i128,
                    ("trailing_zeros", []) => u.trailing_zeros().min(bits) as i128,
                    ("pow", [e]) => v.checked_pow(*e as u32).ok_or("overflow in constant")?,
                    ("min", [b]) => v.min(*b),
                    ("max", [b]) => v.max(*b),
                    ("abs", []) => v.abs(),
                    _ => return Err(format!("unsupported method `{m}` in constant")),
                }
            }
            _ => return Err("unsupported expression in constant".into()),
        })
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

    /// sid is a placeholder (Program::placeholder); cheap when none was made
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

    /// 具体型 t の関連型 name (`impl Iterator for Counter { type Item = u32; }` なら u32)
    pub fn assoc_of(&self, t: Ty, name: &str) -> Option<Ty> {
        thread_local! {
            static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        }
        // (`impl<F: Future> IntoFuture for F { type Item = F::Item; }` refers to another trait's `Item` of the same
        // type: a cycle through the blanket impl. Bounded, and such a definition is not used to answer its own name)
        if DEPTH.with(|d| d.get()) > 32 {
            return None;
        }
        DEPTH.with(|d| d.set(d.get() + 1));
        let r = self.assoc_of0(t, name);
        DEPTH.with(|d| d.set(d.get() - 1));
        r
    }

    fn assoc_of0(&self, t: Ty, name: &str) -> Option<Ty> {
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
            let mut sol = vec![None; a.ngen];
            if self.match_ty(a.self_ty, t, &mut sol) {
                let Some(mut args) = sol.iter().copied().collect::<Option<Vec<Ty>>>() else {
                    continue;
                };
                args.extend(std::iter::repeat_n(Ty::Unit, if a.ngat > 0 { a.ngat + 1 } else { 0 }));
                return Some(self.subst(a.value, &args));
            }
        }
        None
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
                let mut next = 0i64;
                let mut env: Vec<(String, i128)> = vec![];
                for v in vs {
                    let d = if v.discr_src.is_empty() {
                        v.discr.unwrap_or(next)
                    } else {
                        // (other constants, and the variants before it: `Y = Foo::X as isize - 3`)
                        let e = crate::parse::parse_expr(&v.discr_src).map_err(|e| format!("{line}:1: {e}"))?;
                        let x = self.eval_const(&subst_consts(&e, &env), 1).map_err(|e| format!("{line}:1: {e}"))?;
                        i64::try_from(x).or_else(|_| u64::try_from(x).map(|u| u as i64)).map_err(|_| format!("{line}:1: discriminant out of range"))?
                    };
                    if !v.discr_src.is_empty() || vs.iter().any(|w| !w.discr_src.is_empty()) {
                        env.push((format!("{}::{}", sdefs[id].1, v.name), d as i128));
                        env.push((format!("Self::{}", v.name), d as i128));
                    }
                    if discrs.contains(&d) {
                        return Err(format!(
                            "{line}:1: discriminant value `{d}` assigned more than once"
                        ));
                    }
                    discrs.push(d);
                    next = d + 1;
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
            if let Some(ids) = self.by_last.get(s) {
                fns.extend_from_slice(ids);
            }
            if let Some(&i) = self.struct_index.get(s) {
                sts.push(i);
            }
            if has_consts {
                if let Some(&i) = self.const_index.get(s) {
                    cs.push(i);
                }
                if has_assoc {
                    if let Some(ids) = self.const_by_last.get(s) {
                        cs.extend_from_slice(ids);
                    }
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
        // `core::option::Option::Some` などは最後の 2 つで見る
        let segs: &[PSeg] = if p.segs.len() > 2 && matches!(p.segs[0].name.as_str(), "core" | "crate" | "std") {
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
                    if self.struct_index.get(n).is_some_and(|&id| !self.structs[id as usize].is_enum) {
                        return None;
                    }
                    return self.variant_names.get(n).copied().flatten();
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
        let sid = if tn == "Self" { self_sid? } else { *self.struct_index.get(tn)? };
        let sd = &self.structs[sid as usize];
        if !sd.is_enum {
            return None;
        }
        let vn = &segs[1].name;
        sd.variants.iter().position(|v| &v.name == vn).map(|vi| (sid, vi))
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

/// e with the names bound in env (the latest binding of each) replaced by their integer values
fn subst_consts(e: &crate::parse::Expr, env: &[(String, i128)]) -> crate::parse::Expr {
    use crate::parse::Expr;
    let r = |x: &Expr| Box::new(subst_consts(x, env));
    match e {
        Expr::Path(None, p) if p.segs.len() == 1 && p.segs[0].args.is_empty() => {
            match env.iter().rev().find(|(n, _)| *n == p.segs[0].name) {
                Some((_, v)) if *v < 0 => Expr::Paren(Box::new(Expr::Unary("-".into(), Box::new(Expr::Lit((-*v).to_string(), p.pos)), Default::default()))),
                Some((_, v)) => Expr::Lit(v.to_string(), p.pos),
                None => e.clone(),
            }
        }
        // (`Foo::X`: an earlier variant's discriminant, for an enum's discriminant)
        Expr::Path(None, p) if p.segs.len() == 2 && p.segs.iter().all(|s| s.args.is_empty()) && !env.is_empty() => {
            let n = format!("{}::{}", p.segs[0].name, p.segs[1].name);
            match env.iter().rev().find(|(x, _)| *x == n) {
                Some((_, v)) if *v < 0 => Expr::Paren(Box::new(Expr::Unary("-".into(), Box::new(Expr::Lit((-*v).to_string(), p.pos)), Default::default()))),
                Some((_, v)) => Expr::Lit(v.to_string(), p.pos),
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
        Expr::Block(l, u, b) => {
            let mut b = b.clone();
            for st in &mut b.stmts {
                if let crate::parse::Stmt::Expr(x, semi) = st {
                    *st = crate::parse::Stmt::Expr(subst_consts(x, env), *semi);
                }
            }
            Expr::Block(l.clone(), *u, b)
        }
        e => e.clone(),
    }
}
