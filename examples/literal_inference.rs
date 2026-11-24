// Numeric literals whose type comes from somewhere other than where they are written: an intrinsic's result, a
// turbofish, a float method's parameter through a generic call, and a negated float literal without a dot.
use std::mem;

struct Foo(u8, u16);

fn make<B: Default>() -> B {
    B::default()
}

fn id<T>(t: T) -> T {
    t
}

// (a default method's trait parameter from the impl for the receiver, and a generic associated type)
trait Positioned<S> {
    fn set_x(&mut self, s: S);
    fn x(&self) -> S;
}
trait Movable<S: std::ops::Add<Output = S>>: Positioned<S> {
    fn translate(&mut self, dx: S) {
        let x = self.x() + dx;
        self.set_x(x);
    }
}
struct Point {
    x: isize,
}
impl Positioned<isize> for Point {
    fn set_x(&mut self, x: isize) {
        self.x = x;
    }
    fn x(&self) -> isize {
        self.x
    }
}
impl Movable<isize> for Point {}

trait Build {
    type Output<O>;
    fn build<O>(self, input: O) -> Self::Output<O>;
}
struct IdentityBuild;
impl Build for IdentityBuild {
    type Output<O> = O;
    fn build<O>(self, input: O) -> Self::Output<O> {
        input
    }
}

fn main() {
    let n = 3 + mem::size_of::<Foo>();
    let k: usize = n;
    let b = make::<u16>();
    let c = 1 + 1;
    assert_eq!(b + 2, c);
    let x: f32 = 1.5;
    let m = x.min(id(9.0));
    let p = 25f32.powf(-2f32);
    let q = 23.2f64.max(-2f64);
    println!("{k} {b} {c} {m} {p} {q}");
    let mut pt = Point { x: 1 };
    pt.translate(3_000_000_000);
    let byte: u8 = IdentityBuild.build(200);
    println!("{} {}", pt.x, byte);
}
