# zust-ide

Editor analyses for zust-analyzer. The compiler does not depend on this crate.

It reads zust-syntax's token stream only, with no syntax tree and no type check, so it answers as far as it can
in a source that is broken in the middle of an edit.

| module | what |
|---|---|
| `symbols` | documentSymbol / workspace/symbol: items and their members |
| `navigate` | the per-file index (`FileIndex`), and definition / references / hover / completion |

Depends on: zust-syntax. Tests: `tests/golden.rs` (golden files under `tests/golden`), `navigate_tests.rs` (unit
tests written to kill surviving mutants).
