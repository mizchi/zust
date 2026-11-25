# zust-typeck

The type check of each fn body, and what is checked on its result.

`acg` walks a fn's AST together with its inferred types, checks it, and records the fn's instructions into a
`zust_ir::Recorder`. `zustc check` and the LSP stop there and drop the recording; a build hands it to
zust-codegen, which replays it into a backend. One walk does both, so check and build cannot disagree.

| module | what |
|---|---|
| `acg/mod.rs` | the per-fn driver of the walk, and the checks that need a whole fn |
| `acg/p1_expr.rs` .. `acg/p9_inline.rs` | the walk by construct: expressions and places, calls, operators, methods, loops and macros, `match` and drops, formatting/hashing/comparisons, the rest, inlining (`-O`) |
| `acg/clif.rs` | a Cranelift-shaped `FunctionBuilder` API over the recorder |
| `acg/exhaust.rs` | `match` exhaustiveness (E0004), a small usefulness algorithm |
| `acg/borrowck.rs` | the borrow check on the CFG that acg emits (moves and loans: E0502, E0507, E0596, E0597, E0716, ...) |
| `acg/regionck.rs` | the region check against signatures (#1): NLL's check of universal regions |
| `acg/simd.rs` | `core::arch::wasm32` SIMD (#30) |
| `cache2` | the per-fn result cache (`Blob`) and its serialization, with no external crate |

Depends on: zust-sema, zust-ir. Used by: zust-codegen, zust-core.

The region and borrow checks are developed in their own series of PRs ("Region check part N"); coordinate
before changing them.
