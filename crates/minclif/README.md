# minclif

The minimal backend: x86-64, System V, ELF objects. Its API follows Cranelift's `FunctionBuilder` (blocks, block
parameters, variables, stack slots, instructions), so the same recorded stream drives either backend.

Code generation is deliberately simple: every value lives in an 8-byte frame slot and is loaded into rax / rcx for
each instruction. Integers are kept zero-extended to their type's width, sign-extended only for signed compares
and extensions.

No dependencies, so zustc can compile it itself; it is the base for replacing Cranelift in a self-hosted zustc.
Select it with `--backend=minclif` (Cranelift is the default).

Depends on: zust-ir. Tests: `tests/minclif.rs` (links with cc and runs), the self-host test (the objects match
rustc's build byte for byte).
