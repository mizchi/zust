// Methods of a trait the program declares at its root and implements for `()`: on a `()` receiver (`().b()`), through
// `&()` / `&mut ()`, a trait's default method, a generic trait's, and on a type parameter that is `()`
trait B {
    fn b(self) -> u8;
    fn r(&self) -> u8 {
        4
    }
    fn w(&mut self) -> u8 {
        6
    }
}
impl B for () {
    fn b(self) -> u8 {
        7
    }
}
trait T<A> {
    fn dummy(&self, _a: A) -> u8 {
        5
    }
}
impl<A> T<A> for () {}
struct D;
trait C {
    fn c(self) -> &'static str;
}
impl C for () {
    fn c(self) -> &'static str {
        "c"
    }
}
fn via(x: &()) -> u8 {
    x.b() + x.r()
}
fn generic<A: T<D>>(a: A) -> u8 {
    a.dummy(D)
}
fn generic_b<X: B + Copy>(x: X) -> u8 {
    x.r() + x.b()
}
// (two traits with a method of one name for `()`: a generic call takes the bound's (CodeRabbit on #224))
trait P1 {
    fn pm(&self) -> u8;
}
trait P2 {
    fn pm(&self) -> u8;
}
impl P1 for () {
    fn pm(&self) -> u8 {
        11
    }
}
impl P2 for () {
    fn pm(&self) -> u8 {
        12
    }
}
fn via_p1<X: P1>(x: X) -> u8 {
    x.pm()
}
fn via_p2<X: P2>(x: X) -> u8 {
    x.pm()
}
fn main() {
    let mut u = ();
    println!("{} {} {}", ().b(), u.r(), u.w());
    let r = &u;
    println!("{} {}", via(r), r.r());
    let mm = &mut u;
    println!("{}", mm.w());
    println!("{} {} {}", ().dummy(D), generic(()), generic_b(()));
    println!("{} {}", ().c(), B::b(()));
    println!("{} {}", via_p1(()), via_p2(()));
}
