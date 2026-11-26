// (an `Fn` closure that calls the `&self` method of a field and assigns to it: E0594 at the assignment; the root's
// type has an associated function and a trait default `&mut self` method of the name)
struct Inner(u8);
impl Inner {
    fn touch(&self) {}
}
struct W(Inner);
impl W {
    fn touch() {}
}
trait T {
    fn touch(&mut self) {}
}
impl T for W {}
fn takes_fn<F: Fn()>(f: F) {
    f()
}
fn main() {
    let mut w = W(Inner(0));
    takes_fn(|| {
        w.0.touch();
        w.0 .0 = 1;
    });
    W::touch();
}
