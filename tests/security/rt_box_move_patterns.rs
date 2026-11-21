//! Neighbors of rt_box_move_out: values moved out of a Box by patterns. Each value must be dropped once and each
//! Box freed once (valgrind sees double frees and leaks).
//@ pattern: drops
//@ todo(valgrind): the same leak through patterns (`let Pair { a, b } = *b`, `match *b`); no double free
//@ expect: run
use std::cell::Cell;
struct D<'a>(u32, &'a Cell<u32>);
impl Drop for D<'_> {
    fn drop(&mut self) {
        self.1.set(self.1.get() + 1);
    }
}
struct Pair<'a> {
    a: D<'a>,
    b: D<'a>,
}
fn main() {
    let n = Cell::new(0);
    {
        let b = Box::new(Pair { a: D(1, &n), b: D(2, &n) });
        let Pair { a, b } = *b;
        println!("{} {} {}", a.0, b.0, n.get());
    }
    println!("{}", n.get());
    {
        let b = Box::new((D(3, &n), String::from("s")));
        let (d, s) = *b;
        println!("{} {s} {}", d.0, n.get());
    }
    println!("{}", n.get());
    {
        let b: Box<Option<D>> = Box::new(Some(D(4, &n)));
        match *b {
            Some(d) => println!("some {}", d.0),
            None => println!("none"),
        }
    }
    println!("{}", n.get());
}
