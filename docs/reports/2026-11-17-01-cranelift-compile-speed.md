# Faster compilation of functions on the Cranelift path (2026-11-17)

Date: 2026-11-17

Time (ms) to compile the compiler itself (out/selfhost/compiler_main.rs, 38886 lines, 9178 functions) with `--no-cache`.
The output .o is the same at every stage.

| Change | -j 1 | -j 4 |
|---|---:|---:|
| Before | 4650 | 2040 |
| Turn off verification of the Cranelift IR (enable it with `ZUSTC_CLIF_VERIFY=1`) | 4140 | |
| Read the syntax tree of a function body only once per body | 3950 | |
| Use FxHash for the number tables; reuse `Context` / `FunctionBuilderContext` | 3810 | |
| Do not re-read the enclosing function each time a closure body is looked up (remember the closure expressions per enclosing function) | about −450 | |
| Change the expression shape hash (`expr_hash`) from a Debug string to a hash that walks the tree (subexpressions are remembered by address) | 3740 | |
| In the cache key, remember the hash of the referenced signatures per function | 3690 | |
| Do not build a String per character when decoding string literals (the 286KB prelude embedded in the compiler) | 3640 | 1740 |

(Measurement noise is about ±200 ms. The first 3 rows are from the previous commit.)

- The expression shape hash built a Debug string for every subexpression recorded during inference, so its cost grew with depth
  (9% of the total). Now it walks the tree once and mixes in the subexpression hashes (the same Debug string gives the same value).
- Of the rest, Cranelift itself is about half (regalloc2 22%, lower 6%, emit 4%), acg code generation is 20%, and type inference is 5%.
- Switching to the `single_pass` regalloc (`ZUSTC_CLIF_FAST_RA=1`) goes from 3640 → 3050 ms, but the .o becomes 43% larger and
  the resulting compiler runs about 40% slower (75 → 104 ms on lexer_main.rs). The default stays backtracking.
