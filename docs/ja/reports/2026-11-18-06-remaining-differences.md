# TODO.md の rust-analyzer との残りの違いを詰める

日付: 2026-11-18

TODO.md に残っていた項目 (zust-analyzer の答えが rust-analyzer と違うもの、fixture の残り、参照の速さ) に取り組んだ。
それぞれを小さなクレートで再現して rust-analyzer の答えと比べ、fixture のワークスペース (`bench/ide_golden_ra.py`) と
zustc 自身のソース (`bench/lsp_parity.py`) で確かめた。

## 型推論

- **前置の演算子。** 生ポインタ `*const T` への `&*p` は `&T`、`&T` / `Box<T>` への `*b` は `T`。前に付いた `&` / `&mut` /
  `*` を、演算子のかかる式の型に当てはめる (後ろに二項演算子があるときは除く)。生ポインタの注釈 (`p: *const T`) も
  受け付ける。`unsafe { .. }` のブロックも、普通のブロックと同じく中を見る。
- **組み込みの型の関連定数。** `u32::MAX` が `char`、`i64::MIN` が `f32` になっていた。小文字の修飾をモジュールとして
  読み、古いモジュールの定数 `core::char::MAX` / `core::f32::MIN` を拾っていた。組み込みの型の名前は型として扱うように
  したので、`u32::MAX` は `u32`、`usize::BITS` は `u32` (定義の行き先も rust-analyzer と同じ)。
- **同じ名前を頭に持つ別名。** `std::io::Result<T>` (= `Result<T, Error>`) が開かれていなかった。右辺が同じ名前の別名は、
  ループを避けるために飛ばしていたため。型引数の数が違えば開くようにし、メソッドを引くときは impl の型引数の数に合う
  まで受け手を開く。`fs::write(..).map_err(|e| e.to_string())` の `e` は `io::Error`。同じ名前の別名がいくつもあれば、
  同じファイル、同じクレート、ワークスペースの順に選ぶ。
- **範囲のループ変数。** 括弧のない `for k in 0..n` の `k` は `n` の型、`for k in 0..3` は `i32` (rust-analyzer の既定)。
  `a..` は `RangeFrom<T>`。
- **配列の値。** `["a", "b"].into_iter()` が `[&str; 2]::IntoIter` のままだった。射影の前の型を、`[&str; 2]` の中の空白で
  切っていた。今は `IntoIter<&str>` で、`find(|k| ..)` の `k` は `&&str` (なので `k.len()` は rust-analyzer と同じく
  `str::len`)。
- **タプルのパターンの match の腕。** `(TokenTree::Literal(x), TokenTree::Literal(y)) => ..` で、`x` から後ろへ戻ると前の
  腕の本体を越え、その `==` で止まっていた。先に腕のパターンの範囲を求めるようにした。
- **依存クレートの繰り返し。** serde_json の `Map::iter()` は `map::Iter` (型引数なし) を返す。これを `option::Iter<'a, A>`
  と、次に `std::path::Iter` と取り違えていた。メソッドが返す型引数のない型は、名前がほかと重なればモジュールで修飾し、
  関連型はそのモジュールの中だけで引く。impl そのものはマクロで作られているので、要素の型は間違いではなく「分からない」
  になった (TODO.md)。
- **束縛での `zustc/typeAt`。** デバッグ用の問い合わせが、使う所だけでなく束縛そのもの (パターン・引数・クロージャの引数)
  でも答える。

## 定義と参照

- **`name!(..)` はマクロだけに解決する。** `vec!` が `pub mod vec` に、`format!` がどこにも行かなかった
  (`fn fmt::format` と重なっていた)。`format!` の参照は rust-analyzer と同じ 3 件になった。
- **str のパターンの繰り返し。** `s.split(',').next()` が slice の `Split::next` に行っていた。core のマクロ
  `generate_pattern_iterators!` の中のメソッドを `__str_pat_iter` として索引し (`uint_impl!` と同じ)、受け手と型引数の数が
  合わない impl を外す。
- **クレートの名前。** サーバが、どのファイルが各クレートの根か (ワークスペースのクレート、std / core / alloc、依存
  クレート) を zust-analyzer に伝える。`std` と `fixture` はクレートの根へ、`std::collections` は `collections/mod.rs` へ
  行く。ワークスペースのクレートの参照には、rust-analyzer と同じく、その根を指す `crate::`・`pub(crate)`・`super` も
  入る (`fixture` で 7/7)。ライブラリのクレートには別々のクレート番号を付け、`mod x;` を同じクレートの中で解決する。
- **`cfg(miri)`。** rust-analyzer の `cargo.cfgs` の既定は `["debug_assertions", "miri"]` なので、`#[cfg(miri)]` のアイテムは
  有効。zust-analyzer も同じにした。
- **cfg の述語の中の名前。** `#[cfg(test)]` の `test` を、属性マクロ `#[test]` の参照に数えていた。`#[cfg(..)]`・
  `#[cfg_attr(..)]` の述語・`cfg!(..)` の中の名前は解決しない。
- **ゴールデンのテスト。** ide のゴールデンのテストも、サーバと同じくクレートの根を設定し、ライブラリのファイルに
  クレート番号を付ける。fixture の小さな std に、fixture が使うマクロと属性 (`assert!`・`assert_eq!`・`vec!`・`format!`・
  `test`・`derive`・`Copy`) を足し、`bench/ide_golden_ra.py` が同じ条件で比べられるようにした。

## 速さ

参照はファイルごとに独立に探す。slim なファイルを先にメインのスレッドで読み直し、そのあとファイルを最大 8 本の
スレッドで探す (ファイルが 8 個以上のとき)。4 コアで、zustc 自身のソースの遅い問い合わせ (よくある名前のメソッド) は
110〜230 ms から 55〜205 ms になった。

## 結果

`python3 bench/ide_golden_ra.py` (fixture を rust-analyzer と比べる):

| | 前 | 後 |
|---|---:|---:|
| definition | 465/467 | 467/467 |
| references | 609/629 | 627/629 |
| hover | 615/636 | 633/636 |

残った参照の 2 件は std のモジュールで、rust-analyzer は std のソースの中のパスも数える。

`python3 bench/lsp_parity.py`:

| 対象 | 一致 (zust-analyzer / rust-analyzer) | references の p50 / p90 |
|---|---|---|
| synth | 99.7% / 99.1% | 0.4 / 0.9 ms |
| fixture | 97.3% / 95.6% | 0.3 / 0.9 ms |
| zustc | 96.3% / 95.5% | 2.9 / 32.8 ms (前は 4.2 / 65 ms。rust-analyzer は 14.2 / 880 ms) |
| ide | 99.0% / 97.5% (+0.9。references は 100%、+2.5) | 4.6 / 19.1 ms |

zustc の対象で残った違いは、意図した `Default::default` の rename と、rust-analyzer が `crates/syntax/build.rs` (ビルド
スクリプト自身の `type Key = u128`) で数える `Key` の参照 1 件。メモリは 66 MB のまま。残りは TODO.md にある。
