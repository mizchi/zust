# Fixing what the formal models found: the borrow checker giving up, Vec overflow and allocation failure, the cache's tmp file

Date: 2026-11-20

This fixes defects 1–4 of [report 2026-11-19-01](2026-11-19-01-formal-models.md). Each failure was reproduced by a test or a model before it was fixed. Each fix is one commit, with one row of KPIs (`bench/kpi_log.md`). Defect 5 (`std::thread::spawn` running the closure at once) is a design question and is left as it is.

## 1. Borrow checking: the fixpoint runs to its end

`Bk::check` returned no error (`None`) once the worklist had run more than `50 * blocks + 1000` rounds.

The limit is now a formula derived from the lattice's height:

- The state at a block's entry only grows by join, and it grows at most H times, the lattice's height.
- H is the sum of the numbers of: moved paths (from moves and uninitialized binds), immutable locals already assigned, holds entries and their loans, and dead and escaped loans.
- A block goes on the worklist only when the state at its entry grew.
- So there are at most `n * (H + 1) + 1` rounds.

Going past that cannot happen, but if it did, the program is rejected with an error instead of accepted (an assert in debug builds).

- The test is `fixpoint_does_not_give_up` in `borrowck_props.rs`. It builds a chain of 700 references directly as events. It failed before the fix.
- `formal/repro/borrowck_rounds.py` now finds E0506 at K = 260 and at K = 1000.

## 2. Vec capacity overflow

`Vec::reserve` changes as follows:

- It panics with `panic!("capacity overflow")` when `len + extra` does not fit in usize, or when the block would be larger than isize::MAX bytes (the same limit as std).
- The doubling saturates.
- This computation is in `__exr_vec_new_cap`, a function that is not generic, so it is not checked and compiled per element type (see the KPI section).

`Vec::<u64>::with_capacity(1 << 61)` followed by a push was a segmentation fault. It now ends with "capacity overflow" and exit code 101, as with rustc. `tests/alloc_limits.rs` tests these three cases, and a Vec that grows normally:

- `with_capacity(1 << 61)`
- `reserve(usize::MAX)` on a Vec with elements
- `reserve(usize::MAX / 4)`

## 3. Allocation failure

The prelude's 12 calls to `realloc` (Vec, sort buffers, HashMap, VecDeque and others) now go through `__exr_realloc_ok`:

- When realloc returns null, it prints a message ("memory allocation failed") and aborts, as std's `handle_alloc_error` does.
- `realloc(p, 0)` may return null; that is not a failure and is not treated as one.
- `Box`'s malloc is called directly by the code generator and is not changed here.

The Kani harnesses copy the fixed code and check it. The `bug_*` harnesses are replaced by three:

- `with_capacity_any_size`
- `reserve_any_size`
- `alloc_failure_stops`

Each is `#[kani::should_panic]`, and shows that any size, and a failing malloc, stop with a panic or an abort. Kani's pointer checks (out of bounds, a null dereference) do not count as panics, so a single path that writes out of bounds makes the harness fail.

- On the prelude before the fix, all three were checked to fail with "failures other than panics".
- What a Vec holds is still checked by the `*_matches_model` harnesses.

## 4. The tmp file of the cache and the stamps

`Cache2::save` wrote `<path>.tmp` and then renamed it, and every process used the same tmp name.

- The name is now `<path>.<pid>.tmp`. `cache2.rs` is also compiled by zustc itself, so the pid comes from libc's `getpid`.
- When the write or the rename fails, the tmp file is removed.
- `src/stamp.rs` saved the stamps the same way and gets the same change.

`formal/tla/CacheSave.cfg` checks the code as it is now (a name per process), and the whole state space passes (72 states). The old way is kept as `CacheSaveShared.cfg`, and `formal/run.sh` checks that it still gives the counterexample (8 steps). The test is `save_tests::save_then_open` in `cache2.rs`.

## What was checked

- `cargo test --release --workspace` passes.
  - Without rust-src, two of zust-analyzer's LSP tests fail, before the fix too. They were run with rust-src installed, as the README requires.
- `tests/compare.sh`: all 42 ok (with valgrind).
- The tests/ui baseline check: 0 regressions. `--check` fails on 2 crashes (a stack overflow on circular modules, `parser/circular_modules_main.rs` and one more), which zustc before the change has too.
- `formal/run.sh`: the 7 TLA+ configurations, the `props` tests and the 14 Kani harnesses all give their expected results.

The KPIs are the four rows of 2026-11-20 in `bench/kpi_log.md`:

- The first row is a reference: the code before the change (main + formal/), measured again on this machine. The row before it was measured on another machine, so the difference shows up there.
- The other three rows are the differences of fixes 1–3, one each.

| change | no-op build | edit one fn | check | traits check | traits edit | crates no-op | crates edit | tests/ui (run / check / build / compile-fail) |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| reference (0765679) | 0.9 M | 580.8 M | 3851.5 M | 1178.8 M | 61.0 M | 10.8 M | 316.2 M | 1698 / 1417 / 95 / 3327 |
| 1. borrow checking | +0.00% | -0.09% | -0.06% | +0.03% | -0.01% | +0.00% | -0.03% | 1698 / 1417 / 95 / 3326 |
| 4. cache | +0.00% | +0.02% | -0.00% | -0.03% | +0.00% | +0.00% | +0.02% | 1698 / 1417 / 95 / 3327 |
| 2 and 3. Vec | +0.23% | -0.06% | +0.09% | -0.26% | +0.23% | +0.13% | +0.60% | 1698 / 1417 / 95 / 3327 |

- Every change is below the 5% threshold. The cache size does not change.
- A program that uses Vec is about 540 bytes larger, for the two checks and their panic / abort paths. `macros.cache_bytes` in `bench/budget.txt` had only 0.8% left (66655 of 67160) and is now 67398. That entry alone is raised to 70768 by the rule `UPDATE_BUDGET=1` follows (the current value + 5%). The other entries are the sizes with CI's wild and are left as they are (all within their limits).
- compile-fail goes between 3327 and 3326 by one test from row to row. It goes back with the cache fix too, which has nothing to do with borrow checking, so the test is timing-dependent. The baseline check finds 0 regressions.
- The Vec fix first put its checks in `reserve` itself. `reserve` gets an instance per element type, and traits check went up 2.65%. With the overflow computation moved to a function that is not generic, `__exr_vec_new_cap`, it is -0.26%.

## What is left

- Defect 5: `std::thread::spawn` runs the closure at once. Real threads would need `Arc` changed as the atomic version of `ArcDrop.tla` does.
- Allocation failure in `Box`.
- A checksum for the cache. With a tmp file per process, a mixed file is no longer read, but a checksum is still needed to discard a file corrupted on disk.
