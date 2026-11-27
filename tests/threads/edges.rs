// What a panic, a disconnect or a lock does at the edges: thread_local! values dropped in reverse of the order each
// thread made them; a OnceLock whose initializer panicked on a spawned thread runs the next caller's; a Once poisoned
// by such a panic (call_once panics, call_once_force sees it); a rendezvous send whose receiver went fails; the values
// a dropped receiver can never get are dropped with it; an RwLock's Debug while a writer holds it; a StdoutLock held
// while printing (reentrant), and one taken before the first spawn, which a worker's println waits for; a value whose
// initializer made another one first is dropped before it (its drop still reaches the other); a program's own
// `AccessError` beside a thread_local!
use std::cell::RefCell;
use std::io::Write;
use std::sync::mpsc;
use std::sync::{Once, OnceLock, RwLock};
use std::thread;
use std::time::Duration;

struct D(&'static str);
impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

thread_local! {
    static A: RefCell<Option<D>> = RefCell::new(None);
    static B: RefCell<Option<D>> = RefCell::new(None);
}

// (the program's own type of std's name)
#[derive(Debug)]
struct AccessError(u8);

struct Outer;
impl Drop for Outer {
    fn drop(&mut self) {
        println!("drop outer; inner still there: {:?}", INNER.try_with(|i| *i).is_ok());
    }
}

thread_local! {
    // (OUTER's initializer makes INNER first: INNER is registered first, so it is dropped after OUTER)
    static OUTER: Outer = { INNER.with(|_| ()); Outer };
    static INNER: u32 = 7;
}

static CELL: OnceLock<u32> = OnceLock::new();
static ONCE: Once = Once::new();

fn main() {
    // (a StdoutLock taken before any thread: the worker's println waits until it is dropped)
    {
        let out = std::io::stdout();
        let mut lock = out.lock();
        let h = thread::spawn(|| println!("worker, after the lock"));
        writeln!(lock, "locked a").unwrap();
        thread::sleep(Duration::from_millis(50));
        writeln!(lock, "locked b").unwrap();
        drop(lock);
        h.join().unwrap();
    }
    thread::spawn(|| OUTER.with(|_| ())).join().unwrap();
    println!("{:?}", AccessError(1));

    // (main makes A first, so A has the lower key; the worker makes B first)
    A.with(|a| a.borrow().is_none());
    B.with(|b| b.borrow().is_none());
    thread::spawn(|| {
        B.with(|b| *b.borrow_mut() = Some(D("b, made first")));
        A.with(|a| *a.borrow_mut() = Some(D("a, made second")));
    })
    .join()
    .unwrap();

    let r = thread::spawn(|| -> u32 { *CELL.get_or_init(|| panic!("init fails")) }).join();
    println!("first init: {}", r.is_err());
    println!("second init: {}", CELL.get_or_init(|| 7));

    let r = thread::spawn(|| ONCE.call_once(|| panic!("once fails"))).join();
    println!("once: {} {}", r.is_err(), ONCE.is_completed());
    let r = thread::spawn(|| ONCE.call_once(|| println!("not run"))).join();
    println!("call_once after: {}", r.is_err());
    ONCE.call_once_force(|st| println!("force: poisoned {}", st.is_poisoned()));
    println!("completed {}", ONCE.is_completed());

    // (the sender says it is about to send; the receiver goes only after that, and a while after, so the send is
    // waiting with its value when it does)
    let (tx, rx) = mpsc::sync_channel::<u32>(0);
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let h = thread::spawn(move || {
        ready_tx.send(()).unwrap();
        match tx.send(5) {
            Err(e) => Some(e.0),
            Ok(()) => None,
        }
    });
    ready_rx.recv().unwrap();
    thread::sleep(Duration::from_millis(100));
    drop(rx);
    println!("rendezvous send after the receiver went: value back {:?}", h.join().unwrap());

    let (tx, rx) = mpsc::channel::<D>();
    tx.send(D("queued 1")).unwrap();
    tx.send(D("queued 2")).unwrap();
    drop(rx);
    println!("receiver dropped; send now: err {}", tx.send(D("late")).is_err());
    drop(tx);

    let l = RwLock::new(vec![1, 2]);
    {
        let _w = l.write().unwrap();
        println!("{:?}", l);
    }
    println!("{:?}", l);

    let out = std::io::stdout();
    let mut lock = out.lock();
    writeln!(lock, "through the lock").unwrap();
    println!("println while holding it");
    drop(lock);
    println!("end");
}
