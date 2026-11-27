// A Condvar wait that no thread can notify: std's waits forever, and so does zustc's
use std::sync::{Condvar, Mutex};

fn main() {
    let m = Mutex::new(false);
    let cv = Condvar::new();
    let mut g = m.lock().unwrap();
    println!("waiting");
    while !*g {
        g = cv.wait(g).unwrap();
    }
    println!("notified");
}
