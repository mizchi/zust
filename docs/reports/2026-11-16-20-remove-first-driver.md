# Removing the first driver (syn + Cranelift)

Date: 2026-11-16

- Moved `zustc check` and the LSP to driver2's `Session2::check`. It compiles every function (including unreachable ones) and the requested generic function instances,
  collects the errors, and puts the results in the cache (the next check or build uses them)
- Deleted `crates/legacy` (syn, cranelift-object, zstd), the tests that compared against the syn version (ast_closure / ast_consteval / ast_infer /
  ast_summary / ast_types), and the self-hosting elfz. The expected outputs became golden files in `tests/expected/`
  (generated with the syn version), and `tests/driver2.rs` compares against them
- Bug found: after `Session2::save_cache`, the in-memory cache was empty (the set of used keys was cleared first).
  Consecutive builds in the same Session recompiled every function. Found by an assert in the benchmark's `build/edit_1fn`
- Added a location (the position of the path) to the error for the wrong number of fields in a tuple struct pattern
