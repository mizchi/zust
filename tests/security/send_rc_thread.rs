//! `Rc` is not `Send` (E0277): moving it to another thread would race on its count.
//@ expect: reject
use std::rc::Rc;
fn main() {
    let rc = Rc::new(5);
    let h = std::thread::spawn(move || { //~ ERROR E0277
        println!("{}", rc);
    });
    h.join().unwrap();
}
