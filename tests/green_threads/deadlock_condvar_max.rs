// A Condvar wait_timeout whose deadline is past what an Instant holds (Duration::MAX), which no thread can notify: it
// never times out, so std's waits forever; zustc's panics, as for `wait` (before, it reported a timeout at once)
use std::sync::{Condvar, Mutex};
use std::time::Duration;

fn main() {
    let m = Mutex::new(false);
    let cv = Condvar::new();
    let mut g = m.lock().unwrap();
    println!("waiting");
    while !*g {
        let (g2, r) = cv.wait_timeout(g, Duration::MAX).unwrap();
        g = g2;
        println!("timed out: {}", r.timed_out());
    }
    println!("notified");
}
