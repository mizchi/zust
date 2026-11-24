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
}
