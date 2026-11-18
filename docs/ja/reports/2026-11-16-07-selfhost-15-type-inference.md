# セルフホストの第 15 段階: 型推論を自前の構文木へ移す

日付: 2026-11-16

- `src/ast_infer.rs`：infer.rs の構文木を辿る部分（パターン・ブロック・式・メソッド呼び出し・クロージャ・書式マクロの引数など 1,900 行）を
  自前の構文木に移植した。単一化器は solver.rs を共有する。`tests/ast_infer.rs` が probe / examples と zustc 自身のモジュール
  （`out/selfhost/program_main.rs`）の 885 関数・2,380 個の let で syn 版と同じ型になることを確かめる
- 推論の結果をノードのアドレスに加えてソース上の位置でも記録し、codegen が書き換えで複製したノード（`__exr_slice_to_vec(&a[..])` の中の式など）も型を引けるようにした
- 足したもの：`DerefMut` trait、Deref 越しのフィールド（`self.prog` で self: `&mut Inf`）、`Option::iter` / `into_iter` と `it.chain(Some(x))`、
  クロージャで `split`（`s.split(|c| ..)`）
- 修正：
  - `let Some(name) = name else { .. }`（外の変数を同じ名前で隠す）で、外の変数の drop をやめず二重に解放していた
  - 一時値のフィールドのムーブ（`f().0`）で一時値を丸ごと drop していた（残りのフィールドだけ drop する）
  - 相互再帰の型（`Ty` ↔ `FnTy`）の deep hash が同じになり、別の型のインスタンス（`HashMap::find<&FnTy>` と `<&Ty>`）のキャッシュキーがぶつかっていた
  - 自前の構文解析で `&&T` を 3 重の参照と読んでいた
  - `*const T` の `const` をモジュールの名前空間の定義と見ていた
- 回避（zustc が未対応のもの）：スコープにない trait のメソッドを候補から外さない（`Solver::deref` を `deref_var` に改名）、
  `std::mem::discriminant`（形の番号の関数にした）
