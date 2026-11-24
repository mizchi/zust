# trait の解決、最初の一歩: impl の境界を入れ子の goal にする (#2)

日付: 2026-11-24

[#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011) は、rustc の次世代 trait solver の設計に向けた 6 段階を計画している。これはその段階 2 の中心にあたる。型の impl を選ぶのは、その型が impl の境界を満たすときだけにし、その境界をそれぞれ独立した goal として証明する。

## 何がおかしかったか

`Program::implements(t, tr)` は、構造体については、その基の型に対する `tr` の impl がプログラムに*何か*あれば「はい」と答えていた。`tr` にブランケット impl があれば、どんな型にも「はい」と答えていた。impl の自己型と境界は見ていなかった。次のどれも受理されていたが、rustc 1.97 はどれも E0277 で拒否する:

```rust
trait Bar {}
trait Foo { fn foo(&self) {} }
struct W<T>(T);
impl Bar for i32 {}
fn need<F: Foo>(_: F) {}

impl<T: Bar> Foo for W<T> {}          // need(W(1u8))、W(1u8).foo()
impl<T> Foo for W<T> where T: Bar {}  // need(W(1u8))
impl<T: Bar> Foo for T {}             // need(1u8)
impl Foo for W<i32> {}                // need(W(1u8))
```

`Program::impl_applies` は、impl の自己型をインスタンスと突き合わせ、境界も確かめていた。ただ、trait の境界の判定はそれを呼んでいなかった。使っていたのはメソッド呼び出しの一つの経路だけだった。

## 変えたこと

- **構造体とタプルへの impl**: `implements_concrete` は `impl_applies` も求める（`impl Foo for (i32,)` は `(u8,)` への impl ではない）。impl の自己型が一致し、境界が成り立たなければならない。境界は、型パラメータの境界と、それらに対する where 句の境界で、要約がもともと一つにまとめている。
- **ブランケット impl**: `implements` が `tr` のブランケット impl を使うのは、`t` がその境界を満たすときだけ（`blanket_holds`）。trait オブジェクトは例外にした。`dyn Trait` は `Trait` の supertrait を自分で実装するので、ブランケット impl が何を求めても関係ない（`traits/stack-error-order-dependence.rs`）。
- **入れ子の goal**: それぞれの境界を `goal_holds` で証明する。`goal_holds` は証明中の goal のスタックを持つ。
  - すでにスタックにある goal（循環: `impl<T: Q> P for T` と `impl<T: P> Q for T`）は成り立つとみなす。
  - 64 段より深い goal（オーバーフロー）も同じ。
  - どちらにも当たらずに証明した goal は、いちばん外の goal が終わるまで覚えておく。`impl<X: T1 + T0> T2 for X`、`impl<X: T2 + T1> T3 for X`、… のような連なりで、goal を 2^n 回ではなく一度ずつ証明する（`T35` で、以前は 4.2 秒、今は 6 ミリ秒）。循環やオーバーフローに当たった goal は推測に頼っているので、覚えない。
  - どちらも、rustc が出さないかもしれないエラーではなく、以前の答えを返す。新しい solver はこれらを「不明」にし、auto trait を通る循環は余帰納的に扱う。
- **impl ごと**: ブランケット impl はそれぞれ自分の境界を持ち、境界がすべて成り立つものが一つあればよい。`impl<T: A> Tr<u8> for T` と `impl<T: B> Tr<u16> for T` があるとき、`i32: Tr<u8>` に `A` と `B` の両方を求めない。
- **生ポインタ**: impl の自己型は、可変性が同じときだけ一致する。参照はもともとそうで、生ポインタもそうした。`impl Foo for W<*const i32>` は `W<*mut i32>` への impl でも `W<&i32>` への impl でもない。
- **プログラム自身の trait だけ**: `impl_applies` がしていたのと同じく、std の trait の境界は成り立つとみなす。zustc の std の trait への答えは場合ごとに書かれていて（`implements_std`）、そこでのエラーは誤ったエラーになる。
- **supertrait**: `trait B: A` の impl は、自己型が `A` を実装していることを必要とする（`privacy/sealed-traits/false-sealed-traits-note.rs`）。これは、型パラメータのない impl について、プログラム自身の supertrait に対して確かめる。自己型に trait オブジェクトを含むものは確かめない。zustc は `&dyn T2` と `&(dyn T2 + 'static)` を文字列で区別するが、rustc はライフタイムだけが違う一つの型として見る。
- **参照への impl**: `impl Tr for &mut i32` は `i32` への impl としても記録されていた。そのため `fn f<T: Tr>` に対する `f(1i32)` が受理されていた（`suggestions/issue-84973-2.rs`）。
  - そういう記録には印を付けた（`Program::prim_ref_only`）。プリミティブへの境界はそれを数えない（`prim_has`）。
  - メソッドの検索はこれまでどおり見つける。`impl First for &&u32` での `First::m(&rr)` がそれに頼っているから（`tests/probe/ref_impl_layers.rs`）。
  - そういう impl での trait のデフォルトメソッドの `Self` も数える。zustc はそれをプリミティブでインスタンス化するから。

## 測った結果

main（affe051）に対する `bench/rust_ui.py --check`:

- compile-fail 4679 → 4683（+4）: `privacy/sealed-traits/false-sealed-traits-note.rs`、`privacy/sealed-traits/sealed-trait-local.rs`、`suggestions/issue-84973-2.rs`、`trait-bounds/super-assoc-mismatch.rs`。どれも、テストが試すエラーで拒否されていることを確かめた。
- run-pass、check-pass、build-pass は変わらず、回帰は 0 件。ベースラインとラチェットを書き直した。
- `associated-types/mismatched-types-in-trait-impl-65230.rs` は、最初の版では別の理由（上の、trait オブジェクトの文字列）で通っていた。trait オブジェクトの例外はそこから来ている。

tests/ui での伸びが小さいことは、穴がどこにあるかを示している。zustc が受理してしまう E0277 の compile-fail テスト 59 件の多くは、std の trait（`Sized`、`Send`、`Copy`、`Iterator`）、構造体の境界の well-formedness（#81）、関連型、ジェネリックな本体についてのもの。この段階はそのどれにも触れていない。直したのは上の場合で、`tests/check.rs` の `impl_bounds_are_nested_goals` にある。このテストは main では失敗する。

`zustc check -j 1` の命令数（cachegrind）、main とこの変更:

| | main | この変更 | 変化 |
|---|---:|---:|---:|
| gen5000 | 3802.20 M | 3802.18 M | -0.00% |
| patterns（`mix 8`） | 1353.24 M | 1354.25 M | +0.07% |
| traits（`gen_traits.py 300`） | 10079.76 M | 10095.03 M | +0.15% |

`cargo test --release --workspace` は、この環境では main でも失敗する `crates/analyzer/tests/lsp.rs` の 2 件を除いて通る。

## 残っていること（#2 の次の段階）

- `bool` の代わりに、独自の答え（はい、いいえ、不明）と推論の制約を持つ goal。「不明」がないと、推論は goal を待つことも、goal に導かれることもできない。`traits/next-solver` の「cannot infer type parameter」という誤ったエラーはそこから来ている。
- 関連型への境界（`where T::A: Bar`）を normalizes-to の goal にすること。
- 候補の優先順位: impl より where 句を先に、そして alias の境界。
- std の trait を goal にすること。誤ったエラーを出さないほど答えが正確になってから。
- パラメータ以外への where 句（`impl<T> Foo for T where W<T>: Bar`）は、以前と同じく確かめない。impl はどの `T` にも使われる。確かめるには、句の型に impl のパラメータを入れる必要がある。
- 循環（`impl<T: Q> P for T` と `impl<T: P> Q for T`）は成り立つとみなす。rustc では、それを通る要求（`fn req<T: P>` への `req(1u8)`）は E0275 になる。誤ったエラーではなく誤った受理で、「不明」が入るところ。
- ブランケット impl の trait の引数を goal の引数と突き合わせていない。`impl<T: A> Tr<u8> for T` と `impl<T: B> Tr<u16> for T` があると、`i32: Tr<u16>` が `i32: A` で成り立ってしまう。以前と同じ誤った受理。goal の引数は `implements` に文字列で届き、文字列で比べると型エイリアスを通して誤ったエラーになる。
- ジェネリックな impl の supertrait（`impl<T: A> B for W<T>` は、impl 自身の境界から `W<T>: A` を必要とする）。
- ジェネリックな本体: 何も失敗する型でインスタンス化しないジェネリック関数の、プレースホルダでだけ失敗する境界は報告されない。プレースホルダでの検査は、境界のメソッドを呼ぶ本体を飛ばす。
