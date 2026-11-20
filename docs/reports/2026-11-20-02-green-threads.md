# `std::thread` as green threads

Date: 2026-11-20

zustc's `std::thread::spawn` ran the closure at once, and `join` gave its value (defect 5 of
[report 2026-11-19-01](2026-11-19-01-formal-models.md)). That is one of the schedules Rust allows, but only for a thread
that never waits for something another thread does later. Two programs went wrong (`formal/repro/thread_spawn_*.rs`):

- `thread_spawn_channel.rs`: the child calls `recv` and main sends after the spawn. rustc prints `child got 42`, but
  under zustc the child received on an empty channel and panicked.
- `thread_spawn_mutex.rs`: main holds a lock while it spawns, and the child locks too. The output was right, but the
  child got the lock while main still held it (`Mutex::lock` never blocked).

A spin on an atomic flag that main sets after the spawn (`while !FLAG.load(..) {}`) never ended.

## Choice

Three ways were weighed:

- Real OS threads (pthread). `Arc` and the atomics would have to become real atomics, which needs atomic instructions
  in both backends (Cranelift and minclif). `Mutex` and the channels would need futexes, the prelude's `static mut`
  items would have to become thread-local, and a panic in a child would have to become `join`'s `Err`. This is the
  largest change, and it reaches code generation.
- Deferred closures, run at `join` or when a thread would wait. This is small, but it cannot handle threads that take
  turns (a ping-pong over two channels).
- Green threads: a stack per thread and a switch at the points where a thread waits. This is what was chosen. It
  handles taking turns, and since there is still one OS thread, `Arc` (an `Rc`) and the atomics (plain cells) stay
  sound: nothing runs between two switch points.

## Design

The runtime is in the prelude (`crates/syntax/src/prelude_lib.rs.in`, `std::thread`):

- A scheduler holds the threads (thread 0 is main), the current one, and an epoch that grows whenever something
  another thread may wait for happens: a send, an unlock, a thread ending.
- `spawn` allocates a 1 MiB stack and a `ucontext_t`, and sets the entry with libc `getcontext` / `makecontext`. It then
  switches to the new thread at once (`swapcontext`). A thread that never waits therefore runs as it did before, which
  keeps the order of output of such programs.
- A thread waits in `join`, in `recv` on an empty channel (and in the channel's `iter`), in a `Mutex` another thread
  holds, and in `sleep` and `yield_now`. An atomic `load` also yields while other threads are alive, so a spin loop
  lets the others run. Waiting runs the next thread that is not done, round robin.
- A deadlock is when every other thread waits and a whole round changed nothing. Then `join` panics with "deadlock",
  `recv` returns `Err` (as if every sender had hung up, which is what it did before for a single thread), and `lock`
  goes on.
- `MutexGuard` now unlocks in its `Drop`. If a guard is not dropped where Rust drops it, a later `lock` waits for the
  other threads and then goes on, so a program with one thread behaves as before.

One prelude fix was needed. The prelude is cut down to the items a program uses, by name, and the name of
`static mut X` was taken as `mut`. Since almost every program writes `mut`, every `static mut` item of the prelude was
included in every program. The scheduler's static pulled the thread structs into `hello` and conflicted with programs
that define their own `Box`. `static mut X` is now named `X` (`crates/syntax/src/prelude.rs`, `defined_name`).

## What was checked

- `tests/green_threads.rs` builds the programs in `tests/green_threads/` and compares their output with rustc's. They
  cover the two reproductions, a ping-pong over two channels, four producers and one consumer, a `Mutex` held while the
  child tries to lock (the child must wait), 8 threads × 1000 increments through a `Mutex` with `yield_now` inside the
  critical section, a spin on an atomic flag, 200 threads that each spawn one more, and a program with no threads.
  Before the change, the zustc on main fails `flag` with a timeout.
- tests/ui: 0 regressions. Two tests pass that did not before, `threads-sendsync/issue-4448.rs` and
  `threads-sendsync/send-resource.rs`; the baseline is updated.
- `cargo test --release --workspace` (116 passed), `tests/compare.sh` (42 ok), the size budget.
- KPIs: the `KPI` workflow comment on the pull request has the change against the base.

## What is left

- No parallelism, and a thread that never waits keeps the others from running. A `thread_local!` is shared by all
  threads. A panic in a thread still stops the process (there is no unwinding), so `join` never returns `Err` for a
  panic.
- Real OS threads stay on the roadmap. `formal/tla/ArcDrop.tla` is the check `Arc` has to pass then.
