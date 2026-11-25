//! Thread parallelism for the front end (prototype, docs/parallel-frontend.md): an ordered parallel map on scoped
//! threads. What the forked children of driver2 do (item summaries, check_many), in one process: the work is handed
//! out from a shared counter, and the results come back in the order of the indices, so the caller merges them as it
//! does one process's.
//!
//! Not self-hosted: zustc's threads are green threads (one OS thread) and it has no `thread::scope`; a self-host build
//! would put the sequential loop of `map_with` (jobs 1) in place of this module.

use std::sync::atomic::{AtomicUsize, Ordering};

/// How the front end runs its parallel parts (`ZUSTC_PAR`, or `Options2::par`): `fork` (forked children, the default;
/// only in a process of one thread), `threads` (scoped threads, in any process), `off` (one thread)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Fork,
    Threads,
    Off,
}

/// The mode `ZUSTC_PAR` asks for (read once). A host that runs the driver in process chooses per session instead
/// (driver2's `Options2::par`)
pub fn mode() -> Mode {
    static M: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();
    *M.get_or_init(|| match std::env::var("ZUSTC_PAR").as_deref() {
        Ok("threads") | Ok("thread") => Mode::Threads,
        Ok("off") | Ok("0") => Mode::Off,
        _ => Mode::Fork,
    })
}

/// A worker's stack: as large as the main thread's usual 8 MiB (lowering and inference recurse on deep expressions;
/// a spawned thread's default is 2 MiB). Only the pages it touches are resident
const STACK: usize = 8 << 20;

/// How many workers started, over the process (tests read it, as driver2::forks)
static SPAWNED: AtomicUsize = AtomicUsize::new(0);

pub fn spawned() -> usize {
    SPAWNED.load(Ordering::Relaxed)
}

/// `f(&mut state, i)` for i in 0..n on up to `jobs` threads (the calling thread is one of them), the results in the
/// order of i. Each thread makes its state once with `init` (on that thread: it also sets the thread-locals the caller
/// set on its own); the indices are taken `chunk` at a time from a shared counter, so a slow one does not hold up a
/// fixed stripe of others. An index whose `f` panicked gives None, as one a forked child did not send: the caller does
/// it again on its own thread. With jobs 1 (or one chunk) it is a plain loop on the calling thread
pub fn map_with<S, T: Send>(jobs: usize, n: usize, chunk: usize, init: &(dyn Fn() -> S + Sync), f: &(dyn Fn(&mut S, usize) -> T + Sync)) -> Vec<Option<T>> {
    run(jobs, n, chunk, init, &|st, i| Some(f(st, i)), false)
}

/// map_with for work that a loop would end at its first failure (a loop that returns at its first error): `f` gives
/// None for an index that failed, and no index after the first that failed is started (those already started are
/// finished). Every index before it is done, so the caller, going in order, meets the same first failure as the loop.
/// Without this, a parallel run of work that fails early does all of it before the caller sees the failure (a
/// document of the LSP whose item summaries fail at the start: issue #165)
pub fn map_until<S, T: Send>(jobs: usize, n: usize, chunk: usize, init: &(dyn Fn() -> S + Sync), f: &(dyn Fn(&mut S, usize) -> Option<T> + Sync)) -> Vec<Option<T>> {
    run(jobs, n, chunk, init, f, true)
}

fn run<S, T: Send>(jobs: usize, n: usize, chunk: usize, init: &(dyn Fn() -> S + Sync), f: &(dyn Fn(&mut S, usize) -> Option<T> + Sync), until: bool) -> Vec<Option<T>> {
    let chunk = chunk.max(1);
    let jobs = jobs.min(n.div_ceil(chunk)).max(1);
    if jobs <= 1 {
        let mut st = init();
        let mut out: Vec<Option<T>> = Vec::with_capacity(n);
        for i in 0..n {
            let r = f(&mut st, i);
            let failed = r.is_none();
            out.push(r);
            if until && failed {
                out.extend((i + 1..n).map(|_| None));
                break;
            }
        }
        return out;
    }
    let next = AtomicUsize::new(0);
    // (until: the least index that failed)
    let first_fail = AtomicUsize::new(usize::MAX);
    let work = |out: &mut Vec<(usize, Option<T>)>| {
        let mut st = init();
        loop {
            let s = next.fetch_add(chunk, Ordering::Relaxed);
            if s >= n || s > first_fail.load(Ordering::Relaxed) {
                break;
            }
            for i in s..(s + chunk).min(n) {
                if i > first_fail.load(Ordering::Relaxed) {
                    break;
                }
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut st, i))).ok().flatten();
                if until && r.is_none() {
                    first_fail.fetch_min(i, Ordering::Relaxed);
                }
                out.push((i, r));
            }
        }
    };
    let mut slots: Vec<Option<T>> = (0..n).map(|_| None).collect();
    std::thread::scope(|s| {
        let hs: Vec<_> = (1..jobs)
            .filter_map(|_| {
                let h = std::thread::Builder::new().stack_size(STACK).spawn_scoped(s, || {
                    let mut out = vec![];
                    work(&mut out);
                    out
                });
                h.ok().inspect(|_| {
                    SPAWNED.fetch_add(1, Ordering::Relaxed);
                })
            })
            .collect();
        let mut mine = vec![];
        work(&mut mine);
        for (i, r) in mine {
            slots[i] = r;
        }
        for h in hs {
            if let Ok(out) = h.join() {
                for (i, r) in out {
                    slots[i] = r;
                }
            }
        }
    });
    slots
}

/// map_with without a state
pub fn map<T: Send>(jobs: usize, n: usize, chunk: usize, init: &(dyn Fn() + Sync), f: &(dyn Fn(usize) -> T + Sync)) -> Vec<Option<T>> {
    map_with(jobs, n, chunk, init, &|_: &mut (), i| f(i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_and_complete() {
        for jobs in [1, 2, 4, 7] {
            for chunk in [1, 3, 64] {
                let v = map(jobs, 1000, chunk, &|| {}, &|i| i * 2);
                assert_eq!(v.len(), 1000);
                assert!(v.iter().enumerate().all(|(i, x)| *x == Some(i * 2)), "jobs {jobs} chunk {chunk}");
            }
        }
    }

    #[test]
    fn a_panic_is_none() {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let v = map(4, 100, 1, &|| {}, &|i| if i == 37 { panic!("x") } else { i });
        std::panic::set_hook(prev);
        assert_eq!(v[37], None);
        assert_eq!(v.iter().filter(|x| x.is_some()).count(), 99);
    }

    #[test]
    fn until_the_first_failure() {
        for jobs in [1, 2, 4] {
            let calls = AtomicUsize::new(0);
            let v = map_until(jobs, 10_000, 4, &|| (), &|_, i| {
                calls.fetch_add(1, Ordering::Relaxed);
                if i == 37 || i == 80 { None } else { Some(i) }
            });
            // (everything before the first failure is done, and it is the first None)
            assert!(v[..37].iter().enumerate().all(|(i, x)| *x == Some(i)), "jobs {jobs}");
            assert_eq!(v[37], None);
            // (and little after it was started: not the 10000)
            assert!(calls.load(Ordering::Relaxed) < 1000, "jobs {jobs}: {}", calls.load(Ordering::Relaxed));
        }
    }

    #[test]
    fn state_per_thread() {
        // (each thread's state counts the indices it did: together, all of them)
        let v = map_with(4, 500, 7, &|| 0usize, &|st, _| {
            *st += 1;
            *st
        });
        assert!(v.iter().all(|x| x.is_some()));
    }
}
