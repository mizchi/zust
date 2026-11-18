# LSP の比較: rust-analyzer / rust-glancer (2026-11-17、`python3 bench/lsp_compare.py --cold`)

日付: 2026-11-17

同じ手順で 3 つの LSP サーバを動かす (stdio、サーバからの要求には空で答える):
initialize → initialized → didOpen → workspace/symbol が既知の名前を返すまで 0.1 秒ごとに問い合わせ (ready) →
documentSymbol を 10 回 (中央値) → workspace/symbol を 5 つの問い合わせ × 5 回 (中央値) → 関数を 1 つ足す didChange の直後の
documentSymbol (edit→docsym) と、publishDiagnostics が来るまで (edit→diag、20 秒で打ち切り)。peak RSS はサーバのプロセスの
VmHWM と、括弧の中は子孫のプロセスも足した RSS の最大 (50 ms ごと)。

- 対象 synth: 生成した Cargo のクレート (モジュール 40 個・各 25 関数 + struct / enum / impl、約 4400 行)。開くのは src/m7.rs
- 対象 zustc: このディレクトリ (Cargo のワークスペース、依存に cranelift など)。開くのは crates/core/src/driver2.rs
- rust-analyzer 1.100.0 (rustup の component)。rust-glancer は github.com/rust-glancer/rust-glancer (50dd51a) を
  `cargo build --release -p rust-glancer` でビルドしたもの (`RUST_GLANCER=<バイナリ>` で指定。rust-src が要る)。
  `--cold` は rust-glancer の索引 (target/rust_glancer) を消してから測る

| 対象 | サーバ | init | ready | docsym (数) | wsym (見つかった) | edit→docsym | edit→diag | peak RSS (子も) |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| synth | zustc | 2.2 ms | 9.6 ms | 0.6 ms (37) | 0.7 ms (5/5) | 2.0 ms | 1.3 ms | 7.5 MB (5.1 MB) |
| synth | zustc (定義・参照を足した後) | 2.0 ms | 9.7 ms | 0.6 ms (37) | 0.6 ms (5/5) | 1.8 ms | 1.6 ms | 10.7 MB (6.4 MB) |
| synth | rust-analyzer | 18.7 ms | 3243 ms | 1.0 ms (37) | 2.3 ms (5/5) | 8.3 ms | 309 ms | 607 MB (878 MB) |
| synth | rust-glancer | 36.4 ms | 5799 ms | 1.9 ms (38) | 27.0 ms (5/5) | 43.3 ms | - | 16.9 MB (888 MB) |
| zustc | zustc | 2.3 ms | 55.0 ms | 1.8 ms (123) | 3.5 ms (5/5) | 9.7 ms | 7.8 ms | 10.9 MB (9.9 MB) |
| zustc | zustc (定義・参照を足した後) | 1.9 ms | 91.4 ms | 2.1 ms (123) | 1.3 ms (5/5) | 13.0 ms | 10.4 ms | 29.9 MB (29.4 MB) |
| zustc | rust-analyzer | 23.0 ms | 5954 ms | 3.6 ms (124) | 2.0 ms (5/5) | 26.2 ms | - | 1360 MB (2723 MB) |
| zustc | rust-glancer | 130 ms | 10593 ms | 7.5 ms (113) | 262 ms (5/5) | 48.6 ms | - | 30.7 MB (1798 MB) |

読み方の注意:
- 機能の幅が違う。rust-analyzer はクレートの依存まで名前解決・型推論をして、補完・定義へのジャンプなどもできる。
  rust-glancer は依存まで索引を作る (ディスクに置く) 軽量版。zustc lsp は開いたファイルを 1 つのクレートとして検査し
  (Cargo の依存・`mod x;` のほかのファイルは読まない)、シンボルは字句の列から読むだけ。zustc の ready が速いのは、依存も
  型の索引も作らず、ワークスペースの .rs のシンボルを読むだけだから。
- edit→diag: zustc は開いたファイルだけの検査の結果 (driver2.rs 単体なのでエラーが出る)。rust-analyzer は自前の診断を
  push する (zustc の対象では 20 秒の間に来なかった)。rust-glancer の診断は既定で切ってある (cargo check を設定で入れる)。
- edit→docsym は didChange を受けた後の処理を含む (zustc は didChange のたびに check するので、その時間も入る)。
- rust-analyzer と rust-glancer は子プロセス (cargo metadata・rustc・索引の作業) が大きい。rust-glancer は本体は 17〜31 MB。
- シンボルの数の違い: rust-glancer は impl の中の関連アイテムなどの数え方が違う (synth の 38 は impl の名前の出し方)。
