# zust-codegen

The backend side of zustc.

| module | what |
|---|---|
| `lower` | replays the recorded instructions (`zust_ir::Recorder`) into minclif and returns machine code; the Cranelift path replays into zust-clif instead |
| `link` | finds the linker (wild, then mold, ld.lld, ld; `--linker=NAME` picks one), saves its command line, and runs it directly |

Depends on: zust-typeck, minclif. Used by: zust-core's `build` feature only. The LSP is built without it and links
no backend; the `zustc` binary has it (and the backends) for every command, and `zustc check` runs none.
