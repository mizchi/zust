// `impl<T: Name> Name for &T` whose method does more than forward to T's (#144): a call through a bound with N = &S
// runs the impl's method (T = S), and so does a call on `&&S`; a direct call on `&S` takes S's own `name(&self)` at
// the first step of the probe. Paths that name the trait take Self as the argument has it, with no autoderef
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
fn show<N: Name>(n: N) -> String {
    n.name()
}
fn by_ref<N: Name>(n: &N) -> String {
    n.name()
}
fn path_call<N: Name>(n: N) -> String {
    N::name(&n)
}
fn trait_call<N: Name>(n: N) -> String {
    Name::name(&n)
}
fn qual_call<N: Name>(n: N) -> String {
    <N as Name>::name(&n)
}
// (the bound written as a path)
fn show_path<N: crate::Name>(n: N) -> String {
    n.name()
}
struct W<T> {
    inner: T,
}
impl<T: Name> W<T> {
    fn go(&self) -> String {
        self.inner.name()
    }
}
// (no Name of its own: `(&&u).name()` is U's inherent method, through the derefs, not the impl's for `&U`)
struct U;
impl U {
    fn name(&self) -> String {
        "U inherent".into()
    }
}
fn main() {
    let s = S;
    println!("{}", show(&S));
    println!("{}", show(S));
    println!("{}", show(&&S));
    println!("{}", show_path(&S));
    println!("{}", (&s).name());
    let r = &s;
    println!("{}", r.name());
    println!("{}", s.name());
    println!("{}", (&&s).name());
    let rr = &r;
    println!("{}", rr.name());
    println!("{}", by_ref(&s));
    println!("{}", by_ref(&&s));
    println!("{}", path_call(&s));
    println!("{}", trait_call(&s));
    println!("{}", qual_call(&s));
    println!("{}", Name::name(&&s));
    println!("{}", Name::name(&s));
    println!("{}", <&S as Name>::name(&&s));
    println!("{}", W { inner: &s }.go());
    println!("{}", W { inner: S }.go());
    let u = U;
    println!("{} {}", (&&u).name(), (&&&u).name());
}
