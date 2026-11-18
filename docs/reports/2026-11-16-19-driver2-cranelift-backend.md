# A Cranelift backend for driver2 (compared with minclif)

Date: 2026-11-16

minclif's FunctionBuilder records every instruction acg calls as an `Op`. We added a backend that rebuilds that sequence as Cranelift IR and
compiles it (`src/clif_backend.rs`, `ZUSTC_BACKEND=cranelift`). acg, inlining, the cache and .o
writing are shared, so only the backend is compared, with the same frontend and the same instruction sequence.

- Values map minclif value numbers → Cranelift values. Blocks, variables, slots and functions are also mapped by number (recording now also keeps
  each instruction's return value). acg sometimes uses a value in a block that the value does not dominate (no problem for minclif, which keeps values in slots), so
  values used in a segment other than their definition's become Cranelift Variables, and cranelift_frontend builds the SSA
- Drop-flag slots are created midway, so after building, 0 is written at the start of the entry block
- Relocations are returned by name (calls are PLT32, symbols are GOTPCREL). Trap codes are also the same as minclif's
- `tests/driver2_cranelift.rs`: builds probe / examples / acg without -O and with -O, and checks that output, stderr and exit code are the same as minclif (all match).
  The self-hosted compiler built with Cranelift also emits the same .o as the one built with minclif

Run times (`python3 bench/runtime.py`, ms):

| | driver2 -O (minclif) | driver2 + Cranelift | driver2 + Cranelift -O | previous Cranelift path (`zustc -O`) | rustc -O |
|---|---:|---:|---:|---:|---:|
| fib | 110 | 91 | 79 | 79 | 31 |
| hashmap | 214 | 155 | 144 | 148 | 112 |
| matmul | 168 | 105 | 76 | 85 | 40 |
| nbody | 522 | 232 | 86 | 105 | 54 |
| sieve | 158 | 150 | 154 | 134 | 94 |
| sort | 334 | 294 | 233 | 248 | 208 |
| strings | 111 | 91 | 68 | 73 | 42 |

Compared on the self-hosted compiler (about 33 thousand lines):

| Backend | Compiling it (-j 1, including acg) | Resulting executable | Time for the resulting compiler to compile itself |
|---|---:|---:|---:|
| minclif | 2.40 seconds | 8.8 MB | 11.5 seconds |
| minclif -O | 3.15 seconds | 10.6 MB | 10.1 seconds |
| Cranelift (opt none) | 5.07 seconds | 5.4 MB | 6.9 seconds |
| Cranelift -O (speed, backtracking) | 8.26 seconds | 6.6 MB | **6.0 seconds** |
| Cranelift -O (speed, single_pass) | 6.95 seconds | 9.2 MB | 8.9 seconds |

(The compiler built with rustc -O takes 1.6 seconds.) The workspace benchmark (`bench/ws_bench.py`) with minclif / Cranelift: cold 440 / 727 ms,
no changes 7 / 6 ms, function body change 42 / 36 ms, bin change 74 / 67 ms (with the per-function cache, compilation after a change is small).

What we learned: minclif compiles about 2 times faster than Cranelift (opt none), and about 2.6 times faster when both use -O, but the code it emits is 1.0 to 6 times slower
(1.7 times for the self-hosted compiler). Incremental builds compile only the changed functions, so the difference in compile speed
only matters for cold builds.

At this point, the modules that do not depend on external crates (lexer / enc / fxhash / prelude / link / testgen / program / ast_consteval / solver / ctors / ast_infer / minclif / ast_codegen), and
those with small dependencies for which we could write replacements (elfz: zstd, macros: proc_macro2), can be compiled with zustc and give the same results as rustc.
The rest, front (lookups that walk the syn syntax tree) / infer (the syn version; ast_infer, the version for our own syntax tree, exists) / consteval (constant promotion in codegen) / closure / codegen / driver / cache, is written on top of syn and cranelift.
The replacement for syn will be moved over starting from front, built on src/parse.rs (our own syntax tree). The replacement for cranelift (our own code generation) comes after that.
