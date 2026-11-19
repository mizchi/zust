# Formal models and bounded proofs: borrow checking, the thread model, the allocator

Date: 2026-11-19

This report formalizes three important parts of zustc: borrow checking, concurrency, and the memory management of `Vec`. It uses two tools:

- [idealis](https://github.com/mizchi/idealis): finite TLA+ model checking in Rust, through rtla → rtla-compiler → rtlc.
- [Kani](https://github.com/model-checking/kani): bounded model checking of Rust code with CBMC.

The models and harnesses are in [formal/](../../formal/README.md), and `formal/run.sh` runs them again.

The work found five defects:

1. The borrow checker, when it stops its fixpoint at a round limit, accepts the program as error-free.
2. The capacity computation of `Vec::with_capacity` overflows, so safe code corrupts the heap.
3. A failed `realloc` (null) is not checked.
4. The cache's temporary file name is shared by every process, so a mixed file can be accepted.
5. `std::thread::spawn` runs the closure at once, which differs from rustc's semantics.

zustc's code is not changed here. Each section gives a proposed fix.

## Prior work

| area | prior work | how it is used here |
|---|---|---|
| borrow checking | [Wu, Wang, Chen: Verified Implementation of the Next-gen Borrow Checking Algorithm for Rust via Abstract Interpretation (arXiv 2611.06319, 2026)](https://arxiv.org/abs/2611.06319) states Polonius as forward abstract interpretation and proves it sound in Rocq | the closest to zustc's design (a forward analysis over the CFG, one origin per local); a template for stating the domain's lattice and monotonicity |
| | [RustCompCert (arXiv 2602.07455, 2026)](https://arxiv.org/abs/2602.07455) implements Polonius as Kildall-style transfer functions and proves the domain a semilattice | a template for proving that the fixpoint is well defined and terminates; the lattice checks in this report are a finite version of it |
| | [Polonius rules](https://rust-lang.github.io/polonius/), [Oxide (arXiv 1903.00982)](https://arxiv.org/abs/1903.00982), RustBelt (POPL 2018) | the specification; candidate oracles for differential testing on small CFGs |
| threads | RustBelt's Send/Sync, RbRlx (POPL 2020), [RustMC / GenMC (arXiv 2502.06293)](https://arxiv.org/abs/2502.06293), Loom, Shuttle | Kani handles sequential code only ([the Kani paper, arXiv 2607.01504](https://arxiv.org/abs/2607.01504), and its documentation), so concurrency is done in TLA+ |
| builds and caches | Build systems à la carte (ICFP 2018), [Forward Build Systems, Formally (CPP 2022)](https://arxiv.org/abs/2202.05328), [CloudMake (FM 2014)](https://www.microsoft.com/en-us/research/wp-content/uploads/2014/05/krml235.pdf) | correctness means "what comes from the cache is what a rebuild would make"; no TLA+ model of a fork/pipe worker pool, or of how a content-addressed cache is saved, was found |
| allocators | [verify-rust-std](https://github.com/model-checking/verify-rust-std) (challenge 19, RawVec, solved with VeriFast; 23/24, Vec, open), [RefinedRust (PLDI 2024)](https://plv.mpi-sws.org/refinedrust), [verified-memory-allocator (Verus)](https://github.com/verus-lang/verified-memory-allocator), Gillian-Rust (PLDI 2025) | verify-rust-std's harnesses for out-of-bounds accesses, invalid derefs and uninitialized reads are the template for the prelude's `Vec` |
| linking spec and code | quint-connect, tla-connect, Using Lightweight Formal Methods to Validate a Key-Value Storage Node in Amazon S3 (SOSP 2021) | the next step (replaying the spec's traces against the implementation) |

The survey checked the URL and abstract of each entry. RustBelt, RbRlx, Build systems à la carte and the S3 paper are well-known works, cited from memory.

## 1. Borrow checking (the lifetime analysis)

The analysis of [report 08](2026-11-18-08-borrow-checking.md) is a worklist fixpoint that grows the state at each block's entry with `State::join`. For it to be right, the lattice operations need the following properties:

- The sorted sets stay sorted and without duplicates (`binary_search` relies on this).
- The result of a join is exactly the union of both sides.
- Whenever the state changes as a set, the join reports a change.
- Joining the same thing again changes nothing, so the loop stops.
- The place order (`prefix`) is reflexive, transitive and antisymmetric, and `overlap` is symmetric.

`crates/typeck/src/acg/borrowck_props.rs` checks this exhaustively over a small universe (`cargo test -p zust-typeck props`, 0.04 s):

- `add_sorted` is tried with every set over 0..6 and every value.
- `join` is tried with every pair of values of each of its five parts, over 10,000 pairs.
  - `join` handles each part in its own loop, and its change flag is the or of the parts. So enumerating each part covers the whole.
  - For holds, the values include a local that is absent, a local whose set is empty, and locals in reverse order.
- The place order is tried on every pair and every triple of paths of length up to 2.

Everything holds.

These checks were first written as Kani harnesses. CBMC ran out of memory (about 15 GiB, while converting to propositional form) on a single `insert` at a symbolic index into a `Vec<u32>` of 4 elements. The same function in a crate of its own did the same. Kani does not suit properties of values such as `State`, which nest `Vec`s.

**Defect 1: stopping the fixpoint misses errors.** `Bk::check` returns `None` (no error) once the worklist has run more than `50 * blocks + 1000` rounds. The comment says "should not happen: the states only grow".

- Termination itself holds: join is monotone, and the lattice has finite height.
- The number of rounds, though, grows with the product of the lattice height and the number of blocks. The limit is proportional to the number of blocks alone.
- To reproduce, run `formal/repro/borrowck_rounds.py K`. It shifts K references by one step in a loop, in reverse order, so each round moves the loan one step. Then it writes `x = 1`.
  - rustc reports E0506 for every K.
  - zustc finds the error up to K = 259, and from K = 260 on misses it and accepts the program.

Proposed fixes:

- Replace the limit with a provable bound, `n * (H + 1) + 1`. H is the lattice height: moved places + locals + loans × (locals + 2).
- Or, when the limit is exceeded, report an error instead of accepting.
- A reverse-postorder worklist would also lower the number of rounds.

## 2. The thread model

### zustc's runtime

The prelude's `std::thread::spawn` runs the closure at once. `Arc` is the same as `Rc`, and atomics are plain cells. With one thread, `Arc`'s reference count is safe, but the semantics differ in two ways (`formal/repro/`):

- `thread_spawn_channel.rs`: the child calls `rx.recv()` and the parent sends after the spawn. rustc prints `child got 42`. With zustc, the child calls `recv` on an empty channel, gets `Err`, and panics. Running the child at once is only one of Rust's schedules. It does not hold when the child waits for something the parent does after the spawn.
- `thread_spawn_mutex.rs`: the parent spawns while it holds a lock, and the child locks too. With rustc the child waits until the parent unlocks. With zustc the child takes the lock anyway and prints 1. The output is the same, but mutual exclusion is broken (`Mutex` does not block).

`formal/tla/ArcDrop.tla` is the check to pass before real threads come in. Two threads each drop a clone.

- With the prelude's `strong -= 1; if strong == 0 { free }` (a separate load and store), rtlc finds a counterexample in 7 steps. The store of a clone is overwritten by the store of a drop, and the box is freed while a clone is still held (use after free).
- With `fetch_sub(1) == 1`, the whole state space passes. This assumes sequential consistency: the Release/Acquire argument belongs to RbRlx and is outside the model.

### The compiler's own concurrency

zustc uses no threads. What runs concurrently is the forked child processes, and other zustc processes running at the same time on the same output.

`formal/tla/ForkPool.tla` (driver2.rs's summaries, and build.rs's functions and crates) models what can happen:

- fork fails;
- a child dies at any point, or its write stops in the middle of a record (a signal, a short write);
- a writer waits while its pipe is full;
- a child inherits every pipe write end the parent holds at the time of the fork.

On top of that it checks three things: the values the parent takes are right (NoWrongValue), every item is right at the end (Correct), and there is no deadlock.

- With the code's order (the parent closes the write end of pipe k right after forking child k), these hold for 2 children and 4 items (257 states) and for 3 children and 6 items (1862 states).
- As a control, the model also checks a version that closes the write ends only after forking every child. Child 2 holds pipe 1's write end while it waits for room in pipe 2, and the parent waits for EOF on pipe 1. rtlc finds this deadlock in 6 steps. This confirms that the code avoids exactly this trap.

**Defect 4: saving the cache.** See `formal/tla/CacheSave.tla`. `Cache2::save` writes `<path>.tmp` and then renames it. `Cache2::open` accepts a file whose header and lengths are in bounds; there is no checksum. When two zustc processes save to the same output at the same time (a build and a `zustc check` side by side, for example), this can happen:

- Since the tmp name is the same, B reopens A's half-written tmp with O_TRUNC.
- After A renames it, B keeps writing into that file, which is now `<path>`.
- A reader can accept a file that is structurally valid but mixes B's header with the rest of A's file. rtlc gives an 8-step counterexample.
- An entry then has B's key and contents partly made of A's bytes, so wrong machine code could come out of the cache.
- B's rename also fails, which only gives a warning.

The window is narrow, but POSIX allows it. With a tmp name per process (`<path>.<pid>.tmp`), the whole state space passes (72 states). A checksum in the saved file would also let a torn read be discarded.

Reading the code also confirmed the following (not in the model):

- `read_all` and `write_all` also stop on EINTR. Both fall back to the parent rebuilding the item, so the result is right; it is only slower.
- When fork or pipe fails during build.rs's function compilation, it returns `Err` without waiting for the children already forked.

## 3. The allocator (the prelude's `Vec`)

The `build.rs` of `formal/kani-prelude` copies `Vec<T>`, 16 of its methods and its `Drop` from `crates/syntax/src/prelude_lib.rs.in` as they are. The harnesses therefore check the code zustc compiles, and follow the prelude when it changes.

What zustc supplies is modelled:

- `realloc` / `free` are a model in which a block knows its size. `realloc` copies `min(old, new)` bytes and, when allowed, returns null.
- `__exr_drop_in_place` is the drop of T.

What is verified (up to capacity 4, from any state reached by `with_capacity(0..4)` followed by 0..3 pushes):

- `push`, `pop`, `insert` (positions 0/1/2, one harness each), `remove`, `swap_remove`, `truncate` and `reverse` each give the same contents as an array reference model.
- `len <= cap` holds.
- Nothing outside the allocated range is touched (Kani's pointer checks).
- The drop frees every block.
- `drops_each_element_once`: after any operation, every element is dropped exactly once.

What was found:

- **Defect 2:** `bug_with_capacity_overflow`. `reserve`'s `c * size_of::<T>()` (and `len + extra`) can overflow.
  - The block is then small while `cap` stays large, and a push writes outside the block.
  - `formal/repro/vec_capacity_overflow.rs` (`Vec::<u64>::with_capacity(1 << 61)` followed by pushes) is a segmentation fault with zustc. rustc panics with "capacity overflow".
  - This is heap corruption in safe code. The proposed fix is `checked_add` / `checked_mul`, with a panic on failure.
- **Defect 3:** `bug_alloc_failure_unchecked`. When `realloc` returns null, `reserve` uses it anyway and a push writes through null. std aborts in `handle_alloc_error`.

Limits of Kani here:

- With std's `Vec` as the reference model, CBMC ran out of memory, so a fixed-size array is used instead.
- `insert` at a symbolic index also ran out of memory, so it is split into one harness per position.
- The proofs are bounded, up to capacity 4. Any length would need Kani's loop contracts (`-Z loop-contracts`) or deductive verification such as Verus or RefinedRust.

## Running again

```bash
formal/run.sh tla
```

```bash
formal/run.sh kani
```

```bash
cargo test -p zust-typeck props
```

Each of the 7 TLA+ configurations takes about 1 ms. The 12 Kani harnesses take about 8 minutes (10 are verified, and the 2 `bug_*` harnesses fail as expected) and 5–10 GiB. idealis is pinned by git rev (89f54a9).

## Next steps

- Fix defects 1–4: a provable bound for the borrow checker, overflow and null checks in `Vec`, and a per-process tmp name for the cache.
- Differential testing of the borrow checker. Generate every small CFG and event list, and compare against a reference that applies the Polonius rules path by path. Where gen/kill is distributive, the meet over all paths (MOP) should equal the fixpoint (MFP).
- Linking the specs to the code. Replay the traces of ForkPool / CacheSave against the real driver2, as quint-connect does, with idealis's Driver / `idealis-testing`.
- Before threads come in, `ArcDrop.tla` must pass with the atomic version, and `spawn` must stop running the closure at once.
