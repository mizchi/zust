# Widening the LSP comparison and closing the remaining differences (2026-11-18)

Date: 2026-11-18

**Fixed the benchmark (`bench/lsp_parity.py`)**:

- Four targets: synth (40 generated modules), fixture (the ide test workspace, new), zustc (driver2.rs), and
  ide (navigate.rs, new: a file in a different style with many lexical parts).
- Wait until rust-analyzer is ready: it keeps analyzing types after names resolve, so watch `$/progress` and
  `experimental/serverStatus` (quiescent) and wait until the work is done (before, rust-analyzer completion looked like 0% on fixture).
- Times are p50 and p90. Also the time from startup until names resolve and until work is done, and peak memory (RSS of the server and its child processes).
- "Match average" (the average of the per-feature match) as a single number.
- `--json bench/lsp_parity.json` saves the zustc numbers, and `--compare` adds columns with the difference from the previous run (points for match, ms for time).
- `--detail` also prints differences in workspace/symbol and documentSymbol. `--detail-out` writes them to a file.

**Fixed** (each one was matched only after confirming the behavior by querying rust-analyzer on a small workspace):

- **std re-exports were missing**: `FileIndex::library` collected `pub use` after discarding the token stream.
  This affects completion of `use std::sync::Mu|` and resolution of paths such as `std::collections::HashMap`.
- **hover on string and char literals**: `&'static str` and the value (same shape as rust-analyzer).
- **`include!` and OUT_DIR**: also read the files of `include!("x.rs")` and `include!(concat!(env!("OUT_DIR"), "/x.rs"))`
  (OUT_DIR is the newest build script output under target).
- **`Self`**: definition is the type name in the impl header; references are that and the `Self` in the same impl (same as rust-analyzer).
- **Items inside functions** (`fn walk` written inside `fn new`) and the static in **`thread_local! { static X }`** become definitions
  (things inside thread_local are not shown in documentSymbol, same as rust-analyzer).
- **documentSymbol kinds**: associated functions that do not take `self` are Function, statics are Constant, and `extern "C" { .. }` items are children of a
  block symbol (Namespace).
- **workspace/symbol**: matched rust-analyzer's rules.
  - First search only types (struct / enum / union / trait / type and crate names); if none, search everything (excluding fields, and members inside impls and
    trait impls).
  - Matches where the characters appear in order. A query that contains uppercase letters is case-sensitive. Up to 128 results, in (lowercase) name order.
  - Crate names: package targets (lib / bin / tests / benches / examples / build.rs), dependencies in Cargo.lock, and std.
  - Also aliases from `use a::B as C` and re-exports from `pub use`.
- **On keywords such as `fn`**: definition goes to that name. references / documentHighlight on `fn` give the function's exits
  (`fn`, `return`, `?`, and the last expression; if the last expression is a match / if, the last expression of each match arm and each block).
- **Types of closure parameters**: parameters of `v.iter().filter(|d| ..)` / `opt.map(|x| ..)` / `fold(0, |acc, x| ..)` get the element type of the receiver
  (a reference for filter and similar). This affects completion, hover and definition.
- **rename**: an `enum E { A, B }` declaration was mistaken for struct shorthand and turned into `A: new`. If there is nothing to rewrite, refuse.

**Made faster**:

- references: for each candidate identifier, name candidates were searched across all definitions (more than 100,000, including std). Now a name
  index (`named`) is built for the duration of one query. For a member reference `Q::name`, if Q is a concrete type that is not the owner, it is not resolved at all.
  In-server time 2.5 ms → 0.5 ms (synth).
- documentHighlight: it used to search references across the whole workspace and then filter; now it searches only that file (zustc: 14.9 → 1.2 ms).
- hover / signatureHelp: when re-reading a std file, read only the text (no index is built).
- Responses are written in one write (splitting header and body caused 3 writes because of per-line stdout flushes).

Results (`python3 bench/lsp_parity.py --compare`, zustc / rust-analyzer; the match-average difference is versus before this work):

| Target | Match average zustc | rust-analyzer | Diff | references p50 zustc / ra (ms) | Startup to names / memory zustc | rust-analyzer |
|---|---:|---:|---:|---|---|---|
| synth | 99.7% | 99.1% | +1.3 pts | 0.7 / 0.7 | 13 ms / 17 MB | 2689 ms / 868 MB |
| fixture | 97.3% | 95.6% | +15.4 pts | 0.9 / 0.9 | 7 ms / 17 MB | 332 ms / 833 MB |
| zustc | 96.3% | 95.5% | +4.3 pts | 5.4 / 22.4 | 139 ms / 53 MB | 4932 ms / 2368 MB |
| ide | 96.0% | 97.5% | +8.4 pts | 3.6 / 2.3 | 170 ms / 53 MB | 5391 ms / 1874 MB |

- workspace/symbol is 100% on all four (before: 62–87%), documentSymbol is 100% including kinds, and documentHighlight is 100%.
- rust-analyzer's match average is not 100% because places where it did not answer (such as "answered" for definition) are counted.
- The large difference on fixture includes the share where the old comparison queried before rust-analyzer finished analyzing (completion at 0%).
- Completion for `x.|` on ide looks like -10 points, but navigate.rs changed during the work, so the compared positions are different.
- Remaining differences:
  - rename of `Default::default` (rust-analyzer rewrites into dependency crates; deliberately refused), and
    on Point in `use a::{Point as Pt}` (rust-analyzer changes only the alias and not the uses; zustc changes the definition).
  - references for `Key`: rust-analyzer counts a different `Key` inside lexer.rs, which is shared with build.rs.
  - Completion on receivers of unknown type (`x.as_str()` and so on), signatureHelp for std methods (`map(` and so on), and methods of numeric
    types (`wrapping_sub`: generated by macros in core).
