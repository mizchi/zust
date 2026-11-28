// A `_` parameter given a const of its type's name gets the const's value, not the caller's (rustc: 1): no forward (#144)
#[derive(Clone, Copy)]
enum Mode { Fast, Slow }
#[allow(non_upper_case_globals)]
const Mode: Mode = Mode::Fast;
trait Tr { fn m(&self, mode: Mode) -> u32; }
struct S;
impl Tr for S { fn m(&self, mode: Mode) -> u32 { match mode { Mode::Fast => 1, Mode::Slow => 2 } } }
impl<T: Tr> Tr for &T { fn m(&self, _: Mode) -> u32 { (**self).m(Mode) } }
fn run<X: Tr>(x: X) -> u32 { x.m(Mode::Slow) }
fn main() { println!("{}", run(&S)); }
