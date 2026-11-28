// thiserror's `impl<T: Display> AsDisplay for &T` (#143): T is bounded by another trait, so X has no AsDisplay of its
// own; `(&x).as_display()` takes the impl's by the autoref, and so does a call through a bound
use std::fmt::Display;
trait AsDisplay<'a> {
    type Target: Display;
    fn as_display(&'a self) -> Self::Target;
}
impl<'a, T: Display + ?Sized + 'a> AsDisplay<'a> for &T {
    type Target = &'a T;
    fn as_display(&'a self) -> &'a T {
        *self
    }
}
struct Field {
    name: String,
    n: u32,
}
fn shown<'a, D: AsDisplay<'a>>(d: &'a D) -> String {
    format!("[{}]", d.as_display())
}
fn main() {
    let s = String::from("hi");
    println!("{}", (&s).as_display());
    let f = Field { name: "x".to_string(), n: 7 };
    println!("{} = {}", (&f.name).as_display(), (&f.n).as_display());
    let r = &f.n;
    println!("{}", r.as_display());
    println!("{}", shown(&&s));
    println!("{}", (&"str").as_display());
}
