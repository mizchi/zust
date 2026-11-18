# セルフホストの第 19 段階: zustc で zustc (syn・Cranelift を使わない経路) をコンパイルする

日付: 2026-11-16

- `tests/selfhost/build_compiler.py`：driver2 + acg + minclif と、それが使うモジュール全部 (lexer / macros / prelude / types / parse /
  ast_* / program / solver / ctors / ast_infer / ast_consteval / link) を 1 ファイル (約 3 万 1 千行) のコンパイラにまとめる
  (`compiler <in.rs> -o <out> [-O]`)
- これを zustc (driver2) でコンパイルしたものが、実際にプログラムをコンパイルして動かせる (zustc が自分自身をコンパイルした)
- `tests/selfhost_compiler.sh`：rustc 版と zustc 版の 2 つのコンパイラで probe / examples / acg をコンパイルし、.o と実行結果を比べる。
  さらに zustc 版で自分自身をもう一度コンパイルする (2 段目)
- 結果: **139 個のプログラムすべてで、rustc 版と zustc 版のコンパイラが同じバイト列の .o を出し、実行結果も同じ**。
  zustc 版のコンパイラで自分自身をコンパイルし直すと、1 段目と同じ .o (約 18 MB) になる (不動点)
- 比べて見つかったもの: `matches!(tmp, Some(V(x)) if ..)` で束縛が一時値からムーブして二重に drop していた (syn 版にもあった)、
  acg の i128 の `abs` / 加減算のオーバーフロー検査
- そこまでに直したこと: モジュールを平らにするときの名前 (`std::path::Path` と同名の定義、`use super::*` で見える use、`&'static`)、
  型推論 (受け手の型が分からない `len`、注釈つきの let、`x.clone()` の整数)、acg の deref の強制変換 (`&&T` → `&T`、`&String` → `&str`)、
  範囲の添字の受け手の自動参照、`matches!` の guard の中のクロージャ、minclif の bitcast の 0 拡張。zustc がまだ扱えない書き方
  (分解代入・`collect::<Option<Vec<_>>>`・クロージャから fn ポインタへの変換・`std::f64::consts` の値など) はソースを書き換えた
