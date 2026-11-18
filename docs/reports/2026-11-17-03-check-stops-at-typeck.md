# Stop check at type checking to make the LSP faster (2026-11-17)

Date: 2026-11-17

- check (`zustc check` and the LSP) now stops after acg code generation (type errors are reported there) and does not produce machine code
  (Cranelift / minclif) (`Inline::check_only`). The "checked" marker is "an empty Blob that holds only the relocations to the requested
  generic function instances", stored under a key separate from the build Blob (`CHECK_SALT`) (if a build Blob exists, it also counts as checked).
  `ZUSTC_CHECK_CODEGEN=1` produces machine code too.
- Prelude selection (`select_with`) searched the whole source with `contains` about 80 times. It now searches per item and remembers the
  result by item hash (`PreludeScan`). Until now `SelectCache` was rebuilt on every call, so the per-item memory had no effect;
  Session2 now owns it (`FrontMemo`).
- `lexer::split` is now incremental (`split_inc`): it remembers the previous input and result, reuses the items in the unchanged parts
  before and after (shifting position and line for the part after), and re-splits only the middle. Boundaries are top-level items that
  start after whitespace (impl / trait as a whole). If the end of the middle may be inside a comment or string, or if split fails,
  the whole file is split. tests/split_inc.rs keeps applying random edits to real source and compares each result with a full split.
  `ZUSTC_SPLIT_VERIFY=1` compares every time; `ZUSTC_NO_SPLIT_INC=1` turns it off.

| | Before | After |
|---|---:|---:|
| `zustc check` compiler_main.rs (38886 lines) | 3.6 s | 1.6 s |
| LSP didOpen / didChange, 1000 generated functions (17019 lines, over stdio) | 135 / 12.2 ms | 122 / 9.6 ms |
| LSP didOpen / didChange, compiler_main.rs (editing 1 function) | 2.09 s / 523 ms | 1.63 s / 476 ms |
| split (didChange on 1000 generated functions) | 2.0 ms | 0.6 ms |

Remaining (breakdown of the 476 ms didChange on compiler_main.rs): preprocessing that rewrites the whole file (macro expansion 97 ms,
`mod` flattening 63 ms, moving nested items 47 ms, constructor rewriting 33 ms; each one re-lexes the whole file), Program and references 47 ms,
cache keys and read-back for all functions (9115) 100 ms. All of these rebuild everything, so the next step is to make them incremental per item.
