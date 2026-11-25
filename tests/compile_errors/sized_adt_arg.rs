struct Foo<T>(T);

fn main() {
    let _x: Option<&Foo<str>> = None;
}
