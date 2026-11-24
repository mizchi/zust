# 最後のフィールドが unsized な struct、その 2: `dyn` の末尾、`size_of_val`、Box

日付: 2026-11-24

[その 1](2026-11-24-04-dst-struct-tails.md) (スライスの末尾) の「やっていないこと」を順に進めた。

## vtable が大きさと揃えを持つ

`dyn Tr` の末尾の位置は具体型で決まる。`struct Foo<T: ?Sized> { tag: u8, inner: T }` の末尾は、`Foo<u8>` では 1、
`Foo<u64>` では 8 にある。`&Foo<dyn Tr>` は `(データ, vtable)` なので、位置は vtable から分かる必要がある。そこで
どの vtable にも、drop の欄の後ろに具体型の大きさと揃えを数として置くようにした。関数の番号の中では
`VT_LIT | n` で表し、オブジェクトの書き出しはその数を書く。オブジェクトのキャッシュにも数の欄のタグを足し、
キャッシュの形式の版を上げた。

## `dyn` の末尾

- struct の末尾のフィールド `T` が unsized なのは、`T` が `[U]`・`str`・`dyn Tr`・そういう struct のときで、
  さらにそのパラメータが `?Sized` と宣言されている場合だけ。`struct Bar<T>(T)` の `Bar<dyn Debug>` は rustc では
  エラー (E0277) で、unsized な struct ではない。そのため summary に struct ごとの `?Sized` なパラメータ
  (`adt_unsized`) を記録するようにした。この確認がないと `structs/field-implied-unsizing-wfcheck.rs` が失敗しなく
  なっていた。
- 強制変換 `&S<T>` → `&S<dyn Tr>` は `(アドレス, T の vtable)` を作る。`&mut`・生ポインタ・`Box` と、入れ子の末尾
  (`struct Outer<T: ?Sized> { a: u16, f: Foo<T> }` での `&Outer<u64>` → `&Outer<dyn Tr>`) にも効く。vtable は
  `&T` → `&dyn Tr` と同じ `to_dyn` で作るので、トレイトの確認 (E0277) も同じになる。unsized な型の引数は、定義の
  最後のフィールドを unsized な末尾と照合して求めるようにした。以前は末尾と同じ引数を置き換えていたが、これは
  末尾が `T` でなく `Foo<T>` のときに誤る。
- フィールドへのアクセスでは、末尾の位置を実行時に計算する: 静的な位置 (前の部分の終わり) を、vtable から読んだ
  揃えに切り上げる。入れ子の末尾では、struct の静的な揃えと、その末尾の揃えの大きい方を使う。
- `dyn` の場所へのメソッド呼び出し (`x.inner.get()`) は、`&dyn Tr` と同じく vtable を通す。
- 推論: 印付きインスタンスの引数のうち `dyn Tr` のものは、リテラルの引数と単一化しない (Tr を実装する型なら何で
  もよい)。unsized になりうる同じ struct の 2 つのインスタンスで、引数が違うと分かっているもの
  (`[&Foo<dyn Get>; 2]` としての `[&Foo<u8>, &Foo<u64>]`) はつながない。要素はそれぞれ別に強制変換される。

## `size_of_val` / `align_of_val`

これまではどの T でも `size_of::<T>()` だったので、`size_of_val(&[1u32, 2, 3][..])` が 0 になっていた。unsized な
型へのポインタでは、組の後半の語を読むようにした:

- スライス: 長さ × 要素の大きさ
- `str`: 長さ
- `dyn Tr`: vtable の欄
- struct: 末尾の実行時の位置 + 末尾の大きさを、大きい方の揃えに切り上げたもの

## Box

`Box<Foo<[T]>>` と `Box<Foo<dyn Tr>>` には専用の drop glue がある。sized なフィールドを drop し、次に末尾を drop
する (スライスなら要素ごと、`dyn` の末尾は vtable の drop の欄で、入れ子の末尾は再帰で)。最後に領域を解放する。
`&Foo<..>` が期待される場所の `&Box<Foo<..>>` は、Box の組そのものになる。`&Box<T>` → `&T` の規則は Box を通して
読み込んでいたが、これはこの Box では誤り。`[T] | str | dyn` を手で並べていた箇所 (Box の deref の強制変換、
fat から thin へのポインタのキャスト、unsized な場所を値として読む確認) は `Ty::is_unsized` を使うようにした。

## ついでに直したもの

- `&[u8]` に対する `assert_eq!(slice_ref, [1, 2, 3])` と `assert_eq!(*s, [1, 2, 3])` で、リテラルの要素が `i32`
  になっていた (誤った「mismatched types」)。`assert_eq!` は両辺を `==` と同じ規則で単一化するようにし、`==` は
  参照越しにスライスの要素を見るようにした。
- `b: Box<dyn Tr>` や `&str` の `*b` は、スライスの参照の `*` と同じく場所になる。それを値として読むのは E0277
  (「the size for values of type ... cannot be known」) で、「dereferencing a slice is not supported」ではない。

## テスト

- `examples/dst_dyn_tails.rs` (期待出力は rustc のもの): 揃え 1・4・8 の末尾を持つ `&Foo<dyn Get>`、`&mut`、入れ子の
  末尾、`dyn Debug`、`Box<Foo<dyn Tr>>`、パターン、あらゆる種類の `size_of_val` / `align_of_val`、
  `Box<Foo<dyn Tr>>` / `Box<Foo<[T]>>` の drop。
- `tests/compile_errors/dst_struct_unsize_dyn_unimplemented.rs`: `impl Get for u8` がないのに `&Foo<u8>` を
  `&Foo<dyn Get>` にする。

## 結果

退行はない。新しく通るもの: `dst/match-dst-struct.rs`、`dst/dst-trait.rs`、`unsized/unchanged-param.rs`、
`methods/method-probe-no-guessing-dyn-trait.rs`。compile-fail では `dst/dst-bad-assign.rs`、`dst/dst-bad-assign-2.rs`、
`dst/dst-bad-assign-3.rs`、`dst/dst-bad-coerce3.rs`。どれも行だけでなく rustc のエラーの内容とも照らし合わせた。

| 種類 | 前 | 後 |
|---|---:|---:|
| run-pass | 1863 | 1868 |
| check-pass | 1527 | 1527 |
| build-pass | 104 | 104 |
| compile-fail | 4555 | 4559 |

その 1 で失敗していた analyzer のテスト 2 つ (`hover_and_completion`、`type_inference_like_rust_analyzer`) は
`rust-src` を必要とする。CI はこれを入れているが、このコンテナには入っていなかった。コンポーネントを足すと通る。

## main のマージ

main は 374 コミット進んでいて、その中に main 自身のトレイトの作業 (#2: `Program::evaluate`、ゴールの答えを
yes・maybe・no で返す) があった。2 つが出会うのはインスタンスの境界の確認 (`Insts::request`):

- main の循環 (`Maybe(Cycle)`) を先に E0275 として報告する。rustc も同じ: `tests/compile_errors/trait_solver_cycle.rs`
  の期待は E0277 でなく E0275 にした。
- そのあとにこのブランチの trait solver の確認を行う。その std 側はもともと `implements` に聞いており、main ではそれが
  `evaluate(..) != No` になっている。
- プログラムのトレイトで solver が燃料を使い切る場合 (60 段の blanket impl の連鎖、main の
  `impl_bounds_are_nested_goals`) は、引数のない境界に限って `evaluate` の no で決める。`evaluate` はライフタイムを
  読まないので、`for<'a> Yokeable<'a>` では誤った no になる。
- Self がプリミティブになるデフォルトメソッドのインスタンスを飛ばす main の規則は、本来の対象である「参照への impl
  だけがある」場合 (`prim_ref_only`) に限った。そうしないと `impl<'a> Visit for () where (): Array<Element = &'a ()>`
  が飛ばされていた。
- このブランチの `-> impl Tr` の確認は、末尾が `if` / `match` の本体を腕の確認に任せる: 記録される型は最初の腕の
  もので、型の違う腕は E0308 になる。

両方の baseline を合わせたものに対して退行はなく、さらに 3 つ通るようになった。

| 種類 | main (その ratchet) | このブランチ | マージ後 |
|---|---:|---:|---:|
| run-pass | 1858 | 1868 | 1892 |
| check-pass | 1525 | 1527 | 1540 |
| build-pass | 102 | 104 | 104 |
| compile-fail | 4640 | 4559 | 4750 |

## レビューでの修正 (#128)

プルリクエストのレビューで指摘されたもの。どれも先にそれぞれのプログラムを書き、rustc と比べてから直した:

- 誤った結果だったもの (正しくなった):
  - dyn 呼び出しが別の self 型の impl を選んでいた (`W<u8>` に対して `impl Conv<u8> for W<u16>`)。
  - 期待型が `B::f` の戻り値の型だと、`A::f(&s)` が `B::f` を選んでいた (今は rustc と同じ E0308)。
- 誤ったエラーだったもの (出なくなった):
  - `<Self as Parent>::A` に対する `where Self: Child` (`trait Child: Parent`)。
  - 境界で縛った関連型が一致しない impl への E0119 (`impl<T: Select<Item = u8>> Mark for T` と `impl Mark for u8`、
    u8 の `Item = u16`)。
  - `impl<const N: usize> Foo<N> for Const<N>` に対する `foo(Const)`。
  - impl 自身の where 句が与えるスーパートレイト (`impl<T> Child for W<T> where W<T>: Parent`)。数えるのは self 型
    そのもの (か `Self`) についての述語だけで、`where T: Parent` は今も E0277。
- 見逃していたエラーを報告するようにした (rustc と同じ): 生ポインタへの `size_of_val` と `*const` への
  `drop_in_place` (E0308)、`S: Local` がないときの `-> impl Local` で、腕が `S` と `S` を返す呼び出しの末尾 `if` (E0277)。
- zustc が評価できない const 引数の射影 (`<S as Pick<{ match .. }>>::Out`) は、当てはまる impl が 1 つならそれを
  取る。複数あるとき、または self 型の impl にトレイト引数を解決できていないもの (`impl Pick<{ <u8 as K>::N }>`)
  があるときは、self 型だけで最初の impl を選ばず「未対応」のエラーにする。
- 型エイリアスを通した末尾 (`struct Outer<T: ?Sized>(u16, Alias<T>)`) は、`Outer` のフィールドを解決する前に
  インスタンスを求められても unsized とする。
- prep: 文字列や括弧を含む属性の後のアイテム (`#[doc = "]"] fn inner`) があるソースは、全体の前処理に回す。
- main が報告していて見逃すようになっていたエラーを戻した: `impl Mark for u8` だけがあるときの `require::<i32>()`。
  solver はゴールの i32 を、zustc が早く既定の型にしたリテラルかもしれないので任意の整数として扱う。turbofish の
  ある呼び出しでは型が書かれているので、厳密に確かめる。
- レイアウト: クロージャのローカル 1 つを並べた `[c, c]` は、fn ポインタの配列でなくクロージャの配列 (大きさ 0) の
  ままにする。
- unsized な末尾:
  - メソッドの受け手 `&Wrap<[u8]>` (Box 越しも) が組を保つ。
  - 明示的な `&Wrap { .. }` パターン。
  - 再帰する struct のインスタンスは、再帰した要求に番号を返す前に印を付ける。
  - struct の適用を通した入れ子の末尾 (`Outer<T>(u32, Middle<T>)`、`Middle<T>(u16, Inner<T>)`)。
  - Box を drop するとき、struct 自身の `impl Drop` を (組を `&mut self` として) フィールドより先に呼ぶ。
  - `Box<W<[u8]>>` が期待される `Box::new(W { .. })` は、値を配列の型で作る。
  - vtable の数の欄は 1 GiB までの大きさを持ち、それより大きい型は大きさ 0 でなくエラーにする。
  - ほかに型が決まらない引数は、推論の最後にスライスにする。
- 見逃していたエラー: 境界と修飾付きの射影で const 引数を保つ (`S: Tr<2>`、`<S as Tr<2>>::Item`)。`W: Fn() -> u8`
  の後ろの where 節も読む (`->` の `>` は何も閉じていなかった)。
- turbofish の位置は const 引数も数える (`f::<3, u8>`)。`() == &()` を `true` に畳まない。
- 頑丈さ: E0119 の自由なパラメータの印は予約された印より下に収める。occurs check は束縛をそれぞれ 1 度だけ訪れる
  (束縛されたパラメータの数に対して指数的だった)。unsizing をたどる処理は同じ組を 2 度訪れない。
- check のメモのキーに `dangerous_implicit_autorefs` の水準を入れた。
- `formal/run.sh lean` は propext・Classical.choice・Quot.sound 以外の公理と、ライブラリのどこかの `sorry` を
  失敗にする。`Solver.lean` の概要に `solve_complete` の前提を書いた。
- `examples/dst_review.rs` が編集対象の例に加わると、`tests/prep_inc.rs` が古い食い違いを見つけた: 入れ子のアイテム
  の持ち上げは字下げしたアイテムしか確かめないので、行の途中の波括弧の後ろのアイテム (`f({ fn inner() {} })`) を
  動かすかどうかがソースの残りで変わり、単独で前処理したチャンクが食い違っていた。そういうソースは差分の前処理を
  使わず、全体の前処理に任せる。

残っているもの (TODO.md): `vec!`・配列・腕の中の Box のリテラルの unsizing、リントの範囲、そこに挙げた見逃し
(main にもある)。

## やっていないこと

- 末尾が unsized な struct に利用者の `impl Drop` があっても、その Box を drop するときに呼ばれない (フィールドは
  drop される)。
- `str` の末尾はテストしていない。それを作る強制変換がない (rustc にもない)。
