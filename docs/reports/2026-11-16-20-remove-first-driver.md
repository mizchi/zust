# 最初のドライバ (syn + Cranelift) を外す

日付: 2026-11-16

- `zustc check` と LSP を driver2 の `Session2::check` に移した。全関数 (到達しないものも) と要求された総称関数のインスタンスを
  コンパイルしてエラーを集め、結果はキャッシュに入れる (次の検査・ビルドはそれを使う)
- `crates/legacy` (syn・cranelift-object・zstd) と、syn 版と比べていたテスト (ast_closure / ast_consteval / ast_infer /
  ast_summary / ast_types) とセルフホストの elfz を削除した。期待する出力は `tests/expected/` の golden ファイル
  (syn 版で作ったもの) にして、`tests/driver2.rs` が比べる
- 見つかったバグ: `Session2::save_cache` の後にメモリ上のキャッシュが空になっていた (使ったキーの集合を先に消していた)。
  同じ Session で続けてビルドすると全関数をコンパイルし直していた。ベンチの `build/edit_1fn` の assert で見つかった
- タプル構造体のパターンのフィールドの数のエラーに場所 (パスの位置) を付けた
