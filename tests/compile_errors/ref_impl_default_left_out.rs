// `impl<A: Access> Access for &mut A` that leaves out a default method of the trait: it would run with Self = &mut A,
// not A's override, which a call through the deref reaches. Not supported
trait Access {
    fn next(&mut self) -> Option<u8>;
    fn size_hint(&self) -> usize {
        0
    }
}
struct S(u8);
impl Access for S {
    fn next(&mut self) -> Option<u8> {
        Some(self.0)
    }
    fn size_hint(&self) -> usize {
        1
    }
}
impl<'a, A: Access> Access for &'a mut A {
    fn next(&mut self) -> Option<u8> {
        (**self).next()
    }
}
fn hint<A: Access>(a: A) -> usize {
    a.size_hint()
}
fn main() {
    let mut s = S(3);
    println!("{}", hint(&mut s));
}
