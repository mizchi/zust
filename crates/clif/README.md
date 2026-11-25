# zust-clif

The Cranelift backend. It reads the same recorded instruction stream as minclif (in the order acg made it) and
builds Cranelift IR from it, so the two backends can be compared on the same front end and the same instructions.

It returns what minclif returns: machine code, relocations (name and ELF kind), and trap locations. The cache and
the object writer are shared.

Not part of the self-host build (it depends on cranelift-codegen, cranelift-frontend, cranelift-native).

It is zustc's default backend (`--backend=cranelift`; `--backend=minclif` or `ZUSTC_BACKEND=minclif` selects
minclif).

Depends on: minclif. Tests: `tests/driver2_cranelift.rs`.
