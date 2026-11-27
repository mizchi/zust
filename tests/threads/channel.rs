// rustc: prints "child got 42" and "done". zustc: panics at the `unwrap()` of `rx.recv()`.
// zustc's `std::thread::spawn` runs the closure at once, so the child receives before the parent has sent.
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
