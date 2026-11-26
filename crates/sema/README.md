# zust-sema

The semantic layer, shared by the compiler and the editor tooling: it builds the program's tables from the item
summaries and infers the types of fn bodies.

| module | what |
|---|---|
| `program` | `Program`: fn signatures, struct and enum definitions and layouts, traits and impls, generic instances, associated types, `implements` for std's traits, constant evaluation of discriminants and lengths |
| `traits` | the trait solver for the program's own traits (modeled and proved in Lean first, `formal/lean`): goals `T: Tr<A..>`, projections, coherence (E0119), auto traits as coinductive goals |
| `solver` | the unifier of type inference: type variables, union-find, deferred associated types and bounds |
| `ast_infer` | type inference of one fn body over the AST |
| `ast_consteval` | evaluates a static's initializer into data bytes and relocations |
| `simd_sig` | signatures of the SIMD intrinsics (#30) |

Depends on: zust-syntax. Used by: zust-typeck.

Tests: unit tests, `tests/check.rs` (the diagnostics), `formal/connect` (the prelude and the solver against the
TLA+ and Lean models).
