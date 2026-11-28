// std's lock results (LockResult, TryLockResult, TryLockError), try_lock / try_read / try_write against a lock another
// thread holds, a static AtomicPtr to a type that is not Sync (as parking_lot_core's HASHTABLE), fmt::Error as a
// std::error::Error (as serde's `impl ser::Error for fmt::Error`), thread::Result (as crossbeam's and rayon's), and a
// JoinHandle's pthread_t (std::os::unix::thread::JoinHandleExt, as crossbeam's ScopedJoinHandle)
use std::cell::Cell;
use std::fmt;
use std::os::unix::thread::{JoinHandleExt, RawPthread};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, LockResult, Mutex, PoisonError, RwLock, TryLockError, TryLockResult};
use std::thread;

struct HashTable {
    n: usize,
    prev: *const HashTable,
    hits: Cell<u32>,
}

static HASHTABLE: AtomicPtr<HashTable> = AtomicPtr::new(std::ptr::null_mut());

fn table() -> &'static HashTable {
    let p = HASHTABLE.load(Ordering::Acquire);
    if !p.is_null() {
        return unsafe { &*p };
    }
    let new = Box::into_raw(Box::new(HashTable { n: 7, prev: std::ptr::null(), hits: Cell::new(0) }));
    match HASHTABLE.compare_exchange(std::ptr::null_mut(), new, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => unsafe { &*new },
        Err(old) => {
            unsafe { drop(Box::from_raw(new)) };
            unsafe { &*old }
        }
    }
}

// (crossbeam's ShardedLock: its own results over a std lock)
struct Sharded<T> {
    lock: RwLock<()>,
    value: T,
}

impl<T> Sharded<T> {
    fn into_inner(self) -> LockResult<T> {
        if self.lock.is_poisoned() { Err(PoisonError::new(self.value)) } else { Ok(self.value) }
    }
    fn try_read(&self) -> TryLockResult<&T> {
        match self.lock.try_read() {
            Ok(_g) => Ok(&self.value),
            Err(TryLockError::Poisoned(err)) => {
                drop(err.into_inner());
                Err(TryLockError::Poisoned(PoisonError::new(&self.value)))
            }
            Err(TryLockError::WouldBlock) => Err(TryLockError::WouldBlock),
        }
    }
}

// (serde's `trait Error: Sized + StdError`, implemented for fmt::Error)
trait SerError: Sized + std::error::Error {
    fn custom(msg: &str) -> Self;
}

impl SerError for fmt::Error {
    fn custom(_msg: &str) -> Self {
        fmt::Error
    }
}

// (crossbeam's `scope` and rayon's `halt_unwinding`: a join's result as thread::Result)
fn run<R: Send + 'static>(f: fn() -> R) -> thread::Result<R> {
    thread::spawn(f).join()
}

fn seven() -> u32 {
    7
}

fn describe<E: std::error::Error>(e: E) -> String {
    format!("{e} / {e:?}")
}

fn main() {
    // the static AtomicPtr, set once by whichever thread comes first
    let hs: Vec<_> = (0..4).map(|_| thread::spawn(|| table().n)).collect();
    let ns: Vec<usize> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    let t = table();
    t.hits.set(t.hits.get() + 1);
    println!("table {:?} prev null {} hits {}", ns, t.prev.is_null(), t.hits.get());

    // try_lock while another thread holds the Mutex, and once it lets go
    let m = Arc::new(Mutex::new(5));
    let (held_tx, held_rx) = channel();
    let (go_tx, go_rx) = channel::<()>();
    let m2 = Arc::clone(&m);
    let h = thread::spawn(move || {
        let g = m2.lock().unwrap();
        held_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        *g + 1
    });
    held_rx.recv().unwrap();
    match m.try_lock() {
        Err(TryLockError::WouldBlock) => println!("try_lock: would block"),
        Err(TryLockError::Poisoned(_)) => println!("try_lock: poisoned"),
        Ok(g) => println!("try_lock: got {}", *g),
    }
    go_tx.send(()).unwrap();
    println!("holder saw {}", h.join().unwrap());
    match m.try_lock() {
        Ok(mut g) => {
            *g += 10;
            println!("try_lock after: {}", *g)
        }
        Err(e) => println!("try_lock after: {e}"),
    }
    println!("mutex poisoned {}", m.is_poisoned());

    // RwLock: readers share, a writer excludes
    let rw = RwLock::new(vec![1, 2, 3]);
    {
        let r = rw.read().unwrap();
        println!("try_read under a reader: {}", rw.try_read().map(|v| v.len()).unwrap_or(0));
        match rw.try_write() {
            Err(TryLockError::WouldBlock) => println!("try_write under a reader: would block"),
            _ => println!("try_write under a reader: ?"),
        }
        println!("reader sees {:?}", *r);
    }
    {
        let mut w = rw.write().unwrap();
        w.push(4);
        match rw.try_read() {
            Err(e) => println!("try_read under a writer: {e}"),
            Ok(_) => println!("try_read under a writer: ?"),
        }
    }
    match rw.try_write() {
        Ok(mut w) => {
            w.push(5);
            println!("try_write free: {:?}", *w)
        }
        Err(_) => println!("try_write free: ?"),
    }
    println!("rwlock poisoned {}", rw.is_poisoned());
    println!("into_inner {:?}", rw.into_inner().unwrap());

    let s = Sharded { lock: RwLock::new(()), value: String::from("shard") };
    println!("sharded try_read {:?}", s.try_read().map(|v| v.len()).ok());
    println!("sharded into_inner {}", s.into_inner().unwrap());
    let would: TryLockError<()> = TryLockError::WouldBlock;
    println!("{would} / {would:?}");

    // fmt::Error as an error
    println!("{}", describe(fmt::Error));
    println!("{}", describe(<fmt::Error as SerError>::custom("x")));
    let b: Box<dyn std::error::Error> = Box::new(fmt::Error);
    println!("boxed: {b}");
    println!("format fails: {:?}", fails(true));

    let r: thread::Result<u32> = run(seven);
    match r {
        Ok(v) => println!("thread::Result ok {v}"),
        Err(e) => println!("thread::Result err {e:?}"),
    }
    pthreads();
}

unsafe extern "C" {
    fn pthread_join(t: RawPthread, ret: *mut *mut u8) -> i32;
}

// (crossbeam's ScopedJoinHandle: its own JoinHandleExt over a std handle)
struct Scoped<T> {
    h: Option<thread::JoinHandle<T>>,
}

impl<T> JoinHandleExt for Scoped<T> {
    fn as_pthread_t(&self) -> RawPthread {
        self.h.as_ref().unwrap().as_pthread_t()
    }
    fn into_pthread_t(self) -> RawPthread {
        self.as_pthread_t()
    }
}

fn pthreads() {
    let s = Scoped { h: Some(thread::spawn(|| 1u8)) };
    println!("scoped pthread_t nonzero {}", s.as_pthread_t() != 0);
    println!("scoped joined {}", s.h.unwrap().join().unwrap());
    // (a handle given up for its pthread_t: the thread is joined through it)
    let (tx, rx) = channel();
    let pt = thread::spawn(move || tx.send(9u32).unwrap()).into_pthread_t();
    println!("pthread_join {} got {}", unsafe { pthread_join(pt, std::ptr::null_mut()) }, rx.recv().unwrap());
}

fn fails(x: bool) -> fmt::Result {
    if x { Err(fmt::Error) } else { Ok(()) }
}
