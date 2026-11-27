// A park that nothing can unpark: std's blocks forever, and so does zustc's
use std::thread;

fn main() {
    let h = thread::spawn(|| 1);
    println!("joined {}", h.join().unwrap());
    thread::park();
    println!("unparked");
}
