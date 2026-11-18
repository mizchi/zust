# Survey notes: blaze / relax / wild

Date: 2026-11-14

To use in the design of zustc, I read three projects and organized the findings (I only read the source of main as of 2026-11-14 and did not build them. Only wild was actually used, by installing 0.10.0 from crates.io).

- blaze: https://github.com/indexable-inc/blaze
- relax: https://github.com/makepad/relax
- wild: https://github.com/wild-linker/wild

## Overview

| | blaze | relax | wild |
|---|---|---|---|
| What it is | A replacement for cargo. A resident daemon builds without using rustc | A Rust JIT and live compiler | A fast ELF linker |
| Frontend | A fork of rust-analyzer (rowan, hir-def/hir-ty). The trait solver is vendored from rustc | All its own (lexer, Pratt parser, typeck and next-solver ported from rustc). No dependency crates | - |
| IR | HIR → MIR → (CLIF / LIR) | typeck::Body → RIR (non-SSA vregs, CFG) | - |
| Backend | Currently MIR → CLIF. v2 plans copy-and-patch stencils for dev builds and LLVM for release builds, and will not use Cranelift for native code generation | Its own (interp → O1 → O3, linear scan regalloc) | - |
| Compilation unit | One monomorphized function instance | One function instance (lazily, on first call) | Input sections |
| Cache key | BLAKE3, with the body α-normalized and no spans. Callees by interface hash only | 128bit. compiler ID + instance + interface fingerprints of dependencies + body | - |
| Linking | Its own linker (about 89k lines). In-place patching and jump tables | No linker (JIT). Has a startup image | Parallel. `--gc-sections` is on by default, no ICF, LTO only via plugins |

## What can be copied from blaze

1. **Per-function cache + callees by interface hash only** (`crates/blaze-codegen/src/template.rs`).
   Compiled code holds callees by "position in the reference list", so editing a callee's body does not recompile the caller. Only the relocation names are reassigned.
   → zustc adopted this as-is (relocations in a `Blob` are held by function ID or name).
2. **No whitespace, comments or spans in the body hash** (`docs/v2/plan/design/item-hash.md`).
   blaze also α-normalizes local variables into de Bruijn-style indices, so renaming a local variable does not invalidate either.
   → zustc only goes as far as lexer-level normalization. No α-normalization yet.
3. **Reachability**: compile only what is reachable from main, tests and exports.
4. **Do ICF and polymorphization in the compiler, not the linker**:
   - Keys are made from layout (size/align/ABI), so `Vec<A>` and `Vec<B>` can share code.
   - Identical functions are folded by a hash of code + relocation targets (`dedupe.rs`).
   - Functions that differ only in constants are parameterized (`fold.rs`).
5. **In dev builds, make symbol names short hashes and keep only the line table**.
6. **Incremental linking**: leave slack in `.text`, append changed functions at the end, and call through a jump table. It is enough to consider this once linking dominates.
7. **What is not needed**: the daemon, DBSP, stencils. These are monorepo-scale mechanisms and overkill for personal use.

## What can be copied from relax

1. **Demand-driven**: both name resolution and type checking are done only for what is reached (`front/src/lazy.rs`, `demand.rs`). For AOT this becomes "compile only the set reachable from main".
2. **Cache format** (`compiler/src/cache.rs`, `store.rs`):
   - 1 crate = 1 file. After the header comes a sorted index, then an append-only payload. It switches between A/B index slots to be crash-safe.
   - Relocations are saved with stable IDs (definition keys, string bytes, and so on).
   - When inlining, it also records the keys of the inlined functions.
   → zustc also uses one file. Index + payload, decoding only on a hit.
3. **Its own small std** (about 120k lines). It does not compile rust-src, so both build time and binaries are smaller.
4. **Key points of the optimization tier** (1.12x vs LLVM):
   - Inlining (rustc's cost model).
   - SROA-style work on stack slots. Cranelift has no SROA, so this part has to be written by hand.
   - LICM, bounds check elimination via value range analysis.
5. **The parser has no error recovery and stops at the first error**. That is enough for personal use.

## What can be copied from wild / what zustc should do itself

1. **How to emit objects**:
   - Put each function in its own section (`ObjectBuilder::per_function_section(true)`). Without this, `--gc-sections` removes nothing.
   - Make non-exported things Local/hidden.
   - Use `-no-pie` to avoid emitting `R_*_RELATIVE`.
   - Put strings in `SHF_MERGE|SHF_STRINGS` sections. But merge sections are not GC'd, so do not put dead data in them.
2. **LTO is only via linker plugins (LLVM bitcode / GIMPLE)**. Cranelift has no matching IR, so wild is just a native linker.
   **Inlining, DCE, deduplication of monomorphizations and ICF can only be done whole-program in the compiler.**
   → zustc makes 1 program = 1 object and does DCE by reachability in the compiler.
3. **Do not go through the cc driver**:
   - Measured: cc + lld took 21ms; starting wild directly took 5ms.
   - With the `libwild` crate it can be called as a library, and the `FileSystem` trait lets objects be passed directly from memory. There is no need to write a `.o`.
4. **Incremental linking is wild's end goal, but it is not implemented yet**.
5. Implementation techniques:
   - mmap and borrow with `'data`, no copying.
   - Use dense `u32` IDs and Vec tables.
   - Compute name hashes once and reuse them.
   - Size the output first, then split it into disjoint `&mut` slices and write in parallel.
   - fork and move cleanup to the background.

## What zustc adopted (in priority order)

| Idea | Source | Status |
|---|---|---|
| Per-function machine code cache. Only callee signatures go into the key | blaze template / relax cache | Implemented |
| Item hash that ignores whitespace and comments (own lexer) | blaze item-hash | Implemented |
| Parse only changed items with syn (item summary cache) | relax demand / blaze early cutoff | Implemented |
| Reachability from main. Unreachable functions are not even parsed | blaze / relax | Implemented |
| One-file cache. Only entries used this time are written back, so GC is automatic | relax store | Implemented |
| Per-function sections, Local symbols, `-no-pie`, `--gc-sections`, strip | wild | Implemented |
| Cache the link line and start the linker (wild) directly | wild measurement | Implemented |
| Skip linking if the object is identical | - | Implemented |
| Thread parallelism only when there are many misses (cold). Each thread parses only its assigned items | relax compile_split | Implemented |
| α-normalized hash (renaming local variables does not invalidate) | blaze | Not implemented |
| ICF (hash of code + relocation targets) | blaze dedupe | Not implemented |
| Monomorphization keyed by layout | blaze polymorph | Not implemented (no generics yet) |
| Own inlining + slot forwarding (with -O) | relax | Not implemented |
| Embedding libwild, objects in memory | wild | Not implemented |
| Incremental linking (slack + jump table) | blaze patch-in-place | Not implemented |
