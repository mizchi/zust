//! A `let` initializer's temporaries are dropped at the end of the `let` statement (as rustc does), unless the value
//! bound may borrow from them. zustc kept them all to the end of the block, so a RefCell guard in
//! `let n = *c.borrow();` stayed alive and the next `borrow_mut` panicked in a correct program.
//@ pattern: drops (each value dropped once, and at the time rustc drops it)
//@ expect: run
struct D(u32);
impl Drop for D {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
impl D {
    fn get(&self) -> u32 {
        self.0
    }
}
fn mk(n: u32) -> D {
    D(n)
}
fn main() {
    {
        let x = mk(1).get();
        println!("after let x = {x}");
        mk(2).get();
        println!("after a statement");
        let y = mk(3).get() + mk(4).get();
        println!("after let y = {y}");
        let kept = mk(5);
        let r = &mk(6);
        println!("after the bindings {} {}", kept.0, r.0);
    }
    println!("end of the block");
}
