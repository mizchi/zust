use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
fn main() {
    let m = Arc::new(Mutex::new(Vec::new()));
    let g = m.lock().unwrap();
    let m2 = m.clone();
    let h = thread::spawn(move || {
        m2.lock().unwrap().push("child");
    });
    thread::sleep(Duration::from_millis(10));
    // (the child must not have pushed: main holds the lock)
    let seen = g.len();
    drop(g);
    h.join().unwrap();
    m.lock().unwrap().push("main");
    println!("{seen} {:?}", *m.lock().unwrap());
    let c = Arc::new(Mutex::new(0u32));
    let mut hs = Vec::new();
    for _ in 0..8 {
        let c2 = c.clone();
        hs.push(thread::spawn(move || {
            for k in 0..1000 {
                let mut g = c2.lock().unwrap();
                let v = *g;
                if k % 7 == 0 { thread::yield_now(); }
                *g = v + 1;
            }
        }));
    }
    for h in hs { h.join().unwrap(); }
    println!("{}", *c.lock().unwrap());
}
