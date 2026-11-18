# 生成したプログラムのインクリメンタルビルド (N = 1000 / 5000)

日付: 2026-11-15

## N = 1000 関数（320KB）

環境は Linux x86-64 のクラウドコンテナ（4 コア相当）。rustc 1.97.0、wild 0.10.0、Cranelift 0.137。
`bench/gen.py` が生成する「N 個の関数 + 相互呼び出し」のプログラムで測った。どのツールの出力も rustc の実行結果と一致した。

| tool | cold ms | no-op ms | 1 関数編集 ms | peak RSS (cold) | binary | cache / incr dir |
|---|---:|---:|---:|---:|---:|---:|
| zustc（wild を直接起動） | 128 | 11.7 | **19.0** | **13.4 MiB** | 413 KiB | **498 KiB** |
| zustc（bfd を直接起動） | 129 | 12.1 | 33.2 | 13.2 MiB | 422 KiB | 498 KiB |
| zustc（lld を直接起動） | 141 | 12.0 | 36.7 | 58.7 MiB | 412 KiB | 498 KiB |
| rustc `-Cincremental -Cdebuginfo=0` | 887 | 217 | 461 | 164 MiB | 5013 KiB | 22151 KiB |
| rustc `-Cincremental -Cdebuginfo=2` | 1015 | 218 | 563 | 183 MiB | 5557 KiB | 24592 KiB |
| rustc `-O`（参考、非 incremental） | 12445 | - | - | 147 MiB | 382 KiB | - |

## N = 5000 関数（1.6MB）

| tool | cold ms | no-op ms | 1 関数編集 ms | peak RSS (cold) | binary | cache / incr dir |
|---|---:|---:|---:|---:|---:|---:|
| zustc（wild を直接起動） | 497 | 63.6 | **87.0** | **34.4 MiB** | 2030 KiB | **2479 KiB** |
| zustc（bfd を直接起動） | 471 | 53.9 | 87.0 | 34.6 MiB | 2038 KiB | 2479 KiB |
| rustc `-Cincremental -Cdebuginfo=0` | 3810 | 756 | 1911 | 499 MiB | 8040 KiB | 113644 KiB |

rustc と比べると、1 関数編集は 20〜24 倍速く、cold の RSS は 1/12〜1/14、キャッシュは 1/45 になった。
5000 関数の no-op が 60ms 前後あるのは、ファイル全体の lex、`.o` の作り直し、キャッシュの読み書きが N に比例するため。
マルチファイル化してファイル単位の要約にすれば、ここも O(変更量) にできる。

1 関数を編集したときの zustc の内訳（`--time`、N=1000）：

| フェーズ | ms |
|---|---:|
| lex | 2.6 |
| 要約（1 アイテムだけ parse） | 1.3 |
| plan | 0.9 |
| キャッシュ参照 | 0.5 |
| codegen（1 関数） | 0.4 |
| .o 生成 | 3.6 |
| キャッシュ保存 | 1.2 |
| link（wild） | 5.6 |
| **合計** | **15.9（ピーク RSS 12 MiB）** |

hello（`examples/hello.rs`）の出力サイズは、zustc で 6.0 KiB、rustc の debug（strip あり）で 333 KiB。
rustc 側はほとんどが std 由来。
