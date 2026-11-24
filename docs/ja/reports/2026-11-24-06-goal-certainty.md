# trait の解決、段階 1: goal の答えを「はい」「不明」「いいえ」にする (#2)

日付: 2026-11-24

[#2](https://github.com/mizchi/zust/issues/2#issuecomment-7110129011) は、rustc の次世代 trait solver の設計に向けた 6 段階を計画している。その段階 1 は、確実さを持つデータとしての goal だ。`T: Trait` への答えを `bool` ではなく、「はい」「不明」（曖昧かオーバーフロー）「いいえ」にする。前の段階（[2026-11-24-05](2026-11-24-05-impl-bounds-nested-goals.md)）は impl の境界を入れ子の goal にし、循環とオーバーフローを成り立つとみなした。この段階ではその二つに独自の答えを与え、それを一か所で使う。goal が自分自身を必要とする境界は、rustc と同じく E0275 にする。

## 変えたこと

- **`Certainty`**（`crates/sema/src/program.rs`）: `Yes`、`Maybe(Cycle | Overflow)`、`No`。候補と境界の上での `or` / `and` と `any` / `all` を持つ。候補の間では `Yes` が `Maybe` に、`Maybe` が `No` に勝ち、境界の間ではその逆になる。
- **`Program::evaluate(t, tr) -> Certainty`** が goal に答える。`implements` は `evaluate != No` なので、`implements` を呼ぶところはどれも以前と同じものを見る。`Maybe` は成り立つ。
- 入れ子の goal（`goal`、以前の `goal_holds`）は、証明中に再び出会った goal には `Maybe(Cycle)` を、64 段より深い goal には `Maybe(Overflow)` を返す。ブランケット impl（`blanket_certainty`）と構造体やタプルへの impl（`impl_certainty`）は、境界の答えを組み合わせる。
- `Maybe` を持つのは、プログラム自身の trait の goal だけ。`implements` の残りは場合ごとに書かれていて、`Yes` か `No` を返す。構造体やタプルへの impl の答えは `implements_concrete` から `bool` として出てくるので、その横で `evaluate` に渡す（`impl_applies_noted`）。その答えは `implements_concrete` 自身の戻り値なので、間で何も走らない。ほかの経路（derive した impl や `Box` の引数）で失われた `Maybe` は `Yes` になり、以前と同じく受理する。
- **E0275**: インスタンスの境界（`Insts::request`）が `Maybe(Cycle)` なら、「overflow evaluating the requirement」（E0275）を出す。プログラムの trait の goal は帰納的なので、自分自身を必要とする goal は成り立たない。エラーは rustc が出す行に出る。
- `Maybe(Overflow)` はこれまでどおり受理する。rustc もそこで E0275 を出すが、64 段ではなく自身の再帰の上限（既定で 128）でのことなので、ここでのエラーは誤ったものになりうる。
- 境界が自分の trait である impl（`impl<T: P> P for T`）も goal になり、したがって循環になる。以前はその境界を飛ばしていた。

```rust
trait P {}
trait Q {}
impl<T: Q> P for T {}
impl<T: P> Q for T {}
fn need<X: P>(_: X) {}
fn main() { need(1u8); } // E0275: overflow evaluating the requirement `u8: P`
```

エラーでない循環は受理したままにした。どれも rustc 1.97 と突き合わせた（`tests/check.rs` の `goals_have_a_certainty`）:

- 同じ impl で、それを通る要求がないもの
- ジェネリック関数自身の境界（`fn g<Y: P>(y: Y) { need(y) }`: where 句で、たどらない）
- 互いを通る構造体への impl が、具体的な impl で終わるもの（`W<u8>: P` が `u8: Q` で成り立つ）
- 再帰的な型のリスト（`impl<H: P, T: P> P for (H, T)`）
- 一つの trait の二つのブランケット impl で、一方が成り立ち、もう一方の goal が循環するもの

## 測った結果

main（d6202b6）をマージした上での `bench/rust_ui.py --check`:

- compile-fail は main に対して 4628 → 4635（+7）。うち 3 件がこの段階のもの:
  - `traits/inductive-overflow/simultaneous.rs`
  - `traits/inductive-overflow/supertrait.rs`
  - `traits/inductive-overflow/two-traits.rs`

  どれも、テストが示す行で E0275 として拒否される。残りの 4 件は前の段階のもの。
- run-pass、check-pass、build-pass は変わらず、回帰は 0 件。ベースラインとラチェットを書き直した。
- `traits/inductive-overflow/lifetime.rs` はまだ受理される。その循環は関連型への境界 `where T::P: NotAuto` を通るもので、段階 3 にあたる。

`zustc check -j 1` の命令数（cachegrind）、前のコミット（ef23222d）とこの変更:

| | 前 | この変更 | 変化 |
|---|---:|---:|---:|
| gen5000 | 3802.12 M | 3802.10 M | -0.00% |
| patterns（`mix 8`） | 1354.96 M | 1355.02 M | +0.00% |
| traits（`gen_traits.py 300`） | 10096.80 M | 10097.77 M | +0.01% |

`cargo test --release --workspace` は、この環境では main でも失敗する `crates/analyzer/tests/lsp.rs` の 2 件を除いて通る。

## 残っていること

- goal はまだ `(Ty, trait の名前)` のまま。前の段階から残った二つの誤った受理には、trait の引数を型で持つデータとしての goal が要る。ブランケット impl の trait の引数（`Tr<u8>` と `Tr<u16>`）と、パラメータ以外への where 句（`where W<T>: Bar`）だ。
- `Maybe` はまだ推論に届かない（段階 4）。`traits/next-solver` の「cannot infer type parameter」という誤ったエラーには、`bool` ではなく、止まった goal を後でやり直すことが要る。
- auto trait は goal ではないので、余帰納的な循環はまだない（段階 5）。`Send` と `Sync` は構造的に判定していて（`auto_trait`）、再帰的な型はすでに受理している。
