use std::thread;
fn fib(n: u64) -> u64 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }
fn main() {
    let hs: Vec<_> = (0..200u64).map(|i| thread::spawn(move || {
        let inner = thread::spawn(move || fib(i % 20));
        inner.join().unwrap() + i
    })).collect();
    let s: u64 = hs.into_iter().map(|h| h.join().unwrap()).sum();
    println!("{s}");
    let h = thread::spawn(|| String::from("hello"));
    println!("{} {}", h.join().unwrap(), thread::spawn(|| 5).join().unwrap());
}
