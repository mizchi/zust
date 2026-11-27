//! A closure held in a local before it is spawned is checked as one written in the call: its `Rc` capture is not
//! `Send` (E0277), and on an OS thread its count would race.
//@ rule: Send / Sync
//@ expect: reject
use std::rc::Rc;
fn main() {
    let rc = Rc::new(5);
    let f = move || {
        println!("{}", rc);
    };
    let h = std::thread::spawn(f); //~ ERROR E0277
    h.join().unwrap();
}
