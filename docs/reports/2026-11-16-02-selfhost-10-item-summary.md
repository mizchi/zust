# Self-hosting stage 10: moving item summaries to our own syntax tree

Date: 2026-11-16

- `src/ast_summary.rs`: ported front.rs's `summarize` / `parse_sig` / `lift_closures` / `parse_and_summarize` (syn syntax tree → item summary) to our own syntax tree.
  `tests/ast_summary.rs` checks that all 2199 items, after the same preprocessing as the driver, produce the same item summary as the syn version
  (the text of const initializers and bounds uses the same spacing as proc_macro2's output)
- The item summary types (`Summary` / `SigSyn` / `VariantSyn` / `ClosureSyn` / `Owner`, etc.) also moved to types.rs
- Aliases in `use a::{X as Y}` inside modules (also when names do not collide)
- Fix: in `let nn = ..; if let Some(nn) = nn { v.push(nn) }` (a move through a binding with the same name as the match scrutinee), the outer variable was not marked as moved, so it was freed twice
- The type of an `if` / block expression is looked up with `peek` (`if c { "a" } else { "b" }.to_string()`)
