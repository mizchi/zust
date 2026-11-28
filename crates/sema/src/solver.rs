//! 型推論の単一化器 (型変数・形・union-find・関連型と境界の後回し)。infer.rs (syn の構文木を辿る) から
//! 構文木に依存しない部分を分けたもの。外部クレートに依存しないので zustc 自身でもコンパイルできる。

use crate::program::{Program, TyEnv};
use crate::types::{Ty, intern};

pub type R<T> = Result<T, String>;

thread_local! {
    /// how deep the typing of generic `impl Trait` fns' bodies is nested (Solver::apply_self)
    static OPAQUE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Copy)]
pub enum Shape {
    Known(Ty),
    /// 参照。中身は別の型変数
    Ref(bool, u32),
    /// ジェネリックな struct / enum (定義の id) に、型引数の型変数 (adt_args[start..start+len]) を入れたもの
    Adt(u32, u32, u32),
    /// `[T; N]` (要素の型変数, 長さ)
    Arr(u32, u32),
    /// `[T]`
    Slc(u32),
    /// `*const T` (false) / `*mut T` (true)
    Ptr(bool, u32),
    /// クロージャ: adt_args[start..start+n] が引数、その次が戻り値の型変数
    Clo(u32, u32),
    /// `Box<T>`
    Bx(u32),
}

pub struct Solver<'p> {
    pub prog: &'p Program,
    /// impl の中なら self 型
    pub self_sid: Option<u32>,
    pub env: &'p TyEnv,
    pub parent: Vec<u32>,
    pub shape: Vec<Option<Shape>>,
    /// 整数リテラル由来 (決まらなければ i32 にしてよい) か
    pub int_lit: Vec<bool>,
    /// 接尾辞のない浮動小数点リテラルの型変数 (決まらなければ f64)
    pub float_lit: Vec<bool>,
    pub scopes: Vec<Vec<(String, u32)>>,
    pub lets: Vec<(usize, u32)>,
    pub ret: u32,
    /// Shape::Adt の型引数の型変数
    pub adt_args: Vec<u32>,
    /// resolve の入れ子の深さ (形の循環で止まらなくならないように)
    resolve_depth: u32,
    /// occurs の見た印 (型変数ごとの世代の番号) と今の世代
    occ_mark: Vec<u32>,
    occ_gen: u32,
    /// resolve の記憶 (代表の型変数 → 型) と、それが正しい版 (link のたびに rver が進む)
    rcache: crate::fxhash::FxMap<u32, Option<Ty>>,
    rcache_ver: u64,
    rver: u64,
    /// 呼び出しの引数を推論している深さと、本体の推論を後回しにしたクロージャ (式, 引数, 戻り値)
    pub defer: u32,
    /// Unifying with a type the program writes (a turbofish): it is the type, whatever the value's arguments say
    pub written: u32,
    /// 囲んでいるループ / ラベル付きブロック: (ラベル, 値の型, ブロックか, break があったか)
    pub brk: Vec<(Option<String>, u32, bool, bool)>,
    /// 後で解く関連型 (基になる型の変数, 名前, 結果の変数)
    pub assoc_later: Vec<(u32, String, u32)>,
    /// 0 でなければ式の型を記録しない (一時的に parse したノードの推論中)
    pub no_record: u32,
    /// 0 でなければ式の型をソース上の位置で記録する (マクロの引数: codegen も同じ位置のノードを parse し直す)
    pub pos_record: u32,
    /// `it.collect()` の (受け手, 結果): 最後に結果が Vec<T> / HashMap<K, V> なら Item と単一化する
    pub collects: Vec<(u32, u32)>,
    /// クロージャの型を解くための文脈: (根の関数, closure_key → 前順の番号)
    pub clo_ctx: Option<(u32, crate::fxhash::FxMap<(usize, usize), u32>)>,
    /// Shape::Clo の st → (クロージャの関数 id, キャプチャの型変数, 注釈のない引数の位置)
    pub clo_info: crate::fxhash::FxMap<u32, (u32, Vec<u32>, Vec<usize>)>,
    /// 後でもう一度見る関連型の境界 (呼び先, 型パラメータの型変数)
    pub late: Vec<(u32, Vec<Option<u32>>)>,
    /// the callee's type parameters a turbofish gives (index, variable), for the next apply_self (`tr::<u16>()`)
    pub pre_pv: Vec<(usize, u32)>,
    /// (a type argument of `S<..>` passed where `S<[T]>` is expected (a coercion), the `T`): the argument may be
    /// any `[T; N]`, so only its element is unified, once the argument is known to be an array (finish_unsize)
    pub unsize_later: Vec<(u32, u32)>,
}

impl<'p> Solver<'p> {
    pub fn new(prog: &'p Program, env: &'p TyEnv) -> Solver<'p> {
        Solver {
            prog,
            self_sid: env.self_sid,
            env,
            parent: vec![],
            shape: vec![],
            int_lit: vec![],
            float_lit: vec![],
            scopes: vec![vec![]],
            lets: vec![],
            ret: 0,
            adt_args: vec![],
            resolve_depth: 0,
            occ_mark: vec![],
            occ_gen: 0,
            rcache: Default::default(),
            rcache_ver: 0,
            rver: 0,
            defer: 0,
            written: 0,
            brk: vec![],
            assoc_later: vec![],
            no_record: 0,
            pos_record: 0,
            collects: vec![],
            clo_ctx: None,
            clo_info: Default::default(),
            late: vec![],
            pre_pv: vec![],
            unsize_later: vec![],
        }
    }
}

impl Solver<'_> {
    pub fn var(&mut self) -> u32 {
        self.parent.push(self.parent.len() as u32);
        self.shape.push(None);
        self.int_lit.push(false);
        self.float_lit.push(false);
        self.parent.len() as u32 - 1
    }
    pub fn with(&mut self, sh: Shape) -> u32 {
        let v = self.var();
        self.shape[v as usize] = Some(sh);
        v
    }
    pub fn known(&mut self, t: Ty) -> u32 {
        match t {
            Ty::Ref(m, inner) => {
                let i = self.known(*inner);
                self.with(Shape::Ref(m, i))
            }
            Ty::Box(inner) => {
                let i = self.known(*inner);
                self.with(Shape::Bx(i))
            }
            // 関連型 (まだ決まっていない) は分からないものとして扱う
            Ty::Assoc(_) => self.var(),
            // (`_` written in a type: a new variable)
            Ty::Param(crate::program::HOLE) => self.var(),
            Ty::Array(e, n) => {
                let i = self.known(*e);
                self.with(Shape::Arr(i, n))
            }
            Ty::Ptr(m, e) => {
                let i = self.known(*e);
                self.with(Shape::Ptr(m, i))
            }
            Ty::Slice(e) => {
                let i = self.known(*e);
                self.with(Shape::Slc(i))
            }
            // 型パラメータを含む (まだ記号的な) インスタンスは Adt の形にする
            Ty::Struct(_) if self.prog.has_param(t) => {
                let mut pv = vec![];
                self.known_g(t, &mut pv)
            }
            t => self.with(Shape::Known(t)),
        }
    }
    pub fn adt(&mut self, base: u32, vars: Vec<u32>) -> u32 {
        let start = self.adt_args.len() as u32;
        let len = vars.len() as u32;
        self.adt_args.extend(vars);
        self.with(Shape::Adt(base, start, len))
    }
    pub fn adt_vars_of(&self, start: u32, len: u32) -> Vec<u32> {
        self.adt_args[start as usize..(start + len) as usize].to_vec()
    }
    /// v をジェネリックな定義 base のインスタンスとして見たときの型引数の型変数
    /// (分からなければ v にその形を与える)。非ジェネリックなら空
    pub fn adt_view(&mut self, v: u32, base: u32) -> Vec<u32> {
        let n = self.prog.structs[base as usize].generics.len();
        if n == 0 {
            return vec![];
        }
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Adt(b, st, len)) if b == base => self.adt_vars_of(st, len),
            Some(Shape::Known(Ty::Struct(i))) if self.prog.base_of(i) == base => {
                let args = self.prog.structs[i as usize].args.clone();
                args.into_iter().map(|a| self.known(a)).collect()
            }
            None => {
                let vars: Vec<u32> = (0..n).map(|_| self.var()).collect();
                let a = self.adt(base, vars.clone());
                self.unify(r, a);
                vars
            }
            _ => (0..n).map(|_| self.var()).collect(),
        }
    }
    /// ジェネリックな定義 base のフィールド型 (Param を含む) を、型引数の型変数 vars で具体化した型変数
    pub fn field_var(&mut self, t: Ty, vars: &[u32]) -> u32 {
        let mut pv: Vec<Option<u32>> = vars.iter().map(|&v| Some(v)).collect();
        self.known_g(t, &mut pv)
    }
    pub fn find(&mut self, mut v: u32) -> u32 {
        while self.parent[v as usize] != v {
            let p = self.parent[v as usize];
            self.parent[v as usize] = self.parent[p as usize];
            v = p;
        }
        v
    }
    pub fn link(&mut self, from: u32, to: u32) {
        self.rver += 1;
        self.parent[from as usize] = to;
        self.int_lit[to as usize] |= self.int_lit[from as usize];
        self.float_lit[to as usize] |= self.float_lit[from as usize];
    }
    /// `==` の両辺: Vec と配列 (`PartialEq<[U; N]> for Vec<T>`) なら要素の型どうし、それ以外は普通に単一化する
    pub fn seq_unify(&mut self, a: u32, b: u32) {
        let (fa, fb) = (self.find(a), self.find(b));
        let elem = |s: &mut Self, v: u32| -> Option<u32> {
            // (`&[T]` against `[U; N]` (`PartialEq<[U; N]> for &[T]`): the slice's element. So for a reference to an
            // array, a `&[_]` initialized from one: `let e: &[_] = &[1, 2, 3]; assert_eq!(e, ORDER)`)
            let v = match s.shape[v as usize] {
                Some(Shape::Ref(_, i)) => {
                    let i = s.find(i);
                    if matches!(s.shape[i as usize], Some(Shape::Slc(_) | Shape::Arr(..))) { i } else { v }
                }
                _ => v,
            };
            match s.shape[v as usize] {
                Some(Shape::Arr(x, _)) | Some(Shape::Slc(x)) => Some(x),
                Some(Shape::Adt(vb, st, 1)) if s.is_vec(vb) => Some(s.adt_args[st as usize]),
                Some(Shape::Known(Ty::Struct(sid))) if s.is_vec(s.prog.base_of(sid)) => {
                    s.prog.structs[sid as usize].args.first().map(|&t| s.known(t))
                }
                Some(Shape::Known(Ty::Array(e, _) | Ty::Slice(e))) => Some(s.known(*e)),
                _ => None,
            }
        };
        let same_kind = shape_tag(&self.shape[fa as usize]) == shape_tag(&self.shape[fb as usize]);
        match (elem(self, fa), elem(self, fb)) {
            (Some(x), Some(y)) if !same_kind || matches!(self.shape[fa as usize], Some(Shape::Adt(..) | Shape::Known(Ty::Struct(_)))) => {
                // (要素どうしも型が違ってよい (`Vec<String> == [&str]`): 片方が分からないときだけ決める)
                self.elem_hint(x, y)
            }
            _ => {
                // (片方がまだ分からず、もう片方が配列・スライス (の参照): Vec かもしれないので決めない
                //  (`v == b"crate"` の v = `x.to_vec()`))
                let seq_or_ref = |s: &mut Self, v: u32| -> bool {
                    let v = s.find(v);
                    let v = match s.shape[v as usize] {
                        Some(Shape::Ref(_, i)) => s.find(i),
                        _ => v,
                    };
                    matches!(s.shape[v as usize], Some(Shape::Arr(..) | Shape::Slc(_)) | Some(Shape::Known(Ty::Array(..) | Ty::Slice(_))))
                };
                let unknown = |s: &Self, v: u32| s.shape[v as usize].is_none() && !s.int_lit[v as usize];
                if (unknown(self, fa) && seq_or_ref(self, fb)) || (unknown(self, fb) && seq_or_ref(self, fa)) {
                    return;
                }
                self.unify(a, b)
            }
        }
    }

    /// 要素の型: 片方がまだ決まっていなければ単一化する (整数・浮動小数のリテラルを相手の型にする)
    pub fn elem_hint(&mut self, x: u32, y: u32) {
        let (fx, fy) = (self.find(x), self.find(y));
        // (数のリテラルだけ: 要素の型がまだ決まっていない Vec と文字列の配列は単一化しない)
        let lit = |s: &Self, v: u32| s.shape[v as usize].is_none() && (s.int_lit[v as usize] || s.float_lit[v as usize]);
        if lit(self, fx) || lit(self, fy) {
            self.unify(x, y);
        }
    }

    /// 型変数 v が t の形の中に現れるか (単一化で無限の型を作らないため)。形は共有された部分を持つ (DAG) ので、
    /// 見た型変数に印を付けて 1 度だけ辿る (印は世代の番号で、毎回消さない)
    fn occurs(&mut self, v: u32, t: u32, _depth: u32) -> bool {
        self.occ_gen = self.occ_gen.wrapping_add(1);
        if self.occ_gen == 0 {
            self.occ_mark.iter_mut().for_each(|m| *m = 0);
            self.occ_gen = 1;
        }
        let g = self.occ_gen;
        let mut stack = vec![t];
        while let Some(t) = stack.pop() {
            let t = self.find(t);
            if t == v {
                return true;
            }
            let ti = t as usize;
            if self.occ_mark.len() <= ti {
                self.occ_mark.resize(self.shape.len().max(ti + 1), 0);
            }
            if self.occ_mark[ti] == g {
                continue;
            }
            self.occ_mark[ti] = g;
            match self.shape[ti] {
                Some(Shape::Ref(_, i) | Shape::Arr(i, _) | Shape::Slc(i) | Shape::Ptr(_, i) | Shape::Bx(i)) => stack.push(i),
                Some(Shape::Adt(_, st, n)) => stack.extend_from_slice(&self.adt_args[st as usize..(st + n) as usize]),
                Some(Shape::Clo(st, n)) => stack.extend_from_slice(&self.adt_args[st as usize..(st + n + 1) as usize]),
                _ => {}
            }
        }
        false
    }

    pub fn unify(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        match (self.shape[a as usize], self.shape[b as usize]) {
            (Some(Shape::Known(Ty::Never)), _) | (_, Some(Shape::Known(Ty::Never))) => {}
            // (出現検査: `f = Box::new(f)` の f を `Box<f>` にはしない (無限の型)。決まらないまま残す)
            // (an integer literal's type and a float literal's: no one type, `let mut x = 2; x = 5.0;` is a mismatch
            // at the assignment)
            (None, None) if (self.int_lit[a as usize] && self.float_lit[b as usize]) || (self.float_lit[a as usize] && self.int_lit[b as usize]) => {}
            (None, _) => {
                if !self.occurs(a, b, 0) {
                    self.link(a, b)
                }
            }
            (_, None) => {
                if !self.occurs(b, a, 0) {
                    self.link(b, a)
                }
            }
            (Some(Shape::Ref(ma, x)), Some(Shape::Ref(mb, y))) => {
                // `&T` と `&dyn Trait`: 強制変換なので中身は単一化せず、dyn の方を代表にする
                let is_dyn = |s: &mut Self, v: u32| {
                    let r = s.find(v);
                    matches!(s.shape[r as usize], Some(Shape::Known(Ty::Dyn(_))))
                };
                let is_clo = |s: &mut Self, v: u32| {
                    let r = s.find(v);
                    matches!(s.shape[r as usize], Some(Shape::Clo(..)))
                };
                match (is_dyn(self, x), is_dyn(self, y)) {
                    (true, false) => {
                        self.link(b, a);
                        // `&|x| ..` を `&dyn Fn(A) -> B` に: クロージャの引数・戻り値は決まる
                        if is_clo(self, y) {
                            self.unify(x, y);
                        }
                    }
                    (false, true) => {
                        self.link(a, b);
                        if is_clo(self, x) {
                            self.unify(x, y);
                        }
                    }
                    _ => {
                        // (`&Vec<T>` / `&[T; N]` と `&[T]` は強制変換で渡すだけ: 参照どうしは同じ型にしない。
                        //  つなぐとクロージャがキャプチャした `v: &Vec<T>` が `&[T]` になってしまう)
                        let slc = |s: &mut Self, v: u32| {
                            let r = s.find(v);
                            matches!(s.shape[r as usize], Some(Shape::Slc(_)))
                        };
                        let seqish = |s: &mut Self, v: u32| {
                            let r = s.find(v);
                            match s.shape[r as usize] {
                                Some(Shape::Arr(..)) => true,
                                Some(Shape::Adt(b, _, 1)) => s.is_vec(b),
                                Some(Shape::Known(Ty::Struct(sid))) => s.is_vec(s.prog.base_of(sid)),
                                _ => false,
                            }
                        };
                        // (`&String` を `&str` に渡すのも同じ)
                        let is_str = |s: &mut Self, v: u32| {
                            let r = s.find(v);
                            matches!(s.shape[r as usize], Some(Shape::Known(Ty::Str)))
                        };
                        let is_string = |s: &mut Self, v: u32| {
                            let r = s.find(v);
                            matches!(s.shape[r as usize], Some(Shape::Known(Ty::Struct(sid)))
                                if s.prog.structs[sid as usize].base.is_none() && s.prog.structs[sid as usize].name == "String")
                        };
                        let coerce = self.unsize_pair(x, y)
                            || (slc(self, x) && seqish(self, y))
                            || (slc(self, y) && seqish(self, x))
                            || (is_str(self, x) && is_string(self, y))
                            || (is_str(self, y) && is_string(self, x));
                        // (`&Box<T>` を `&T` に: 中身の T どうしを単一化する。参照どうしはつながない)
                        let bx = |s: &mut Self, v: u32| {
                            let r = s.find(v);
                            match s.shape[r as usize] {
                                Some(Shape::Bx(i)) => Some(i),
                                _ => None,
                            }
                        };
                        let known_non_box = |s: &mut Self, v: u32| {
                            let r = s.find(v);
                            matches!(s.shape[r as usize], Some(Shape::Adt(..)))
                                || matches!(s.shape[r as usize], Some(Shape::Known(t)) if !matches!(t, Ty::Box(_)))
                        };
                        if let (Some(i), true) = (bx(self, x), known_non_box(self, y)) {
                            self.unify(i, y);
                            return;
                        }
                        if let (Some(i), true) = (bx(self, y), known_non_box(self, x)) {
                            self.unify(x, i);
                            return;
                        }
                        // (`&&T` を `&T` に、`&&&u8` を `&&u8` に (deref の強制変換): 参照の深さが違えば参照どうしは
                        //  つながず、最も内側の T どうしを単一化する。つなぐと、`let c = &v[0]; c` (v: Vec<&str>、
                        //  `&str` を返す) の c が `&str` になる。深さは、参照でない分かった形で終わるときだけ数える)
                        let depth = |s: &mut Self, v: u32| -> Option<(u32, u32)> {
                            let (mut v, mut d) = (v, 0u32);
                            loop {
                                let r = s.find(v);
                                match s.shape[r as usize] {
                                    Some(Shape::Ref(_, i)) if d < 16 => {
                                        v = i;
                                        d += 1;
                                    }
                                    Some(Shape::Ref(..)) | None => return None,
                                    Some(_) => return Some((d, v)),
                                }
                            }
                        };
                        if !coerce {
                            if let (Some((dx, ix)), Some((dy, iy))) = (depth(self, x), depth(self, y)) {
                                if dx != dy {
                                    self.unify(ix, iy);
                                    return;
                                }
                            }
                        }
                        // (`&mut T` を `&T` に渡すのも強制変換: つなぐと、`let x = &mut v; f(x);` (f が `&T` を取る) の
                        //  x が `&T` になり、後の `*x = ..` を `&` 越しの書き込みとしてしまう)
                        if !coerce && ma == mb {
                            self.link(a, b);
                        }
                        self.unify(x, y);
                    }
                }
            }
            (Some(Shape::Known(x)), Some(Shape::Known(y))) if x == y => self.link(a, b),
            (Some(Shape::Arr(x, n)), Some(Shape::Arr(y, m))) if n == m => {
                self.link(a, b);
                self.unify(x, y);
            }
            (Some(Shape::Slc(x)), Some(Shape::Slc(y)))
            | (Some(Shape::Bx(x)), Some(Shape::Bx(y)))
            | (Some(Shape::Ptr(_, x)), Some(Shape::Ptr(_, y))) => {
                // (`Box<S<[T; N]>>` to `Box<S<[T]>>`: a coercion, not one type)
                if !self.unsize_pair(x, y) {
                    self.link(a, b);
                }
                self.unify(x, y);
            }
            // (two instances of one struct that may have an unsized tail, with arguments known to differ
            // (`[&Foo<u8>, &Foo<u64>]` as `[&Foo<dyn Tr>; 2]`, each coerced): not one type, nothing to unify)
            // (or with an argument known to differ, whatever the struct (`let foo = Foo(1u8); takes_foo(foo);` with
            // `fn takes_foo(_: Foo<i32>)`): the value keeps the type it was made with, and the mismatch is the use's,
            // where rustc reports it)
            (Some(Shape::Adt(x, s1, l1)), Some(Shape::Adt(y, s2, l2))) if x == y => {
                // (two instances of one struct that may have an unsized tail, with arguments known to differ
                // (`[&Foo<u8>, &Foo<u64>]` as `[&Foo<dyn Tr>; 2]`, each coerced): not one type, nothing to unify. Or
                // of any struct, with an argument certainly another type (`let foo = Foo(1u8); takes_foo(foo);` with
                // `fn takes_foo(_: Foo<i32>)`): the value keeps the type it was made with, and the mismatch is the
                // use's, where rustc reports it)
                if self.adt_args_clash(x, s1, s2, l1.min(l2)) {
                    return;
                }
                // (`S<[T; N]>` passed as `S<[T]>` (a coercion, a struct with an unsized tail): not one type, only
                // the arguments are unified (an array's element with the slice's))
                if !self.unsize_pair(a, b) {
                    self.link(a, b);
                }
                for (p, q) in self
                    .adt_vars_of(s1, l1)
                    .into_iter()
                    .zip(self.adt_vars_of(s2, l2))
                {
                    self.unify(p, q);
                }
            }
            (Some(Shape::Adt(x, st, l)), Some(Shape::Known(Ty::Struct(i))))
            | (Some(Shape::Known(Ty::Struct(i))), Some(Shape::Adt(x, st, l)))
                if self.prog.base_of(i) == x =>
            {
                // (an argument already known to be another type (`let foo = Foo(1u8); takes_foo(foo);` with
                // `fn takes_foo(_: Foo<i32>)`): no one type. The value keeps the type it was made with, and the
                // mismatch is the use's, where rustc reports it)
                if !Ty::Struct(i).is_dst_struct() && self.written == 0 {
                    let prog = self.prog;
                    for (j, &t) in prog.structs[i as usize].args.iter().enumerate().take(l as usize) {
                        let r = self.find(self.adt_args[st as usize + j]);
                        if self.arg_clash((self.shape[r as usize], self.int_lit[r as usize]), (Some(Shape::Known(t)), false)) {
                            return;
                        }
                    }
                }
                // 具体型の方を代表にする
                let (adt_root, known_root) =
                    if matches!(self.shape[a as usize], Some(Shape::Adt(..))) {
                        (a, b)
                    } else {
                        (b, a)
                    };
                // (`&S<[T; N]>` を `&S<[T]>` に渡す (強制変換): 同じ型ではないので引数だけ単一化する)
                if !Ty::Struct(i).is_dst_struct() {
                    self.link(adt_root, known_root);
                }
                let dst = Ty::Struct(i).is_dst_struct();
                let args = self.prog.structs[i as usize].args.clone();
                for (p, t) in self.adt_vars_of(st, l).into_iter().zip(args) {
                    if let (true, Ty::Slice(e)) = (dst, t) {
                        let k = self.known(*e);
                        self.unsize_elem(p, k);
                        continue;
                    }
                    // (`S<T>` to `S<dyn Tr>`: T is any type implementing Tr)
                    if let (true, Ty::Dyn(_)) = (dst, t) {
                        continue;
                    }
                    let k = self.known(t);
                    self.unify(p, k);
                }
            }
            // `&Vec<T>` を `&[T]` に渡す (強制変換): 要素の型だけ単一化する
            (Some(Shape::Slc(x)), Some(Shape::Adt(vb, st, 1)))
            | (Some(Shape::Adt(vb, st, 1)), Some(Shape::Slc(x)))
                if self.is_vec(vb) =>
            {
                let y = self.adt_args[st as usize];
                self.unify(x, y)
            }
            (Some(Shape::Slc(x)), Some(Shape::Known(Ty::Struct(sid))))
            | (Some(Shape::Known(Ty::Struct(sid))), Some(Shape::Slc(x)))
                if self.is_vec(self.prog.base_of(sid)) =>
            {
                if let Some(&a) = self.prog.structs[sid as usize].args.first() {
                    let k = self.known(a);
                    self.unify(x, k);
                }
            }
            // (`&T` to `*const T` (a coercion): the pointees are one type, the reference and the pointer are not)
            (Some(Shape::Ref(_, x)), Some(Shape::Ptr(_, y))) | (Some(Shape::Ptr(_, y)), Some(Shape::Ref(_, x))) => self.unify(x, y),
            // `&[T; N]` を `&[T]` に渡す (強制変換): 要素の型だけ単一化する
            (Some(Shape::Slc(x)), Some(Shape::Arr(y, _)))
            | (Some(Shape::Arr(y, _)), Some(Shape::Slc(x))) => self.unify(x, y),
            (Some(Shape::Clo(s1, n1)), Some(Shape::Clo(s2, n2))) if n1 == n2 => {
                self.link(a, b);
                for i in 0..=n1 {
                    let (p, q) = (self.adt_args[(s1 + i) as usize], self.adt_args[(s2 + i) as usize]);
                    self.unify(p, q);
                }
            }
            (Some(Shape::Clo(st, n)), Some(Shape::Known(t)))
            | (Some(Shape::Known(t)), Some(Shape::Clo(st, n))) => {
                let (ca, ka) = if matches!(self.shape[a as usize], Some(Shape::Clo(..))) {
                    (a, b)
                } else {
                    (b, a)
                };
                if let Some((ps, r)) = self.closure_sig(t) {
                    if ps.len() == n as usize {
                        // `dyn Fn` とは同じ型ではない (強制変換されるだけ) ので要素だけ単一化する
                        // (nor a type with holes, `let g: fn(_, _) -> _ = |x, y| y;`: linked, the closure would be
                        // that type, and its parameters would not learn what a later use (`g` returned) gives)
                        if !matches!(t, Ty::Dyn(_)) && !self.prog.has_param(t) {
                            self.link(ca, ka);
                        }
                        for (i, t) in ps.into_iter().chain([r]).enumerate() {
                            let k = self.known(t);
                            let p = self.adt_args[st as usize + i];
                            self.unify(p, k);
                        }
                    }
                }
            }
            _ => {} // 矛盾は codegen で報告
        }
    }
    /// 具体型のクロージャ (struct のインスタンス / `&dyn Fn`) の引数と戻り値の型
    pub fn closure_sig(&self, t: Ty) -> Option<(Vec<Ty>, Ty)> {
        match t {
            Ty::Fn(f) => Some((f.params.clone(), f.ret)),
            Ty::Struct(sid) => {
                let cid = *self.prog.closure_of_struct.get(&self.prog.base_of(sid))?;
                let args = &self.prog.structs[sid as usize].args;
                let s0 = &self.prog.sigs[cid as usize];
                Some((
                    s0.params[1..].iter().map(|&p| self.prog.subst(p, args)).collect(),
                    self.prog.subst(s0.ret, args),
                ))
            }
            Ty::Dyn(s) => {
                let (_, ps, r) = crate::ast_summary::parse_fn_bound(s)?;
                let ps = ps
                    .iter()
                    .map(|t| self.prog.resolve_env(t, self.env).ok())
                    .collect::<Option<Vec<Ty>>>()?;
                let r = self.prog.resolve_env(&r, self.env).ok()?;
                // (a callee's `dyn Fn(T) -> T` whose T is not given: its parameter's mark (Program::dyn_canon) is
                // the callee's, not a type here)
                if ps.iter().chain([&r]).any(|&t| self.prog.has_param(t)) {
                    return None;
                }
                Some((ps, r))
            }
            _ => None,
        }
    }
    /// 呼び出せる値の (引数, 戻り値) の型変数。参照は剥がす
    pub fn callable(&mut self, f: u32, n: usize) -> Option<(Vec<u32>, u32)> {
        let mut f = f;
        loop {
            let r = self.find(f);
            match self.shape[r as usize] {
                Some(Shape::Ref(_, i)) | Some(Shape::Bx(i)) => f = i,
                Some(Shape::Clo(st, k)) if k as usize == n => {
                    let vs = self.adt_args[st as usize..=(st + k) as usize].to_vec();
                    return Some((vs[..n].to_vec(), vs[n]));
                }
                Some(Shape::Known(Ty::Ref(_, t))) => {
                    let (ps, r) = self.closure_sig(*t)?;
                    let ps = ps.into_iter().map(|t| self.known(t)).collect();
                    return Some((ps, self.known(r)));
                }
                Some(Shape::Known(t)) => {
                    let (ps, r) = self.closure_sig(t)?;
                    let ps = ps.into_iter().map(|t| self.known(t)).collect();
                    return Some((ps, self.known(r)));
                }
                _ => return None,
            }
        }
    }
    /// 型変数 v の関連型 name の型変数 (`Range<?T>` の Item → ?T、`Filter<I, P>` の Item → I の Item)
    pub fn assoc_var(&mut self, v: u32, name: &str) -> Option<u32> {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Known(t)) => {
                let a = self.prog.assoc_of(t, name)?;
                Some(self.known(a))
            }
            Some(Shape::Adt(b, st, l)) => {
                let prog = self.prog;
                let (name, tr) = crate::ast_types::split_trait_assoc(name);
                let def = prog.assocs.iter().find(|a| {
                    a.name == name
                        && tr.is_none_or(|tr| a.trait_name.is_empty() || a.trait_name == tr)
                        && matches!(a.self_ty, Ty::Struct(sid) if prog.base_of(sid) == b)
                })?;
                let Ty::Struct(ssid) = def.self_ty else { return None };
                let sargs = prog.structs[ssid as usize].args.clone();
                let vars = self.adt_vars_of(st, l);
                let mut pv: Vec<Option<u32>> = vec![None; def.ngen];
                for (a, &vv) in sargs.iter().zip(&vars) {
                    if let Ty::Param(i) = a {
                        if let Some(slot) = pv.get_mut(*i as usize) {
                            *slot = Some(vv);
                        }
                    }
                }
                let value = def.value;
                Some(self.known_g(value, &mut pv))
            }
            // (参照の型への impl の関連型: `impl IntoIterator for &Coll` の IntoIter)
            Some(Shape::Ref(..)) => {
                let t = self.resolve(v)?;
                let a = self.prog.assoc_of(t, name)?;
                Some(self.known(a))
            }
            _ => None,
        }
    }

    /// 型パラメータの関連型の制約 (`Self: Iterator<Item = S>`) で単一化する
    pub fn unify_assoc_bounds(&mut self, generics: &[(String, Vec<String>)], pv: &mut Vec<Option<u32>>) {
        let names: Vec<&str> = generics.iter().map(|g| g.0.as_str()).collect();
        for (g, (_, bounds)) in generics.iter().enumerate() {
            let Some(v) = pv.get(g).copied().flatten() else { continue };
            for b in bounds {
                if !b.contains("Item") || !b.contains('=') {
                    continue;
                }
                for (an, ts) in crate::ast_summary::assoc_bindings(b) {
                    let Ok(want) = self.prog.resolve_g(&ts, &names) else { continue };
                    let w = self.known_g(want, pv);
                    if let Some(actual) = self.assoc_var(v, &an) {
                        self.unify(w, actual);
                    }
                }
            }
        }
    }

    /// 型パラメータに `Fn(A) -> B` の境界があり、渡したのがクロージャなら A / B と単一化する
    pub fn unify_fn_bounds(&mut self, generics: &[(String, Vec<String>)], pv: &mut Vec<Option<u32>>) {
        self.int_lit_by_bounds(generics, pv);
        for (g, (_, bounds)) in generics.iter().enumerate() {
            if !bounds.iter().any(|b| b.starts_with("Fn")) {
                continue;
            }
            let Some(v) = pv.get(g).copied().flatten() else { continue };
            let Some((_, ps, r)) = bounds.iter().find_map(|b| crate::ast_summary::parse_fn_bound(b)) else {
                continue;
            };
            let names: Vec<&str> = generics.iter().map(|g| g.0.as_str()).collect();
            let Some((cps, cr)) = self.callable(v, ps.len()) else { continue };
            for (t, c) in ps.iter().zip(cps) {
                if let Ok(t) = self.prog.resolve_g(t, &names) {
                    let k = self.known_g(t, pv);
                    self.unify(c, k);
                }
            }
            if let Ok(t) = self.prog.resolve_g(&r, &names) {
                let k = self.known_g(t, pv);
                self.unify(cr, k);
            }
        }
    }
    /// An integer literal's variable passed as a type parameter bounded by traits of the program's (`foo2(42)` with
    /// `fn foo2<I: Foo>` and `impl Foo for isize` only): the one integer type that implements them, as rustc selects
    /// `{integer}: Foo`, not the i32 default
    fn int_lit_by_bounds(&mut self, generics: &[(String, Vec<String>)], pv: &[Option<u32>]) {
        for (g, (_, bounds)) in generics.iter().enumerate() {
            if bounds.is_empty() {
                continue;
            }
            let Some(v) = pv.get(g).copied().flatten() else { continue };
            let root = self.find(v);
            if self.shape[root as usize].is_some() || !self.int_lit.get(root as usize).copied().unwrap_or(false) {
                continue;
            }
            self.int_by_traits(v, bounds);
        }
    }

    #[cold]
    #[inline(never)]
    fn int_by_traits(&mut self, v: u32, bounds: &[String]) {
        let user = |tr: &str| self.prog.traits.get(tr).is_some_and(|ds| ds.iter().all(|&d| self.prog.item_of.get(d as usize).copied().flatten().is_some_and(|x| x < self.prog.prelude_from)));
        let trs: Vec<&str> = bounds.iter().map(|b| b.split('<').next().unwrap_or(b).trim()).filter(|b| user(b)).collect();
        if trs.is_empty() {
            return;
        }
        let mut hit = None;
        for t in [Ty::I8, Ty::I16, Ty::I32, Ty::I64, Ty::I128, Ty::Isize, Ty::U8, Ty::U16, Ty::U32, Ty::U64, Ty::U128, Ty::Usize] {
            if trs.iter().all(|tr| self.prog.implements(t, tr)) {
                if hit.is_some() {
                    return;
                }
                hit = Some(t);
            }
        }
        if let Some(t) = hit {
            let k = self.known(t);
            self.unify(v, k);
        }
    }

    /// v's type is known in full, before any default: no variable is left in it, not even an integer literal's that
    /// resolve would make an i32 (depth stops a cycle of shapes)
    pub fn known_in_full(&mut self, v: u32, depth: u32) -> bool {
        let mut memo = crate::fxhash::FxMap::default();
        self.known_in_full_m(v, depth, &mut memo)
    }

    /// known_in_full, each type variable once per question: shapes share their parts (`pair(pair(..))` gives both
    /// arguments one variable), which a walk without it would go through as a tree, in steps exponential in the
    /// nesting. A part is kept known in full, or not, only where the depth cut off nothing below it: a true, or the
    /// answer of its own shape
    fn known_in_full_m(&mut self, v: u32, depth: u32, memo: &mut crate::fxhash::FxMap<u32, bool>) -> bool {
        if depth > 64 {
            return false;
        }
        let r = self.find(v);
        if let Some(&k) = memo.get(&r) {
            return k;
        }
        let (k, own) = match self.shape[r as usize] {
            Some(Shape::Known(t)) => (!self.prog.has_param(t), true),
            Some(Shape::Ref(_, i) | Shape::Bx(i) | Shape::Arr(i, _) | Shape::Slc(i) | Shape::Ptr(_, i)) => (self.known_in_full_m(i, depth + 1, memo), false),
            Some(Shape::Adt(_, st, l)) => (
                (st..st + l).all(|k| {
                    let a = self.adt_args[k as usize];
                    self.known_in_full_m(a, depth + 1, memo)
                }),
                false,
            ),
            _ => (false, true),
        };
        if k || own {
            memo.insert(r, k);
        }
        k
    }
    /// 型変数 v の型 (まだ分からなければ None)。形の循環 (無限の型) があっても止まるように、深さを数える
    pub fn resolve(&mut self, v: u32) -> Option<Ty> {
        if self.resolve_depth > 256 {
            return None;
        }
        // (同じ型変数を何度も解かない: 入れ子のクロージャの型 (`dup(dup(..))`) は同じ部分を共有していて、記憶しないと
        // 段の数の指数になる。型変数の形が変わったら (link) 消す)
        let r = self.find(v);
        if self.rcache_ver != self.rver {
            self.rcache.clear();
            self.rcache_ver = self.rver;
        }
        if let Some(t) = self.rcache.get(&r) {
            return *t;
        }
        self.resolve_depth += 1;
        let t = self.resolve0(v);
        self.resolve_depth -= 1;
        if self.rcache_ver == self.rver {
            self.rcache.insert(r, t);
        }
        t
    }

    fn resolve0(&mut self, v: u32) -> Option<Ty> {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Known(t)) => Some(t),
            Some(Shape::Ref(m, i)) => Some(Ty::Ref(m, intern(self.resolve(i)?))),
            Some(Shape::Bx(i)) => Some(Ty::Box(intern(self.resolve(i)?))),
            Some(Shape::Arr(e, n)) => Some(Ty::Array(intern(self.resolve(e)?), n)),
            Some(Shape::Slc(e)) => Some(Ty::Slice(intern(self.resolve(e)?))),
            Some(Shape::Ptr(m, e)) => Some(Ty::Ptr(m, intern(self.resolve(e)?))),
            Some(Shape::Adt(b, st, l)) => {
                let mut args = vec![];
                for v in self.adt_vars_of(st, l) {
                    args.push(self.resolve(v)?);
                }
                self.prog.instantiate(b, args).ok().map(Ty::Struct)
            }
            Some(Shape::Clo(st, n)) => {
                // クロージャの struct: 親の型引数 + キャプチャ + 注釈のない引数 + 戻り値
                let (fid, caps, free) = self.clo_info.get(&st)?.clone();
                let cd = self.prog.closures.get(&fid)?;
                let (n_parent, sid) = (cd.n_parent, cd.sid);
                let moved = cd.moved.clone();
                let mut args = vec![];
                for i in 0..n_parent {
                    args.push(self.env.params.get(i)?.1);
                }
                for (i, cv) in caps.into_iter().enumerate() {
                    let t = self.resolve(cv)?;
                    args.push(if moved.get(i).copied().unwrap_or(false) { t } else { Ty::Ptr(true, intern(t)) });
                }
                for i in free {
                    let pv = self.adt_args[st as usize + i];
                    args.push(self.resolve(pv)?);
                }
                let rv = self.adt_args[(st + n) as usize];
                args.push(self.resolve(rv)?);
                self.prog.instantiate(sid, args).ok().map(Ty::Struct)
            }
            None if self.float_lit[r as usize] => Some(Ty::F64),
            None if self.int_lit[r as usize] => Some(Ty::I32),
            None => None,
        }
    }
    pub fn deref_var(&mut self, v: u32) -> u32 {
        let r = self.find(v);
        let sid = match self.shape[r as usize] {
            Some(Shape::Ref(_, i)) | Some(Shape::Ptr(_, i)) | Some(Shape::Bx(i)) => return i,
            Some(Shape::Known(Ty::Struct(sid))) => sid,
            Some(Shape::Adt(b, _, _)) => b,
            _ => return self.var(),
        };
        // (a type with a Deref impl, `*b` of a `Ref<T>`: its target, as field_at takes it (`let p; p = &*b;`))
        let d = self.prog.method(sid, "deref");
        match d.map(|d| self.prog.sigs[d as usize].ret) {
            Some(Ty::Ref(_, t)) if !self.prog.has_param(*t) && *t != Ty::Struct(sid) => self.known(*t),
            Some(Ty::Ref(_, Ty::Param(i))) => {
                let vars = self.adt_view(v, self.prog.base_of(sid));
                match d.and_then(|d| self.deref_arg(d, *i)).and_then(|k| vars.get(k)) {
                    Some(&k) => k,
                    None => self.var(),
                }
            }
            // (String and Vec, whose derefs the compiler knows: `str`, `[T]`)
            None => {
                let b = self.prog.base_of(sid);
                match (self.prog.structs[b as usize].name.as_str(), self.prog.structs[b as usize].generics.len()) {
                    ("String", 0) => self.known(Ty::Str),
                    ("Vec", 1) => {
                        let vars = self.adt_view(v, b);
                        self.with(Shape::Slc(vars[0]))
                    }
                    _ => self.var(),
                }
            }
            _ => self.var(),
        }
    }
    /// Of a Deref impl's `deref` (d) whose target is its parameter i, the place of the type's argument that is that
    /// parameter: `impl<B, A> Deref for Wrap<A, B> { type Target = A }` gives Wrap's first, not its second
    fn deref_arg(&self, d: u32, i: u8) -> Option<usize> {
        let Some(Ty::Struct(st)) = self.prog.sigs[d as usize].self_ty else { return None };
        self.prog.structs[st as usize].args.iter().position(|&a| a == Ty::Param(i))
    }
    /// 自動 deref しながらフィールドの型を引く
    pub fn field(&mut self, base: u32, name: &str) -> u32 {
        self.field_at(base, name, 0)
    }
    /// (depth: the derefs followed. As far as rustc's recursion limit (128): a cycle through other types (`A` to `B` to
    /// `A`) ends, and lowering reports it (E0055))
    fn field_at(&mut self, base: u32, name: &str, depth: u32) -> u32 {
        let v = self.deref_expr(base);
        let r = self.find(v);
        let sid = match self.shape[r as usize] {
            Some(Shape::Known(Ty::Struct(sid))) => sid,
            Some(Shape::Adt(b, _, _)) => b,
            _ => return self.var(),
        };
        let b = self.prog.base_of(sid);
        let vars = self.adt_view(v, b);
        match self.prog.field(b, name) {
            Some((ft, _)) => self.field_var(ft, &vars),
            // (フィールドがなく Deref を実装した型 (`self.prog` で self: &Inf → Solver): deref の先のフィールド)
            None => match self.prog.method(sid, "deref").map(|d| (d, self.prog.sigs[d as usize].ret)) {
                // (not to itself: `impl Deref for Foo { type Target = Foo }` would never end)
                Some((_, Ty::Ref(_, t))) if depth < 128 && !self.prog.has_param(*t) && !matches!(t, Ty::Str | Ty::Slice(_) | Ty::Dyn(_)) && *t != Ty::Struct(sid) => {
                    let k = self.known(*t);
                    self.field_at(k, name, depth + 1)
                }
                // (a deref to the struct's own parameter, `Ref<T>` → `&T`: the argument's type variable)
                Some((d, Ty::Ref(_, Ty::Param(i)))) if depth < 128 && self.deref_arg(d, *i).is_some_and(|k| k < vars.len()) => {
                    let k = vars[self.deref_arg(d, *i).unwrap_or(0)];
                    self.field_at(k, name, depth + 1)
                }
                _ => self.var(),
            },
        }
    }

    /// struct (vi = None) / バリアント (vi = Some) の値を作る式: 実引数をフィールド型と単一化する
    pub fn adt_ctor(&mut self, sid: u32, vi: Option<usize>, args: Vec<(String, u32)>) -> u32 {
        let base = self.prog.base_of(sid);
        let n = self.prog.structs[base as usize].generics.len();
        let vars: Vec<u32> = (0..n).map(|_| self.var()).collect();
        let fields: Vec<(String, Ty)> = {
            let sd = &self.prog.structs[base as usize];
            let f = match vi {
                Some(vi) => &sd.variants[vi].fields,
                None => &sd.fields,
            };
            f.iter().map(|x| (x.0.clone(), x.1)).collect()
        };
        for (name, a) in args {
            if let Some((_, ft)) = fields.iter().find(|f| f.0 == name) {
                let k = self.field_var(*ft, &vars);
                self.unify(a, k);
            }
        }
        if n == 0 {
            self.known(Ty::Struct(base))
        } else {
            self.adt(base, vars)
        }
    }

    /// 配列 / スライス (参照越しも) の要素の型変数
    pub fn elem_of(&mut self, v: u32) -> Option<u32> {
        let v = self.deref_expr(v);
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Arr(e, _)) | Some(Shape::Slc(e)) => Some(e),
            // Vec<T> も添字・for で T を取り出せる
            Some(Shape::Known(Ty::Struct(sid))) if self.is_vec(self.prog.base_of(sid)) => {
                let a = *self.prog.structs[sid as usize].args.first()?;
                Some(self.known(a))
            }
            Some(Shape::Adt(b, st, l)) if self.is_vec(b) && l == 1 => {
                Some(self.adt_args[st as usize])
            }
            // (a RefCell's Ref / RefMut or a MutexGuard: the elements of what it guards, `for x in c.borrow().iter()`)
            Some(Shape::Known(Ty::Struct(sid))) if self.is_guard(self.prog.base_of(sid)) => {
                let a = *self.prog.structs[sid as usize].args.first()?;
                let k = self.known(a);
                self.elem_of(k)
            }
            Some(Shape::Adt(b, st, l)) if self.is_guard(b) && l == 1 => {
                let a = self.adt_args[st as usize];
                self.elem_of(a)
            }
            _ => None,
        }
    }

    /// RefCell's Ref / RefMut, or MutexGuard: a guard that derefs to its one type argument
    pub fn is_guard(&self, base: u32) -> bool {
        matches!(self.prog.structs[base as usize].name.as_str(), "Ref" | "RefMut" | "MutexGuard" | "PeekMut")
    }

    pub fn is_vec(&self, base: u32) -> bool {
        self.prog.struct_index.get("Vec") == Some(&base)
    }

    /// Option<t> の型変数
    pub fn option_of(&mut self, t: u32) -> u32 {
        match self.prog.struct_index.get("Option") {
            Some(&ob) => self.adt(ob, vec![t]),
            None => self.var(),
        }
    }

    /// パターンのフィールド: v (auto-deref 済み) を base のインスタンスとして見て、フィールド型の型変数を返す
    pub fn pat_fields(&mut self, v: u32, sid: u32, vi: Option<usize>) -> Vec<(String, u32)> {
        let base = self.prog.base_of(sid);
        let vars = self.adt_view(v, base);
        if vars.is_empty() {
            let k = self.known(Ty::Struct(base));
            self.unify(v, k);
        }
        let fields: Vec<(String, Ty)> = {
            let sd = &self.prog.structs[base as usize];
            let f = match vi {
                Some(vi) => &sd.variants[vi].fields,
                None => &sd.fields,
            };
            f.iter().map(|x| (x.0.clone(), x.1)).collect()
        };
        fields
            .into_iter()
            .map(|(n, t)| (n, self.field_var(t, &vars)))
            .collect()
    }
    /// 呼び出し: 実引数を仮引数型と単一化して戻り値型を返す。skip はレシーバの分
    /// 型パラメータを含む型を、呼び出しごとの新しい型変数 (pv) で置き換えて作る
    pub fn known_g(&mut self, t: Ty, pv: &mut Vec<Option<u32>>) -> u32 {
        match t {
            // (each `_` written in a type is a variable of its own: `Result<_, Result<_, _>>`)
            Ty::Param(crate::program::HOLE) => self.var(),
            Ty::Param(i) => match pv.get(i as usize).copied().flatten() {
                Some(v) => v,
                None => {
                    let v = self.var();
                    if pv.len() <= i as usize {
                        pv.resize(i as usize + 1, None);
                    }
                    pv[i as usize] = Some(v);
                    v
                }
            },
            Ty::Ref(m, inner) => {
                let i = self.known_g(*inner, pv);
                self.with(Shape::Ref(m, i))
            }
            Ty::Box(inner) => {
                let i = self.known_g(*inner, pv);
                self.with(Shape::Bx(i))
            }
            Ty::Array(e, n) => {
                let i = self.known_g(*e, pv);
                self.with(Shape::Arr(i, n))
            }
            Ty::Ptr(m, e) => {
                let i = self.known_g(*e, pv);
                self.with(Shape::Ptr(m, i))
            }
            Ty::Slice(e) => {
                let i = self.known_g(*e, pv);
                self.with(Shape::Slc(i))
            }
            Ty::Struct(sid) if self.prog.has_param(t) => {
                let sd = &self.prog.structs[sid as usize];
                let (base, args) = (sd.base.unwrap(), sd.args.clone());
                let vars: Vec<u32> = args.into_iter().map(|a| self.known_g(a, pv)).collect();
                self.adt(base, vars)
            }
            Ty::Assoc(a) => {
                let b = self.known_g(a.base, pv);
                let name = a.name.clone();
                match self.assoc_var(b, &name) {
                    Some(v) => v,
                    // (`T::Err` の T がまだ分からない: 最後に解く)
                    None => {
                        let v = self.var();
                        self.assoc_later.push((b, name, v));
                        v
                    }
                }
            }
            t => self.with(Shape::Known(t)),
        }
    }

    /// 呼び出し: 実引数を仮引数型と単一化して戻り値型を返す。skip はレシーバの分、
    /// self_ty はレシーバの型 (trait のデフォルトメソッドの Self を決める)
    pub fn apply(&mut self, callee: Option<u32>, args: Vec<u32>, skip: usize) -> u32 {
        self.apply_self(callee, args, skip, None)
    }

    /// メソッド呼び出し: recv はレシーバ (auto-deref 済み) の型変数。`Self` (impl の self 型 / trait の Self) と単一化する
    /// 関連型の境界 (`U: __Flat<Item = B>`) は、クロージャの本体を推論した後でもう一度見る
    pub fn late_bounds(&mut self, id: u32, pv: Vec<Option<u32>>) {
        let gs = &self.prog.sigs[id as usize].generics;
        if gs.iter().any(|g| g.1.iter().any(|b| b.contains("Item") && b.contains('='))) {
            self.late.push((id, pv));
        }
    }

    pub fn apply_self(
        &mut self,
        callee: Option<u32>,
        args: Vec<u32>,
        skip: usize,
        self_ty: Option<Ty>,
    ) -> u32 {
        self.apply_with(callee, args, skip, self_ty, vec![])
    }

    /// apply_self, with the type variables of some of the callee's generics given (a turbofish's: `id::<u32>(c)`)
    pub fn apply_with(
        &mut self,
        callee: Option<u32>,
        args: Vec<u32>,
        skip: usize,
        self_ty: Option<Ty>,
        pre: Vec<Option<u32>>,
    ) -> u32 {
        match callee {
            Some(id) => {
                let sig = &self.prog.sigs[id as usize];
                let (params, ret) = (sig.params[skip.min(sig.params.len())..].to_vec(), sig.ret);
                let mut pv: Vec<Option<u32>> = vec![None; sig.generics.len()];
                for (i, v) in pre.into_iter().enumerate() {
                    if i < pv.len() {
                        pv[i] = v;
                    }
                }
                for (k, v) in std::mem::take(&mut self.pre_pv) {
                    if let Some(slot) = pv.get_mut(k) {
                        *slot = Some(v);
                    }
                }
                if let (Some(t), true) = (self_ty, sig.trait_of.is_some()) {
                    pv[0] = Some(self.known(t));
                }
                for (a, p) in args.into_iter().zip(params) {
                    let k = self.known_g(p, &mut pv);
                    self.unify(a, k);
                }
                let prog = self.prog;
                self.unify_fn_bounds(&prog.sigs[id as usize].generics, &mut pv);
                self.unify_assoc_bounds(&prog.sigs[id as usize].generics, &mut pv);
                // (a generic fn returning `impl Trait`, with its arguments known: the type of its body for them)
                if let Some(body) = prog.opaque_generic.get(&id).cloned().filter(|_| OPAQUE_DEPTH.with(|d| d.get() < 4)) {
                    let ts: Vec<Option<Ty>> = pv.iter().map(|v| v.and_then(|v| self.resolve(v))).collect();
                    if let Some(ts) = ts.into_iter().collect::<Option<Vec<Ty>>>().filter(|ts| ts.iter().all(|&t| !prog.has_param(t))) {
                        let sig0 = &prog.sigs[id as usize];
                        let sig = crate::program::FnSig { params: sig0.params.iter().map(|&t| prog.subst(t, &ts)).collect(), ..sig0.clone() };
                        let env = crate::program::TyEnv {
                            self_sid: sig.impl_of,
                            params: sig0.generics.iter().map(|g| g.0.clone()).zip(ts.iter().copied()).collect(),
                            self_ty: sig.self_ty,
                        };
                        // (bounded: generic fns returning each other's results)
                        OPAQUE_DEPTH.with(|d| d.set(d.get() + 1));
                        let (_, rt, _) = crate::ast_infer::infer_ctx(prog, body.2, &body.0, &body.1, &sig, &env, &[], None, Some((id, 0)));
                        OPAQUE_DEPTH.with(|d| d.set(d.get() - 1));
                        if let Some(t) = rt.filter(|&t| !prog.has_param(t)) {
                            self.late_bounds(id, pv);
                            return self.known(t);
                        }
                    }
                }
                let r = self.known_g(ret, &mut pv);
                self.late_bounds(id, pv);
                r
            }
            None => self.var(),
        }
    }

    /// 参照型の変数を辿って中身へ。参照を通ったら束縛モードを Ref にする
    /// FromIterator の代わり: collect の結果が Vec<T> なら Item = T、HashMap<K, V> なら Item = (K, V)
    /// `extend` の引数の要素の型 (Vec / BinaryHeap は要素、参照・配列は要素の参照、イテレータは Item)
    pub fn extend_item(&self, t: Ty) -> Option<Ty> {
        let elem_of = |t: Ty| -> Option<Ty> {
            match t {
                Ty::Array(e, _) | Ty::Slice(e) => Some(*e),
                Ty::Struct(s)
                    if matches!(
                        self.prog.structs[self.prog.base_of(s) as usize].name.as_str(),
                        "Vec" | "BinaryHeap"
                    ) =>
                {
                    self.prog.structs[s as usize].args.first().copied()
                }
                _ => None,
            }
        };
        match t {
            Ty::Ref(_, i) => elem_of(*i).map(|e| Ty::Ref(false, intern(e))),
            Ty::Array(e, _) => Some(Ty::Ref(false, e)),
            t => elem_of(t).or_else(|| self.prog.assoc_of(t, "Item")),
        }
    }

    /// x and y have shapes that are certainly different types (a scalar and another scalar or a compound type, two
    /// different structs): what a coercion may join (`&Foo<u8>` and `&Foo<(u16, u32)>`, both to `&Foo<dyn Tr>`)
    fn clash(&mut self, x: u32, y: u32) -> bool {
        let (x, y) = (self.find(x), self.find(y));
        match (self.shape[x as usize], self.shape[y as usize]) {
            (Some(Shape::Known(a)), Some(Shape::Known(b))) => a != b && (scalar_ty(a) || scalar_ty(b)),
            (Some(Shape::Known(a)), Some(_)) | (Some(_), Some(Shape::Known(a))) => scalar_ty(a),
            (Some(Shape::Adt(a, ..)), Some(Shape::Adt(b, ..))) => a != b,
            _ => false,
        }
    }

    /// The arguments s1.. and s2.. (n of them) of two instances of struct x are not one type's: see unify
    fn adt_args_clash(&mut self, x: u32, s1: u32, s2: u32, n: u32) -> bool {
        let uns = self.prog.unsized_params.contains_key(&x);
        if !uns && self.written > 0 {
            return false;
        }
        for j in 0..n {
            let (p, q) = (self.find(self.adt_args[(s1 + j) as usize]), self.find(self.adt_args[(s2 + j) as usize]));
            let c = if uns { self.clash(p, q) } else { self.arg_clash((self.shape[p as usize], self.int_lit[p as usize]), (self.shape[q as usize], self.int_lit[q as usize])) };
            if c {
                return true;
            }
        }
        false
    }

    /// Two type arguments of one generic type that are certainly not one type: two scalars (`Foo<u8>` and
    /// `Foo<i32>`), a scalar and a struct, structs of two definitions, an integer literal's and a scalar that is no
    /// integer or a struct (`Foo<{integer}>` and `Foo<()>`). Not a reference, a box or a pointer and another type:
    /// inference guesses those (`v.clone()` of a `&Vec<T>` that is not Clone is the reference, `(*p).clone()` of a
    /// `Box<i32>` the box), and a later use corrects the guess. Each is a representative's shape and whether it is
    /// an integer literal's
    fn arg_clash(&self, (x, xi): (Option<Shape>, bool), (y, yi): (Option<Shape>, bool)) -> bool {
        let scalar = |s: Option<Shape>| matches!(s, Some(Shape::Known(t)) if t != Ty::Never && scalar_ty(t));
        let is_struct = |s: Option<Shape>| matches!(s, Some(Shape::Known(Ty::Struct(_)) | Shape::Adt(..)));
        // (an integer literal's type not known yet, and a scalar that is no integer or a struct)
        let no_int = |s: Option<Shape>| is_struct(s) || matches!(s, Some(Shape::Known(t)) if t != Ty::Never && scalar_ty(t) && !t.is_int());
        match (x, y) {
            (None, None) => false,
            (None, _) => xi && no_int(y),
            (_, None) => yi && no_int(x),
            (Some(Shape::Known(a)), Some(Shape::Known(b))) if a == b => false,
            _ if scalar(x) => scalar(y) || is_struct(y),
            _ if scalar(y) => is_struct(x),
            // (structs of two definitions)
            (Some(Shape::Adt(a, ..)), Some(Shape::Adt(b, ..))) => a != b,
            (Some(Shape::Adt(a, ..)), Some(Shape::Known(Ty::Struct(i)))) | (Some(Shape::Known(Ty::Struct(i))), Some(Shape::Adt(a, ..))) => self.prog.base_of(i) != a,
            (Some(Shape::Known(Ty::Struct(i))), Some(Shape::Known(Ty::Struct(j)))) => self.prog.base_of(i) != self.prog.base_of(j),
            _ => false,
        }
    }

    /// x and y are a struct with an array tail and the same struct with a slice tail (`S<[T; N]>` and `S<[T]>`,
    /// in either order): what a reference or a box to one coerces to one to the other
    pub fn unsize_pair(&mut self, x: u32, y: u32) -> bool {
        self.unsize_pair_d(x, y, 0)
    }

    fn unsize_pair_d(&mut self, x: u32, y: u32, depth: u32) -> bool {
        let (x, y) = (self.find(x), self.find(y));
        // (one type is no coercion; and the walk is bounded (shared children are not visited twice per level))
        if x == y || depth > 16 {
            return false;
        }
        let base = |s: &Self, v: u32| match s.shape[v as usize] {
            Some(Shape::Adt(b, ..)) => Some((b, false)),
            Some(Shape::Known(t @ Ty::Struct(i))) => Some((s.prog.base_of(i), t.is_dst_struct())),
            _ => None,
        };
        match (base(self, x), base(self, y)) {
            (Some((b1, d1)), Some((b2, d2))) if b1 == b2 => {
                let may_unsize = self.prog.unsized_params.contains_key(&b1);
                if d1 != d2 {
                    return true;
                }
                // (two generic shapes: an argument that is an array on one side and a slice on the other)
                let args = |s: &mut Self, v: u32| -> Vec<u32> {
                    match s.shape[v as usize] {
                        Some(Shape::Adt(_, st, l)) => s.adt_vars_of(st, l),
                        _ => vec![],
                    }
                };
                let (p, q) = (args(self, x), args(self, y));
                let mut seen: Vec<(u32, u32)> = vec![];
                p.into_iter().zip(q).any(|(p, q)| {
                    let (p, q) = (self.find(p), self.find(q));
                    if p == q || seen.contains(&(p, q)) {
                        return false;
                    }
                    seen.push((p, q));
                    // (two array tails of different lengths (`vec![Box::new(W { t: [1, 2] }), Box::new(W { t: [3] })]`
                    // as `Vec<Box<W<[u8]>>>`): both coerce to the slice tail)
                    matches!(
                        (&self.shape[p as usize], &self.shape[q as usize]),
                        (Some(Shape::Slc(_)), Some(Shape::Arr(..))) | (Some(Shape::Arr(..)), Some(Shape::Slc(_)))
                    ) || (may_unsize && matches!((&self.shape[p as usize], &self.shape[q as usize]), (Some(Shape::Arr(_, n)), Some(Shape::Arr(_, m))) if n != m))
                        || self.unsize_pair_d(p, q, depth + 1)
                })
            }
            _ => false,
        }
    }

    /// the element of an array argument where a slice one is expected (`S<[u8; 3]>` to `S<[u8]>`); later, if the
    /// argument is not known yet
    fn unsize_elem(&mut self, p: u32, k: u32) {
        let r = self.find(p);
        match self.shape[r as usize] {
            Some(Shape::Arr(x, _) | Shape::Slc(x)) => self.unify(x, k),
            Some(Shape::Known(Ty::Array(e, _) | Ty::Slice(e))) => {
                let x = self.known(*e);
                self.unify(x, k)
            }
            None => self.unsize_later.push((p, k)),
            _ => {}
        }
    }

    pub fn finish_unsize(&mut self) {
        for _ in 0..2 {
            let ls = std::mem::take(&mut self.unsize_later);
            for (p, k) in ls {
                self.unsize_elem(p, k);
            }
        }
        // (an argument nothing else told: the slice itself (`&Wrap<[u8]>` passed where `&Wrap<T>` is taken))
        for (p, k) in std::mem::take(&mut self.unsize_later) {
            let r = self.find(p);
            if self.shape[r as usize].is_none() {
                let s = self.with(Shape::Slc(k));
                self.unify(p, s);
            }
        }
    }

    pub fn finish_late_bounds(&mut self) {
        let ls = std::mem::take(&mut self.late);
        for (id, mut pv) in ls {
            let prog = self.prog;
            self.unify_assoc_bounds(&prog.sigs[id as usize].generics, &mut pv);
        }
    }

    pub fn finish_assocs(&mut self) {
        for _ in 0..2 {
            let ls = std::mem::take(&mut self.assoc_later);
            for (b, name, v) in ls {
                match self.assoc_var(b, &name) {
                    Some(a) => self.unify(a, v),
                    None => self.assoc_later.push((b, name, v)),
                }
            }
        }
    }

    pub fn finish_collects(&mut self) {
        let cs = std::mem::take(&mut self.collects);
        for (recv, r) in cs {
            self.collect_into(recv, r);
        }
    }

    /// The `collect`s whose iterator's item and result's type are known now (a `let` with `Vec<_>` written): their
    /// elements are taken from the item, as rustc selects the `FromIterator` impl before the statements after it.
    /// The others wait for finish_collects
    pub fn ready_collects(&mut self) {
        let cs = std::mem::take(&mut self.collects);
        for (recv, r) in cs {
            let root = self.find(r);
            let known = matches!(self.shape[root as usize], Some(Shape::Adt(..) | Shape::Known(Ty::Struct(_))));
            let rr = self.find(recv);
            if !known || self.shape[rr as usize].is_none() || !self.collect_into(recv, r) {
                self.collects.push((recv, r));
            }
        }
    }

    /// A `collect`'s elements from its iterator's item (false: not known yet)
    fn collect_into(&mut self, recv: u32, r: u32) -> bool {
        let Some(item) = self.assoc_var(recv, "Item") else { return false };
        let root = self.find(r);
        let (b, vars) = match self.shape[root as usize] {
            Some(Shape::Adt(b, st, l)) => (b, self.adt_vars_of(st, l)),
            Some(Shape::Known(Ty::Struct(sid))) => {
                let args = self.prog.structs[sid as usize].args.clone();
                (self.prog.base_of(sid), args.into_iter().map(|a| self.known(a)).collect())
            }
            _ => return false,
        };
        let name = self.prog.structs[b as usize].name.clone();
        if matches!(name.as_str(), "Vec" | "BinaryHeap") && vars.len() == 1 {
            self.unify(item, vars[0]);
        } else if matches!(name.as_str(), "Option" | "Result") && !vars.is_empty() {
            // `Option<Vec<T>>` / `Result<Vec<T>, E>`: 要素は Option<T> / Result<T, E>
            let r0 = self.find(vars[0]);
            let inner = match self.shape[r0 as usize] {
                Some(Shape::Adt(ib, st, l)) if self.prog.structs[ib as usize].name == "Vec" => {
                    self.adt_vars_of(st, l).first().copied()
                }
                Some(Shape::Known(Ty::Struct(s))) if self.prog.structs[self.prog.base_of(s) as usize].name == "Vec" => {
                    self.prog.structs[s as usize].args.first().copied().map(|a| self.known(a))
                }
                _ => None,
            };
            let Some(t) = inner else { return false };
            let mut a = vec![t];
            a.extend(vars[1..].iter().copied());
            let it = self.adt(b, a);
            self.unify(item, it);
        } else if name == "HashMap" && vars.len() == 2 {
            let tb = self.prog.tuple_base(2);
            let t = self.adt(tb, vec![vars[0], vars[1]]);
            self.unify(item, t);
        }
        true
    }

    /// 算術・比較の被演算子: 整数 (か未知) への参照なら中身
    pub fn num_operand(&mut self, v: u32) -> u32 {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Ref(_, i)) => {
                let ri = self.find(i);
                match self.shape[ri as usize] {
                    None => i,
                    Some(Shape::Known(t)) if t.is_int() || t.is_float() || matches!(t, Ty::Char | Ty::Bool) => i,
                    _ => v,
                }
            }
            _ => v,
        }
    }

    /// 式の文脈 (フィールド・メソッド・添字) の自動 deref: 参照と Box を剥がす
    pub fn deref_expr(&mut self, mut v: u32) -> u32 {
        loop {
            let r = self.find(v);
            match self.shape[r as usize] {
                Some(Shape::Ref(_, i)) | Some(Shape::Bx(i)) => v = i,
                _ => return v,
            }
        }
    }

    pub fn auto_deref(&mut self, mut v: u32, mut mode: Option<bool>) -> (u32, Option<bool>) {
        loop {
            let r = self.find(v);
            match self.shape[r as usize] {
                Some(Shape::Ref(m, i)) => {
                    v = i;
                    mode = Some(if mode == Some(false) { false } else { m });
                }
                _ => return (v, mode),
            }
        }
    }

    /// As auto_deref, but only through a reference to a reference: it stops at the last reference (`&&str` -> `&str`,
    /// `&?T` stays)
    pub fn deref_refs_to_ref(&mut self, mut v: u32, mut mode: Option<bool>) -> (u32, Option<bool>) {
        loop {
            let r = self.find(v);
            let Some(Shape::Ref(m, i)) = self.shape[r as usize] else { return (v, mode) };
            let ri = self.find(i);
            if !matches!(self.shape[ri as usize], Some(Shape::Ref(..))) {
                return (v, mode);
            }
            v = i;
            mode = Some(if mode == Some(false) { false } else { m });
        }
    }

    pub fn bind_name(&mut self, name: String, v: u32, mode: Option<bool>) {
        let v = match mode {
            Some(m) => self.with(Shape::Ref(m, v)),
            None => v,
        };
        self.scopes.last_mut().unwrap().push((name, v));
    }

    /// (two plain loops: a `flat_map` over the reversed scopes was not inlined, and cost about 8M instructions of a
    /// check of the 1.6 MB program)
    pub fn lookup(&self, n: &str) -> Option<u32> {
        for s in self.scopes.iter().rev() {
            for (x, v) in s.iter().rev() {
                if x.len() == n.len() && x.as_bytes() == n.as_bytes() {
                    return Some(*v);
                }
            }
        }
        None
    }

}

/// 形の種類の番号 (`std::mem::discriminant` の代わり)
pub fn shape_tag(s: &Option<Shape>) -> u8 {
    match s {
        None => 0,
        Some(Shape::Known(_)) => 1,
        Some(Shape::Ref(..)) => 2,
        Some(Shape::Adt(..)) => 3,
        Some(Shape::Arr(..)) => 4,
        Some(Shape::Slc(_)) => 5,
        Some(Shape::Ptr(..)) => 6,
        Some(Shape::Clo(..)) => 7,
        Some(Shape::Bx(_)) => 8,
    }
}

/// A type that is no other type whatever its parameters (an integer, `bool`, `()`): not a struct, reference, pointer
/// or other type that a coercion or a type parameter may stand for
fn scalar_ty(t: Ty) -> bool {
    !matches!(t, Ty::Struct(_) | Ty::Ref(..) | Ty::Ptr(..) | Ty::Box(_) | Ty::Array(..) | Ty::Slice(_) | Ty::Assoc(_) | Ty::Param(_) | Ty::Fn(_) | Ty::Dyn(_) | Ty::Never)
}
