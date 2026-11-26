// Which method `x.f()` is when an associated function of the name (no `self`) comes first and methods with `self` of
// the name take their receiver in different ways (#207): rustc's probe tries the receiver steps in order (the type as
// the receiver reaches it by value, then `&` and `&mut` of it) and prefers an inherent method only within one step. So
// `Rs(0).f()` with `ByVal::f(self)` beside the inherent `f(&self)` is ByVal's, `(&rs).f()` the inherent one; with
// `ByMut::g(&mut self)` beside the inherent `g(&self)`, `Rs(0).g()` is the inherent one and `(&mut rs).g()` ByMut's. Also
// through a Box, for a Copy type, and for impls of instances of a generic type. The program has no `mod` (the same
// cases with one are in assoc_fn_not_method).

struct Rs(u8);
trait RsAssoc {
    fn f();
    fn g();
}
impl RsAssoc for Rs {
    fn f() {}
    fn g() {}
}
trait ByVal {
    fn f(self) -> u8;
}
impl ByVal for Rs {
    fn f(self) -> u8 { 11 + self.0 }
}
impl Rs {
    fn f(&self) -> u8 { 21 + self.0 }
}
trait ByMut {
    fn g(&mut self) -> u8;
}
impl ByMut for Rs {
    fn g(&mut self) -> u8 {
        self.0 += 1;
        12
    }
}
impl Rs {
    fn g(&self) -> u8 { 22 + self.0 }
}

#[derive(Clone, Copy)]
struct Rk(u8);
impl RsAssoc for Rk {
    fn f() {}
    fn g() {}
}
impl ByVal for Rk {
    fn f(self) -> u8 { 13 + self.0 }
}
impl Rk {
    fn f(&self) -> u8 { 23 }
}

struct Rg<T>(T);
impl<T> RsAssoc for Rg<T> {
    fn f() {}
    fn g() {}
}
impl ByVal for Rg<u8> {
    fn f(self) -> u8 { 14 }
}
impl Rg<u16> {
    fn f(&self) -> u8 { 24 }
}

fn main() {
    let rs = Rs(0);
    let r = &rs;
    println!("{} {} {} {}", Rs(0).f(), (&rs).f(), r.f(), Box::new(Rs(0)).f());
    let mut rm = Rs(0);
    println!("{} {}", Rs(0).g(), (&mut rm).g());
    let m = &mut rm;
    println!("{} {}", m.g(), rm.0);
    let c = Rk(0);
    let cr = &c;
    println!("{} {}", cr.f(), c.f());
    println!("{} {}", Rg(1u16).f(), Rg(1u8).f());
    <Rs as RsAssoc>::f();
    <Rk as RsAssoc>::g();
}
