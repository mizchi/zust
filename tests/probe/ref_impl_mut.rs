// `impl<T: Counter> Counter for &mut T` whose methods do more than forward (#144): a call through a bound with
// B = &mut C runs them; on a `&mut C` receiver `bump(&mut self)` is C's at the first step, while `get(&self)` takes
// the impl's for `&mut C` by the autoref before C's (one step later). `(*self).get()` in the impl's `&self` method
// is the impl's own again (rustc recurses), not T's
use std::sync::atomic::{AtomicU32, Ordering};
static DEPTH: AtomicU32 = AtomicU32::new(0);
trait Counter {
    fn bump(&mut self) -> u32;
    fn get(&self) -> u32;
    fn again(&self) -> u32;
}
struct C(u32);
impl Counter for C {
    fn bump(&mut self) -> u32 {
        self.0 += 1;
        self.0
    }
    fn get(&self) -> u32 {
        self.0
    }
    fn again(&self) -> u32 {
        1
    }
}
impl<T: Counter + ?Sized> Counter for &mut T {
    fn bump(&mut self) -> u32 {
        (**self).bump();
        (**self).bump() + 100
    }
    fn get(&self) -> u32 {
        (**self).get() * 10
    }
    fn again(&self) -> u32 {
        if DEPTH.fetch_add(1, Ordering::Relaxed) < 3 { (*self).again() + 10 } else { (**self).again() }
    }
}
fn twice<B: Counter>(mut b: B) -> u32 {
    b.bump();
    b.bump()
}
fn peek<B: Counter>(b: &B) -> u32 {
    b.get()
}
fn through<B: Counter>(b: &mut B) -> u32 {
    b.bump();
    b.bump()
}
fn main() {
    let mut c = C(0);
    println!("{}", twice(&mut c));
    println!("{}", c.0);
    println!("{}", (&mut c).bump());
    println!("{}", peek(&&mut c));
    println!("{}", peek(&c));
    let mut r = &mut c;
    println!("{}", r.bump());
    println!("{}", r.get());
    println!("{}", (&*r).get());
    println!("{}", (&mut r).bump());
    println!("{}", (&r).get());
    println!("{}", through(&mut r));
    println!("{}", through(&mut c));
    println!("{}", c.get());
    println!("{}", (&mut c).again());
}
