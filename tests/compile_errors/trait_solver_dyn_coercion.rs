// A coercion to `dyn Trait<isize>` needs `Struct: Trait<isize>`; an impl of `Trait<&str>` is another
trait Trait<T> {
    fn f(&self, x: T);
}
struct Struct;
impl Trait<&'static str> for Struct {
    fn f(&self, _x: &'static str) {}
}
fn main() {
    let s: Box<dyn Trait<isize>> = Box::new(Struct);
    s.f(1);
}
