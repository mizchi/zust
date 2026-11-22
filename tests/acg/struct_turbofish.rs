// A struct literal and struct patterns with type arguments on the path (`Foo::<i32> { .. }`, `S::<u8>(x)`)
#[derive(Debug)]
struct Foo<T> {
    value: T,
}
struct S<T>(T);
fn take(S::<u8>(x): S<u8>) -> u8 {
    x
}
fn keep<'a>(c: &'a u32) -> Foo<&'a u32> {
    Foo::<&'a u32> { value: c }
}
fn main() {
    let n = 7u32;
    println!("{}", keep(&n).value);
    let foo = Foo::<i32> { value: 10 };
    let Foo::<i32> { value } = foo;
    println!("{value}");
    let v = Foo::<Vec<u8>> { value: vec![] };
    println!("{:?} {}", v, take(S(4)));
}
