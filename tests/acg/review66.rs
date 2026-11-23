// Review of #66: a call through `T: A` (or `<S as A>::foo`) is A's even when S's `impl B` has a `foo`; a `&self`
// method beats a `&mut self` one of another trait (the first receiver candidate decides), a generic inherent method
// beats two traits' defaults, and `self` of one trait beats `&self` of another on a value but not on a reference;
// `<T as Add>::Output` of a number is the number, a program's own trait's comes from its impl; the ends of i128 and
// u128 are literals that fit
use std::ops::Add;

trait A {
    fn foo(&self) -> u8 {
        1
    }
}
trait B {
    fn foo(&self) -> u8;
}
struct S;
impl A for S {}
impl B for S {
    fn foo(&self) -> u8 {
        3
    }
}
fn through_a<T: A>(t: T) -> u8 {
    t.foo()
}
fn through_b<T: B>(t: &T) -> u8 {
    t.foo()
}

trait Shared {
    fn bar(&self) -> u8 {
        10
    }
}
trait Exclusive {
    fn bar(&mut self) -> u8 {
        20
    }
}
struct M;
impl Shared for M {}
impl Exclusive for M {}

struct W<T>(T);
impl<T> W<T> {
    fn baz(&self) -> u8 {
        30
    }
}
trait C {
    fn baz(&self) -> u8 {
        31
    }
}
trait D {
    fn baz(&self) -> u8 {
        32
    }
}
impl C for W<u8> {}
impl D for W<u8> {}

trait ByValue {
    fn qux(self) -> u8;
}
trait ByRef {
    fn qux(&self) -> u8;
}
#[derive(Clone, Copy)]
struct Q;
impl ByValue for Q {
    fn qux(self) -> u8 {
        40
    }
}
impl ByRef for Q {
    fn qux(&self) -> u8 {
        41
    }
}

trait Local {
    type Output;
}
impl Local for u8 {
    type Output = u16;
}

fn main() {
    println!("{} {} {}", through_a(S), <S as A>::foo(&S), through_b(&S));
    let mut m = M;
    println!("{} {}", m.bar(), Exclusive::bar(&mut m));
    println!("{} {} {}", W(1u8).baz(), C::baz(&W(1u8)), D::baz(&W(1u8)));
    let q = Q;
    println!("{} {} {}", q.qux(), (&q).qux(), (&&q).qux());
    let x: <i32 as Add>::Output = 3;
    let y: <u8 as Local>::Output = 300;
    let z: <&i32 as std::ops::Add<i32>>::Output = 4;
    println!("{x} {y} {z}");
    let lo = -170141183460469231731687303715884105728i128;
    let hi = 170141183460469231731687303715884105727i128;
    let top = 340282366920938463463374607431768211455u128;
    println!("{lo} {hi} {top}");
}
