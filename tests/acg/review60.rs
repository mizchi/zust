// From the review of #60: a literal whose type arguments name lifetimes keeps its borrowed temporaries for the let
// (as rustc extends them); a trait's default `clone` on a type that is not Clone is that method
struct Foo<T> {
    value: T,
}
struct S(u8);
trait Duplicate {
    fn clone(&self) -> S {
        S(7)
    }
}
impl Duplicate for S {}
fn main() {
    let f = Foo::<&'_ String> { value: &String::from("xyz") };
    println!("{}", f.value);
    let s = S(1);
    let t: S = s.clone();
    let r = &s;
    let u: S = r.clone();
    println!("{} {} {}", s.0, t.0, u.0);
}
