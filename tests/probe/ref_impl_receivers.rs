// Calls through a bound whose receiver is not a parameter of the type parameter itself (#144): a field of the type
// parameter (`self.inner` of `Take<T>` with T = &mut X), a where clause, a supertrait's method, a closure, a nested
// generic fn, `&mut self` methods on a by-value `mut b: B` and on `b: &mut B`, a default method's `self`, and `let`
// bindings that copy the value. Each runs the impl for `&mut T` where rustc does
trait Buf {
    fn remaining(&self) -> usize;
    fn advance(&mut self, n: usize);
    fn has_remaining(&self) -> bool {
        self.remaining() > 1040
    }
}
struct Bytes(usize);
impl Buf for Bytes {
    fn remaining(&self) -> usize {
        self.0
    }
    fn advance(&mut self, n: usize) {
        self.0 -= n;
    }
}
impl<T: Buf + ?Sized> Buf for &mut T {
    fn remaining(&self) -> usize {
        (**self).remaining() + 1000
    }
    fn advance(&mut self, n: usize) {
        (**self).advance(n * 2)
    }
}
struct Take<T> {
    inner: T,
    limit: usize,
}
impl<T: Buf> Buf for Take<T> {
    fn remaining(&self) -> usize {
        self.inner.remaining().min(self.limit)
    }
    fn advance(&mut self, n: usize) {
        self.inner.advance(n);
        self.limit -= n;
    }
}
struct Wrap<T> {
    inner: T,
}
impl<T> Wrap<T>
where
    T: Buf,
{
    fn rem(&self) -> usize {
        self.inner.remaining()
    }
    fn rem_ref(&self) -> usize {
        let r = &self.inner;
        r.remaining()
    }
}
trait Peek: Buf {}
impl<T: Buf> Peek for T {}
fn via_super<L: Peek>(l: L) -> usize {
    l.remaining()
}
fn via_closure<B: Buf>(b: B) -> usize {
    let f = || b.remaining();
    f()
}
fn nested<B: Buf>(b: B) -> usize {
    inner(&b)
}
fn inner<C: Buf>(c: &C) -> usize {
    c.remaining()
}
fn adv<B: Buf>(mut b: B) -> usize {
    b.advance(1);
    b.advance(1);
    b.remaining()
}
fn adv_ref<B: Buf>(b: &mut B) -> usize {
    b.advance(1);
    b.remaining()
}
fn copied<B: Buf>(b: B) -> usize {
    let mut c = b;
    c.advance(1);
    let d: B = c;
    d.remaining()
}
fn main() {
    let mut x = Bytes(50);
    let t = Take { inner: &mut x, limit: 2000 };
    println!("{}", t.remaining());
    let mut t = t;
    t.advance(3);
    println!("{} {}", t.remaining(), t.limit);
    println!("{}", x.0);
    let w = Wrap { inner: &mut x };
    println!("{} {}", w.rem(), w.rem_ref());
    println!("{} {}", (&mut x).has_remaining(), x.has_remaining());
    println!("{}", via_super(&mut x));
    println!("{}", via_closure(&mut x));
    println!("{}", nested(&mut x));
    println!("{}", adv(&mut x));
    println!("{}", x.0);
    let mut r = &mut x;
    println!("{}", adv_ref(&mut r));
    println!("{}", copied(&mut x));
    println!("{}", x.0);
}
