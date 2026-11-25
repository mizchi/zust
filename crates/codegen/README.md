# zust-codegen

The backend side of zustc.

| module | what |
|---|---|
| `lower` | replays the recorded instructions (`zust_ir::Recorder`) into minclif and returns machine code; the Cranelift path replays into zust-clif instead |
| `link` | finds the linker (wild, then mold, ld.lld, ld; `--linker=NAME` picks one), saves its command line, and runs it directly |

Depends on: zust-typeck, minclif. Used by: zust-core's `build` feature only, so check and the LSP do not link a
backend.
