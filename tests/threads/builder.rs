// thread::Builder (a name, a stack size), park_timeout, recv_timeout, a Receiver's into_iter, Barrier, and a closure that shares
// what it borrows (Sync through its by-reference captures)
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

fn par_for<I: Iterator, F: Fn(I::Item) + Sync>(iter: I, f: F)
where
    I::Item: Send,
{
    for item in iter {
        f(item)
    }
}

fn need_send<F: FnOnce() + Send>(f: F) {
    f()
}

fn main() {
    // (a name the thread sees; a small stack, rounded up)
    let h = thread::Builder::new()
        .name("worker".to_string())
        .stack_size(20000)
        .spawn(|| {
            let t = thread::current();
            format!("{:?}", t.name())
        })
        .unwrap();
    println!("{}", h.join().unwrap());
    let h = thread::Builder::new().spawn(|| thread::current().name().map(|s| s.to_string())).unwrap();
    println!("{:?}", h.join().unwrap());
    // (a panic in a named thread)
    let h = thread::Builder::new().name("boom".to_string()).spawn(|| panic!("named")).unwrap();
    println!("{}", h.join().is_err());

    // (park_timeout: it ends by itself; a token given before takes it at once; an unpark from another thread wakes it)
    thread::park_timeout(Duration::from_millis(10));
    thread::current().unpark();
    thread::park_timeout(Duration::from_secs(100));
    let me = thread::current();
    let w = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        me.unpark();
    });
    thread::park_timeout(Duration::from_secs(100));
    w.join().unwrap();
    println!("parked");

    // (recv_timeout: a timeout while this thread keeps the sender (nothing can come); values that come in time, one
    // sent while it waits; a disconnect)
    let (idle_tx, idle_rx) = channel::<u32>();
    println!("{:?}", idle_rx.recv_timeout(Duration::from_millis(10)));
    drop(idle_tx);
    let (tx, rx) = channel::<u32>();
    let t = thread::spawn(move || {
        tx.send(1).unwrap();
        thread::sleep(Duration::from_millis(100));
        tx.send(2).unwrap();
    });
    println!("{:?}", rx.recv_timeout(Duration::from_secs(5)));
    // (a deadline past what a timespec holds: the wait is as long as it can be, not one that fails at once)
    println!("{:?}", rx.recv_timeout(Duration::MAX));
    t.join().unwrap();
    println!("{:?}", rx.recv_timeout(Duration::from_millis(10)));
    println!("{} / {}", RecvTimeoutError::Timeout, RecvTimeoutError::Disconnected);

    // (into_iter: every value, until the senders are gone)
    let (tx, rx) = channel();
    for i in 0..4 {
        let tx = tx.clone();
        thread::spawn(move || {
            for j in 0..100 {
                tx.send(i * 100 + j).unwrap();
            }
        });
    }
    drop(tx);
    let total: u64 = rx.into_iter().sum();
    println!("{}", total);

    // (a Barrier: no thread passes before all have come; one of each round is the leader)
    let n = 6;
    let b = Arc::new(Barrier::new(n));
    let at = Arc::new(Mutex::new(0usize));
    let hs: Vec<_> = (0..n)
        .map(|_| {
            let (b, at) = (b.clone(), at.clone());
            thread::spawn(move || {
                let mut leader = 0;
                let mut early = 0;
                for round in 0..50 {
                    *at.lock().unwrap() += 1;
                    if b.wait().is_leader() {
                        leader += 1;
                    }
                    if *at.lock().unwrap() < (round + 1) * n {
                        early += 1;
                    }
                    b.wait();
                }
                (leader, early)
            })
        })
        .collect();
    let (mut leaders, mut early) = (0, 0);
    for h in hs {
        let (l, e) = h.join().unwrap();
        leaders += l;
        early += e;
    }
    println!("{} {} {:?}", leaders, early, Barrier::new(1).wait());

    // (a closure that borrows a Mutex is Sync)
    let sum = Mutex::new(0);
    let xs = [1, 2, 3, 4, 5, 6, 7];
    par_for(xs.windows(3), |w| *sum.lock().unwrap() += w.len());
    println!("{}", *sum.lock().unwrap());

    // (a closure that assigns a borrowed Cell, or borrows it mutably, holds a `&mut Cell`: Send, as Cell is)
    let mut c = std::cell::Cell::new(0);
    need_send(|| c = std::cell::Cell::new(4));
    need_send(|| {
        let r = &mut c;
        r.set(r.get() + 1);
    });
    let mut cells = vec![std::cell::Cell::new(1)];
    need_send(|| cells[0] = std::cell::Cell::new(7));
    println!("{} {}", c.get(), cells[0].get());
}
