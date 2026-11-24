// `(*self).name()` in a `&self` method of `impl<T: Name> Name for &mut T` is not a forward: `*self` is the `&mut T`,
// whose lookup takes the impl's own `name` before T's (rustc recurses). The bound is written in parentheses
#![allow(unconditional_recursion)]
trait Name {
    fn name(&self) -> u8;
}
struct S;
impl Name for S {
    fn name(&self) -> u8 {
        1
    }
}
impl<'a, T: ?Sized + Name> Name for &'a mut T {
    fn name(&self) -> u8 {
        (*self).name()
    }
}
fn show<N: (Name)>(n: N) -> u8 {
    n.name()
}
fn main() {
    let mut s = S;
    println!("{}", show(&mut s));
}
