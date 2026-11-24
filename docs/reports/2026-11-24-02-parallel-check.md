# `check` in forked children (#101)

Date: 2026-11-24

`zustc check` ran in one process: item summaries and the check loop, while a build already forked for both. On 4
cores, a cold `check` of gen5000 took about as long as a cold build at `-j 4`, although it generates no code. This
work splits each wave of the check across forked children, as the build does
([#101](https://github.com/mizchi/zust/issues/101)).

## What changed

- `zustc check` takes `-j N` and uses the number of CPUs by default. Before, it always used one job, so even the item
  summaries were not split.
- The check loop (`Session2::check`, `crates/core/src/driver2.rs`) now works on a wave in three steps:
  1. In order, the memo and the cache are looked up. The misses are collected.
  2. `check_many` checks the misses: in this process, or, when there are at least 32 per job
     (`ZUSTC_CHECK_FORK_MIN`), in forked children. Child k checks the misses with `i % jobs == k` and sends back each
     one's Blob, encoded by name as the cache keeps it, or its error.
  3. In the order of the wave, the results are taken: the cache is written, errors are kept, and the record (the
     check memo) is made.
  Reading a child's Blob back requests again the instances it requested, so the next wave is as with one process,
  and the diagnostics do not depend on `-j`.
- zust-analyzer still checks with one job (its sessions are `jobs: 1`). An edit-time check of one function never
  reaches the threshold either way.

## What a child has to send, besides its result

Comparing `-j 1` with `ZUSTC_CHECK_FORK_MIN=1 -j 4` (a fork for nearly every wave) over tests/ui found three things
the first version lost:

1. **Instances that lowering makes but does not return.** Some instances that lowering requests go to `pending` but
   not into the `req` it returns. In one process, the next wave checks them; from a child, the parent never heard
   of them. The child now sends the instances made while checking the fn (the growth of its `pending`, in order), then
   the rest of `req`.
2. **What a failed fn left behind.** A fn whose check fails has still requested instances (a closure's body) and told
   its closures about its regions (`clo_facts`). The child sent only the message. It now sends a Blob of those
   instances and facts with the error, and the parent reads it back.
3. **Instances whose type arguments the cache's encoding does not take.** The child panicked in `types_enc`
   (`pattern/slice-patterns-irrefutable.rs`). Such a fn is now marked as not sent, and the parent checks it itself.

## Measured

4 cores, main d2146f7 against this branch.

Wall time, median of 5 runs:

| | main `-j 1` | main `-j 4` | #101 `-j 1` | #101 `-j 4` |
|---|---:|---:|---:|---:|
| `check` gen5000 | 529 ms | 516 ms | 536 ms | **250 ms** |
| `check` patterns (`gen_patterns.py mix 8`) | 279 ms | 261 ms | 276 ms | **181 ms** |

Before this change, `-j` made no difference to `check`, because it was ignored.

`ZUSTC_PROF=1` on gen5000 at `-j 4`: summaries 89 → 53 ms, check 298 → 122 ms.

Instructions (cachegrind, `-j 1`, which the KPI scripts now pass):

| | main | #101 | change |
|---|---:|---:|---:|
| gen5000 check | 3769.85 M | 3772.64 M | +0.07% |
| patterns check | 1335.27 M | 1336.54 M | +0.09% |

The increase is the wave's three steps (the misses and the record's entries are kept in vectors); no fork happens at
`-j 1`.

Peak RSS of the `zustc` process on gen5000: 137 MiB at `-j 1`, 49 MiB at `-j 4`. At `-j 4` the function bodies' syntax
trees are made in the children, not the parent. The children's own peaks were not measured.

## Same output at any `-j`

- tests/ui, 6000 files with a fixed seed, `-j 1` against `ZUSTC_CHECK_FORK_MIN=1 -j 4`: 5996 the same and 3 timeouts.
  The one that differs is `layout/uninitialized-gat-projection-cycle-issue-153205.rs`, which overflows its stack on
  main too: forked, the child overflows, the parent checks the fn again and overflows too, so the message is printed
  twice.
- The same 6000 files, this branch's `-j 1` against main: the same apart from the backtraces of two tests that panic
  on main too (`codegen/normalization-overflow/*`), which have the new `check_many` frame. The thread ids in panic
  messages were masked, since they differ on every run.
- gen5000, `gen_traits.py 2000` and `gen_patterns.py mix 8`: byte-identical at `-j 1`, `-j 4`, and with
  `ZUSTC_CHECK_FORK_MIN=1`.
- `tests/check_parallel.rs`: 300 fns, plus the cases above (a failing fn that requested a closure, a failed lowering
  with region facts for its closure, an error in a closure, a placeholder instance). It checks them at `jobs: 1` and
  `jobs: 4`, without and with the cache. Without the fix for (2) it fails. It is one test in its own binary, so that
  no other test thread holds a lock at the fork.
- `cargo test --release --workspace`: all pass except `zust-analyzer`'s `tests/lsp.rs` (`hover_and_completion`,
  `type_inference_like_rust_analyzer`), which fail on main too, in this environment.

## KPI scripts

valgrind does not count forked children, so the scripts that count instructions run `check -j 1`:
`bench/kpi_track.py`, `bench/ci_bench.py`, `bench/patterns.py`. `bench/rust_ui.py` also passes `-j 1`, since it runs
many checks at once. The wall-time `check` rows (`ci_bench.py`, and so the README's numbers) now measure the parallel
check, so they drop on CI's runners.

## Left

- Self-hosting could not be checked: `tests/selfhost/build_compiler.py`'s source stops on main already
  (`file not found for module dead`, and rustc does not build it either, as TODO.md says).
- The smaller steps before the check (`prep`, `split`, the prelude, `program`: about 40 ms of gen5000) are still in
  one process.
- A child that crashes on a fn makes the parent check the rest of that child's share itself, so a crash shows twice.
