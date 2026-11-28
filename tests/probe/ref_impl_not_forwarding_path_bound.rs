// As ref_impl_not_forwarding, with the bound written as a path (`N: crate::Name`): a call through it runs the `&T`
// impl's method
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
fn show<N: crate::Name>(n: N) -> String {
    n.name()
}
fn main() {
    println!("{}", show(&S));
}
