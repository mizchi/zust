// A call through the bound N: Name on a method's result (`n.unwrap()`, of type N = &S here): rustc runs
// `impl<T: Name> Name for &T`'s method ("ref S"), which zustc cannot tell from a call on a `&S` of its own for a
// receiver of this form, so it reports it rather than run S's (#144)
trait Name {
    fn name(&self) -> String;
}
struct S;
impl Name for S {
    fn name(&self) -> String {
        "S".into()
    }
}
impl<'a, T: ?Sized + Name> Name for &'a T {
    fn name(&self) -> String {
        format!("ref {}", (**self).name())
    }
}
fn opt<N: Name>(n: Option<N>) -> String {
    n.unwrap().name()
}
fn main() {
    println!("{}", opt(Some(&S)));
}
