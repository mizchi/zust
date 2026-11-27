//! A closure held in a local that borrows a local of the function may outlive it once spawned (E0373): on an OS
//! thread it would read the vector after the function returned and freed it.
//@ rule: borrow checking (the loans and moves that keep references valid)
//@ expect: reject
fn main() {
    let v = vec![1, 2, 3];
    let f = || println!("{:?}", v);
    let h = std::thread::spawn(f); //~ ERROR E0373
    h.join().unwrap();
}
