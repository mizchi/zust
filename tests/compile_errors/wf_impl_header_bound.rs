trait Bound {}
struct Foo<T: Bound>(T);
trait Trait2 {}
impl<T> Trait2 for Foo<T> {}
fn main() {}
