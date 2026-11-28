// Beside a fn returning an opaque `impl Name` that is `&S` (#144): a `&S` from a field of a struct with no type
// parameters, or from a method or fn declared to return `&S`, is no opaque value, so `.name()` is S's own at the first
// step; the opaque value itself calls through its bound, the impl's for `&S`
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
struct W<'a> {
    inner: &'a S,
}
impl<'a> W<'a> {
    fn get(&self) -> &'a S {
        self.inner
    }
}
fn plain(s: &S) -> &S {
    s
}
fn main() {
    let s = S;
    let w = W { inner: &s };
    println!("{} {} {}", w.inner.name(), w.get().name(), plain(&s).name());
    println!("{}", make(&s).name());
}
