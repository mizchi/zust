// `std::sync::Condvar` with a Mutex: a waiter woken by a notify from another thread (`wait` lets the Mutex go and
// waits on the Condvar's futex until a notify comes after it)
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

struct Queue {
    items: Mutex<Vec<u32>>,
    ready: Condvar,
}

fn main() {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    let pair2 = Arc::clone(&pair);
    let h = thread::spawn(move || {
        let (lock, cvar) = &*pair2;
        let mut started = lock.lock().unwrap();
        *started = true;
        cvar.notify_one();
        println!("child notified");
    });
    let (lock, cvar) = &*pair;
    let mut started = lock.lock().unwrap();
    while !*started {
        started = cvar.wait(started).unwrap();
    }
    println!("started: {}", *started);
    drop(started);
    h.join().unwrap();

    // (a producer and two consumers, through wait_while and notify_all)
    let q = Arc::new(Queue { items: Mutex::new(Vec::new()), ready: Condvar::new() });
    let mut hs = vec![];
    for id in 0..2 {
        let q = Arc::clone(&q);
        hs.push(thread::spawn(move || {
            let g = q.items.lock().unwrap();
            let mut g = q.ready.wait_while(g, |v| v.is_empty()).unwrap();
            let x = g.pop().unwrap();
            (id, x)
        }));
    }
    {
        let mut g = q.items.lock().unwrap();
        g.push(10);
        g.push(20);
        q.ready.notify_all();
    }
    let mut got: Vec<u32> = hs.into_iter().map(|h| h.join().unwrap().1).collect();
    got.sort();
    println!("{:?}", got);
    println!("{:?}", Condvar::new());
}
