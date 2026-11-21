//! The temporaries of an `if let` / `let ... else` scrutinee are dropped before the else block (Rust 2024), and a
//! destructuring `let` drops its initializer's temporaries at its end, as a plain `let` does. zustc kept them to the
//! end of the statement / block, so a RefCell guard in the scrutinee was alive in the else block and a `borrow_mut`
//! there panicked in a correct program. The drops of `D` show the order rustc runs them in.
//@ pattern: drops (each value dropped once, and at the time rustc drops it)
//@ expect: run
use std::cell::RefCell;
struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn mk(n: u32) -> Option<D> {
    if n % 2 == 0 { Some(D(n)) } else { None }
}
fn first(c: &RefCell<Vec<i32>>) -> i32 {
    let Some(x) = c.borrow().first().copied() else {
        c.borrow_mut().push(7);
        return -1;
    };
    x
}
fn main() {
    let c: RefCell<Vec<i32>> = RefCell::new(vec![]);
    if let Some(x) = c.borrow().first() {
        println!("first {x}");
    } else {
        c.borrow_mut().push(1);
    }
    if let Some(_) = c.borrow().get(5) {
    } else if let Some(x) = c.borrow().first() {
        println!("then first {x}");
    } else {
        c.borrow_mut().push(2);
    }
    let e: RefCell<Vec<i32>> = RefCell::new(vec![]);
    println!("{} {}", first(&e), first(&e));
    let (n, _) = (c.borrow().len(), 0);
    c.borrow_mut().push(n as i32);
    println!("{:?} {:?}", c, e);
    for k in 0..3 {
        if let Some(d) = mk(k) {
            println!("some {}", d.0);
        } else {
            println!("none {k}");
        }
    }
    let v = if let Some(D(k)) = mk(4).as_ref() { *k } else { 0 };
    println!("v {v}");
}
