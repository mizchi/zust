// Calls whose Self rustc's method probe makes `&S` beside `impl<T: Tr> Tr for &T` (#144): through a bound `T: Tr`
// (a parameter, `&T`, a field of a generic struct), a left-out default (`twice`, `consume(self)`) run with Self = &S,
// and direct calls on `&S` / `&&S`, the qualified path, a `self` method on `&S`
trait Tr {
    fn name(&self) -> String;
    fn twice(&self) -> String { format!("{}{}", self.name(), self.name()) }
    fn consume(self) -> String where Self: Sized { format!("c{}", self.name()) }
}
struct S;
impl Tr for S {
    fn name(&self) -> String { "S".to_string() }
    fn twice(&self) -> String { "S-override".to_string() }
}
impl<T: Tr + ?Sized> Tr for &T {
    fn name(&self) -> String { format!("&{}", (**self).name()) }
}
fn call<T: Tr>(t: T) -> String { t.name() }
fn call2<T: Tr>(t: T) -> String { t.twice() }
fn call3<T: Tr>(t: &T) -> String { t.name() }
fn call4<T: Tr>(t: T) -> String { t.consume() }
struct W<T> { t: T }
impl<T: Tr> W<T> { fn go(&self) -> String { self.t.name() } }
fn main() {
    let s = S;
    println!("{}", call(&s));
    println!("{}", call2(&s));
    println!("{}", call3(&&s));
    println!("{}", call4(&s));
    println!("{}", W { t: &s }.go());
    println!("{}", (&s).name());
    println!("{}", (&&s).name());
    println!("{}", <&S as Tr>::name(&&s));
    println!("{}", (&s).consume());
}
