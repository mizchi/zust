// std::sync::atomic::fence / compiler_fence (crossbeam-utils' seq_lock), and Condvar::wait_timeout with
// WaitTimeoutResult (its parker): a notify that comes, and one that cannot (the time is up)
use std::sync::atomic::{self, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

fn main() {
    let a = AtomicUsize::new(1);
    atomic::fence(Ordering::Acquire);
    a.store(2, Ordering::Relaxed);
    atomic::fence(Ordering::Release);
    atomic::compiler_fence(Ordering::SeqCst);
    println!("{}", a.load(Ordering::Relaxed));

    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let p2 = Arc::clone(&pair);
    let h = thread::spawn(move || {
        let (m, c) = &*p2;
        *m.lock().unwrap() = true;
        c.notify_one();
    });
    let (m, c) = &*pair;
    let mut ready = m.lock().unwrap();
    while !*ready {
        // (how many times it waits depends on the threads' order: nothing printed here)
        let r = c.wait_timeout(ready, Duration::from_secs(5)).unwrap();
        ready = r.0;
    }
    drop(ready);
    h.join().unwrap();
    println!("ready");

    let lone = Mutex::new(0);
    let cv = Condvar::new();
    let (g, res) = cv.wait_timeout(lone.lock().unwrap(), Duration::from_millis(10)).unwrap();
    println!("{} {:?}", *g, res);
}
