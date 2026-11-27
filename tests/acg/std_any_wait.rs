// `&dyn std::any::Any` named by its path only (no `use`, no thread::Result), and Condvar::wait_timeout that ends at
// its duration while another thread keeps running (spinning on an AtomicBool, never waiting), or with no other
// thread at all (#292's review)
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn show(x: &dyn std::any::Any) -> bool {
    x.is::<u8>()
}

fn main() {
    let b: Box<dyn std::any::Any> = Box::new(5u8);
    println!("{} {} {}", show(&3u8), show(&3u16), b.type_id() == std::any::TypeId::of::<u8>());

    let stop = Arc::new(AtomicBool::new(false));
    let s2 = Arc::clone(&stop);
    let spinner = thread::spawn(move || {
        let mut n = 0u64;
        while !s2.load(Ordering::Acquire) {
            n += 1;
            thread::yield_now();
        }
        n > 0
    });
    let m = Mutex::new(());
    let cv = Condvar::new();
    let t0 = Instant::now();
    let (g, r) = cv.wait_timeout(m.lock().unwrap(), Duration::from_millis(30)).unwrap();
    let waited = t0.elapsed();
    drop(g);
    stop.store(true, Ordering::Release);
    println!("{} {} {}", r.timed_out(), waited >= Duration::from_millis(30), waited < Duration::from_secs(5));
    println!("{}", spinner.join().unwrap());

    let lone = Mutex::new(0);
    let t1 = Instant::now();
    let (_g, r) = cv.wait_timeout(lone.lock().unwrap(), Duration::from_millis(20)).unwrap();
    println!("{} {}", r.timed_out(), t1.elapsed() >= Duration::from_millis(20));
}
