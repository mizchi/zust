// A trait's `fn bar() -> impl Shape`: each impl's method returns a type that is Shape
trait Shape {}
struct Circle;
impl Shape for Circle {}
trait Factory {
    fn make() -> impl Shape;
}
struct A;
struct B;
impl Factory for A {
    fn make() -> Circle {
        Circle
    }
}
impl Factory for B {
    fn make() -> u8 {
        0
    }
}
fn main() {}
