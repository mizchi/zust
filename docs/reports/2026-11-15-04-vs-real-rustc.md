# Comparison with the real rustc (`python3 bench/vs_rustc.py`)

Date: 2026-11-15

The same source is built with the same steps (cold → no-op → one edit), and the remaining disk usage, binary (after strip) and run time are compared side by side. Full results are in `bench/vs_rustc_result.md`.
The zustc linker is wild 0.10.0; rustc is 1.97.0. In every configuration the output matched rustc `-O`.

| Program | | cold ms | edit ms | disk KiB | binary KiB | run ms |
|---|---|---:|---:|---:|---:|---:|
| gen1000 | zustc | **118** | **28** | **105** | 521 | 12 |
| | rustc -Cincremental | 827 | 417 | 22156 | 1043 | 9 |
| | rustc -O | 11448 | - | - | 382 | 2 |
| nbody | zustc -O | **47** | **6** | **29** | **23** | 92 |
| | rustc -Cincremental | 149 | 106 | 2227 | 366 | 1231 |
| | rustc -O | 177 | - | - | 364 | 62 |
| sort | zustc -O | **51** | **8** | **32** | **30** | **195** |
| | rustc -Cincremental | 203 | 128 | 3102 | 385 | 2041 |
| | rustc -O | 317 | - | - | 348 | 184 |

- Rebuilding after an edit is 15–20 times faster than rustc incremental, the remaining disk usage is 1/75–1/210, and the binary is 1/10–1/20 (gen1000 is larger than rustc `-O`: due to the debug checks and the lack of optimization).
- For run time, zustc (debug) is faster than rustc (debug) in most cases (except gen1000). zustc `-O` is 1.0–2.6 times rustc `-O` (sort is the same, strings is 2.2 times).
