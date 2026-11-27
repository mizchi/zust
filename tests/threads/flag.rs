use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
static FLAG: AtomicBool = AtomicBool::new(false);
fn main() {
    let h = thread::spawn(move || {
        let mut spins = 0u64;
        while !FLAG.load(Ordering::Acquire) {
            spins += 1;
        }
        spins < u64::MAX
    });
    println!("main sets the flag");
    FLAG.store(true, Ordering::Release);
    println!("child done: {}", h.join().unwrap());
}
