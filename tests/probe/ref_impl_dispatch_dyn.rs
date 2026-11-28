// `impl<T: Tr> Tr for &T` inside closures of a generic fn, through `impl Tr`, and as trait objects made of `&S` (the
// vtable takes the impl's methods and the defaults it leaves out, with Self = `&S`, #144)
trait Tr {
    fn name(&self) -> String;
    fn twice(&self) -> String { format!("{}{}", self.name(), self.name()) }
}
struct S;
impl Tr for S { fn name(&self) -> String { "S".to_string() } fn twice(&self) -> String { "S2".to_string() } }
impl<T: Tr + ?Sized> Tr for &T { fn name(&self) -> String { format!("&{}", (**self).name()) } }
fn clo<T: Tr>(t: T) -> String { let f = |x: &T| x.twice(); f(&t) }
fn clo2<T: Tr>(t: T) -> String { let f = move || t.name(); f() }
fn imp(t: impl Tr) -> String { t.twice() }
fn dynr(d: &dyn Tr) -> String { format!("{} {}", d.name(), d.twice()) }
fn boxed<T: Tr + 'static>(t: T) -> Box<dyn Tr> { Box::new(t) }
fn opt<T: Tr>(o: Option<T>) -> String { o.map(|t| t.name()).unwrap_or_default() }
fn main() {
    let s = S;
    println!("{}", clo(&s));
    println!("{}", clo2(&s));
    println!("{}", imp(&s));
    let rs = &s;
    println!("{}", dynr(&rs));
    static SS: S = S;
    let b = boxed(&SS);
    println!("{} {}", b.name(), b.twice());
    println!("{}", opt(Some(&s)));
}
