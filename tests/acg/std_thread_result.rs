// thread::Result<T> (a panic's payload, Box<dyn Any + Send>, for the error), std::any::Any (dyn Any's is),
// panic::catch_unwind / resume_unwind (no unwinding: the closure's value), and std::os::unix::thread's RawPthread /
// JoinHandleExt (#250: rayon-core, crossbeam-utils)
use std::any::Any;
use std::panic;
use std::thread;
fn halt() -> thread::Result<i32> {
    panic::catch_unwind(|| 5)
}
fn resume(payload: Box<dyn Any + Send>) -> ! {
    panic::resume_unwind(payload)
}
fn join_it() -> Option<u32> {
    let h = thread::spawn(|| 40 + 2);
    h.join().ok()
}
fn main() {
    let r = halt();
    println!("{:?}", r.as_ref().ok());
    println!("{:?}", join_it());
    let b: Box<dyn Any + Send> = Box::new(7u8);
    println!("{} {}", b.is::<u8>(), b.is::<u16>());
    if r.is_err() { resume(Box::new(1)) }
    let v: Vec<thread::JoinHandle<u8>> = (0..3u8).map(|i| thread::spawn(move || i * 2)).collect();
    let s: u8 = v.into_iter().map(|h| h.join().unwrap()).sum();
    println!("{}", s);
}
mod unix_ext {
    use std::os::unix::thread::{JoinHandleExt, RawPthread};
    pub fn raw(h: &std::thread::JoinHandle<u8>) -> RawPthread { h.as_pthread_t() }
}
pub fn raw_nonzero() -> bool {
    let h = std::thread::spawn(|| 1u8);
    let r = unix_ext::raw(&h) != 0;
    h.join().unwrap();
    r
}
