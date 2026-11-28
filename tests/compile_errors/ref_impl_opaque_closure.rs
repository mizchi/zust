// A closure's result that is a value of an opaque type (`impl Name`, here `&S`): rustc calls through its bound, the
// impl for `&T` ("ref S"). zustc cannot follow the value through the closure, so it reports the call rather than run
// S's own `name` (#144)
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
fn make<'a>(s: &'a S) -> impl Name + 'a {
    s
}
fn main() {
    let s = S;
    let f = || make(&s);
    println!("{}", f().name());
}
