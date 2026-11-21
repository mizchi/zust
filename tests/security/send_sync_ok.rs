//! The other side of the send_* / sync_* cases: what is Send / Sync must still build. Mutex and atomics share
//! between threads, an `unsafe impl Send` is trusted, `dyn Fn + Send` moves, and a static of a Sync type is fine.
//@ rule: Send / Sync
//@ expect: run
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
static COUNT: AtomicUsize = AtomicUsize::new(0);
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static GREETING: &str = "hi";
struct Raw(*mut u8);
unsafe impl Send for Raw {}
fn main() {
    let shared = Arc::new(Mutex::new(vec![1u32]));
    let s2 = shared.clone();
    let owned = vec![String::from("moved")];
    let raw = Raw(std::ptr::null_mut());
    let f: Box<dyn Fn() -> u32 + Send> = Box::new(|| 7);
    let h = std::thread::spawn(move || {
        s2.lock().unwrap().push(2);
        COUNT.fetch_add(1, Ordering::SeqCst);
        NAMES.lock().unwrap().push(owned[0].clone());
        let r = raw;
        let _ = r.0.is_null();
        f()
    });
    let r = h.join().unwrap();
    let v: Vec<u32> = (*shared.lock().unwrap()).clone();
    let names: Vec<String> = (*NAMES.lock().unwrap()).clone();
    println!("{} {:?} {} {:?} {}", r, v, COUNT.load(Ordering::SeqCst), names, GREETING);
}
