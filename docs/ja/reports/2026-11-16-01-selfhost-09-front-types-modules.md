# セルフホストの第 9 段階: front.rs の型の部分を分ける・モジュールの名前空間

日付: 2026-11-16

- `src/types.rs`：front.rs から syn に依存しない部分（`Ty` / `TySyn` / `ArrLen` / intern）を分けた（front は `pub use` で見せる）
- `src/ast_types.rs`：front.rs の `parse_tysyn`（syn の型 → TySyn）を自前の構文木に移植。`tests/ast_types.rs` が全ファイルの 3914 個の型で syn 版と一致することを確かめる
  （`dyn` の trait は proc_macro2 と同じ空け方のテキストにする）
- モジュールの名前空間：別々のモジュールで同じ名前（`types::Ty` と `parse::Ty`）を定義したら、平らにする前にモジュールの中の定義を `m__Ty` にし、
  中の参照・`use crate::m::{Ty as X}`（別名も）・`m::Ty` / `crate::m::Ty` / `super::Ty` のパスを直す（enum のバリアント名・フィールドは変えない）。Debug の表示は元の名前
- 関数の中の同じ名前の static（`intern` ごとの `static TABLE`）は、外に出す前に関数ごとに名前を変える
- `std::sync::Mutex`、`Box::leak`、`std::ptr::eq`、`Result::as_ref`、`HashSet<&T>::get(&t)`（`&T` を `&&T` に）、生ポインタのハッシュ、`&&str == &&str`
- 修正：参照を通して再帰する型（`enum T { R(&'static T) }`）の `==` でコンパイラがスタックを使い切っていた（glue 関数で比べる）
- 修正：型推論で `&String` を `&str` の引数に渡すと参照どうしを同じ型にしていた（ガードつきの腕で `t.len()` が壊れた値を読んだ）
- 修正：`v == b"crate"`（v は後で型の決まる `x.to_vec()`）で v をバイト列の型にしていた
- 修正：`String` の `contains` などの書き換えで作ったノードに型の記録がなく、メソッドが見つからなかった
- クロージャの変数の呼び出し `tok(k)` の戻り値の型を `peek` で引く
