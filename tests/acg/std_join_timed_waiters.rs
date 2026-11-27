// thread::Result as `join`'s type, with std's `Any { .. }` for `{:?}` of a `dyn Any` (alone, and inside an Err); two
// timed waiters on one Condvar whose short deadline comes first: it is not slept past while the long one waits, nor is
// main's join of it taken for a deadlock; a join or recv of a thread in a timed wait, which is no deadlock either; and
// a wait_timeout of Duration::MAX, which only a notify ends (#292's review)
use std::any::Any;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    let r: thread::Result<u8> = thread::spawn(|| 4u8).join();
    println!("{:?}", r.is_ok());
    let hs: Vec<_> = (0..3).map(|i| thread::spawn(move || i * 2)).collect();
    let s: i32 = hs.into_iter().map(|h| h.join().unwrap()).sum();
    println!("{}", s);
    match thread::spawn(|| "x").join() {
        Ok(x) => println!("{}", x),
        Err(e) => println!("{:?}", e),
    }
    let e: Box<dyn Any + Send> = Box::new(3u8);
    println!("{:?} {}", e, e.is::<u8>());
    let r: Result<u8, Box<dyn Any + Send>> = Err(Box::new(1));
    println!("{:?}", r);

    two_waiters(false);
    two_waiters(true);
    waited_for();
    max_wait();
}

// (a timed wait that only its deadline ends: std's wakeups may be spurious, so it waits again on one, for what is left
// of the time; whether it ended no sooner than its deadline)
fn wait_out<T>(m: &Mutex<T>, cv: &Condvar, dur: Duration) -> bool {
    let end = Instant::now() + dur;
    let mut g = m.lock().unwrap();
    loop {
        let (g2, r) = cv.wait_timeout(g, end.saturating_duration_since(Instant::now())).unwrap();
        g = g2;
        if r.timed_out() {
            return Instant::now() >= end;
        }
    }
}

// (each order of the spawns: before #292's fix, the 10 ms waiter came 5 s late when it was spawned first. The long
// waiter says it is ready while it holds the lock, which it gives up only in wait_timeout, so main's notify cannot come
// before it waits, however late an OS thread starts; it waits again on a wakeup with no value, as std's may be spurious)
fn two_waiters(short_first: bool) {
    // (the value, and whether the long waiter is ready)
    let state = Arc::new((Mutex::new((0u32, false)), Condvar::new(), Condvar::new()));
    let t0 = Instant::now();
    let spawn_long = |state: &Arc<(Mutex<(u32, bool)>, Condvar, Condvar)>| {
        let p = Arc::clone(state);
        thread::spawn(move || {
            let (m, cv, ready) = &*p;
            let mut g = m.lock().unwrap();
            g.1 = true;
            ready.notify_all();
            loop {
                let (g2, r) = cv.wait_timeout(g, Duration::from_secs(5)).unwrap();
                g = g2;
                if g.0 != 0 || r.timed_out() {
                    return (g.0, r.timed_out());
                }
            }
        })
    };
    let spawn_short = |state: &Arc<(Mutex<(u32, bool)>, Condvar, Condvar)>| {
        let p = Arc::clone(state);
        thread::spawn(move || {
            let (m, cv, _) = &*p;
            (wait_out(m, cv, Duration::from_millis(10)), t0.elapsed() < Duration::from_secs(2))
        })
    };
    let (short, long) = if short_first {
        let s = spawn_short(&state);
        (s, spawn_long(&state))
    } else {
        let l = spawn_long(&state);
        (spawn_short(&state), l)
    };
    let (timed_out, fast) = short.join().unwrap();
    println!("short first {}: timed out {} before the long deadline {}", short_first, timed_out, fast);
    {
        let (m, cv, ready) = &*state;
        let mut g = m.lock().unwrap();
        while !g.1 {
            g = ready.wait(g).unwrap();
        }
        g.0 = 7;
        cv.notify_all();
    }
    let (v, timed_out) = long.join().unwrap();
    println!("long: value {} timed out {} within 2s {}", v, timed_out, t0.elapsed() < Duration::from_secs(2));
}

// (a thread in a timed wait that nothing notifies, joined by main after a lock of its own, and one that sends once its
// time is up: before #292's fix, main took each for a deadlock)
fn waited_for() {
    let pair = Arc::new((Mutex::new(()), Condvar::new()));
    let p = Arc::clone(&pair);
    let h = thread::spawn(move || {
        let (m, cv) = &*p;
        wait_out(m, cv, Duration::from_millis(30))
    });
    let other = Mutex::new(0);
    *other.lock().unwrap() += 1;
    println!("join: {}", h.join().unwrap());
    let (tx, rx) = mpsc::channel();
    let p = Arc::clone(&pair);
    let h = thread::spawn(move || {
        let (m, cv) = &*p;
        tx.send(wait_out(m, cv, Duration::from_millis(30))).unwrap();
    });
    *other.lock().unwrap() += 1;
    println!("recv: {:?}", rx.recv());
    h.join().unwrap();
}

// (a deadline past what an Instant holds: the wait never times out, and ends by the notify of a thread that needs the
// lock the waiter gives up)
fn max_wait() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let p = Arc::clone(&pair);
    let (m, cv) = &*pair;
    let mut g = m.lock().unwrap();
    let h = thread::spawn(move || {
        let (m, cv) = &*p;
        *m.lock().unwrap() = true;
        cv.notify_all();
    });
    let mut timed_out = false;
    while !*g {
        let (g2, r) = cv.wait_timeout(g, Duration::MAX).unwrap();
        g = g2;
        timed_out = timed_out || r.timed_out();
    }
    drop(g);
    h.join().unwrap();
    println!("max: notified, timed out {}", timed_out);
}
