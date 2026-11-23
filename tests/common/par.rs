//! A test's cases on every core: each case builds and runs programs, and one at a time they take most of
//! `cargo test`'s time. Results come back in the order of the cases.
//! (Included with `#[path = "common/par.rs"] mod par;`: files under tests/common are not test targets of their own.)

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(items.len()).max(1);
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(t) = items.get(i) else { break };
                    let r = f(t);
                    out.lock().unwrap().push((i, r));
                }
            });
        }
    });
    let mut out = out.into_inner().unwrap();
    out.sort_by_key(|r| r.0);
    out.into_iter().map(|r| r.1).collect()
}
