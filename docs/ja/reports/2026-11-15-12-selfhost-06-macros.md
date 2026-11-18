# セルフホストの第 6 段階: macros.rs

日付: 2026-11-15

- struct と同じ名前のバリアント（`TokenTree::Punct(Punct { .. })`：裸の名前は struct を優先）
- `&a == &b`（a, b が struct などの参照どうし）、`impl PartialEq` と `impl PartialEq<str>` の両方
- `Option::flatten`、HashMap の `clone`（以前は浅いコピーで二重解放になっていた）、`String::replace_range` / `into_bytes`
- 修正：`None` などの大文字の名前のパターンをクロージャのキャプチャ候補に数えていた
- 修正：`include_str!` の展開が、同じ行の 2 つ目以降のコメント・文字列の中のものも展開していた
