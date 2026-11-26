# zust-core

The driver. `session::Session` runs the analysis, `check` (the LSP's diagnostics), the caches, and, with the
`build` feature (the default), the workspace build: parallel compilation, machine code, objects and linking.

```
source -> zust-syntax (srcmap, cfg, prep, lexer, parse, ast_summary) -> zust-sema (Program, inference)
       -> zust-typeck (acg: check + record) -> [build] zust-codegen (lower, link)
```

| module | what |
|---|---|
| `session.rs` | `Session`: the item and fn caches (memory and disk), incremental re-checking, the checks that need the whole program (well-formedness, coherence, lints read from the source, E0xxx placed at rustc's positions) |
| `session/build.rs` | the build: lowering each fn, the fork-based parallel compile, objects, linking, multi-crate workspaces |
| `session/dead.rs` | `dead_code` below the item level |
| `session/dyncompat.rs` | dyn compatibility (E0038) |
| `testgen` | the program generator the benches and tests use (`bench/gen.py`'s output) |

The lower layers' modules are reachable from here too (`zust_core::lexer`).

Depends on: zust-typeck, and zust-codegen with `build`. Used by: zustc (`src/`), zust-analyzer
(`default-features = false`), zurgo.

Tests: most of `tests/` runs through `Session` (`tests/check.rs`, `tests/run_examples.rs`, `tests/incremental.rs`, `tests/cranelift_backend.rs`, `tests/opt_inline.rs`, `tests/workspace.rs`, ...).
