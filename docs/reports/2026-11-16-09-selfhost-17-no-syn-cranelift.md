# Self-hosting stage 17: building programs through a path that uses neither syn nor Cranelift

Date: 2026-11-16

- `src/ast_codegen.rs`: code generation that produces machine code with minclif from our own syntax tree and the types from ast_infer (the start of replacing codegen.rs).
  `build_object(src)` goes lexer → prelude → ast_summary → program → ast_infer → ast_codegen → minclif → ELF .o (using neither syn nor Cranelift)
- Currently handled: integers, bool, char, casts, let / assignment / compound assignment, arithmetic, bitwise ops, comparisons, `&&` / `||`, if / while / loop / break / continue / return,
  calls to user functions and extern "C" functions. `tests/ast_codegen.rs` links `tests/acg/*.rs` (no_std) with cc and compares the output with the rustc version
- v1: value representation now follows codegen.rs. Local variables are stack slots; struct / enum / array / tuple / fat pointer are handled by address and copied with memcpy;
  aggregate arguments are passed by address; aggregate return values use a hidden first argument (sret). struct / tuple / array / enum values and patterns (match / if let / while let / let-else),
  fields, indexing (bounds checks), `&a[i..j]`, `&[T; N]` → `&[T]`, string / byte-string literals (.data), `len` / `as_bytes` / `as_ptr`,
  for over ranges, arrays and slices, non-generic methods. `tests/acg/aggs.rs` gives the same output as rustc
  (ast_infer now also records the types of array, tuple, reference, operator and cast expressions)
