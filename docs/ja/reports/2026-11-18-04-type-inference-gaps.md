# 型推論の不足を詰める (2026-11-18)

日付: 2026-11-18

rust-analyzer の hover の型を正解にして、よく書く式 39 個 (繰り返しの鎖・Option / Result・HashMap・タプル・
ターボフィッシュ・数の型のメソッド) を比べた。前は 17 個のうち 7 個しか合っていなかった (`Option<U>` /
`Option<Iter<'_, ..>::Item>` / 型が出ない) のが、39 個全部 (クロージャの型の表示も) 同じになった。この比べ方は
`crates/analyzer/tests/lsp.rs` の `type_inference_like_rust_analyzer` (本物の std を使う) に残した。

- **繰り返しの要素の型** (`iter_item`): `Iter<'_, T>` → &T、`IterMut` → &mut T、`IntoIter` → T、HashMap の
  `Iter<'_, K, V>` → (&K, &V)、`Map<I, impl FnMut(..) -> R>` → R、`FilterMap` / `FlatMap` / `Flatten` / `Enumerate` /
  `Zip` / `Cloned` / `Copied` / `Rev` / `Skip` .. 、`Chars` / `Lines` / `Range` ..。`Self::Item` / `<A as Iterator>::Item` /
  `U::IntoIter` の射影を置き換える (`norm_proj`)。
- **メソッドの型引数をクロージャから決める**: 定義に、シグネチャの `F: FnMut(Self::Item) -> B` などから
  (引数の番号, 型引数, 戻り値の形, 引数の形) を持たせる (`fn_bounds`)。呼び出しの引数のクロージャの戻り値の型を形に
  当てはめて (`unify`: `Option<U>` と `Option<char>` → U = char)、`map` / `and_then` / `filter_map` / `fold` の結果の型を
  決める。型引数そのものの引数 (`zip(other: U)`) は引数の式の型で。ターボフィッシュ (`collect::<Vec<_>>()` /
  `sum::<u32>()` / `parse::<u8>()`) は最初の型引数に (`_` は要素の型)。
- **クロージャの引数の型**: シグネチャの `F: FnOnce(&T)` の引数の形から (`KEY.with(|m| ..)` の m は &T、`fold` の 2 つめは
  要素)。分からなければ受け手の要素の型。タプルのパターン `|(k, v)|` / `for (i, x) in` / `let (a, b) =` も。
- **式の型**: タプルの式 `(a, b)`・タプルの添字 `x.0` (の後の添字 `u.1[0]` も)・括弧の式・配列の式 `[a, b]` / `[x; n]`・
  単位構造体の値・`match` / `if` / ブロックの式 (腕・ブロックの最後の式の型)・`&x` の参照・ローカルのクロージャの呼び出し・
  寿命の名前を外す (`&'a str` → `&str`)。
- **std のマクロの中の impl**: slice の `Iter` などの `impl Iterator` は core のマクロの中にあって見えないので、繰り返しの型に
  Iterator を持たせる。数の型のメソッド (`wrapping_sub` / `checked_sub` / `count_ones` ..) は core の `uint_impl!` /
  `int_impl!` の本体のアイテムを `impl __uint` / `impl __int` にして、u8 .. / i8 .. の持ち主に足す。数の型には `Ord` /
  `PartialOrd` / `Clone` / `Hash` .. も (`n.min(m)`)。`Iterator for Map<I, F>` の `B` のような impl の決まらない型引数は要素の型。
  `thread_local!` の static の型は `LocalKey<T>`。
- **型引数の束縛**: 受け手が型引数 `T` なら、`impl<T: Clone>` / `where T: Show` の trait のメソッド。
- **定義の行き先**: derive がある型の `Point::default()` / `x.clone()` は std の trait の宣言より derive の名前に (hover は
  trait のメソッドのシグネチャ、参照も数える)。`let h: Hidden = n.into();` は Hidden の `impl From<i32> for Hidden` の from。
  ライブラリの同じ持ち主・同じ名前のメソッドがいくつかのファイルにあるとき (`From` の impl) は先のもの。
- **構造体の省略形** `T { x }`: definition はローカルとフィールドの両方、documentHighlight も両方、rename は書き足す形
  (`x: new` / `new: x`)。`.` の後の補完には self を取らない関連関数を出さない。
- **長いシグネチャ**: 記号の detail を 120 文字で切っていたので、引数の多い関数の戻り値の型が取れていなかった
  (1,000 文字まで)。切ったシグネチャで std の索引のスレッドが落ちていた不具合も直し、std のファイル 1 つで索引を作るのに
  失敗しても、そのファイルだけ飛ばすようにした。

結果: fixture の definition は 462 → 465/467、references は 606 → 609/629。`python3 bench/lsp_parity.py` の一致の平均は
synth 99.7% / fixture 97.3% / zustc 96.3% (rust-analyzer はそれぞれ 99.1% / 95.6% / 95.5%)。ide (navigate.rs。作業の間に
本文が変わるので位置は毎回違う) は 95〜98% (rust-analyzer 95〜98%)。時間は前と同じ (references の p50 は zustc で 5.4 ms、
rust-analyzer 27.9 ms)。std のメソッドの参照 (`Iterator::map` など) は rust-analyzer の 8 割ほどを数える (前は 0)。
