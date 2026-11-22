struct Foo<T> { value: T }
fn bad() -> &'static u32 { let x = 5u32; let _f = Foo::<&'_ u32> { value: &1u32 }; &x }
fn main() { println!("{}", bad()); }
