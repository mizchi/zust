# セルフホストの第 12 段階: 定数評価を自前の構文木へ移す

日付: 2026-11-16

- `src/ast_consteval.rs`：consteval.rs（static の初期値 → データのバイト列とリロケーション）を自前の構文木に移植し、
  driver の static の評価はこちらを使う（失敗したときだけ、場所つきのメッセージを syn 版で作る）。
  文字列・バイト文字列のリテラルの中身（エスケープ・raw 文字列）も自前で読む。
  `tests/ast_consteval.rs` が probe / examples の全 696 個の const / static で syn 版と同じバイト列・リロケーションになることを確かめる
- 足したもの：`&Box<T>` → `&T` の強制変換（`e = x;` で x: `&Box<Expr>`。推論でも参照どうしをつながず中身を単一化する）
