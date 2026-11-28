// std::thread::Thread: thread::current(), park / unpark, id and name, JoinHandle::thread (a parked thread waits on
// its futex until unpark leaves its token)
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, Thread};

static READY: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

// (as once_cell keeps the threads that wait for a cell)
struct Waiter {
    thread: Option<Thread>,
}

fn main() {
    let me = thread::current();
    println!("{:?} {:?}", me.name(), me.id());
    println!("{:?}", me);
    println!("{}", me.id() == thread::current().id());
    // (an unpark before the park: the token makes the park return at once)
    me.unpark();
    thread::park();
    println!("parked and woken");
    let mut w = Waiter { thread: Some(thread::current()) };
    let main_t = w.thread.take().unwrap();
    let h = thread::spawn(move || {
        let t = thread::current();
        println!("child {:?} {:?} {}", t.name(), t.id(), t.id() != main_t.id());
        READY.store(true, Ordering::Release);
        main_t.unpark();
        7
    });
    while !READY.load(Ordering::Acquire) {
        thread::park();
    }
    println!("ready seen");
    println!("{:?}", h.thread().id());
    println!("joined {}", h.join().unwrap());
    // (a child that parks until main wakes it)
    let h2 = thread::spawn(|| {
        while !DONE.load(Ordering::Acquire) {
            thread::park();
        }
        format!("{:?} woke", thread::current().id())
    });
    DONE.store(true, Ordering::Release);
    h2.thread().unpark();
    println!("{}", h2.join().unwrap());
    let ids: Vec<thread::ThreadId> = (0..3).map(|_| thread::spawn(|| thread::current().id())).map(|h| h.join().unwrap()).collect();
    println!("{:?}", ids);
    println!("{:#?}", thread::current());
    let current = 5;
    println!("{current}");
}
