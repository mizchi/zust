# zust-analyzer

The LSP server (stdio). It supports initialize, didOpen / didChange (full sync) / didClose, shutdown / exit,
documentSymbol, workspace/symbol, definition, references, hover and completion.

- Each open document has its own `Session2` with an in-memory cache. Every open and change runs `Session2::check`
  and publishes diagnostics. Unchanged fns are skipped by their "checked" mark, so the response time follows the size
  of the change.
- Symbols, definitions and references come from zust-ide's index of the open documents and the workspace's `.rs`
  files, remembered by mtime and size. `ZUST_ANALYZER_MEM_MB` (default 4) bounds the text kept in memory; files
  past it keep their definitions and names only, and are read again when a reference or hover needs them.

Depends on: zust-core (without `build`), zust-ide, serde_json. Tests: `tests/lsp.rs`; the KPI's LSP session
(`bench/lsp_parity.py`).
