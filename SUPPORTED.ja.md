# zustc が対応しているもの

2026-11-18 時点（コミット e598fdf）の状況。数字は rustc 自身の UI テスト（`python3 bench/rust_ui.py`、rust-lang/rust の
tests/ui 1.100.0）と KPI のベンチマーク（`bench/ci_bench.py`）による。作業の一覧は [TODO.md](TODO.md)、各段階の説明は
[報告](docs/ja/reports/) にある。

## 概要

| 指標 | 値 |
|---|---|
| tests/ui run-pass（コンパイルでき、実行して 0 で終わる） | **56.5%**（1364 / 2413） |
| tests/ui check-pass（型検査が通る） | **60.8%**（1098 / 1807） |
| tests/ui build-pass | **60.0%**（81 / 135） |
| tests/ui compile-fail（注釈の行でエラー） | **38.2%**（2808 / 7358） |
| 実際のクレート（cratebench: fnv、adler2、hex、itoa、rustc-hash、cfg-if） | 6 / 6 がビルドでき、同じ出力を出す |
| zust-analyzer と rust-analyzer の一致（bench/lsp_parity.py） | 95.4%（rust-analyzer は 96.2%） |

zustc が扱おうとしないものが要るテスト（ほかのターゲット、`aux-build`、proc-macro のクレート、`-Z` のフラグなど）は数えない
（「対象外」）。

## 分野ごとの対応

割合はそのディレクトリの tests/ui の通過率である（run-pass・check-pass・build-pass を合わせたもの。compile-fail は別）。
チェックリストではなく、その分野の実際の Rust がどれだけ動くかを表す。

| 分野 | 通過（run/check/build） | compile-fail | 状態 |
|---|---:|---:|---|
| 数値・算術 | 89.7% | 66.7% | 済み |
| `for` / `while` / `loop` | 88.7% | 37.5% | 済み |
| 列挙型 | 82.8% | 45.8% | 済み。`#[repr(u8)]` と、大きさを自動で決めるタグ |
| `cfg` | 81.2% | 41.7% | 済み |
| 束縛・パターン・or パターン | 72〜81% | 20〜40% | ほぼ。スライスパターン、or パターンとガード |
| `match` | 78.0% | 15.6% | ほぼ |
| region（シグネチャの寿命） | 78.4% | 12.9% | 受け付けるが検査しない（「未着手」を参照） |
| 名前解決（`resolve`） | 77.1% | 46.3% | ほぼ |
| static / const | 75.0% / 54.9% | 11.1% / 9.8% | 整数・浮動小数点・生ポインタ・関数ポインタ・`const fn` の呼び出し。ヒープの値とジェネリックな const fn は未対応 |
| 構造体 | 74.2% | 66.7% | 済み。FRU（`..base`）はフィールドごとにムーブ |
| 借用検査 | 73.9% | 49.4% | NLL 風のローン＋Polonius 風の生存（[報告 08](docs/ja/reports/2026-11-18-08-borrow-checking.md)）。region の誤りはまだ |
| 構文解析 | 73.7% | 64.1% | ほぼ |
| クロージャ | 71.7% | 33.9% | ほぼ。借用検査は場所ごと（RFC 2229）、コード生成は変数ごとにキャプチャ |
| derive | 68.2% | 22.2% | `Debug`、`Clone`、`Copy`、`PartialEq`/`Eq`、`PartialOrd`/`Ord`（NaN を考える）、`Hash`、`Default` |
| import | 67.0% | 9.7% | ほぼ |
| async / await | 64.1% | 31.9% | **その場で実行**（エグゼキュータなし）：`async` のブロック・関数・クロージャ、`.await`、`impl Future<Output = T>` |
| 関連型 | 63.6% | 28.1% | ほぼ |
| lint | 63.4% | 8.3% | 警告は出さない（compile-fail の lint には要る） |
| ジェネリクス | 61.9% | 46.3% | ほぼ。期待される型からの型パラメータの推論は一部 |
| スレッド・Send/Sync | 61.4% | — | **1 スレッド**：`thread::spawn` はクロージャをその場で実行する。`mpsc`、`Arc`、アトミック型 |
| 配列・スライス・Vec | 58.6% | 20.0% | ほぼ |
| drop | 53.6% | 53.8% | drop フラグ、一時値。一部の一時値の drop の順序が違う |
| トレイト | 51.0% | 31.8% | ジェネリックなトレイト、ブランケット impl、`dyn`（vtable）、`impl .. for dyn Tr`。coherence の検査はない |
| マクロ（`macro_rules!`） | 47.1% | 22.2% | `let`・ラベル・アームの束縛の衛生性。断片の種類は一部 |
| 共用体 | 46.2% | 26.7% | レイアウトとリテラル。`repr(packed)` はない |
| const ジェネリクス | 43.6% | 52.7% | 値と配列長。`generic_const_exprs` はない |
| ジェネリックな関連型 | 40.0% | 29.5% | 一部 |
| イテレータ | 35.3% | 36.8% | std のアダプタは prelude にある。`iter::empty()` は行き先から要素型を推論できない |
| キャスト | 34.5% | 56.2% | 一部 |
| `impl Trait` | 29.7% | 31.3% | 戻り値の位置。ジェネリックな関数・入れ子・TAIT は未対応 |
| レイアウト・`repr(packed)` | 12〜33% | 0〜23% | `repr(packed)` と `repr(align)` は無視する |
| FFI（`extern "C"`） | 14.3%（abi） | — | スカラーだけ。構造体の値渡しはしない |
| panic・巻き戻し | 11.1% | — | panic はプロセスを止める。`catch_unwind` も `set_hook` もない |

prelude（zustc 自身の小さな std、`crates/syntax/src/prelude_lib.rs.in`）には、`Option`/`Result`、`Vec`、`String`、
`HashMap`/`HashSet`、`BTreeMap`/`BTreeSet`、`VecDeque`、`BinaryHeap`、`Rc`/`Arc`、`Cell`/`RefCell`、`Mutex`、
`OnceLock`、`ManuallyDrop`、`NonNull`、`UnsafeCell`、`PhantomData`、アトミック型、`mpsc`、`mem::{swap, replace, take,
transmute, forget, size_of_val}`、`ptr::{read, write, drop_in_place, null}`、`iter::{repeat, once, empty, repeat_with,
successors}`、書式化、stdin/stdout/fs/env/process の基本がある。各プログラムには、そのプログラムが名前を出したアイテムだけを付ける。

## 目処がついているもの（やり方が分かっていて、コストが小さい）

| 項目 | 止まっているテスト（失敗している run/check/build-pass） | やり方 |
|---|---:|---|
| 期待される型からしか決まらない型パラメータ（`iter::empty().collect()`） | 約 44 | 期待される戻り値型からも呼び出しのパラメータを解く |
| 前処理が外へ移さない、ブロックの中のアイテム | 約 39 | ほかのブロックの中のアイテムと同じく外へ移す |
| zustc が知らないマクロ（`concat_idents!`、`stringify!` や `matches!` の端の場合など） | 約 36 | マクロごと |
| ジェネリックな関数が返す `impl Trait` | 約 21 | インスタンスごとに不透明な型を決める |
| エグゼキュータなしの `Pin`（`Pin<&mut T>`、`Box::pin`） | 約 3（＋async のもの） | prelude の透過的なラッパー |
| E0038（dyn 互換性）、E0592/E0201 の位置、トレイトの総称引数の数 | compile-fail | 要約の上の検査 |
| `&mut` をキャプチャするクロージャは `Copy` でない | compile-fail | クロージャの型にキャプチャの仕方を持たせる |
| `repr(packed)` / `repr(align)` | 約 16 | 構造体ごとのレイアウトの指定 |

## パフォーマンスの悪化が予想されるもの

互換性のためにはやる価値があるが、KPI が悪くなる。どれも、残す前に計測する。

| 項目 | 影響する KPI | 理由 |
|---|---|---|
| シグネチャに対する region の検査（E0621、E0521、"may not live long enough"） | `check` の時間（借用検査は今 +2.2%） | すべての関数で universal region と outlives の制約 |
| coherence とブランケット impl の境界の検査 | `check` の時間、LSP | 各 impl をそのトレイトのほかのすべての impl と比べる |
| 本当の async（状態機械、`Future::poll`、waker） | ビルド時間、バイナリサイズ | async の関数ごとのコルーチン変換、prelude のエグゼキュータ |
| コード生成でのフィールド単位のキャプチャ | バイナリサイズ | キャプチャした場所ごとに環境のフィールド |
| 巻き戻し（`catch_unwind`、panic 時の drop） | バイナリサイズ、ビルド時間 | drop を持つすべての関数にランディングパッド |
| より大きな prelude（std をもっと） | no-op ビルド、キャッシュのサイズ | prelude はビルドのたびに走査・選別される（今は約 1 万 1 千行） |
| 本当のスレッド | ランタイムライブラリのサイズ | 1 スレッドの代用品の代わりに pthread のランタイム |

## 未着手の大きな機能

| 機能 | 止まっているテスト | 備考 |
|---|---:|---|
| 本当の並行性（並列に走るスレッド、ブロックする `Mutex`） | threads-sendsync の 39% が失敗 | 今は 1 スレッドで、クロージャはその場で走る |
| エグゼキュータ、`Future::poll`、`Waker`、`Pin<Box<dyn Future>>` | 約 25 | async はその場で実行する |
| 巻き戻し、`catch_unwind`、panic のフック | 約 22 | panic は止まる |
| proc macro | ここでは対象外（別のクレート） | |
| lint と警告 | lint の compile-fail 8.3% | 警告の仕組みがない |
| FFI：構造体の値渡し、ほかの ABI | 約 27 | C ABI はスカラーだけ |
| region・寿命の完全な検査 | regions の compile-fail 12.9%、lifetimes 19.5% | 寿命は構文解析の後で消す |
| 完全なトレイトソルバ（特殊化、`dyn`/`async` を通した自動トレイト、高階の境界） | traits の compile-fail 31.8% | |
| エディション | — | エディションの設定がない：2015 だけのエラーは出さない |
| SIMD、インラインアセンブリ | 少数 | |

## 今の KPI

| KPI | 値 |
|---|---|
| gen5000 の no-op ビルド | 661.9 M 命令（この作業の前は 746.0 M） |
| gen5000 の 1 関数を編集した後のビルド | 622.6 M 命令（709.3 M） |
| gen5000 の `check` | 3892.9 M 命令（3971.0 M） |
| gen5000 のビルドキャッシュ | 1902.8 KiB（変わらない） |
| LSP の definition / hover / completion / references の p50 | 0.42 / 0.34 / 0.90 / 1.78 ms（同じマシンでの中央値） |
| LSP の起動から索引ができるまで（zustc と ide の対象の合計） | 277 ms |
