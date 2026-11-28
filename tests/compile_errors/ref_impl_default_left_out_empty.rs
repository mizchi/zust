// An impl for `&T` with no members leaves out the trait's default method, which a call through a bound runs with Self = `&T` (rustc: 1; T's override: 2). Rejected, as before (#144)
trait Tr { fn value(&self) -> u32 { 1 } }
struct S;
impl Tr for S { fn value(&self) -> u32 { 2 } }
impl<T: Tr> Tr for &T {}
fn run<X: Tr>(x: X) -> u32 { x.value() }
fn main() { println!("{}", run(&S)); }
