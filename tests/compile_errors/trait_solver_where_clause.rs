// A blanket impl's where clause is part of the impl: `(): Foo` needs `(): Bar`, and there is no such impl
trait Foo {}
trait Bar {}
impl<T> Foo for T where T: Bar {}
impl Bar for u8 {}
fn needs_foo<T: Foo>(_: T) {}
fn main() {
    needs_foo(1u8);
    needs_foo(());
}
