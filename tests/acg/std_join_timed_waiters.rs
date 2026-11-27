// thread::Result as `join`'s type, with std's `Any { .. }` for `{:?}` of a `dyn Any` (alone, and inside an Err), and
// two timed waiters on one Condvar whose short deadline comes first: it is not slept past while the long one waits,
// nor is main's join of it taken for a deadlock (#292's review)
use std::any::Any;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    let r: thread::Result<u8> = thread::spawn(|| 4u8).join();
    println!("{:?}", r.is_ok());
    let hs: Vec<_> = (0..3).map(|i| thread::spawn(move || i * 2)).collect();
    let s: i32 = hs.into_iter().map(|h| h.join().unwrap()).sum();
    println!("{}", s);
    match thread::spawn(|| "x").join() {
        Ok(x) => println!("{}", x),
        Err(e) => println!("{:?}", e),
    }
    let e: Box<dyn Any + Send> = Box::new(3u8);
    println!("{:?} {}", e, e.is::<u8>());
    let r: Result<u8, Box<dyn Any + Send>> = Err(Box::new(1));
    println!("{:?}", r);

    two_waiters(false);
    two_waiters(true);
}

// (each order of the spawns: before #292's fix, the 10 ms waiter came 5 s late when it was spawned first)
fn two_waiters(short_first: bool) {
    let pair = Arc::new((Mutex::new(0u32), Condvar::new()));
    let t0 = Instant::now();
    let spawn_long = |pair: &Arc<(Mutex<u32>, Condvar)>| {
        let p = Arc::clone(pair);
        thread::spawn(move || {
            let (m, cv) = &*p;
            let g = m.lock().unwrap();
            let (g, r) = cv.wait_timeout(g, Duration::from_secs(5)).unwrap();
            (*g, r.timed_out())
        })
    };
    let spawn_short = |pair: &Arc<(Mutex<u32>, Condvar)>| {
        let p = Arc::clone(pair);
        thread::spawn(move || {
            let (m, cv) = &*p;
            let g = m.lock().unwrap();
            let (_g, r) = cv.wait_timeout(g, Duration::from_millis(10)).unwrap();
            (r.timed_out(), t0.elapsed() < Duration::from_secs(2))
        })
    };
    let (short, long) = if short_first {
        let s = spawn_short(&pair);
        (s, spawn_long(&pair))
    } else {
        let l = spawn_long(&pair);
        (spawn_short(&pair), l)
    };
    let (timed_out, fast) = short.join().unwrap();
    println!("short first {}: timed out {} before the long deadline {}", short_first, timed_out, fast);
    {
        let (m, cv) = &*pair;
        *m.lock().unwrap() = 7;
        cv.notify_all();
    }
    let (v, timed_out) = long.join().unwrap();
    println!("long: value {} timed out {} within 2s {}", v, timed_out, t0.elapsed() < Duration::from_secs(2));
}
