//! Moving a value out of a `Box` (`let x = *b;`) drops the value once (with its new owner) and
//! frees the Box's block once. (A leak is not unsafe, but valgrind counts it: the same paths that
//! leak here would double free if they got the count wrong the other way.)
//@ expect: run
use std::cell::Cell;
struct D<'a>(u32, &'a Cell<u32>);
impl Drop for D<'_> {
    fn drop(&mut self) {
        self.1.set(self.1.get() + 1);
    }
}
fn main() {
    let drops = Cell::new(0);
    {
        let b = Box::new(D(1, &drops));
        let o = *b;
        println!("{} {}", o.0, drops.get());
    }
    println!("{}", drops.get());
    let b = Box::new(String::from("moved out"));
    let s = *b;
    println!("{s}");
}
