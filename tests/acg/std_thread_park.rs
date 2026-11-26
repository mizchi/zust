// std::thread's Thread (current, id, name, unpark), park, Builder (name, stack_size, spawn) and JoinHandle::thread, over the
// green threads: a park waits for the thread's unpark (#250: crossbeam-utils, once_cell)
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, Thread};
static READY: AtomicBool = AtomicBool::new(false);
struct Waiter { thread: Cell<Option<Thread>> }
fn main() {
    let me = thread::current();
    println!("{:?} {:?}", me.name(), me.id() == thread::current().id());
    let w = Waiter { thread: Cell::new(Some(thread::current())) };
    let t = w.thread.take().unwrap();
    let h = thread::Builder::new().name("worker".to_string()).stack_size(64 * 1024).spawn(move || {
        READY.store(true, Ordering::SeqCst);
        t.unpark();
        7
    }).unwrap();
    let other = h.thread().clone();
    while !READY.load(Ordering::SeqCst) {
        thread::park();
    }
    println!("{}", other.id() != me.id());
    println!("{}", h.join().unwrap());
    thread::current().unpark();
    thread::park();
    println!("done");
}
