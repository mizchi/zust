// rustc and zustc both print 1, but under zustc the child takes the lock while the parent still holds it: `spawn`
// runs the closure at once, and `Mutex::lock` never blocks (one thread). With rustc the child waits for `drop(g)`.
use std::sync::{Arc, Mutex};
use std::thread;
fn main() {
    let m = Arc::new(Mutex::new(0));
    let g = m.lock().unwrap();
    let m2 = m.clone();
    let h = thread::spawn(move || {
        *m2.lock().unwrap() += 1;
    });
    drop(g);
    h.join().unwrap();
    println!("{}", *m.lock().unwrap());
}
