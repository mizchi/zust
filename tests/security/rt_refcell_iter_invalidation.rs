//! Iterator invalidation through a RefCell: the borrow checker cannot see it (both borrows are of a shared
//! `&RefCell`), so only the run-time flag stands between the push and a use after free of the iterated buffer. The
//! `borrow_mut` while the loop's `borrow` is alive must panic.
//@ pattern: iterator invalidation (the same mistake as borrow_iter_invalidation, out of the borrow checker's sight)
//@ expect: panic
use std::cell::RefCell;
fn main() {
    let c = RefCell::new(vec![String::from("a"), String::from("b")]);
    for s in c.borrow().iter() {
        c.borrow_mut().push(s.clone());
    }
    println!("not reached {}", c.borrow().len());
}
