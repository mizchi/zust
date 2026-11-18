# zustc が対応しているもの

2026-11-18 時点の状況（作業の回ごとに更新する）。割合は、その分野で通る rustc 自身の UI テストの割合である
（`python3 bench/rust_ui.py`、rust-lang/rust の tests/ui 1.100.0。ディレクトリの run-pass・check-pass・build-pass を合わせたもの）。
作業の一覧は [TODO.md](TODO.md)、各段階の説明は [報告](docs/ja/reports/) にある。

- `[x]` ふつうのコードで動く（その分野のテストの多くが通る）
- `[ ]` 一部だけか、ない。何が残っているかを書き添える

## 全体

- [x] tests/ui run-pass：59.7%（1440 / 2413）
- [x] tests/ui check-pass：63.1%（1141 / 1807）
- [x] tests/ui build-pass：60.7%（82 / 135）
- [ ] tests/ui compile-fail（注釈の行でエラー）：38.3%（2815 / 7358）
- [x] 実際のクレート：fnv、adler2、hex、itoa、rustc-hash、cfg-if がビルドでき、同じ出力を出す（6 / 6）
- [x] zust-analyzer が rust-analyzer と同じように答える：95.4%（rust-analyzer は 96.2%）

## 言語

- [x] 数値・算術・オーバーフローの検査：89.7%
- [x] `for` / `while` / `loop`、ラベル：88.7%
- [x] 列挙型、`#[repr(u8)]`、大きさを自動で決めるタグ：82.8%
- [x] `cfg`：81.2%
- [x] 束縛・パターン・スライスパターン・or パターン（ガード付きも）：72〜81%
- [x] `match`：78.0%
- [x] 名前解決：77.1%
- [x] static：整数・浮動小数点・生ポインタ・関数ポインタ・`const fn` の呼び出し：75.0%
- [x] 構造体、`..base`（フィールドごと）：74.2%
- [x] 構文解析：73.7%
- [x] クロージャ（キャプチャ、`Fn` トレイト、`move`）：71.7%
- [x] derive：`Debug`、`Clone`、`Copy`、`PartialEq`/`Eq`、`PartialOrd`/`Ord`、`Hash`、`Default`：68.2%
- [x] import、`use`：67.0%
- [x] 関連型：63.6%
- [x] ジェネリクス：61.9%（`T: Foo` に渡す整数リテラルは、Foo を実装する唯一の整数型になる。期待される型からしか分からない
  型パラメータは推論しない）
- [ ] const：54.9%（ヒープの値、ジェネリックな `const fn` はない）
- [ ] drop：53.6%（一部の一時値の drop の順序が違う）
- [ ] トレイト：51.0%（ジェネリックなトレイト、ブランケット impl、`dyn`（`dyn Tr<A>` も）、`impl .. for dyn Tr` は動く。coherence はない）
- [ ] `macro_rules!`：47.1%（`let`・ラベル・アームの束縛の衛生性。断片の種類は一部）
- [ ] 共用体：46.2%（`repr(packed)` はない）
- [ ] const ジェネリクス：43.6%（`generic_const_exprs` はない）
- [ ] ジェネリックな関連型：40.0%
- [ ] キャスト：34.5%
- [ ] `impl Trait`：29.7%（戻り値の位置。トレイトのメソッドでも使える。ジェネリックな関数から・入れ子・TAIT はない）
- [ ] レイアウト：`repr(packed)` / `repr(align)` は無視する：12〜33%

## 検査（compile-fail）

- [x] 借用検査：ムーブ、衝突するローン、持ち出し、可変性：borrowck 49.4%
  （[報告 08](docs/ja/reports/2026-11-18-08-borrow-checking.md)）
- [x] 型の誤り、見つからないアイテム、ジェネリクスの宣言（E0403、E0207、E0392、E0072 など）：typeck 53.4%、structs 66.7%
- [ ] シグネチャに対する region の誤り（E0621、E0521）：regions 12.9%、lifetimes 19.5%
- [ ] coherence、ブランケット impl の境界、dyn 互換性（E0038）
- [ ] lint と警告：lint 8.3%
- [ ] エディション（2015 だけのエラー）

## ランタイムと std

- [x] prelude（zustc 自身の小さな std）：`Option`/`Result`、`Vec`、`String`、`HashMap`/`HashSet`、`BTreeMap`/`BTreeSet`、
  `VecDeque`、`BinaryHeap`、`Rc`/`Arc`、`Cell`/`RefCell`、`Mutex`、`OnceLock`、`ManuallyDrop`、`NonNull`、`UnsafeCell`、
  `PhantomData`、アトミック型、`mpsc`、`mem::*`、`ptr::{read, write, drop_in_place, null}`、
  `iter::{repeat, once, empty, repeat_with, successors}`、書式化、stdin/stdout/fs/env/process の基本
- [ ] async / await：64.1%。エグゼキュータなしでその場で実行する（`poll`、`Waker`、`Pin<Box<dyn Future>>` はない）
- [ ] スレッド：61.4%。1 スレッド：`thread::spawn` はクロージャをその場で実行する
- [ ] イテレータ：35.3%（`iter::empty()` は行き先から要素型を推論できない）
- [ ] FFI：14.3%。スカラーだけで、構造体の値渡しはない
- [ ] panic：11.1%。panic は止まる：巻き戻し、`catch_unwind`、`set_hook` はない

## 目処がついているもの（やり方が分かっていて、コストが小さい）

- [ ] 型パラメータ：`let v: Vec<Box<_>> = ..`（注釈の中の `_`）と、値として渡すジェネリックな関数（`.map(drop)`、
  `.map(Result::unwrap)`）は推論するようになった。`iter::empty().collect()`、`let f = id;`、パターンから決まる const
  パラメータはまだ（約 35 テスト）
- [ ] ブロックの中のアイテム：関数の本体の中の `mod`、`trait`、`impl`、`type`、`union`、`pub` のアイテムは外へ移すようになった。
  本体の中でマクロの展開から出るアイテムはまだ（約 30 テスト）
- [ ] 組み込みマクロ：`offset_of!`、`addr_of!` / `addr_of_mut!`、`try!`、`stringify!`、`concat!`、`line!`、`column!`、
  `option_env!`、`assert_matches!`、`format_args!` は使えるようになった。`module_path!`、`file!`、`include!`、`asm!` はまだ（約 8 テスト）
- [ ] ジェネリックな関数が返す `impl Trait`：約 21 テスト
- [ ] `repr(packed)` / `repr(align)`：約 16 テスト
- [ ] エグゼキュータなしの `Pin`（`Pin<&mut T>`、`Box::pin`）
- [ ] dyn 互換性（E0038）、トレイトの総称引数の数
- [ ] `&mut` をキャプチャするクロージャは `Copy` でない

## パフォーマンスの悪化が予想されるもの（残す前に計測する）

- [ ] シグネチャに対する region の検査：`check` の時間（借用検査は今 +2.2%）
- [ ] coherence とブランケット impl の境界：`check` の時間、LSP
- [ ] 本当の async（状態機械、`poll`、エグゼキュータ）：ビルド時間、バイナリサイズ
- [ ] 巻き戻し（ランディングパッド）：ビルド時間、バイナリサイズ
- [ ] コード生成でのフィールド単位のキャプチャ：バイナリサイズ
- [ ] より大きな prelude：no-op ビルドとキャッシュのサイズ（約 1 万 1 千行の prelude をビルドのたびに走査する）
- [ ] 本当のスレッド（pthread のランタイム）：ランタイムのサイズ

## 未着手の大きな機能

- [ ] 本当の並行性（並列に走るスレッド、ブロックする `Mutex`）
- [ ] エグゼキュータ：`Future::poll`、`Waker`、`Pin<Box<dyn Future>>`（約 25 テスト）
- [ ] 巻き戻し：`catch_unwind`、panic のフック（約 22 テスト）
- [ ] FFI：構造体の値渡し、ほかの ABI（約 27 テスト）
- [ ] region・寿命の完全な検査
- [ ] 完全なトレイトソルバ：特殊化、`dyn` / `async` を通した自動トレイト、高階の境界
- [ ] lint と警告
- [ ] エディション
- [ ] SIMD、インラインアセンブリ
- proc macro：対象外（別のクレートなので）

## rustc との速度の比較 (2026-11-18)

- [x] cold ビルド：gen1000 で rustc `-C incremental` の 4.9 倍速い。実際のクレートで 3.0〜4.0 倍
- [x] 編集して再ビルド：gen1000 で 14 倍速い。小さなプログラムで 12〜18 倍、実際のクレートで 5.1〜7.2 倍
- [x] ディスクのビルドキャッシュ：1/14（gen1000 で 766 KiB と 10.8 MiB）
- [x] バイナリ：gen1000 で 1/3.2、実際のクレートで 1/90〜1/550
- [ ] 実行時間：小さなプログラムで zustc は rustc `-O` の 1.1〜4.1 倍遅い（rustc の debug ビルドよりは速い）。
  zustc `-O` は 1.1〜3.2 倍

表は [README](README.ja.md#rustc-との比較) にある。

## 今の KPI

| KPI | 今 | この作業の前 |
|---|---:|---:|
| gen5000 の no-op ビルド | 663.2 M 命令 | 746.0 M |
| gen5000 の 1 関数を編集した後のビルド | 620.8 M 命令 | 709.3 M |
| gen5000 の `check` | 3893.0 M 命令 | 3971.0 M |
| gen5000 のビルドキャッシュ | 1902.8 KiB | 1902.8 KiB |
| LSP の definition / hover / completion / references の p50 | 0.42 / 0.34 / 0.90 / 1.78 ms | — |
| LSP の起動から索引ができるまで（zustc と ide の対象の合計） | 277 ms | — |
