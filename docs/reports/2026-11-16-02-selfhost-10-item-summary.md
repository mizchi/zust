# セルフホストの第 10 段階: アイテムの要約を自前の構文木へ移す

日付: 2026-11-16

- `src/ast_summary.rs`：front.rs の `summarize` / `parse_sig` / `lift_closures` / `parse_and_summarize`（syn の構文木 → 要約）を自前の構文木に移植。
  `tests/ast_summary.rs` が driver と同じ前処理の後の全 2199 アイテムで syn 版と同じ要約になることを確かめる
  （const の初期値のテキスト・境界のテキストは proc_macro2 の表示と同じ空け方）
- 要約の型（`Summary` / `SigSyn` / `VariantSyn` / `ClosureSyn` / `Owner` など）も types.rs へ
- モジュールの中の `use a::{X as Y}` の別名（名前がぶつからないときも）
- 修正：`let nn = ..; if let Some(nn) = nn { v.push(nn) }`（match 対象と同じ名前の束縛でムーブ）で外側の変数をムーブ済みにせず、二重に解放していた
- `if` / ブロックの式の型を `peek` で引く（`if c { "a" } else { "b" }.to_string()`）
