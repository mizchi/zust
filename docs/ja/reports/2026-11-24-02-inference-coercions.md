# 正しいコードへの誤った「mismatched types」: リテラル、ジェネリックな引数、型強制

日付: 2026-11-24

[trait solver、その 5](2026-11-24-01-trait-solver-5.md) の調査で、zustc が「mismatched types」で弾いていた正しいテスト 130 件を
群に分けた。ここではその群を順に扱う。

## 行き先で型が決まる数値リテラル

どのケースもまず小さなプログラムに縮めた。最小の形はほとんど動き、落ちるのは特定の形だった:

- `3 + mem::size_of::<Foo>()`: 推論は `size_of` / `align_of` (コード生成の組み込み) の型を持たず、隣のリテラルが i32 になって
  いた。推論でも usize にした。
- `let b = tr::<u16>(); assert_eq!(b, 1 + 1)`: 自由関数の turbofish は型パラメータをコード生成にしか渡していなかった。推論でも
  使う (`Solver::pre_pv`。呼び出しが埋める)。
- `x.min(black_box(9.0))`: 引数が Self である浮動小数点のメソッド (min、max、powf、atan2、hypot、copysign、mul_add、clamp ..) は、
  整数のメソッドと同じく引数をレシーバと単一化する。
- `25f32.powf(-2f32)`: 負のリテラルの近道が `2f32` (浮動小数点の接尾辞の付いた数字) を整数と読み、i32 にしていた。

## 最初の引数以外でも決まるジェネリックな引数

- `f::<Box<_>>(Box::new(100))`: 穴のある turbofish の型は、それに合う引数が埋める (`Program::match_ty`、`fits_holes`)。照合の
  失敗にしない。`collect::<Vec<_>>()` は変わらない。
- `fn spawn<T>(val: T, f: fn(T) -> T)` と `fn double(isize)` での `spawn(1 + 40, double)`: 型パラメータの上の関数ポインタの
  パラメータと、ジェネリックでない関数アイテムの引数が、引数を順に下ろす前に型パラメータを決める。推論とコード生成の両方で。
  最初の試みはコード生成だけで、ただのリテラル引数は動いたが、式 (`1 + 40`) は推論の i32 のままだった。新しい例が見つけた。

## 型強制

- `let p: *const [u8] = &a;` (生のスライスポインタが期待されるところの配列への参照) は fat pointer。キャスト
  `&a as *const [u8]` がすでにそうしていたのと同じ。
- キャプチャがなく、書かれたシグネチャが同じ別々のクロージャは、match の腕や配列・`vec!` の要素で関数ポインタが期待されて
  いないとき、その関数ポインタに型強制する (`closures_lub`)。rustc と同じ。推論は match に最初の腕のクロージャの型を与えるが、
  それは何も期待されていないものとして扱う。`vec!` の引数はもう一度パースされるので、ポインタではクロージャの定義が見つからない:
  キャプチャは自由な名前で、結果の型は書かれたもので確かめる。

後回し: 構造体への unsize (`&Foo<[u8; 3]>` から `&Foo<[u8]>`、11 件) は、最後のフィールドが unsized な構造体への fat pointer
(レイアウト、フィールドのオフセット、`len`) が要る。zustc にはいまそのどれもなく、推論も型強制を通して単一化してしまう。
それ自体で一つの作業になる。

## 結果

| 種類 | 前 | 後 |
|---|---:|---:|
| run-pass | 1845 | 1854 |
| check-pass | 1521 | 1523 |
| build-pass | 103 | 104 |
| compile-fail | 4549 | 4551 (新しい 1 件は偶然で、一覧に載せた) |

通っていたテストで落ちたものはない。`cargo test --release --workspace`: 378 件成功。失敗する 2 件は zust-analyzer の LSP
hover テストで、rust-src が要る (ここには入っていない)。

新しい例 (どれも rustc の出力と比べる): `examples/literal_inference.rs`、`examples/generic_args.rs`、`examples/coercions.rs`。

## 残っているもの

- 構造体への unsize (上)。
- match の腕の一般の最小上界: クロージャの変数、関数アイテムとクロージャ、注釈のないクロージャの引数、`unimplemented!()` の腕
  (`closures/closure_no_cap_coerce_many_*.rs`)。`vec!` の中の deref の型強制 (`vec![&string, "abc"]`)。
- ジェネリックな関連型 (`type Output<O> = O` での `IdentityBuild.build(10)`) と、デフォルトメソッドのトレイトのパラメータ
  (`traits/issue-3979-generics.rs`) を通る整数リテラルの推論。
- 途中で見つけたもの: zustc には rustc の `dangerous_implicit_autorefs` deny リントがない (生のスライスポインタでの `(*p).len()`)。
