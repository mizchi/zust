# LSP symbol search (2026-11-17)

Date: 2026-11-17

Added support for `textDocument/documentSymbol` (file outline) and `workspace/symbol` (search by name).

- Symbols are read from the token stream by `symbols` in zust-syntax (no syntax tree is built). Even if the syntax is broken in the middle
  of an edit, it outputs what it can read (a body's `{` is matched by braces alone; if it is not closed, it is cut before the next
  unindented item). Positions are UTF-16 columns in the editor's text itself (before preprocessing).
- What is output: mod / struct / enum / union / trait / impl (the name of `impl Tr for S<T>`) / fn (Method inside impl / trait) / const /
  static / type / macro_rules!, fields of structs and variants, enum variants, and declarations inside extern blocks. detail holds the
  fn signature or the field type. The inside of function bodies is not read.
- workspace/symbol searches the open documents and the .rs files under the initialize rootUri (excluding hidden directories, target, out,
  node_modules). Files that are not open are remembered by modification time and size; only changed ones are re-read.
- Match order: exact → prefix → substring from a word start → substring → characters in order (`nrm` → `norm`). Case-insensitive.
  Up to 256 results. containerName is the enclosing mod / impl / trait.
- Speed (with this directory as the workspace): first call 76 ms (reads 261 files), 3 ms from the second call on.
  Reading the 2854 symbols of compiler_main.rs (2.1 MB) takes 40 ms (the result is kept until the document text changes).
- symbols in tests/lsp.rs, and unit tests in crates/syntax/src/symbols.rs.
