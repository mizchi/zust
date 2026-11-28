// A value of an opaque type and a blanket impl's methods with an impl for references to a type parameter (#144).
// `make(&s)` of `-> impl Name` calls through the opaque type's bound: `<&S as Name>::name`, the impl's. A blanket impl
// `impl<T: Name> Loud for T` runs with Self = &S where rustc's probe takes a reference: `(&&s).loud()` by value,
// `(&mut s).loud()` by the autoref, a value of N = &S through a bound; `(&s).loud()` and `s.loud()` with Self = S
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
impl<'a, T: ?Sized + Name> Name for &'a mut T {
    fn name(&self) -> String {
        format!("mut {}", (**self).name())
    }
}
fn make<'a>(s: &'a S) -> impl Name + 'a {
    s
}
struct Maker;
impl Maker {
    fn make<'a>(&self, s: &'a S) -> impl Name + 'a {
        s
    }
}
trait Loud: Name {
    fn loud(&self) -> String {
        self.name().to_uppercase()
    }
}
impl<T: Name + ?Sized> Loud for T {}
trait Take {
    fn take(self) -> String;
}
impl<T: Name> Take for T {
    fn take(self) -> String {
        format!("take {}", self.name())
    }
}
fn loud_of<N: Name>(n: N) -> String {
    n.loud()
}
fn main() {
    let s = S;
    println!("{}", make(&s).name());
    let m = make(&s);
    println!("{} {}", m.name(), (&m).name());
    println!("{}", Maker.make(&s).name());
    println!("{} {}", (&&s).loud(), (&s).loud());
    println!("{}", s.loud());
    let mut t = S;
    println!("{}", (&mut t).loud());
    println!("{}", loud_of(&s));
    println!("{}", loud_of(S));
    println!("{} {}", (&s).take(), S.take());
    println!("{}", Loud::loud(&&s));
    println!("{}", make(&s).loud());
}
