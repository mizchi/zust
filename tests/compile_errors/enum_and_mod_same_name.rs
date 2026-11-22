enum Foo {
    A,
}
mod Foo {
    pub fn f() {}
}
fn main() {
    let _ = Foo::A;
}
