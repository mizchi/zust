// (a method `where Self: Sized` is left out of the vtable: the trait stays dyn compatible)
trait Shape {
    fn new() -> Self
    where
        Self: Sized;
    fn area(&self) -> f64;
}
fn first(shapes: &[Box<dyn Shape>]) -> f64 {
    let n: u8 = shapes.len();
    n as f64
}
fn main() {}
