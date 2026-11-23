// Second review of #66: an impl for one instance of a struct (`impl A for W<u8>`) gives its default to that instance
// only, so `W(0u8).foo()` and `W(0u16).foo()` are not ambiguous; `s.foo::<u8>()` keeps the trait the first receiver
// candidate picks (A's `&self` over B's `&mut self`); `A::foo(&w)` uses the impl for w's own instance
struct W<T>(T);
trait A {
    fn foo(&self) -> u8 {
        1
    }
}
trait B {
    fn foo(&self) -> u8 {
        2
    }
}
impl A for W<u8> {}
impl B for W<u16> {}

trait G {
    fn bar<T>(&self) -> u8 {
        10
    }
}
trait H {
    fn bar<T>(&mut self) -> u8;
}
struct S;
impl G for S {}
impl H for S {
    fn bar<T>(&mut self) -> u8 {
        20
    }
}

trait Q {
    fn qux<X>(&self) -> u8 {
        30
    }
}
struct V<T>(T);
impl Q for V<u8> {
    fn qux<X>(&self) -> u8 {
        31
    }
}
impl Q for V<u16> {}

fn main() {
    println!("{} {}", W(0u8).foo(), W(0u16).foo());
    let mut s = S;
    println!("{} {}", s.bar::<u8>(), H::bar::<u8>(&mut s));
    println!("{} {}", Q::qux::<()>(&V(0u16)), Q::qux::<()>(&V(0u8)));
}
