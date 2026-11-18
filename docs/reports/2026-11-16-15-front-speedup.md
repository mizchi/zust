# Speeding up the first half (front) of the self-hosted compiler

Date: 2026-11-16

Time spent in the first half when the zustc-built compiler (`tests/selfhost/build_compiler.py`) compiles itself (about 33 thousand lines)
(per-stage times from `ZUSTC_PROF=1 ... --time`. We counted where gdb stopped and fixed the most frequent places first):

| Stage | Initial | After | rustc-built compiler (after) |
|---|---:|---:|---:|
| macros | 9870 ms | 499 ms | 96 ms |
| flatten (flatten modules) | 2501 | 552 | 51 |
| hoist + ctor | 613 | 467 | 61 |
| split | 578 | 287 | 18 |
| prelude selection | 1219 | 74 | 16 |
| item summary | about 2400 | 1054 | 110 |
| program + refs | 862 | 645 | 72 |
| **front total** | **18159** | **3745** | **439** |

- macros: the lexer of our own proc_macro2 (mini_pm2) counted columns from the start of the line for every token (it now counts on from the previous one).
  `find_calls` / definition collection copied the contents of every bracket (brackets that contain no macro name are no longer walked).
  If the expansion contains no macro name, there is no further round. In addition, the whole source is scanned with the fast `lexer::tokenize`, and proc_macro2 only reads
  the inside of definitions and call brackets (`tests/macros_expand.rs` checks that the result is the same as the previous version, `expand_reference`)
- flatten: `iter_mut().take(n).skip(m)` used to fill a table walked from the start every time (replaced with an index loop)
- prelude selection: calls `contains` on the whole source about 40 times. The prelude's `str::find` now uses libc's `memmem`
  (this speeds up every program built with zustc)
- split: the item hash (the cache key) used two SipHashes. It now uses a mix of two multiply-and-rotate lanes (`src/key.rs`).
  It does not depend on how calls are split (it buffers 8 bytes at a time), so it gives the same value even when the Hash implementations in std and zustc's prelude split their write calls differently
- item summary: items are independent, so the work is split with fork (the same as function compilation in driver2). The parser's `punct_at` (called
  many times per operator precedence level) compares bytes, and does not search for operators at positions that are not symbols. The lexer looks at ASCII as raw bytes
- refs: before looking up the 4 tables, look up once in the set of names present in any of the tables
- The rest is the quality of the generated code (no inlining of small functions; `Iterator` adapters become function calls). The second half, codegen, takes 15.8 seconds
  (0.95 seconds for the rustc version), for the same reason
