// From the review of #60: a literal whose type arguments name lifetimes keeps its borrowed temporaries for the let
// (as rustc extends them), and the statements after it their inference (`let n; n = 1u8;`); a trait's default `clone`
// on a type that is not Clone is that method, an associated `fn clone()` is not (`r.clone()` copies r); `_` beside a
// written argument
struct Foo<T> {
    value: T,
}
struct S(u8);
struct T;
trait Factory {
    fn clone() -> T {
        T
    }
}
impl Factory for T {}
struct Pair<A, B> {
    a: A,
    b: B,
}
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
    let g = Foo::<&'_ u32> { value: &1u32 };
    let n;
    n = 1u8;
    println!("{} {}", g.value, n);
    let x = T;
    let rx = &x;
    let _c: &T = rx.clone();
    let _d: T = <T as Factory>::clone();
    let p = Pair::<u8, _> { a: 3, b: true };
    println!("{} {}", p.a, p.b);
}
