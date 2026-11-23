struct Foo<T> {
    value: T,
}
fn main() {
    let f = Foo::<u8> { value: 300u16 };
    let _ = f.value;
}
