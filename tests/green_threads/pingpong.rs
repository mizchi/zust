use std::sync::mpsc;
use std::thread;
fn main() {
    let (to_child, from_main) = mpsc::channel::<u32>();
    let (to_main, from_child) = mpsc::channel::<u32>();
    let h = thread::spawn(move || {
        let mut sum = 0;
        for _ in 0..5 {
            let v = from_main.recv().unwrap();
            sum += v;
            to_main.send(v * 2).unwrap();
        }
        sum
    });
    for i in 1..=5u32 {
        to_child.send(i).unwrap();
        let r = from_child.recv().unwrap();
        println!("sent {i} got {r}");
    }
    println!("child sum {}", h.join().unwrap());
}
