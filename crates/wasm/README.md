# zust-wasm

The wasm32-unknown-unknown backend.

- `compile` has the shape of the Cranelift backend: it reads the recorded instructions and returns one function's
  code (a wasm function body) with relocations. Caching, the fork-parallel compile, ICF and the object layout of
  `session/build.rs` work unchanged.
- `write_module` writes one wasm module from the functions and data that the build would have put in an ELF object.
  zustc compiles whole programs, so no wasm linker is needed.

The module documentation in `src/lib.rs` describes the memory model and what keeps a program inside its sandbox.

Depends on: minclif. Tests: `tests/wasm.rs`.
