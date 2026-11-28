// Methods whose result names Self (`by_ref(&self) -> &Self`, `me(self) -> Wrap<Self>`) of `impl<T: Tr> Tr for &mut T`
// through a bound, and `(&mut b).name()` whose probe takes the autoref to `&&mut B` (Self = `&mut B`, #144)
use std::fmt::Debug;
trait Tr {
    fn name(&self) -> String;
    fn by_ref(&self) -> &Self where Self: Sized { self }
    fn me(self) -> Wrap<Self> where Self: Sized { Wrap(self) }
}
#[derive(Debug)]
struct Wrap<T>(T);
#[derive(Debug)]
struct S(u8);
impl Tr for S {
    fn name(&self) -> String { format!("S{}", self.0) }
}
impl<T: Tr + ?Sized> Tr for &mut T {
    fn name(&self) -> String { format!("&mut {}", (**self).name()) }
}
fn f<T: Tr + Debug>(t: T) -> String {
    let r = t.by_ref();
    format!("{} {:?}", r.name(), t.me())
}
fn g<B: Tr>(mut b: B) -> String {
    let x = (&mut b).name();
    let y = b.name();
    format!("{x} | {y}")
}
fn main() {
    let mut s = S(1);
    println!("{}", f(&mut s));
    println!("{}", g(&mut s));
    println!("{}", g(S(2)));
}
