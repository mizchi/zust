//! `Send` / `Sync` are unsafe traits: implementing them without `unsafe impl` is E0200.
//! Otherwise safe code could declare an `Rc`-holding type thread-safe.
//@ rule: the unsafe boundary (E0133 / E0200 / E0512)
//@ expect: reject
use std::rc::Rc;
struct Shared(Rc<u32>);
impl Send for Shared {} //~ ERROR E0200
fn main() {
    let s = Shared(Rc::new(1));
    println!("{}", s.0);
}
