# `std::thread` as OS threads

Date: 2026-11-27

Since [report 2026-11-20-02](2026-11-20-02-green-threads.md), zustc's threads were green threads: one OS thread, a
stack per spawned thread, and a switch where a thread waits. That was sound only because nothing ran between two
switch points, so `Arc` was an `Rc`, the atomics were plain cells, and there was no parallelism. A thread that never
waited kept the others from running. This report replaces them with OS threads, as std's on Linux (issue #8, "Threads").

## What std does on Linux

std's runtime for these types, read in `library/std/src/sys` (rustc 1.97):

- `thread::spawn` calls `pthread_create` with a 2 MiB stack (`RUST_MIN_STACK` changes it). The closure's value goes
  into a packet that the thread and its `JoinHandle` share (an `Arc`); `join` is `pthread_join`, then takes the value.
  A `JoinHandle` dropped without `join` detaches the thread.
- A `Thread` is an `Arc` of its id, name and parker. ThreadIds count from 1 in the order threads are made, so the main
  thread is 1.
- The parker, `Mutex`, `Condvar`, `RwLock` and `Once` are futex state machines:
  - the parker's state is empty / notified / parked;
  - a `Mutex` is 0 unlocked, 1 locked, 2 locked with waiters: `lock` takes it 0 -> 1, or spins a little, sets 2 and
    waits on the futex; `unlock` sets 0 and wakes one waiter if it was 2;
  - a `Condvar` is a counter each notify bumps, which `wait` waits on after it lets the Mutex go;
  - `Once` is incomplete / running / running with waiters / complete.
- `Arc` counts with atomic instructions: a clone adds one (Relaxed; past `isize::MAX` it aborts), a drop takes one
  away (Release), and the drop that takes the last one fences (Acquire) and frees. formal/tla/ArcDrop.tla models this.
- A panic in a spawned thread prints `thread '<unnamed>' panicked at ..`, unwinds that thread (its values are dropped,
  a held `Mutex` is poisoned), runs its thread-locals' drops, and `join` gives `Err` with the payload (its Debug is
  `Any { .. }`). The note on `RUST_BACKTRACE` comes with the first panic only.
- `println!` takes the stdout lock for the whole line, so lines of two threads do not cut into each other.

## What changed

### Atomic instructions (`crates/ir`, minclif, Cranelift, wasm)

The IR gets five instructions, all sequentially consistent (std's orderings are all as strong or weaker):
`AtomicLoad`, `AtomicStore`, `AtomicRmw` (exchange, add), `AtomicCas` and `Fence`, on integers of 1, 2, 4 or 8 bytes.
The other read-modify-writes (`fetch_and`, `fetch_max`, ..) are compare-and-swap loops in the prelude.

- minclif: `mov` for a load (an aligned x86-64 load is atomic and acquires), `xchg` for a store and for an exchange,
  `lock xadd`, `lock cmpxchg`, `mfence`. They use rax, rcx and rdx, which minclif never allocates. An address they
  read counts as a use that is not a load or store, so the stack slot behind it is never turned into variables.
- Cranelift: `atomic_load`, `atomic_store`, `atomic_rmw`, `atomic_cas`, `fence`.
- wasm32: plain loads and stores (one thread there).

The prelude names them as functions (`__exr_atomic_load`, `_store`, `_xchg`, `_xadd`, `_cas`, `_fence`); acg lowers a
call of one to the instruction (`atomic_intrinsic` in `crates/typeck/src/acg/p2_call.rs`), so nothing else in codegen
knows about atomics.

### The prelude (`crates/syntax/src/prelude_lib.rs.in`)

- The atomic types (`AtomicUsize` .. `AtomicI64`, `AtomicBool`, `AtomicPtr`) are the intrinsics; `fence` and
  `compiler_fence` are there too. The spin of an atomic `load` that let the other green threads run is gone.
- `Arc` counts with `xadd`; `try_unwrap` is a compare-and-swap 1 -> 0; `into_inner` is std's (of two threads that
  each give up one of the last two, one gets the value).
- `spawn`, `join`, `JoinHandle`'s drop (detach), the packet, `Thread` (now reference counted), `current`, `park` and
  `unpark` are std's, on `pthread_create` / `pthread_join` / `pthread_detach` and `futex(2)` through libc's `syscall`.
- `Mutex`, `Condvar`, `Once` are std's futex ones. `RwLock` is simpler than std's: the state is the number of
  readers, or `u32::MAX` for a writer, and a thread that cannot have it waits on a second futex word that every unlock
  bumps. `OnceLock` is a `Once` and the value.
- `mpsc` is a queue under the futex Mutex, with a Condvar both ends wait on. `recv` gives `Err` once every sender is
  dropped (before, when every thread waited); `send` fails once the receiver is gone; a `sync_channel(n)` send waits
  while the queue is full, and with bound 0 until its value is received.
- `thread_local!` (`lexer::expand_thread_local`) keeps one value per thread: a key numbered at its first use, and a
  vector of values in each thread's context (a pthread key holds the context). A spawned thread drops its values when
  it ends, newest first, before `join` returns.
- A spawned thread's panic: the message names it `<unnamed>`, the note comes once, then the thread marks its packet
  as panicked, drops its thread-locals and ends with `pthread_exit` (also from the SIGILL handler of an overflow trap).
  `join` gives `Err(Any { .. })`.
- `print!` / `println!` take a lock around the line once a thread has been spawned. The lock is reentrant, and a
  `StdoutLock` holds it for its life, as std's.
- What a panic, a disconnect or a lock does at the edges, as std's (from the review of the pull request):
  - thread_local! values are dropped in reverse of the order their thread made them; one whose drop has begun cannot
    be reached again (`with` panics, `try_with` gives `AccessError`); one first made by another's drop is dropped
    too; a drop that panics aborts the program ("thread local panicked on drop");
  - a spawned thread that panics while it runs a `Once`'s closure poisons it, so the threads that wait wake:
    `call_once` then panics, `call_once_force` runs again and sees `is_poisoned()`, and `OnceLock` runs the next
    caller's initializer; a `StdoutLock` it holds is let go;
  - a `sync_channel(0)` send whose receiver goes before it takes the value fails with it; a dropped receiver drops the
    values left in the queue; the channels' errors are std's `SendError` (its Debug does not need `T: Debug`),
    `RecvError` and `TryRecvError`;
  - `RwLock`'s Debug takes a read lock without waiting, and prints `<locked>` while a writer holds it;
  - a closure held in a local before `spawn(f)` is checked as one written in the call: a capture held by reference
    is E0373 and one that is not `Send` is E0277 (tests/security/spawn_named_closure_borrow.rs,
    send_rc_named_closure_thread.rs); a `StdoutLock` is not `Send` (send_stdout_lock_thread.rs);
  - `try_recv` tells Empty from Disconnected under the same lock as it finds the queue empty;
  - a failed `pthread_create` frees what it was given; a failed `pthread_key_create` is a panic in `spawn`, and a
    failed `pthread_setspecific` in a new thread aborts.

The thread-aware parts of the panic and print paths go through one hook (`__EXR_THREAD_HOOK`), which the first spawn
sets. A program with no threads has none and does what it did before; it pays one static and a test of it in the
panic and print functions. The prelude is still cut down by name: a program that spawns nothing gets no pthread
function. Two of the new types (`__TlsKey`, `__ThreadPanic`) are listed as impl owners in `crates/syntax/src/prelude.rs`,
so their impls come only with them.

## What was checked

- `tests/threads.rs` (was `tests/green_threads.rs`) builds each program of `tests/threads/` with both backends and
  compares the output with rustc's. Three are new:
  - `parallel`: 8 threads × 100 000 increments of an atomic, each through a clone of an `Arc`; 8 × 20 000
    read-modify-writes under a `Mutex`; 4 producers × 10 000 sends; a `sync_channel(0)`; readers and writers of an
    `RwLock`; a consumer on a `Condvar`; `Once` and `OnceLock` from 8 threads; park / unpark; a detached thread. Each
    total is exact only when every operation is atomic. It runs 5 times per backend.
  - `panic`: an index out of bounds, an overflow trap and a `panic!` in spawned threads. Each ends only its thread, its
    `join` gives `Err`, its thread-locals are dropped, and the note comes once.
  - `tls`: each thread's own `thread_local!` values, and their drops when the thread ends.
  - `edges`: the cases of the list above, as rustc's build prints them.
- The deadlock programs (a relock, a park nothing unparks, a `call_once` in its own closure, a Condvar nothing
  notifies) now wait forever, as std's do. The test checks that each is still running after 2 s and printed nothing
  rustc's did not. They used to panic with "deadlock".
- `tests/probe/sync_once.rs` said which of three threads ran a `call_once`. With threads in parallel that is the
  scheduler's choice (rustc's is too), so it now prints that one did.
- `cargo test --release --workspace`. The one failure, `ast_codegen`'s `std_file`, is there on main too: rustc 1.97
  checks `OpenOptions` in another order, and the message differs.
- A CPU-bound loop (200 M xorshift steps per thread, `-O`) on this 4-core machine: 1 thread 0.42 s, 4 threads 0.43 s.
  On green threads 4 threads took 1.63 s, 4 times as long as one.

## Costs

- Every program: the panic path tests the hook, and `println!`'s function does too. `hello`'s binary is 112 bytes
  larger (9 704 -> 9 816) and its cache 349 bytes (8 191 -> 8 540). `bench/budget.txt` had 0.1 % of headroom left for
  `float_fuzz`, and is raised (to the measurement + 5 %, as `UPDATE_BUDGET` does) for its cache and those of
  `options` and `text_more`.
- A program with `Arc`: a clone and a drop are `lock xadd`s instead of plain adds.
- The prelude is as long as before (16 591 lines, 3 fewer): the atomic types shrank by about as much as the runtime grew.

## What is left

In TODO.md, "Threads":

- No poisoning of a Mutex or RwLock, and no drops of a panicking thread's values. Both need unwinding.
- No `thread::scope`, `Condvar::wait_timeout`, `Mutex::try_lock`. (`Builder`, `park_timeout`, `recv_timeout` and
  `Barrier` came after, in [report 2026-11-27-02](2026-11-27-02-threads-ui.md).)
- The main thread's thread-locals are not dropped at exit.
- The location an `unwrap` reports is one static, which another thread may overwrite meanwhile.
- wasm32 has no `spawn` (no pthreads in the host); the atomics and locks work there as plain accesses.
