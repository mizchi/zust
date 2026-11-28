// Path calls of a trait with `impl<T: Tr> Tr for &T` (`Tr::m(&x)`, `T::m(&t)`, `<&S as Tr>::m`), a supertrait's bound,
// `&mut self` defaults with Self = `&S` (the probe's `&mut` autoref), and a closure's call (#144)
trait Tr {
    fn name(&self) -> String;
    fn twice(&self) -> String { format!("{}{}", self.name(), self.name()) }
    fn consume(self) -> String where Self: Sized { format!("c{}", self.name()) }
    fn bump(&mut self) -> String { format!("b{}", self.name()) }
}
struct S;
impl Tr for S { fn name(&self) -> String { "S".to_string() } fn twice(&self) -> String { "S2".to_string() } }
impl<T: Tr + ?Sized> Tr for &T {
    fn name(&self) -> String { format!("&{}", (**self).name()) }
    fn twice(&self) -> String { format!("&twice{}", T::twice(*self)) }
}
trait Sub: Tr { fn sub(&self) -> String { self.name() } }
impl Sub for S {}
impl<T: Sub + ?Sized> Sub for &T {}
fn via_sub<T: Sub>(t: T) -> String { format!("{} {}", t.name(), t.sub()) }
fn ufcs<T: Tr>(t: T) -> String { format!("{} {} {}", Tr::name(&t), T::name(&t), <T as Tr>::twice(&t)) }
struct Hold<T> { t: T }
impl<T: Tr> Hold<T> { fn go(&mut self) -> String { format!("{} {}", self.t.twice(), self.t.bump()) } }
fn main() {
    let s = S;
    println!("{}", Tr::name(&&s));
    println!("{}", Tr::name(&s));
    println!("{}", Tr::consume(&s));
    println!("{}", <&S as Tr>::twice(&&s));
    println!("{}", <&S>::twice(&&s));
    println!("{}", via_sub(&s));
    println!("{}", ufcs(&s));
    println!("{}", Hold { t: &s }.go());
    let mut r = &s;
    println!("{} {}", r.bump(), (&mut r).name());
    let v = vec![&s, &s];
    println!("{}", v.iter().map(|x| x.twice()).collect::<Vec<_>>().join(","));
}
