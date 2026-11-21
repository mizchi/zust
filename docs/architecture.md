# Design

## Crate layout (Cargo workspace)

The layers go in this order: base → syntax → sema → typeck → codegen → core (driver). syntax, sema and typeck are the parts that the compiler, the LSP and analysis
share. The LSP uses core without the feature `build`, and does not depend on the backends (codegen / minclif / Cranelift).

| crate | Contents | Depends on |
|---|---|---|
| `crates/base` (`zust-base`) | fxhash (FxMap / FxSet), key (cache keys), prof (time per stage) | none |
| `crates/syntax` (`zust-syntax`) | front (the flow preprocessing → splitting into items (diff) → attaching the prelude, plus FrontMemo, the memory kept across edits) / lexer / parse (own syntax tree) / cfg / macros / ctors / prep (preprocessing diff) / srcmap / stub / types (item summary types), types_enc / enc (serialization) / ast_types / ast_closure / ast_summary (item summaries) / prelude (selection and prelude_lib.rs.in, build.rs) | base, proc-macro2 |
| `crates/sema` (`zust-sema`) | program (tables of names, types, traits and impls; instances) / solver / ast_infer (type inference) / ast_consteval | syntax |
| `crates/ir` (`zust-ir`) | Types for the instruction sequence (Type / Value / Block / Op …), and Recorder, which only records instructions (it remembers the types of values) | none |
| `crates/typeck` (`zust-typeck`) | acg (records the syntax tree as an instruction sequence; type checking errors come out here) / cache2 (Blob serialization) | sema, ir, proc-macro2 |
| `crates/codegen` (`zust-codegen`) | lower (rebuilds the record into minclif and turns it into machine code; for Cranelift it passes it to zust-clif) / link | typeck, minclif |
| `crates/core` (`zust-core`) | driver2 (Session2: analysis, check, cache), and, with the feature `build` (default), driver2/build.rs (machine code, fork parallelization, objects, linking, workspaces). Re-exports the lower layers | typeck, (with build) codegen |
| `crates/ide` (`zust-ide`) | Analysis for editors: symbols (symbols for documentSymbol / workspace/symbol) / navigate (definition, references, hover, completion; light type estimation from the token sequence and the definition table). Not used by the compiler itself | syntax |
| `crates/analyzer` (`zust-analyzer`) | LSP server (binary `zust-analyzer`). Diagnostics come from driver2's check; everything else from ide. Workspace file contents are kept up to a memory limit | core (without build), ide, serde_json |
| `crates/zurgo` (`zurgo`) | A Cargo-compatible driver (binary `zurgo`, [zurgo.md](zurgo.md)): its own TOML parser, Cargo.toml / Cargo.lock, the sources (members, paths, `$CARGO_HOME/registry/src`, the index cache), feature activation as cargo does it (the feature resolver for builds, the dependency resolver for `metadata`), and a zustc workspace (`CrateSpec`) built in process | `zustc` (the root crate) |
| `crates/minclif` | A small x86-64 backend with a Cranelift-like API (register allocation, turning fields of stack structs into variables, ELF .o). `from_recording` rebuilds a record | ir |
| `crates/clif` (`zust-clif`) | The Cranelift backend of driver2: turns the instruction sequence that minclif built into Cranelift IR and compiles it | minclif, cranelift |
| `.` (`zustc`) | The CLI, and re-exports of the above (for tests and benches). Does not depend on ide or lsp (only the bench lsp/* uses them, as dev-dependencies) | the above (except ide and lsp) |

Each crate's lib.rs re-exports the modules of the lower layers (`pub use zust_syntax::*;` and so on). Inside files, paths are written like
`crate::lexer`. So the same path resolves whichever crate a file is in, and also when self-hosting lays everything out as modules of one crate.

How type checking and the backend are split: acg only records instructions into zust_ir::Recorder (it numbers values, blocks and so on, and remembers the types of values
the same way minclif decides them). check stops there. A build has codegen's lower rebuild the record into minclif's FunctionBuilder in the same order,
then turn it into machine code (minclif creates values and slots inside instructions, so the rebuilding side renumbers them.
`ZUSTC_IR_VERIFY=1` checks that the value types match the record). The output machine code is byte-for-byte the same as before the split.

The only path is **driver2** (`zustc x.rs` uses Cranelift; `--backend=minclif` uses minclif). build, check and the LSP all use driver2.
The first driver (syn + Cranelift, `crates/legacy`) was deleted on 2026-11-16 (it is in the git history).
Self-hosting (tests/selfhost*.sh) combines the sources of base / syntax / sema / ir / typeck / codegen / core and minclif into one file and compiles it with zustc.

## Processing flow (driver2)

The current main path (driver2):

```
src.rs ──srcmap (mod x; files)──▶ macros (macro_rules!) ──▶ flatten modules ──▶ lexer: items + hashes
   ──▶ item summaries (cached, parallel via fork) ──▶ Program (signatures, layouts) ──▶ reachable functions
   ──▶ per function: cached? ── miss ──▶ ast_infer (type inference) → acg (own syntax tree to minclif instruction sequence; -O inlines small functions)
                                        → machine code via Cranelift (zust-clif) or minclif (register allocation) → Blob (parallel via fork)
   ──▶ .o via minclif's ELF writer (includes the panic location table) ──▶ link with wild (skipped if unchanged)
```

| Location | Role |
|---|---|
| `crates/syntax/src/lexer.rs` | Splits top-level items, and hashes the normalized token sequence. Splits `impl` into a header and members |
| `crates/syntax/src/macros.rs` / `srcmap.rs` | Expansion of macro_rules! and include_str!; reading `mod x;` files and mapping lines |
| `crates/syntax/src/parse.rs` / `ast_*.rs` | Own syntax tree, item summaries, type inference, closure analysis, constant evaluation |
| `crates/sema/src/program.rs` | Whole-program tables (signatures, layouts, trait implementations, instances) |
| `crates/typeck/src/acg/` | One-pass lowering from the syntax tree to the instruction sequence (zust_ir::Recorder), and type checking. -O inlines small functions |
| `crates/core/src/driver2.rs` / `cache2.rs` | `Session2`: cache (disk + memory), build / check / workspaces, fork parallelization, writing .o |
| `crates/codegen/src/link.rs` | Extracts the link line from `cc -###`, saves it, and starts the linker directly |
| `crates/minclif` | Instruction recording, register allocation, x86-64 machine code, ELF .o |
| `crates/clif` | Recorded instruction sequence → Cranelift IR → machine code |
| `crates/analyzer/src/lib.rs` | LSP server (holds one `Session2` per document and returns diagnostics from `check`; index of the workspace and std) |
| `crates/ide/src/navigate.rs` / `symbols.rs` | Name resolution for definition, references, hover and completion; symbol lists |
| `src/main.rs` | CLI (build / check / workspace build) |

### Key points of incrementality

(About monomorphization)
- Generic functions and default methods become one instance per requested (function, type arguments).
  The key is "the original function's key + the type arguments (structs by deep hash)".
- Instances are processed with a worklist until a fixed point. The cached Blob of a caller stores its instance references as (original function name, type arguments). So when it is read back, the needed instances are requested again.
- Measured (examples/traits.rs, 25 functions / 11 instances):
  - Editing the body of `total_area<T>` recompiles only its 2 instances.
  - Editing the body of the default method `score` recompiles only the 2 types that use it (Rect, which overrides it, is not affected).
- In check (LSP) too, the "checked" mark includes "the instances this function requested".
  So type errors in generic bodies are reported even when the caller hits the cache.

1. **Changing a callee's body does not recompile the caller.**
   Only the signatures of referenced items go into the key, and the Blob refers to the callee by name (the same idea as blaze's template).
   Only a signature change invalidates the functions that reference it.
2. **Structs are tracked by deep hash.**
   - The deep hash combines the definition hashes of the struct itself and of every struct reachable transitively from its fields. This includes fields reached through references.
   - Structs also go into signature hashes by deep hash.
   - So even a function that does not name `Inner`, like `fn get(o: &Outer) -> i64 { o.inner.b }`, is correctly recompiled when the layout of `Inner` changes (verified).
3. **`impl` blocks are tracked per method.**
   - The lexer splits the `impl X {` header and the members into separate items. The header hash is mixed into each member hash.
   - So editing one method re-parses only that method and recompiles only that method (verified: 1 miss out of 18 functions).
   - Changing a signature recompiles only that method and its callers (2 misses).
   - A method call like `x.len()` enters the reference graph by the last segment of the name (`len`), without looking at types. This counts as referencing `len` of every type, so it is an over-approximation, but it is on the safe side.
4. **Reformatting or editing comments recompiles nothing.**
   The cache hits on everything, and the `.o` is identical, so linking is skipped too.
5. **Unchanged items are not even parsed.**
   Parsing a whole file with syn takes 80ms for 1000 functions (320KB). The own lexer takes only 2.6ms.
6. **Unreachable functions are not parsed, type checked or code generated.**
   As a result, they do not go into the object either.
   A side effect is that type errors inside unreachable functions are not reported (rustc would report them).
7. **Disk usage has an upper bound.**
   Only the entries used in the current build are saved, so old generations disappear automatically. The cache size stays around "the current amount of code".
8. **Memory**
   - Only one function's AST is alive at a time.
   - Cranelift's `Context` and `FunctionBuilderContext` are reused per thread.
   - Incremental builds do not start threads.
   - On cold builds (33 or more item summary misses), the parsing for item summaries runs in parallel, and the ASTs are not brought back.
     Keeping them all made peak RSS 162MB from syn's ASTs alone at 5000 functions. Not bringing them back brought it to 35MB.

## Value representation and ABI (internal)

- **Scalars** (integers, bool, references) are SSA values. References are pointers.
- **Local variables**
  - Scalars whose address is not taken are put in Cranelift `Variable`s (registers).
  - Things referenced by `&x`, and structs, are put in stack slots.
- **Structs**
  - The value is always in memory, and the result of evaluating an expression is "its address".
  - It is copied at let, assignment, argument passing and return (the size is known statically, so it expands to load/store).
  - Arguments: the caller copies into a temporary area and passes its address.
  - Return values: the caller passes the address of an area it allocated as a hidden first argument (sret).
  - Structs cannot be passed by value to or from extern "C" functions (it is an error).
- **Layout**: fields are laid out in declaration order with natural alignment (equivalent to repr(C)).
- **Closures** (`crates/syntax/src/ast_closure.rs`)
  - At the item summary stage, the k-th closure (pre-order) of a body is lifted into the following two (lambda lifting).
    - struct `parent::{closure#k}<parent's type arguments, __C.., __P.., __R>`. The fields are the captures: `*mut T` for a by-reference capture, `T` for `move`.
    - fn `parent::{closure#k}::call(__env: *mut that struct, arguments..) -> __R`.
  - The types of the captures, arguments and return value all go into the struct's type arguments. So the closure's type alone determines the body's instance, and it fits directly into the existing monomorphization and per-instance cache.
  - For the body, the parent item is parsed again, the k-th closure is taken out, made into an ordinary fn and compiled. Captures enter scope as fields of `__env`.
  - A closure that is never called still requests its instance when it is created (to report errors in its body; if unused, `--gc-sections` removes it).
- **Function pointers**: one word holding the function's address. Taken with `func_addr` and called with `call_indirect` (the ABI is the same as an ordinary call).
- **`Box<T>`**: a pointer to the heap (`Box<dyn ..>` is a fat pointer). Auto-deref and conversion to dyn go through the same path as references.
- **drop** (`crates/typeck/src/acg/p6_match_drop.rs`)
  - Local variables and temporaries holding values that need drop are registered as owners, together with a 1-byte drop flag. A move sets the flag to 0, and at scope exit the ones whose flag is 1 are dropped in reverse order (this is correct even when whether a move happens depends on a branch).
  - The drop body is an instance of the prelude's `__exr_drop_in_place::<T>` (drop glue). codegen builds a body that expands only one level per type (call to `impl Drop` → fields / variants / elements → free if Box), and nested types become calls, so expansion stops even for recursive types.
  - The last slot of a vtable is the drop glue of the contained type. Dropping a `Box<dyn Trait>` calls it and then frees.
  - Drop glue is cached per instance. Whether `impl Drop` exists is part of the deep hash, so adding or removing it later rebuilds the functions that use that type.
- **`&dyn Trait`**: 16 bytes of (data pointer, vtable address).
  - A vtable is a sequence of function addresses in the declaration order of the trait's methods, held in the Blob as "a sequence of function IDs".
  - The cache stores them by function name (for an instance, the original function name and type arguments).
  - In the `.o`, they become read-only data deduplicated by content.
  - The vtable of `&dyn Fn(A) -> B` is just the one instance of the closure body.
