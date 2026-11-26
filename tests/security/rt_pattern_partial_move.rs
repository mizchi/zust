//! A pattern that moves some parts of a value and leaves the rest to `_` / `..` (`let (a, _) = pair();`,
//! `let W { a, .. } = w;`, `match pair() { (a, _) => .. }`): the parts left are still owned, and are dropped at the end
//! of the statement (a temporary) or of the owner's scope (a local). zustc drops a local's (split_moved), not a temporary's, so those leak;
//! the counts differ from rustc's (and valgrind sees the Strings lost). No part is dropped twice.
//@ pattern: drops
//@ todo: the parts a pattern does not move out of a temporary are never dropped (a leak, no double free); a local's are
//@ expect: run
use std::cell::Cell;
struct D<'a>(u32, &'a Cell<u32>);
impl Drop for D<'_> {
    fn drop(&mut self) {
        self.1.set(self.1.get() + 1);
    }
}
struct W<'a> {
    a: D<'a>,
    b: D<'a>,
}
fn pair(n: &Cell<u32>) -> (D<'_>, D<'_>) {
    (D(1, n), D(2, n))
}
fn main() {
    let n = Cell::new(0);
    {
        let (a, _) = pair(&n);
        println!("let temp {} {}", a.0, n.get());
    }
    println!("{}", n.get());
    {
        let t = pair(&n);
        let (_, b) = t;
        println!("let local {} {}", b.0, n.get());
    }
    println!("{}", n.get());
    {
        let w = W { a: D(3, &n), b: D(4, &n) };
        let W { a, .. } = w;
        println!("struct {} {}", a.0, n.get());
    }
    println!("{}", n.get());
    match pair(&n) {
        (a, _) => println!("match {} {}", a.0, n.get()),
    }
    println!("{}", n.get());
    if let (_, b) = (String::from("s"), String::from("t")) {
        println!("if let {b}");
    }
    println!("{}", n.get());
}
