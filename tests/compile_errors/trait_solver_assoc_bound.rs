// The bound a trait puts on its associated type, with `Self::Ok` normalized: `<Foo2 as Bar2>::Ok` is u32, not ()
trait Bar {
    type Ok;
    type Sibling: Bar2<Ok = Self::Ok>;
}
trait Bar2 {
    type Ok;
}
struct Foo;
struct Foo2;
impl Bar for Foo {
    type Ok = ();
    type Sibling = Foo2;
}
impl Bar2 for Foo2 {
    type Ok = u32;
}
fn main() {}
