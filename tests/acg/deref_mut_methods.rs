// A `&mut self` method of what a program's `DerefMut` reaches (`w.push(3)`, `w.bump()` through two derefs): autoderef
// takes `deref_mut` for it, once per link, and `deref` for a `&self` one, as rustc does
use std::ops::{Deref, DerefMut};
struct In { n: i32 }
impl In {
    fn bump(&mut self) { self.n += 1; }
    fn get(&self) -> i32 { self.n }
}
trait Tr { fn tr(&mut self); }
impl Tr for In { fn tr(&mut self) { self.n *= 2; } }
struct X(In);
impl Deref for X { type Target = In; fn deref(&self) -> &In { println!("X d"); &self.0 } }
impl DerefMut for X { fn deref_mut(&mut self) -> &mut In { println!("X dm"); &mut self.0 } }
struct W(X);
impl Deref for W { type Target = X; fn deref(&self) -> &X { println!("W d"); &self.0 } }
impl DerefMut for W { fn deref_mut(&mut self) -> &mut X { println!("W dm"); &mut self.0 } }
impl W {
    fn twice(&mut self) { self.bump(); self.bump(); }
}
struct G<T>(T);
impl<T> Deref for G<T> { type Target = T; fn deref(&self) -> &T { println!("G d"); &self.0 } }
impl<T> DerefMut for G<T> { fn deref_mut(&mut self) -> &mut T { println!("G dm"); &mut self.0 } }
fn main() {
    let mut w = W(X(In { n: 1 }));
    w.bump();
    w.twice();
    w.tr();
    let mut b = Box::new(W(X(In { n: 5 })));
    b.bump();
    let r = &mut w;
    r.bump();
    let x: &mut X = &mut *w;
    x.bump();
    let mut g = G(vec![1, 2]);
    g.push(3);
    let n = g.len();
    let last = g.last_mut().unwrap();
    *last = n;
    let mut gs = G(String::from("a"));
    gs.push_str("b");
    let mut gi = G(In { n: 0 });
    gi.bump();
    println!("{} {} {:?} {} {}", w.get(), b.get(), g.0, gs.0, gi.get());
}
