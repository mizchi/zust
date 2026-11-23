// An impl of a trait with Send among its supertraits needs its self type to be Send: Rc<i8> is not
trait Foo: Send {}
struct Holder<T>(T);
impl Foo for Holder<std::rc::Rc<i8>> {}
fn main() {}
