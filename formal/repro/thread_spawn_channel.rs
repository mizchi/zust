// rustc and zustc: print "child got 42" and "done". Before 2026-11-20 zustc's `std::thread::spawn` ran the closure at
// once, so the child received before the parent had sent and panicked at `rx.recv().unwrap()`. Threads are now green
// threads: the child waits in recv and main runs (tests/green_threads.rs).
use std::sync::mpsc;
use std::thread;
fn main() {
    let (tx, rx) = mpsc::channel::<i32>();
    let h = thread::spawn(move || {
        let v = rx.recv().unwrap();
        println!("child got {v}");
    });
    tx.send(42).unwrap();
    h.join().unwrap();
    println!("done");
}
