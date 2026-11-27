use std::sync::{Arc, Mutex, mpsc};
fn main() {
    // one thread, no spawn: what worked before keeps working
    let m = Mutex::new(5);
    { let mut g = m.lock().unwrap(); *g += 1; }
    *m.lock().unwrap() += 1;
    println!("{}", *m.lock().unwrap());
    let (tx, rx) = mpsc::channel();
    tx.send(1).unwrap(); tx.send(2).unwrap();
    println!("{:?} {:?} {:?}", rx.recv(), rx.try_recv(), rx.try_recv().is_err());
    let a = Arc::new(Mutex::new(vec![1]));
    a.lock().unwrap().push(2);
    println!("{:?}", *a.lock().unwrap());
    std::thread::sleep(std::time::Duration::from_millis(1));
    std::thread::yield_now();
}
