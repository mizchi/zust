# Self-hosting stage 12: moving const evaluation to our own syntax tree

Date: 2026-11-16

- `src/ast_consteval.rs`: ported consteval.rs (static initializer → data bytes and relocations) to our own syntax tree,
  and the driver now uses it to evaluate statics (only on failure does it build the message with location using the syn version).
  The contents of string and byte-string literals (escapes, raw strings) are also read by our own code.
  `tests/ast_consteval.rs` checks that all 696 consts / statics in probe / examples produce the same bytes and relocations as the syn version
- Added: the `&Box<T>` → `&T` coercion (`e = x;` with x: `&Box<Expr>`. Inference also no longer links the two reference types and unifies the contents instead)
