// A Condvar waiter notified by a thread that keeps the Mutex and parks: neither can go on (std's wait forever).
// zustc's wait forever too, and the waiter gets no second guard
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

fn main() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let p2 = Arc::clone(&pair);
    let _h = thread::spawn(move || {
        let (m, cv) = &*p2;
        let mut g = m.lock().unwrap();
        *g = true;
        cv.notify_one();
        thread::park();
        drop(g);
    });
    let (m, cv) = &*pair;
    let mut g = m.lock().unwrap();
    while !*g {
        g = cv.wait(g).unwrap();
    }
    println!("woken with the lock");
}
