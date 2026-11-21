//! A thread that borrows a local of the spawning function (E0373): the thread may outlive the frame.
//@ pattern: Send / Sync
//@ expect: reject
fn main() {
    let v = vec![1, 2, 3];
    let h = std::thread::spawn(|| { //~ ERROR E0373
        println!("{:?}", v);
    });
    h.join().unwrap();
}
