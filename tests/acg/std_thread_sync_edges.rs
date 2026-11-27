// thread::Result with `Any` never written (the trait still comes with it), Condvar::wait_timeout's LockResult and
// its PoisonError's into_inner, and an Instant before the clock's origin (`now - 10 years`, as std's signed seconds)
use std::sync::{Condvar, LockResult, Mutex, MutexGuard, WaitTimeoutResult};
use std::thread;
use std::time::{Duration, Instant};

fn run() -> thread::Result<u8> {
    Ok(3)
}

fn main() {
    println!("{:?}", run().ok());
    let h = thread::spawn(|| 4u8);
    let r: thread::Result<u8> = Ok(h.join().unwrap());
    println!("{:?}", r.is_ok());

    let m = Mutex::new(7);
    let cv = Condvar::new();
    let r: LockResult<(MutexGuard<i32>, WaitTimeoutResult)> = cv.wait_timeout(m.lock().unwrap(), Duration::from_millis(1));
    let (g, t) = match r {
        Ok(x) => x,
        Err(e) => e.into_inner(),
    };
    println!("{} {}", *g, t.timed_out());
    drop(g);
    let g: LockResult<MutexGuard<i32>> = cv.wait_while(m.lock().unwrap(), |v| *v > 100);
    println!("{}", *g.unwrap());

    let ten_years = Duration::from_secs(10 * 365 * 86400);
    let now = Instant::now();
    let past = now - ten_years;
    println!("{} {}", now.duration_since(past) == ten_years, now.checked_sub(ten_years).is_some());
    println!("{}", past < now);
}
