// A Mutex locked again by the thread that holds it: std's waits forever, and so does zustc's (not a second guard)
use std::sync::Mutex;

fn main() {
    let m = Mutex::new(1);
    let g1 = m.lock().unwrap();
    println!("locked once: {}", *g1);
    let g2 = m.lock().unwrap();
    println!("two guards: {} {}", *g1, *g2);
}
