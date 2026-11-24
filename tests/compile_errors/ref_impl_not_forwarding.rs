// A method of `impl<T: Name> Name for &T` that does more than forward to T's is not supported (its calls would reach
// T's method through the deref, skipping the body)
trait Name {
    fn name(&self) -> String;
}
struct S;
impl Name for S {
    fn name(&self) -> String {
        "S".to_string()
    }
}
impl<'a, T: ?Sized + Name> Name for &'a T {
    fn name(&self) -> String {
        format!("ref {}", (**self).name())
    }
}
fn show<N: Name>(n: N) -> String {
    n.name()
}
fn main() {
    println!("{}", show(&S));
}
