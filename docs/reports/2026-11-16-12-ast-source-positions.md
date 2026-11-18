# 自前の構文木のソース位置

日付: 2026-11-16

- `parse::Span { start, at }` を、panic の場所に使う式 (Unary / Binary / AssignOp / MethodCall / Index / Macro) に持たせた。
  start は式の先頭 (syn の `e.span()`)、at は式ごとの位置 (メソッド名・`[`・演算子・マクロの開き括弧)
- 式のマクロの引数のテキストは、元の行・桁の配置のまま作る (`group_text_laid_out`)。読み直すときはマクロの位置からずらす
  (`parse_expr_at` / `ast_closure::macro_args_at`) ので、`println!` の引数の中の `unwrap()` なども元の位置になる
- acg の `loc()` がこの位置を使う: 添字の範囲外 (Vec は `[`、配列は式の先頭)・オーバーフロー・0 除算・`unwrap` / `expect` (メソッド名)・
  `panic!` / `assert!` / `dbg!` (マクロの先頭) の場所が rustc と同じになった。`tests/panic_loc.rs` が rustc と比べる
- `tests/driver2.rs` は stderr (panic のメッセージ) も比べるようにした (139 すべて一致)。`matches!` の引数は型推論とコード生成が同じ構文木 (`ast_closure::matches_parts`) を使う
- `dbg!` の桁は syn 版が引数の位置を出していた (rustc はマクロの位置)。syn 版も直した
- 移植の初めに作った ast_codegen.rs (v1) は acg が置き換えたので消した (tests/ast_codegen.rs は driver2 で tests/acg を試す)
- コンパイルエラーにも場所を付ける: Path / Field / Lit にも位置を持たせ、acg のエラーには「位置を持つ一番内側の式」の
  `行:桁:` を付ける (メソッド呼び出し・フィールドは名前の位置、let の型の不一致は初期値、引数の型の不一致はその引数、
  return・末尾の式はその値)。構文エラー・要約のエラーはアイテムの中の行をファイルの行に直す。
  `tests/compile_errors.rs` が tests/compile_errors/*.rs で syn 版のドライバとメッセージ (場所ごと) が同じか比べる
