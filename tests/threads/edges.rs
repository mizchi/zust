// What a panic, a disconnect or a lock does at the edges: thread_local! values dropped in reverse of the order each
// thread made them; a OnceLock whose initializer panicked on a spawned thread runs the next caller's; a Once poisoned
// by such a panic (call_once panics, call_once_force sees it); a rendezvous send whose receiver went fails; the values
// a dropped receiver can never get are dropped with it; an RwLock's Debug while a writer holds it; a StdoutLock held
// while printing (reentrant)
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

static CELL: OnceLock<u32> = OnceLock::new();
static ONCE: Once = Once::new();

fn main() {
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

    let (tx, rx) = mpsc::sync_channel::<u32>(0);
    let h = thread::spawn(move || tx.send(5).is_err());
    thread::sleep(Duration::from_millis(50));
    drop(rx);
    println!("rendezvous send after the receiver went: err {}", h.join().unwrap());

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
