# A front end on threads: rustc's parallel front end, what zustc has, a prototype, and a design

Date: 2026-11-25

rustc's nightly has run its front end on threads by default since `nightly-2026-11-14`
([blog](https://blog.rust-lang.org/2026/11/16/enabling-parallel-frontend-on-nightly/)): 2 threads by default,
`-Zunstable-options --jobs-frontend=N` for more. The blog gives the mean wall-time change of `check` builds as −22% /
−38% / −44% at 2 / 4 / 8 threads. This report asks what zustc's front end should be designed around now. It covers:

1. how rustc does it, read from its source;
2. what zustc already does;
3. a prototype that runs zustc's parallel parts on threads, and what it measured;
4. the design that follows, and the order of the work.

## 1. rustc's parallel front end (from the source)

Read from a clone of rust-lang/rust at 8d773821 (2026-11-25). The paths are under `compiler/`.

- **Thread pool.** `rustc_thread_pool` is rustc's fork of rayon-core. It adds four things:
  - a deadlock handler, called when no worker is active and some are blocked;
  - `mark_blocked_and_wait`, for a thread that waits on another thread's query;
  - jobserver hooks, so a blocked or sleeping worker gives its token back;
  - `tlv`, one thread-local pointer each job carries. rustc's `ImplicitCtxt` (the `TyCtxt` and the current query)
    travels with stolen work this way.

  The pool is started in `rustc_interface/src/util.rs` (`run_in_thread_pool_with_globals`), with 17 MiB stacks. The
  default of 2 threads is not rustc's: bootstrap sets `CFG_DEFAULT_FRONTEND_JOBS=2` for nightly and dev builds only.
  Stable stays sequential.
- **One binary, mode chosen at run time.** `cfg(parallel_compiler)` and `Lrc` are gone. `set_dyn_thread_safe_mode`
  is called before any lock exists. With 1 thread there is no pool:
  - `Lock` is a `Cell<bool>` that panics on re-entry;
  - `Sharded` has a single shard;
  - a query that meets itself is a cycle at once.

  With N threads, `Lock` is a parking_lot mutex and `Sharded` has 32 cache-aligned shards, picked from FxHash bits.
- **What runs in parallel.** It is all `par_for_each_in` over body owners or modules:
  - well-formedness checks;
  - typeck and const evaluation;
  - `mir_borrowck` (MIR is built on demand inside it);
  - lints, privacy and dead-code checks;
  - mono-item collection (sorted afterwards);
  - the first batch of codegen units.

  Parsing, macro expansion, name resolution and AST lowering are still sequential. PR #163495, which lowers the AST
  in parallel, failed the merge queue because crate hashes differed between runs.
- **Queries under threads.** A query another thread is computing is waited on through a latch (a condvar per
  waiter). A cycle across threads is found only once the whole pool is blocked. The deadlock handler then collects
  every active job, searches the wait graph and breaks one cycle per round. A query that panicked poisons its key,
  and its waiters raise a fatal error. Caches are sharded hash maps, or the lock-free `VecCache`. Interners are
  sharded sets whose values are allocated in a per-thread arena (`WorkerLocal<Arena>`).
- **Determinism.**
  - Diagnostic order depends on scheduling, a known issue. The parallel UI-test CI job compares output with its
    lines sorted.
  - Output stays reproducible by remapping what was allocated in parallel. `commit_end_of_determinism` marks the
    point after which DefIndex order varies; metadata renumbers later DefIds by DefPathHash; expansion ids are
    remapped by stable hash. The blog still lists non-reproducible binaries as a known bug.
- **Memory.** The 2023 post said "up to 35%" more. The 2026 post gives no figure, only a warning about running out
  of memory. Scaling is weak above about 8 threads.

The lesson for zustc: most of rustc's machinery serves a demand-driven query graph that threads walk concurrently.
That means latches, the deadlock handler, poisoning, and the remapping that keeps output deterministic. zustc's
check and build are not shaped that way (section 4.1).

## 2. What zustc has now: forked children with an ordered merge

zustc already runs three parts of the work in parallel. All three use `fork()`:

| part | where | unit | when |
|---|---|---|---|
| item summaries | `Session2::analyze` (driver2.rs) | an item | cold: at least 32 misses per child |
| `check` | `check_many` (driver2.rs) | a function of a wave | at least 32 per child (`ZUSTC_CHECK_FORK_MIN`) |
| machine code | `compile_many` (driver2/build.rs) | a function of a wave | at least 16 per child |

The workspace build also forks a child per crate.

Each child works on a copy-on-write snapshot of the parent, takes every `jobs`-th item, and writes its results
through a pipe. Functions are written by name (`encode_blob`), so the child's own instance numbers do not escape. The
parent reads the results back in the order of the wave. Reading a function back requests again the instances it
requested, so the next wave and the diagnostics are what one process would produce. tests/check_parallel.rs checks
this. A child that panics or cannot send something (`sendable`) leaves that item to the parent.

Measured on this machine (4 cores, shared, so the times vary), `zustc check --no-cache` of gen5000 takes 594 ms at
`-j 1` and 261 ms forked at `-j 4`. That is 2.3x: zustc already gets in processes most of what rustc gets from
threads.

The limits of the fork model:

- **No parallelism in a process with threads.** `single_threaded()` refuses to fork when there is more than one
  thread: a lock held by another thread would stay held in the child. zust-analyzer spawns a thread, so its
  diagnostics always run `check` sequentially.
- **Static striping** (`i % jobs == k`). A slow item holds up its own stripe.
- **Cost per region**: a fork, a pipe, and an encode and decode of everything sent. This is why the thresholds are 16
  to 32 items per child.
- **Only what the encoding can write** comes back. Anything else is redone by the parent.
- **Unix only.**

The sequential part is large. At `-j 4` on gen5000, prep (17 ms), split (14 ms), prelude (14 ms), program (4 ms) and
refs (3 ms) come to about 52 ms of about 260 ms.

## 3. A prototype: the same parallel parts on threads

### 3.1 What was changed

- **`zust-base::par`.** `map_with(jobs, n, chunk, init, f)` runs `f(state, i)` over `0..n` on scoped threads, and
  the calling thread is one of them.
  - Each thread makes its state with `init`.
  - Indices are taken `chunk` at a time from an atomic counter: dynamic, not striped.
  - Results come back in index order.
  - An index whose `f` panics gives `None`, and the caller redoes it, as with a dead child.
  - With `jobs` 1 it is a plain loop on the calling thread.
  - Stacks are 8 MiB.

  The mode comes from `ZUSTC_PAR=fork|threads|off` (default `fork`), or per session from `Options2::par` for a
  host that runs the driver in process.
- **Item summaries on threads.** The summaries come back as values, not encoded.
- **`check_many` on threads.**
  - The child's per-function work moved into `send_checked`, which the forked children and the threads now share.
  - Each worker clones `Insts` (the instance table) once, at the start of the wave. That is what a forked child has.
  - The results are encoded by name and read back in the order of the wave, through the same code as the children's.
  - `Insts` and `Inst` derive `Clone` for this.
- **`WorkerTls`.** `ast_closure::WILD_FAKE_READS` (the edition rule for `let _ = place` in closures) is the only
  thread-local the driver sets for the program. It is taken on the calling thread and set on each worker. This plays
  the part of rustc's `tlv`. The other thread-locals belong to a single computation (a depth, a memo) and start empty
  on a new thread.
- **The AST cache** (`Analysis::asts`) had two problems.
  - `compile_id_req` held its lock while it lowered the function, which would have run the workers one at a time. The
    trees are now `Arc`s, and the lock is released before lowering.
  - It kept every function's tree until the `Analysis` was dropped. Dropping them took **83 ms** of a
    `zustc check -j 1` of gen5000, and 110 ms with threads (memory freed on other threads), against 2 ms with fork,
    whose children's trees die with them. Only instances' trees are kept now: a generic function's body is lowered
    once per instance, and a plain function's body only once. The drop is 1–2 ms in every mode. This speeds up the
    sequential check as well.
- **Tests.** tests/check_threads.rs runs `ZUSTC_PAR=threads` at `-j 4` and `-j 3`, with every wave and summary
  pass split (`ZUSTC_CHECK_FORK_MIN=1`, `ZUSTC_SUMMARY_PAR_MIN=1`), and compares with `-j 1`. A second test checks
  that a host with threads of its own starts workers, does not fork, and gets the `-j 1` answer. The program both
  test files check moved to tests/check_parallel_program.rs.

### 3.2 Measurements

`zustc check --no-cache` of the generated programs (`bench/gen.py`) on 4 cores. The times are the best and the
median of 7 runs. The memory is the peak of the summed Pss of the process and its forked children, sampled every
millisecond. Pss is used because the RSS of one process does not see a child's memory. In every row the
diagnostics are the same as at `-j 1`.

| program | mode | -j | best | median | peak Pss |
|---|---|---:|---:|---:|---:|
| gen5000 | sequential | 1 | 593.8 ms | 665.6 ms | 35.7 MiB |
| gen5000 | fork | 2 | 356.3 ms | 443.2 ms | 36.1 MiB |
| gen5000 | threads | 2 | 338.8 ms | 361.3 ms | 44.5 MiB |
| gen5000 | fork | 4 | 260.6 ms | 294.7 ms | 36.7 MiB |
| gen5000 | threads | 4 | 253.6 ms | 284.7 ms | 44.3 MiB |
| gen20000 | sequential | 1 | 2176.1 ms | 2354.8 ms | 101.6 MiB |
| gen20000 | fork | 2 | 1513.1 ms | 1567.1 ms | 109.3 MiB |
| gen20000 | threads | 2 | 1358.9 ms | 1457.0 ms | 140.7 MiB |
| gen20000 | fork | 4 | 959.1 ms | 1003.8 ms | 116.5 MiB |
| gen20000 | threads | 4 | 933.8 ms | 1103.4 ms | 137.7 MiB |

Per phase (`ZUSTC_PROF=1`, gen5000, `-j 4`, two runs) the two modes are within the run-to-run noise: summaries took
44–62 ms forked and 47–63 ms on threads, check 171–183 ms and 167–177 ms.

The diagnostics were also compared over 513 programs (tests/compile_errors, tests/probe, examples, tests/acg,
tests/security). Each was checked at `-j 1`, then on threads at `-j 4` twice, then forked at `-j 4`, with every wave
and summary pass split. There were no differences.
`cargo test --release --workspace` passes, except two tests of crates/analyzer/tests/lsp.rs (hover and type
inference). Those two fail the same way at HEAD in this environment.

What the numbers say:

1. **At 4 threads, threads run as fast as fork.** They are within a few percent either way, and lock contention does
   not show yet: the four global interner mutexes, the `AdtTable` mutexes and the AST cache. So threads do not make
   the command line faster on their own.
2. **Threads take 18–29% more memory**: +8 MiB on gen5000 and +21 to +31 MiB on gen20000. It is not the number of
   malloc arenas. With `MALLOC_ARENA_MAX=2`, peak Pss is the same (134 MiB) and the check takes 1.8–3.7 s instead of
   about 1.0 s, because the threads queue on malloc. Probable causes, not confirmed:
   - each worker's copy of `Insts`;
   - each worker's arena keeping the high-water mark of what it freed. A forked child gives that back when it exits.
3. **What threads do buy is elsewhere:**
   - parallelism in processes with threads, the LSP above all;
   - a lower cost per parallel region, which allows lower thresholds;
   - dynamic scheduling;
   - no encoding needed between workers (not used yet);
   - platforms without `fork`.

## 4. Design

### 4.1 Keep the waves; do not build a query system

rustc's threads walk a demand-driven query graph. A thread that needs a result another thread is computing must
wait for it. That is the source of the latches, the deadlock handler that finds cross-thread cycles, poisoned keys,
scheduling-dependent diagnostics, and the remapping of ids allocated in parallel.

zustc's check and build already run in **waves**, which are bulk-synchronous:

- a wave is a set of independent functions;
- a barrier ends it;
- the results are merged in a fixed order, and the merge produces the next wave.

The design keeps that shape on threads:

- **Deterministic by construction.** The merge runs in the wave's order and the diagnostics are sorted, so `-j` and
  scheduling do not change what is printed. rustc's parallel UI tests have to sort lines to compare; zustc's
  tests/check_parallel.rs compares exactly.
- **Tasks never wait for each other.** So there are no latches, no deadlock handler and no cycles across threads.
- **A task's result depends only on the snapshot at the wave's start** and on what its worker did earlier in the wave,
  as with a forked child.

The costs are the idle time at a barrier and the sequential merge. Chunked dynamic scheduling reduces the first; the
second should be measured (a `ZUSTC_PROF` line for the merge).

### 4.2 Four kinds of state

| kind | examples | rule |
|---|---|---|
| read-only during the parallel part | `Program`'s tables, `items`, `src`, `refs` | shared by `&`, no locks (rustc's `FreezeLock`: built, then frozen) |
| append-only, keyed by value | the interners (`Ty`, `FnTy`, strings, associated types), `AdtTable`'s instances, `normalized` / `inst_keys`, the AST caches | shared behind locks. An id allocated here must not reach the output or the merge order |
| a task's working state | `Insts`: `pending`, `fx_log`, `closure_rg`, `kind_free` | a copy per worker, merged in order by name |
| thread-locals the driver sets | `WILD_FAKE_READS` | taken from the calling thread and set on each worker (`WorkerTls`) |

`Insts` has to be per worker. `send_checked` finds what a function made by offset (`pending[p0..]`,
`fx_log[l0..]`), and that only works when one thread writes the table.

The id rule is rustc's lesson from #163495 and its DefId remapping. zustc's encoding by name already does that
remapping: an `AdtTable` struct instance created on a worker gets a different number in different runs, but the
cache and the diagnostics see only its name. When the build is moved to threads, the `.o` must be checked to be
identical across `-j` and across runs, as the self-host fixpoint already checks.

### 4.3 One binary, mode chosen at run time

As in rustc, one binary does all three modes, and with `jobs` 1 it runs a plain loop. zustc already uses `std` mutexes
everywhere: a compile-time probe while writing this confirmed that `Analysis`, `Program` and `Session2` are `Sync`
and `Insts` is `Send`. An
uncontended mutex is cheap, so rustc's `DynSync` / `Lock` mode machinery is not needed.

The default should follow the host:

- **in a process with threads:** threads. This covers zust-analyzer, which should set `Options2::par`, and with
  2 jobs, rustc's choice for memory and for the editor's own threads;
- **in a single-threaded process:** fork, until threads match it on memory.

### 4.4 Distributing the work

- Dynamic chunks: 4 items for summaries, 2 for check.
- The thresholds were inherited from fork (32 per worker). On threads, a parallel region costs a few thread spawns.
  The thresholds should be measured again, and lowered if that pays. That would let the check of a mid-sized edit run
  in parallel.
- A persistent pool, parked between waves, saves the spawns. But a process with a pool has threads, so it cannot fork
  afterwards. Each process would have to be all-threads or all-fork. Scoped threads per region keep both possible
  until that is decided.

### 4.5 Failures

- **Panics.** A panicking item is caught and redone on the calling thread. Unlike a forked child, a thread that
  panics while holding a shared mutex poisons that mutex for the whole process (the `AdtTable` and the caches). The
  options are:
  - lock the shared caches with `lock().unwrap_or_else(|e| e.into_inner())`, since they hold no invariant across a
    panic;
  - or finish the wave sequentially after a panic.

  rustc's `ParallelGuard` finishes every task of the section and then re-raises the panic.
- **Stacks.** Workers get 8 MiB. rustc uses 17 MiB, and windows-gnu overflowed more often after the parallel default
  (#166530).

### 4.6 Memory

- The KPI "cold build peak RSS" measures one process. A forked child's memory is not in it, but a worker thread's is.
  Before the default changes, the KPI should measure Pss over the whole process tree, as section 3.2 did. Otherwise
  the change looks worse than it is.
- To recover the 18–29%:
  - an overlay `Insts`: a snapshot shared read-only by all workers, plus a small delta per worker, instead of a full
    copy each;
  - `malloc_trim` after a parallel region;
  - an arena per worker for a function's temporary data, like rustc's `WorkerLocal<Arena>`.

### 4.7 Interners and caches

The interners are four global `Mutex<FxSet>`. Contention did not show at 4 threads. rustc's answer is 32 shards
selected by hash bits (`Sharded`, `SHARD_BITS = 5`, tuned on 8 cores) with arenas per thread. The plan is to measure
first: count lock waits on 8 or more cores (CI's runners have 4). Shard the interners (16–32 shards, chosen by FxHash
bits) only if they show up.

### 4.8 What stays sequential (Amdahl)

On gen5000 at `-j 4`, about 52 ms of 260 ms is sequential: prep, split, prelude, program, refs. At 8 threads the
parallel part would be about 100 ms, so the sequential part would be about a third. As in rustc, where expansion,
resolution and lowering are still sequential, this part is the hardest:

- `macro_rules!` expands in order;
- `mod x;` files are read as they are found.

The candidates are lexing and splitting per file in parallel, and the prelude's selection. Something else comes
first: on the self-host compiler's sources (95k lines in one file), prep alone took 1.4 s. That should be profiled
before any parallel work; it may be a hot spot that has nothing to do with threads.

### 4.9 Workspaces and the jobserver

A workspace build forks a child per crate, and each crate splits its functions again, so `-j` is applied at two
levels. `zurgo test` also runs tests in parallel. With threads, one token budget for the whole process prevents
oversubscription. rustc's jobserver `Proxy` has two rules worth copying: never give back the process's last token,
and hand a freed token straight to a waiting thread.

### 4.10 Self-hosting

zustc's own threads are green threads (one OS thread) with no `thread::scope`. `par.rs` is therefore outside the
self-host set: the self-host build would put a sequential `map_with` in its place.

Separately, `tests/selfhost/build_compiler.py` does not inline `driver2/dead.rs` (added in d9e104a), so at HEAD
building the self-host compiler fails with "file not found for module `dead`". That is unrelated to this change.

## 5. Order of the work

1. Threads by default in processes with threads, and in zust-analyzer with 2 jobs. Measure its diagnostics on
   zustc's own sources.
2. `compile_many` on threads. Check that the `.o` is byte-identical across `-j` and runs.
3. Memory: the overlay `Insts` and `malloc_trim`; measure again with Pss over the process tree.
4. Thresholds for threads. Measure a check and a build after editing one function.
5. Measure on 8 or more cores. Shard the interners if they show contention.
6. Profile prep (1.4 s on the self-host sources), then lex and split per file in parallel.
7. Then decide the default for the command line (fork or threads).

Not planned: a query system with latches.

## Not measured

- More than 4 threads: this machine has 4 cores.
- The build on threads (`compile_many` was not changed).
- zust-analyzer's latency on threads.
- A generic-heavy program. Its `Insts` copies are larger, which is where the overlay matters.
