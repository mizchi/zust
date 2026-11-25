// Two impls that both apply to `MyType`: the blanket impl's where clause holds for it (E0119)
trait OtherTrait {}
trait MyTrait {}
impl<T: OtherTrait> MyTrait for T {}
struct MyType;
impl MyTrait for MyType {}
impl OtherTrait for MyType {}
fn main() {}
