// Real parallelism: many threads at once on the atomics, an Arc, a Mutex, channels (a sync_channel of bound 0), an
// RwLock, a Condvar, Once and OnceLock, park / unpark and a detached thread. Every total is exact only when each
// operation is atomic
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Condvar, Once, OnceLock, mpsc};
use std::thread;

static HITS: AtomicUsize = AtomicUsize::new(0);
static INIT: Once = Once::new();
static CELL: OnceLock<usize> = OnceLock::new();

fn main() {
    // atomics: 8 threads x 100000 increments, and an Arc clone / drop per step
    let n = Arc::new(AtomicUsize::new(0));
    let hs: Vec<_> = (0..8).map(|_| {
        let n = n.clone();
        thread::spawn(move || {
            for _ in 0..100000 {
                let m = n.clone();
                m.fetch_add(1, Ordering::Relaxed);
            }
        })
    }).collect();
    for h in hs { h.join().unwrap(); }
    println!("atomic {} arcs {}", n.load(Ordering::SeqCst), Arc::strong_count(&n));

    // Mutex: 8 x 20000, read-modify-write under the lock
    let c = Arc::new(Mutex::new(0u64));
    let hs: Vec<_> = (0..8).map(|_| {
        let c = c.clone();
        thread::spawn(move || {
            for _ in 0..20000 {
                let mut g = c.lock().unwrap();
                let v = *g;
                *g = v + 1;
            }
        })
    }).collect();
    for h in hs { h.join().unwrap(); }
    println!("mutex {}", *c.lock().unwrap());

    // channel: 4 producers x 10000, the receiver ends when every sender is dropped
    let (tx, rx) = mpsc::channel::<u64>();
    for p in 0..4u64 {
        let tx = tx.clone();
        thread::spawn(move || {
            for i in 0..10000u64 { tx.send(p * 100000 + i).unwrap(); }
        });
    }
    drop(tx);
    let mut sum = 0u64;
    let mut cnt = 0;
    for v in rx.iter() { sum += v; cnt += 1; }
    println!("channel {} {}", cnt, sum);

    // sync_channel(0): a rendezvous
    let (tx, rx) = mpsc::sync_channel::<i32>(0);
    let h = thread::spawn(move || { for i in 0..5 { tx.send(i).unwrap(); } });
    let got: Vec<i32> = rx.iter().collect();
    h.join().unwrap();
    println!("sync {:?}", got);

    // RwLock: writers and readers
    let l = Arc::new(RwLock::new(Vec::new()));
    let hs: Vec<_> = (0..4).map(|k| {
        let l = l.clone();
        thread::spawn(move || {
            for i in 0..1000 {
                if i % 10 == 0 { l.write().unwrap().push(k); } else { let _n = l.read().unwrap().len(); }
            }
        })
    }).collect();
    for h in hs { h.join().unwrap(); }
    println!("rwlock {}", l.read().unwrap().len());

    // Condvar: a queue a consumer waits on
    let q = Arc::new((Mutex::new(Vec::<i32>::new()), Condvar::new()));
    let q2 = q.clone();
    let cons = thread::spawn(move || {
        let (m, cv) = &*q2;
        let mut total = 0;
        let mut got = 0;
        while got < 1000 {
            let mut g = m.lock().unwrap();
            while g.is_empty() { g = cv.wait(g).unwrap(); }
            while let Some(x) = g.pop() { total += x; got += 1; }
        }
        total
    });
    for i in 0..1000 {
        let (m, cv) = &*q;
        m.lock().unwrap().push(i);
        cv.notify_one();
    }
    println!("condvar {}", cons.join().unwrap());

    // Once / OnceLock from many threads
    let hs: Vec<_> = (0..8).map(|k| thread::spawn(move || {
        INIT.call_once(|| { HITS.fetch_add(1, Ordering::SeqCst); });
        *CELL.get_or_init(|| { HITS.fetch_add(10, Ordering::SeqCst); k * 0 + 42 })
    })).collect();
    let vs: Vec<usize> = hs.into_iter().map(|h| h.join().unwrap()).collect();
    println!("once {} {:?}", HITS.load(Ordering::SeqCst), vs);

    // park / unpark, and a spin on a flag
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    let parked = thread::spawn(move || {
        while !f2.load(Ordering::Acquire) { thread::park(); }
        "unparked"
    });
    thread::sleep(std::time::Duration::from_millis(20));
    flag.store(true, Ordering::Release);
    parked.thread().unpark();
    println!("park {}", parked.join().unwrap());

    // a detached thread that sends back
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || { tx.send(thread::current().id()).unwrap(); });
    let id = rx.recv().unwrap();
    println!("detached {}", id != thread::current().id());
    println!("{:?}", rx.recv().is_err());
}
