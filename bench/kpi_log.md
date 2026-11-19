# KPI log (one row per change)

Written by `python3 bench/kpi_track.py` after each change (deterministic counts only; see the script). Δ is against the row before, Σ against the first row.

| date | commit | change | no-op build (M instr) | edit one fn (M instr) | check (M instr) | cache (KiB) | LSP session (G instr) | tests/ui run / check / build / compile-fail |
|---|---|---|---:|---:|---:|---:|---:|---|
| 2026-11-19 | 45579fc | reference: before per-change tracking | 625.3 | 626.9 | 3905.2 | 1902.8 | 11.243 | 1536 / 1271 / 89 / 2844 |
| 2026-11-19 | 927540d | pointee as the expected type of `&x` coerced to a raw pointer | 625.3 (Δ +0.00%, Σ +0.00%) | 626.9 (Δ +0.00%, Σ +0.00%) | 3905.4 (Δ +0.00%, Σ +0.00%) | 1902.8 (Δ +0.00%, Σ +0.00%) | 11.246 (Δ +0.02%, Σ +0.02%) | 1538 / 1271 / 89 / 2845 |
| 2026-11-19 | b0edbd7 | `_` in an annotation is a new inference variable (with a Debug-printing check) | 625.1 (Δ -0.03%, Σ -0.03%) | 627.0 (Δ +0.01%, Σ +0.01%) | 3963.5 (Δ +1.49%, Σ +1.49%) | 1902.8 (Δ +0.00%, Σ +0.00%) | 11.249 (Δ +0.02%, Σ +0.05%) | 1545 / 1273 / 89 / 2843 |
| 2026-11-19 | 042f17d | methods of numbers in a Box, blanket-impl methods of numbers | 625.1 (Δ +0.00%, Σ -0.03%) | 627.0 (Δ +0.00%, Σ +0.01%) | 3963.6 (Δ +0.00%, Σ +1.49%) | 1902.8 (Δ +0.00%, Σ +0.00%) | 11.262 (Δ +0.12%, Σ +0.17%) | 1545 / 1273 / 89 / 2850 |
| 2026-11-19 | 89d9fe9 | drop the Debug-printing check on annotated lets | 625.1 (Δ -0.00%, Σ -0.03%) | 626.7 (Δ -0.04%, Σ -0.03%) | 3905.1 (Δ -1.48%, Σ -0.00%) | 1902.8 (Δ +0.00%, Σ +0.00%) | 11.023 (Δ -2.12%, Σ -1.96%) | 1545 / 1273 / 89 / 2850 |
