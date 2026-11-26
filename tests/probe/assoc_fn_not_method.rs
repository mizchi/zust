// An associated function without `self` is no method candidate (#207): with a trait's `fn get(r: &[u8]) -> Self`
// implemented for Vec<T>, `v.get(i)` is still the slice's `get` (it was "`Vec<i32>::get` is an associated function,
// not a method"), also when the trait is in another module and not imported, in a generic fn, and inside std's own
// methods (`v.first()` calls `self.get(0)`). The function is still called by its path (`<Vec<u8> as Wire>::get(..)`,
// `Wire::get(..)`, `T::get(..)`), and a method of the same trait (`put(&self)`) by a method call. Also an
// inherent associated function beside a trait's method of the same name (`s.f()` is the trait's), and a type's
// associated function beside the method of what the type derefs to (`m.val()` is the target's).

use std::ops::Deref;

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
}
