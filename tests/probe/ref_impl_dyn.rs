// A trait object made of a reference with an impl for references to a type parameter (#144): `&r` with r: &S as a
// `&dyn Name` has the vtable of `&S`, the impl's methods (T = S) and the default it leaves out; `&s` has S's. A call
// on the object takes the object's method at the first step, not the impl's for `&dyn Name`
trait Name {
    fn name(&self) -> String;
    fn shout(&self) -> String {
        format!("{}!", self.name())
    }
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
fn main() {
    let s = S;
    let r = &s;
    let d: &dyn Name = &r;
    println!("{} {}", d.name(), d.shout());
    let e: &dyn Name = &s;
    println!("{} {}", e.name(), e.shout());
    let b: Box<dyn Name + '_> = Box::new(&s);
    println!("{} {}", b.name(), b.shout());
    let v: Vec<&dyn Name> = vec![&s, &r];
    for x in v {
        println!("{}", x.name());
    }
    println!("{}", (&d).name());
}
