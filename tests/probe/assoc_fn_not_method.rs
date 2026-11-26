// An associated function without `self` is no method candidate (#207): with a trait's `fn get(r: &[u8]) -> Self`
// implemented for Vec<T>, `v.get(i)` is still the slice's `get` (it was "`Vec<i32>::get` is an associated function,
// not a method"), also when the trait is in another module and not imported, in a generic fn, and inside std's own
// methods (`v.first()` calls `self.get(0)`). The function is still called by its path (`<Vec<u8> as Wire>::get(..)`,
// `Wire::get(..)`, `T::get(..)`), and a method of the same trait (`put(&self)`) by a method call. Also an
// inherent associated function beside a trait's method of the same name (`s.f()` is the trait's), and a type's
// associated function beside the method of what the type derefs to (`m.val()` is the target's): also a trait's default
// one (`d.val()`), and with a `DerefMut` to a `&mut self` method (`e.edit()` goes through the user's `deref_mut`). A
// type with an associated `partial_cmp` / `cmp` and a derived PartialOrd / Ord compares by its fields (`a.partial_cmp(&b)`,
// `a < b`). An associated function registered before inherent methods of the name in impls for two instances (`impl Mk2
// for V<bool> { fn f() }`, `V<u8>`'s `unsafe fn f(&self)`, `V<u16>`'s `fn f(&self)`): `a.f()` on a `V<u16>` is its own,
// safe one. A closure that calls a trait's default `&mut self` method on what it captures mutates it (`unused_mut` does
// not fire on `cnt`). Inherent methods of one name with different receivers in impls for two instances: what the
// receiver's instance has decides (`Tg<u16>`'s `edit(&mut self)` reached through a DerefMut beside `Wr`'s associated
// `edit()` goes through `deref_mut`; `Wc<u16>`'s `m(&self)` in a closure that is not `mut`). An inherent method beside
// a trait's `unsafe fn` and another trait's associated function of the name (`Sa(0).f()` is the inherent, safe one).

#![deny(unused_mut)]

use std::ops::{Deref, DerefMut};

mod helper {
    pub trait Decode: Sized {
        fn get(r: &[u8]) -> Self;
        fn get_mut(r: &mut [u8]) -> Self;
    }
    impl Decode for i32 {
        fn get(r: &[u8]) -> Self { r[0] as i32 }
        fn get_mut(r: &mut [u8]) -> Self { r[0] += 1; r[0] as i32 }
    }
    impl<T: Decode> Decode for Vec<T> {
        fn get(_r: &[u8]) -> Self { Vec::new() }
        fn get_mut(_r: &mut [u8]) -> Self { Vec::new() }
    }
}

trait Wire: Sized {
    fn get(r: &[u8]) -> Self;
    fn put(&self, out: &mut Vec<u8>);
}
impl Wire for u8 {
    fn get(r: &[u8]) -> Self { r[0] }
    fn put(&self, out: &mut Vec<u8>) { out.push(*self) }
}
impl<T: Wire> Wire for Vec<T> {
    fn get(r: &[u8]) -> Self { r.iter().map(|&b| T::get(&[b])).collect() }
    fn put(&self, out: &mut Vec<u8>) {
        out.push(self.len() as u8);
        for x in self {
            x.put(out)
        }
    }
}

// (an inherent associated function beside a trait's method of the same name)
struct S(i32);
impl S {
    fn f(x: &S) -> i32 { x.0 + 100 }
}
trait Tr {
    fn f(&self) -> i32;
}
impl Tr for S {
    fn f(&self) -> i32 { self.0 }
}

// (a type's associated function beside the method of what it derefs to)
struct Inner(i32);
impl Inner {
    fn val(&self) -> i32 { self.0 * 2 }
    fn edit(&mut self) { self.0 += 1 }
}
struct W(Inner);
impl Deref for W {
    type Target = Inner;
    fn deref(&self) -> &Inner { &self.0 }
}
trait Mk {
    fn val(x: i32) -> Self;
}
impl Mk for W {
    fn val(x: i32) -> Self { W(Inner(x)) }
}

// (a trait's default associated function beside the method of what the type derefs to)
struct D(Inner);
impl Deref for D {
    type Target = Inner;
    fn deref(&self) -> &Inner { &self.0 }
}
trait Dflt {
    fn val() -> i32 { 5 }
}
impl Dflt for D {}

// (an associated function beside the `&mut self` method of what the type derefs to with DerefMut)
struct E(Inner, u32);
impl Deref for E {
    type Target = Inner;
    fn deref(&self) -> &Inner { &self.0 }
}
impl DerefMut for E {
    fn deref_mut(&mut self) -> &mut Inner {
        self.1 += 1;
        &mut self.0
    }
}
impl E {
    fn edit() -> i32 { 9 }
}

// (an associated `partial_cmp` / `cmp` beside a derived PartialOrd / Ord)
#[derive(PartialEq, PartialOrd, Debug)]
struct P(i32, i32);
impl P {
    fn partial_cmp() -> i32 { 3 }
}
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Q(u8);
impl Q {
    fn cmp(x: i32) -> i32 { x }
}

// (an associated function first, then inherent methods of the name for two instances, one of them unsafe)
struct V<T>(T);
trait Mk2 {
    fn f() -> i32;
}
impl Mk2 for V<bool> {
    fn f() -> i32 { 1 }
}
impl V<u8> {
    unsafe fn f(&self) -> i32 { self.0 as i32 + 200 }
}
impl V<u16> {
    fn f(&self) -> i32 { self.0 as i32 + 300 }
}

// (a trait's default `&mut self` method called in a closure)
trait Bump {
    fn inc(&mut self);
    fn bump(&mut self) {
        self.inc();
        self.inc();
    }
}
#[derive(Debug)]
struct Cnt(Vec<i32>);
impl Bump for Cnt {
    fn inc(&mut self) { self.0.push(1) }
}

// (inherent methods with different receivers for two instances, the second reached through a DerefMut beside an
// associated function of the name)
struct Tg<T>(T, u32);
impl Tg<u8> {
    fn edit(&self) -> u32 { self.1 }
}
impl Tg<u16> {
    fn edit(&mut self) -> u32 {
        self.1 += 10;
        self.1
    }
}
struct Wr(Tg<u16>, u32);
impl Deref for Wr {
    type Target = Tg<u16>;
    fn deref(&self) -> &Tg<u16> { &self.0 }
}
impl DerefMut for Wr {
    fn deref_mut(&mut self) -> &mut Tg<u16> {
        self.1 += 1;
        &mut self.0
    }
}
impl Wr {
    fn edit() -> i32 { 9 }
}

// (the same in a closure: `Wc<u16>`'s `m` takes `&self`)
struct Wc<T>(T, u32);
impl Wc<u8> {
    fn m(&mut self) -> u32 {
        self.1 += 1;
        self.1
    }
}
impl Wc<u16> {
    fn m(&self) -> u32 { self.1 }
}

// (an inherent method beside a trait's `unsafe fn` and another trait's associated function of the name)
struct Sa(u8);
trait AssocF {
    fn f() -> u8;
}
impl AssocF for Sa {
    fn f() -> u8 { 1 }
}
trait OtherF {
    unsafe fn f(&self) -> u8;
}
impl OtherF for Sa {
    unsafe fn f(&self) -> u8 { 2 }
}
impl Sa {
    fn f(&self) -> u8 { self.0 + 30 }
}

fn total(v: &Vec<u8>) -> u32 {
    let mut t = 0;
    let mut i = 0;
    while let Some(&x) = v.get(i) {
        t += x as u32;
        i += 1;
    }
    t
}

fn second<T: Copy>(v: &Vec<T>) -> Option<T> {
    v.get(1).copied()
}

fn main() {
    // (the issue's program)
    let v = vec![1i32, 2, 3];
    let i: usize = std::env::args().count();
    match v.get(i) {
        Some(x) => println!("{x}"),
        None => println!("-1"),
    }
    let mut v = v;
    if let Some(x) = v.get_mut(0) {
        *x += 10;
    }
    println!("{:?} {:?} {:?} {:?} {:?}", v, v.get(2), v.first(), v.last(), second(&v));
    let mut raw = [7u8];
    let d: i32 = <i32 as helper::Decode>::get_mut(&mut raw);
    println!("{d} {:?}", raw);

    let mut w: Vec<u8> = <Vec<u8> as Wire>::get(&[3u8, 4, 5]);
    let w2: Vec<u8> = Wire::get(&[6u8]);
    println!("{:?} {:?}", w2, w2.get(0));
    println!("{:?} {:?} {:?}", w.get(1), w.first(), w.get(3));
    if let Some(x) = w.get_mut(0) {
        *x += 10;
    }
    println!("{:?} {} {:?}", w, total(&w), second(&w));
    let mut out = vec![];
    w.put(&mut out);
    println!("{:?}", out);
    let back = <Vec<u8> as Wire>::get(&out[1..]);
    println!("{:?} {:?}", back, back.get(9));

    let s = S(7);
    println!("{} {}", s.f(), S::f(&s));
    let m = W::val(21);
    println!("{}", m.val());
    let d = D(Inner(4));
    println!("{} {}", d.val(), <D as Dflt>::val());
    let mut e = E(Inner(1), 0);
    e.edit();
    e.edit();
    println!("{} {} {}", e.0 .0, e.1, E::edit());

    let (a, b) = (P(1, 2), P(1, 3));
    println!("{:?} {:?} {} {} {}", a.partial_cmp(&b), b.partial_cmp(&a), a < b, a >= b, P::partial_cmp());
    println!("{:?} {:?} {}", Q(4).cmp(&Q(2)), Q(1).max(Q(3)), Q::cmp(7));

    let (x, y) = (V(5u16), V(6u8));
    println!("{} {} {}", x.f(), unsafe { y.f() }, <V<bool> as Mk2>::f());
    let mut cnt = Cnt(vec![]);
    let mut bump = || cnt.bump();
    bump();
    bump();
    println!("{:?}", cnt);

    let mut wr = Wr(Tg(1u16, 0), 0);
    wr.edit();
    wr.edit();
    println!("{} {} {} {}", wr.0 .1, wr.1, Wr::edit(), Tg(1u8, 5).edit());
    let wc = Wc(1u16, 7);
    let f = || wc.m();
    let mut wc8 = Wc(1u8, 0);
    let mut g = || wc8.m();
    g();
    println!("{} {}", f(), g());
    println!("{} {} {}", Sa(0).f(), <Sa as AssocF>::f(), unsafe { OtherF::f(&Sa(0)) });
}
