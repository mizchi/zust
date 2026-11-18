# セルフホストの第 11 段階: Program の表を syn から分ける

日付: 2026-11-16

- `src/program.rs`：front.rs の `Program` / `FnSig` / `StructDef` / `AdtTable` / `Insts` と、`build`（要約 → 表）・レイアウト・
  インスタンス化・`implements`・`resolve_with` などを移した。syn の構文木を引く検索（`fn_by_path` / `variant_by_path` / `eval_int` など）は
  front.rs の別の `impl Program` に残る。const の評価（配列の長さ）は自前の構文解析（`parse::parse_expr`）で行う
- `src/key.rs`：キャッシュキーの `KeyHasher` を cache.rs から分けた（cache.rs は codegen に依存するため）
- 足したもの：`thread_local!`（遅延初期化する `static mut` と `with` を持つ unit struct に展開）、unit struct の static、
  モジュールの中の static のパス（`m::S`）、`use crate::m::*` で取り込んだ名前の改名、`&T` → `*const T`、
  `&mut v[i]`（`Vec<Box<T>>`）→ `&mut T`、MutexGuard など Deref の型の `*g` / `g[i]`、`Range::contains`、`next_multiple_of`、
  u128 / i128 の掛け算のオーバーフロー検査（Cranelift に `umul_overflow.i128` がないので 64 bit ずつ組み立てる）、
  タプル・Option の `Hash::hash`（std と同じ順に書き込むので DefaultHasher の値も一致する）、`it.chain([(&a, &b)])`、
  `&&u32 == &&u32`、`print!` 系の型 ()
- 修正：
  - HashMap のキーの Vec を（要素でなく）バッファのアドレスでハッシュしていた（`HashMap<(u32, Vec<Ty>), u32>` が引けない）
  - `*b`（b: Box）から中身をムーブしても b を drop して中身を二重に解放していた
  - `it.zip(slice)` の要素の型を推論せず、`sn == "__array"` で `&String` を `&str` と取り違えていた（推論でも `.iter()` にする）
