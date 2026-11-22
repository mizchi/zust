#[derive(Clone, Copy)]
struct Foo<T> { value: T, k: u8 }
fn main() { let base = Foo { value: 1u16, k: 2 }; let f = Foo::<u8> { k: 3, ..base }; let _ = f.k; }
