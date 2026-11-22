struct Foo<T> { value: T }
fn main() { let f = Foo::<Missing> { value: 1u8 }; let _ = f.value; }
