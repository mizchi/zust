# 本物の rustc との比較（`python3 bench/vs_rustc.py`）

日付: 2026-11-15

同じソースを同じ手順（cold → no-op → 1 か所編集）でビルドし、残るディスク・バイナリ（strip 後）・実行時間を並べる。全結果は `bench/vs_rustc_result.md`。
zustc のリンカは wild 0.10.0、rustc 1.97.0。どの設定でも出力は rustc `-O` と一致した。

| プログラム | | cold ms | edit ms | disk KiB | binary KiB | run ms |
|---|---|---:|---:|---:|---:|---:|
| gen1000 | zustc | **118** | **28** | **105** | 521 | 12 |
| | rustc -Cincremental | 827 | 417 | 22156 | 1043 | 9 |
| | rustc -O | 11448 | - | - | 382 | 2 |
| nbody | zustc -O | **47** | **6** | **29** | **23** | 92 |
| | rustc -Cincremental | 149 | 106 | 2227 | 366 | 1231 |
| | rustc -O | 177 | - | - | 364 | 62 |
| sort | zustc -O | **51** | **8** | **32** | **30** | **195** |
| | rustc -Cincremental | 203 | 128 | 3102 | 385 | 2041 |
| | rustc -O | 317 | - | - | 348 | 184 |

- 編集後の再ビルドは rustc の incremental の 15〜20 倍速く、残すディスクは 1/75〜1/210、バイナリは 1/10〜1/20（gen1000 は rustc `-O` より大きい: debug の検査と最適化なしの分）。
- 実行時間は zustc（debug）が rustc（debug）より速いものが多い（gen1000 を除く）。zustc `-O` は rustc `-O` の 1.0〜2.6 倍（sort は同じ、strings は 2.2 倍）。
