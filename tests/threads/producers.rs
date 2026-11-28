use std::sync::mpsc;
use std::thread;
fn main() {
    let (tx, rx) = mpsc::channel::<u64>();
    let mut hs = Vec::new();
    for t in 0..4u64 {
        let tx = tx.clone();
        hs.push(thread::spawn(move || {
            for i in 0..100u64 {
                tx.send(t * 1000 + i).unwrap();
                if i % 10 == 0 { thread::yield_now(); }
            }
        }));
    }
    drop(tx);
    let mut total = 0u64;
    let mut n = 0;
    for v in rx.iter() {
        total += v;
        n += 1;
    }
    for h in hs { h.join().unwrap(); }
    println!("{n} {total}");
}
