// std::sync::Condvar over the green threads: wait / notify_one / notify_all, wait_while, wait_timeout (#250: crossbeam-utils,
// rayon-core, jobserver)
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;
fn main() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = Arc::clone(&pair);
    let h = thread::spawn(move || {
        let (lock, cvar) = &*pair2;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_one();
        drop(started);
        3
    });
    let (lock, cvar) = &*pair;
    let mut started = lock.lock().unwrap();
    while !*started {
        started = cvar.wait(started).unwrap();
    }
    println!("{} {}", *started, h.join().unwrap());
    drop(started);
    let count = Arc::new((Mutex::new(0u32), Condvar::new()));
    let mut hs = vec![];
    for i in 0..3 {
        let c = Arc::clone(&count);
        hs.push(thread::spawn(move || {
            let (m, cv) = &*c;
            *m.lock().unwrap() += i + 1;
            cv.notify_all();
        }));
    }
    let (m, cv) = &*count;
    let g = cv.wait_while(m.lock().unwrap(), |n| *n < 6).unwrap();
    println!("{}", *g);
    drop(g);
    for h in hs { h.join().unwrap(); }
    let (g, r) = cv.wait_timeout(m.lock().unwrap(), Duration::from_millis(1)).unwrap();
    println!("{} {}", *g, r.timed_out() || !r.timed_out());
}
