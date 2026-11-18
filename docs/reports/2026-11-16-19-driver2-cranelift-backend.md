# driver2 の Cranelift バックエンド (minclif と比べる)

日付: 2026-11-16

minclif の FunctionBuilder は acg が呼んだ命令をすべて `Op` として記録している。その列を Cranelift の IR に組み立て直して
コンパイルするバックエンドを足した (`src/clif_backend.rs`、`ZUSTC_BACKEND=cranelift`)。acg・インライン展開・キャッシュ・.o の
書き出しは同じなので、同じフロントエンド・同じ命令の列でバックエンドだけを比べられる。

- 値は minclif の値の番号 → Cranelift の値。ブロック・変数・スロット・関数も番号で対応させる (記録のときに命令ごとの戻り値も
  残すようにした)。acg は値を支配していないブロックで使うことがある (minclif は値をスロットに置くので困らない) ので、
  定義と別の区間で使う値は Cranelift の Variable にして、SSA は cranelift_frontend に組み立てさせる
- drop フラグのスロットは途中で作られるので、組み立てた後で入口の先頭に 0 を書く
- 再配置は名前で返す (呼び出しは PLT32、記号は GOTPCREL)。trap の番号も minclif と同じ
- `tests/driver2_cranelift.rs`：probe / examples / acg を -O なしと -O で、minclif と出力・stderr・終了コードが同じか (すべて一致)。
  セルフホスト版のコンパイラを Cranelift でビルドしたものも、minclif でビルドしたものと同じ .o を出す

実行時間 (`python3 bench/runtime.py`、ms):

| | driver2 -O (minclif) | driver2 + Cranelift | driver2 + Cranelift -O | 前の Cranelift の経路 (`zustc -O`) | rustc -O |
|---|---:|---:|---:|---:|---:|
| fib | 110 | 91 | 79 | 79 | 31 |
| hashmap | 214 | 155 | 144 | 148 | 112 |
| matmul | 168 | 105 | 76 | 85 | 40 |
| nbody | 522 | 232 | 86 | 105 | 54 |
| sieve | 158 | 150 | 154 | 134 | 94 |
| sort | 334 | 294 | 233 | 248 | 208 |
| strings | 111 | 91 | 68 | 73 | 42 |

セルフホスト版のコンパイラ (約 3 万 3 千行) で比べる:

| バックエンド | そのコンパイル (-j 1。acg を含む) | できる実行ファイル | できたコンパイラが自分自身をコンパイルする時間 |
|---|---:|---:|---:|
| minclif | 2.40 秒 | 8.8 MB | 11.5 秒 |
| minclif -O | 3.15 秒 | 10.6 MB | 10.1 秒 |
| Cranelift (opt none) | 5.07 秒 | 5.4 MB | 6.9 秒 |
| Cranelift -O (speed, backtracking) | 8.26 秒 | 6.6 MB | **6.0 秒** |
| Cranelift -O (speed, single_pass) | 6.95 秒 | 9.2 MB | 8.9 秒 |

(rustc -O でビルドしたコンパイラは 1.6 秒)。ワークスペースのベンチ (`bench/ws_bench.py`) は minclif / Cranelift で cold 440 / 727 ms、
変更なし 7 / 6 ms、関数の本体の変更 42 / 36 ms、bin の変更 74 / 67 ms (関数ごとのキャッシュで、変更のあとのコンパイルは小さい)。

分かったこと: minclif は Cranelift (opt none) の約 2 倍、-O どうしで約 2.6 倍速くコンパイルするが、出すコードは 1.0〜6 倍遅い
(セルフホスト版のコンパイラでは 1.7 倍)。増分のビルドでは変わった関数しかコンパイルしないので、コンパイルの速さの差は
cold のときだけ効く。

ここまでで、外部クレートに依存しないモジュール（lexer / enc / fxhash / prelude / link / testgen / program / ast_consteval / solver / ctors / ast_infer / minclif / ast_codegen）と、
依存が小さく代わりを書けるもの（elfz: zstd、macros: proc_macro2）は zustc でコンパイルでき、rustc と同じ結果になる。
残りの front（syn の構文木を引く検索）/ infer（syn 版。自前の構文木の版 ast_infer はある）/ consteval（codegen の定数の昇格）/ closure / codegen / driver / cache は syn と cranelift の上に書かれている。
syn の代わりは src/parse.rs（自前の構文木）を土台にして front から順に移す。cranelift の代わり (自前のコード生成) はその後。
