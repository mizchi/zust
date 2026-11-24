# trait の解決、段階 3: 射影を normalizes-to の goal にする (#2)

日付: 2026-11-24

[#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011) は、rustc の次世代 trait solver の設計に向けた 6 段階を計画している。その段階 3 は、正規化を goal にすることだ。`T: Iterator<Item = u8>` や `where T::Item: Bar` のような境界を、関連型を名前で突き合わせる代わりに、`<T as Iterator>::Item` への goal にして `Iterator` の impl で解く。前の段階で、impl の境界を入れ子の goal にし（[2026-11-24-06](2026-11-24-06-impl-bounds-nested-goals.md)）、goal に確実さを持たせた（[2026-11-24-07](2026-11-24-07-goal-certainty.md)）。

## 何がおかしかったか

境界の束縛を確かめていなかった。`Program::implements(t, "Trait<AssociatedType = u32>")` は `<` から後ろを捨てていた。型パラメータ以外を主語にする where 句は、要約が捨てていた。impl の関連型を、trait がそれに宣言する境界と突き合わせていなかった。次のどれも受理されていたが、rustc 1.97 はどれも拒否する:

```rust
trait Tr { type A; }
trait Bar {}
impl Tr for u8 { type A = u16; }

fn f<X: Tr<A = u32>>(_: X) {}             // f(1u8): E0271、<u8 as Tr>::A は u16
fn g<X: Tr>(_: X) where X::A: Bar {}      // g(1u8): E0277、u16: Bar
fn h<T>(_: T) where Vec<T>: Bar {}        // h(1u8): E0277、Vec<u8>: Bar

trait Tr2 { type A: Bar; }
impl Tr2 for u8 { type A = u16; }          // E0277、u16: Bar
```

関連型は名前だけで引かれてもいた（`Program::assoc_of`）。同じ名前の関連型を持つ二つの trait があると、最初に見つかった impl が両方に答えていた。

## 変えたこと

- **normalizes-to の goal**（`Program::normalize(t, tr, name)`）: `<t as tr>::name` は、`tr` の impl が `t` に与える値。関連型はそれぞれ、自分の impl の trait を記録するようにした（`AssocDef::trait_name`）。
  - impl を数えるのは、その自己型が `t` にぴったり一致するときだけ（参照とポインタの可変性も含めて）。
  - impl が何も言わないとき、値にまだパラメータが残るとき、一致する二つの impl が食い違うとき（`impl Tr<u8> for X` と `impl Tr<u16> for X`）は、答えは不明（`None`）。
  - 答えは (型, trait, 名前) ごとに覚えておき、impl は名前と自己型の構造体で索引を引く。
- **束縛**（`Program::binding_mismatch`）: インスタンスの境界の束縛は、それぞれが goal になる。束縛の型は関数のパラメータで書かれていて、インスタンスの引数を入れる。別の型に正規化される束縛は、呼び出しの位置で E0271 になる。正規化できない束縛は成り立つとみなす。E0271 は食い違いに対するもので、わからないことに対するものではない。
- **プログラム自身の関数だけ**: prelude は、束縛を std の境界としてではなく、推論の手がかりとして書いている。`sum<S>` は `Self: Iterator<Item = S>` を求めるが、std が求めるのは `S: Sum<Self::Item>` だ。これを確かめると `v.iter().sum::<i32>()` を拒否してしまう。
- **ほかの型への where 句**（`SigSyn::preds`、`Program::fn_preds`）: 要約は、型がパラメータでない関数の where 句を、qualified path の trait とともに残す。それぞれのインスタンスがそれを証明する（`Program::pred_subject`）:
  - `<T as Tr>::A` は `Tr` で、`T::A` は `T` の境界の trait で正規化する。後者は答えが食い違ってはならない。
  - ほかの型（`W<T>`、`&T`）には引数を入れる。中に射影を含むものは除く。`subst` がその射影を名前だけで引いてしまうから。
  - 確かめるのは、プログラム自身の trait の境界。impl の入れ子の goal と同じ。
- **item bound**: プログラム自身の trait の、パラメータのない impl では、関連型を、trait がそれに宣言する境界（`type A: Bar;`）と突き合わせる。規則は supertrait の検査と同じで、その型そのものに対して書かれた impl は数え、`Box`、配列、スライスはそれ以上確かめない。

## 測った結果

main（b376a37）に対する `bench/rust_ui.py --check`。どちらもこの環境で走らせた:

- compile-fail 4695 → 4702（+7）、回帰は 0 件:
  - `error-codes/E0271.rs`
  - `associated-types/mismatch-two-relevant-impls.rs`
  - `associated-types/associated-types-eq-hr.rs`
  - `associated-types/point-at-type-on-obligation-failure-2.rs`（item bound）
  - `impl-trait/in-trait/dont-probe-missing-item-name-4.rs`
  - `suggestions/imm-ref-trait-object-literal-bound-regions.rs`（`where for<'b> &'b X: Trait`）
  - `where-clauses/where-for-self-2.rs`（`where for<'a> &'a T: Bar`。rustc は「not general enough」と言う。型にライフタイムを持たない zustc は、同じ行で境界が成り立たないと言う）
- run-pass、check-pass、build-pass は変わらず。ベースラインとラチェットを書き直した（compile-fail のラチェットは 4639 → 4646）。

`zustc check -j 1` の命令数（cachegrind）、main とこの変更:

| | main | この変更 | 変化 |
|---|---:|---:|---:|
| gen5000 | 3802.60 M | 3803.33 M | +0.02% |
| patterns（`mix 8`） | 1355.02 M | 1355.77 M | +0.06% |
| traits（`gen_traits.py 300`） | 10100.76 M | 10133.55 M | +0.32% |

最初の版は goal ごとにプログラムのすべての関連型を走査していて、traits で +1.56% かかった。キャッシュと索引でそれを下げた。

`cargo test --release --workspace` は、この環境では main でも失敗する `crates/analyzer/tests/lsp.rs` の 2 件を除いて通る。`tests/check.rs` の `projections_are_goals` に上の場合があり、それぞれ rustc と突き合わせた。このテストは main では失敗する。

## 残っていること

- impl の境界の中の束縛と射影（`impl<T: Iterator<Item = u8>> Foo for T`、`impl<T> Foo for T where W<T>: Bar`）は、まだ goal ではない。goal になるのは関数のものだけ。impl の入れ子の goal は、まだ (型, trait の名前) のまま。
- prelude の束縛は、推論の手がかりなので確かめない。確かめるには std の本当の境界（`S: Sum<Self::Item>`）が要る。
- prelude が書き出していない std の impl（`<i32 as Add>::Output`）は正規化できない。そういう goal は不明で、成り立つとみなす。
- ほかの型の中の射影（`where Vec<T::A>: Bar`）、ジェネリックな関連型、`dyn` 型の束縛は正規化しない。これまでどおりにしてある。
- クロージャの戻り値の型に対する E0271（`expected .. to return ..`）は別の検査で、射影ではない。
