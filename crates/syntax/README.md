# zust-syntax

The syntax layer, shared by the compiler and the editor tooling: it turns a crate's source into items, syntax
trees and item summaries.

The pipeline, in order:

1. `srcmap`: a multi-file crate becomes one source (`mod x;` is replaced by `x.rs`), with a map back to the files.
2. `cfg`: conditional compilation removes what a false `#[cfg(..)]` takes out, and checks the predicates.
3. `prep`: the preprocessing. It expands `thread_local!` and `macro_rules!` (`macros`), flattens modules, moves
   items written inside fn bodies out of them, and rewrites constructors used as values (`ctors`).
4. `lexer`: splits the source into top-level items, each with a normalized hash (the incremental unit), and does
   module renaming and glob resolution.
5. `parse`: the Rust parser (no syn). It produces the AST (`parse::Expr`, `parse::Item`, ...).
6. `ast_summary`: item summaries (signatures, fields, impls, bounds), the input of zust-sema. `ast_types`,
   `ast_closure` and `regions` read types, closures and lifetimes from the AST for it.

Other modules:

| module | what |
|---|---|
| `prelude` / `prelude_lib.rs.in` | the built-in replacement for core/std: types and traits written in the subset zustc compiles, selected per program by the names it uses |
| `types` / `types_enc` / `enc` | `Ty` (resolved types), `TySyn` (written types), interning, and the cache encoding of summaries |
| `front` | the shared front-end flow (prep, split, summarize) used by the driver and the analyzer |
| `stub` | a dependency crate's interface, as the crate that uses it sees it |
| `wf` | well-formedness of the types one item writes |
| `pm` | proc macros, run through the toolchain's proc-macro server (#29) |
| `test_harness` | `zustc --test`: the `#[test]` fns and the entry point that runs them |

Diagnostics found here are rustc's messages at rustc's positions. Definition errors that the macro expander notes
while reading `macro_rules!` are kept in thread-locals and placed in the source by the driver.

Tests: unit tests in the modules, `tests/parse.rs` (the parser against syn on zustc's own sources),
`tests/macros_expand.rs` (the fast expander against the reference one), `tests/prep_inc.rs` and
`tests/split_inc.rs` (the incremental preprocessing and split against the full ones), and `tests/selfhost.sh`.

Depends on: zust-base, proc-macro2 (the self-host build uses `tests/selfhost/mini_pm2.rs` instead).
