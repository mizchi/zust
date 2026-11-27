// rustc and zustc both print 1. Before 2026-11-20, under zustc the child took the lock while the parent still held
// it (`spawn` ran the closure at once and `Mutex::lock` never blocked). Since then the child waits in lock until
// `drop(g)`, as with rustc: first on green threads, now on OS threads (tests/threads/mutex.rs checks that it does).
use std::sync::{Arc, Mutex};
use std::thread;
fn main() {
    let m = Arc::new(Mutex::new(0));
    let g = m.lock().unwrap();
    let m2 = m.clone();
    let h = thread::spawn(move || {
        *m2.lock().unwrap() += 1;
    });
    drop(g);
    h.join().unwrap();
    println!("{}", *m.lock().unwrap());
}
