// A blanket impl's method for a reference made a trait object (`impl<A: Display> Foo for A`, `Box::new(v)` with v a
// `&i32` as a `Box<dyn Foo>`): the vtable takes the blanket impl's method, as rustc does. It was "`Foo::get` has no
// implementation for `&i32`"
use std::fmt::Display;
trait Foo { fn get(&self) -> String; }
impl<A: Display> Foo for A {
    fn get(&self) -> String { format!("<{}>", self) }
}
fn rep<'a, A: Foo + 'a>(v: A) -> Box<dyn Foo + 'a> {
    Box::new(v)
}
fn main() {
    let x = 3;
    let s = String::from("s");
    let v: Vec<Box<dyn Foo>> = vec![rep(&x), rep(&s), rep(4u8), Box::new("lit")];
    for b in &v { println!("{}", b.get()); }
}
