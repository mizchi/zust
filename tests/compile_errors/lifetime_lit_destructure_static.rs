struct Foo<T> { value: T }
fn f() { let x = 5u32; let Foo { value } = Foo::<&'static u32> { value: &x }; let _ = value; }
fn main() { f(); }
