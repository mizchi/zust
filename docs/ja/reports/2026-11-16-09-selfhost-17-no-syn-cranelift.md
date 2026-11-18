# セルフホストの第 17 段階: syn・Cranelift を使わない経路でプログラムを作る

日付: 2026-11-16

- `src/ast_codegen.rs`：自前の構文木と ast_infer の型から minclif で機械語を作るコード生成 (codegen.rs の置き換えの始まり)。
  `build_object(src)` が lexer → prelude → ast_summary → program → ast_infer → ast_codegen → minclif → ELF の .o を作る (syn も Cranelift も使わない)
- いま扱えるもの: 整数・bool・char・キャスト、let / 代入 / 複合代入、算術・ビット演算・比較・`&&` / `||`、if / while / loop / break / continue / return、
  ユーザの関数と extern "C" の関数の呼び出し。`tests/ast_codegen.rs` が `tests/acg/*.rs` (no_std) を cc でリンクして rustc 版と出力を比べる
- v1: 値の表し方を codegen.rs に寄せた。ローカル変数はスタックスロット、struct / enum / 配列 / タプル / fat pointer はアドレスで扱い memcpy で複製、
  集約型の引数はアドレス渡し、集約型の戻り値は隠れた第 1 引数 (sret)。struct / タプル / 配列 / enum の値とパターン (match / if let / while let / let-else)、
  フィールド・添字 (範囲検査)・`&a[i..j]`・`&[T; N]` → `&[T]`、文字列 / バイト列リテラル (.data)、`len` / `as_bytes` / `as_ptr`、
  範囲・配列・スライスの for、ジェネリックでないメソッド。`tests/acg/aggs.rs` が rustc と同じ出力になる
  (ast_infer は配列・タプル・参照・演算・キャストの式の型も記録するようにした)
