// `Tr::m(self)` in an impl for `&T` takes Self = `&T`, this impl again, as rustc's does (#144). Bounded here by a
// counter: without one rustc recurses until its stack overflows
use std::sync::atomic::{AtomicU32, Ordering};
static DEPTH: AtomicU32 = AtomicU32::new(0);
trait Tr { fn m(&self) -> u32; }
struct S;
impl Tr for S { fn m(&self) -> u32 { 2 } }
impl<T: Tr> Tr for &T {
    fn m(&self) -> u32 { if DEPTH.fetch_add(1, Ordering::Relaxed) < 3 { Tr::m(self) + 10 } else { (**self).m() } }
}
fn run<X: Tr>(x: X) -> u32 { x.m() }
fn main() { println!("{}", run(&S)); }
