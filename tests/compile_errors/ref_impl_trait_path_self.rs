// `Tr::m(self)` in an impl for `&T` takes Self = `&T`, this impl again (rustc recurses until its stack overflows): no forward (#144)
trait Tr { fn m(&self) -> u32; }
struct S;
impl Tr for S { fn m(&self) -> u32 { 2 } }
impl<T: Tr> Tr for &T { fn m(&self) -> u32 { Tr::m(self) } }
fn run<X: Tr>(x: X) -> u32 { x.m() }
fn main() { println!("{}", run(&S)); }
