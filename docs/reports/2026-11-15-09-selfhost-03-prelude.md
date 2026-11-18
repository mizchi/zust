# セルフホストの第 3 段階: prelude.rs

日付: 2026-11-15

- `include_str!("..")` / `include_bytes!("..")`（入力ファイルからの相対パス。中身はエスケープして 1 行のリテラルにする）
- `std::sync::OnceLock`（`get_or_init` / `get` / `set`）。mut でない static も書き込める所に置く（内部可変性）
- `str::rsplit` / `rsplit_once` / `match_indices`、クロージャのパターンの `starts_with` / `ends_with`、`Iterator::find_map`
- 要素が整数などでないスライスの `contains`（`NAMES.contains(&w)`、PartialEq で比べる）
- 参照の列の `flatten`（`map.get(k)` の `&Vec<T>` などを `&T` で順に）
- const のパターン（`Some(BOX_FREE) | Some(DROP_GLUE)`、初期値がリテラルの const）
- `&s[a..b]`（s: &str / String）の型推論、`matches!` の中のメソッドの連鎖（parse し直した式の型を位置で引く）
- 修正：`a || f(&format!(..))` の右辺の一時値の drop フラグが、右辺を通らないとき初期化されていなかった（入口で 0 にする）
