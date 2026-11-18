# Incremental preprocessing per item (2026-11-17)

Date: 2026-11-17

Preprocessing (thread_local! expansion, macro expansion, `mod` flattening, moving nested items, constructor rewriting) is now gathered in
`crates/syntax/src/prep.rs` and remembered per chunk across edits.

- Chunk: a range of lines cut at item boundaries (also inside `mod` / `impl` / `trait`). Cutting into chunks is also incremental
  against the previous run (the boundaries in the unchanged parts before and after are kept; only the middle is re-lexed).
- Every preprocessing step rewrites token by token using data gathered from the whole file (macro definitions, element counts of tuple
  structs and variants, the module tree and `use`, names of nested items), and does not change the number of lines. The gathered data
  depends only on item declarations, so the same data can be gathered from a "skeleton" in which the expressions inside function
  bodies and the initializers of const / static are removed (nested items, `use`, `macro_rules!` and user macro calls inside bodies
  are kept; a body with nothing to keep is removed together with its braces).
- If the whole skeleton is the same as last time, unchanged chunks reuse the previous result, and only the changed chunks are
  preprocessed in a "source where everything else is skeleton" (2.1 MB → 0.31 MB for compiler_main.rs) and then cut out by line.
  When the skeleton changes (a declaration edit), when braces are unbalanced, or when a moved item crosses a chunk boundary, the
  whole file is preprocessed.
- The first call only preprocesses the whole file (a build that preprocesses just once does not create chunk records).
- tests/prep_inc.rs: keeps applying random edits (inside bodies, and anywhere) to the files of examples and crates/core/src bundled with
  `mod`, and each time compares the text, the column shifts and success/failure with full preprocessing (incremental path 156 times,
  full path 364 times). `ZUSTC_PREP_VERIFY=1` compares every time, `ZUSTC_NO_PREP_INC=1` turns it off, `ZUSTC_PREP_LOG=1` prints the path.
- The random edit test found a hoist bug where renaming nested items in broken source never stopped and caused a stack overflow.
  It is fixed (no retry if the rename changes nothing; at most 8 retries).

| Editing 1 function in compiler_main.rs (38886 lines) | Before | After |
|---|---:|---:|
| Preprocessing | about 270 ms | 28 ms |
| LSP didChange (over stdio) | 476 ms | 238 ms |

Remaining (didChange 238 ms): cache keys and read-back for all functions (9115) about 100 ms, references 30 ms, Program 18 ms, item summary 8 ms,
split 6 ms, prelude selection 7 ms.
