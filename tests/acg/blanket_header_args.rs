// A blanket impl whose header names a parameter inside the trait's argument (`impl<T, U> A<[U; 2]> for T`): U is what
// the header solves (u16 for `A<[u16; 2]>`), not the whole argument. And a method of an impl for `&mut W` is found
// by autoref before a `DerefMut` W's target is
use std::ops::{Deref, DerefMut};

trait A<X> {
    fn size(&self) -> usize;
}

impl<T, U> A<[U; 2]> for T {
    fn size(&self) -> usize {
        std::mem::size_of::<U>()
    }
}

fn f(x: &dyn A<[u16; 2]>) -> usize {
    x.size()
}

struct Inner(i32);

impl Inner {
    fn touch(&mut self) -> i32 {
        self.0 += 1;
        self.0
    }
}

struct W(Inner);

impl Deref for W {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl DerefMut for W {
    fn deref_mut(&mut self) -> &mut Inner {
        &mut self.0
    }
}

trait RefTouch {
    fn touch(&self) -> i32;
}

impl RefTouch for &mut W {
    fn touch(&self) -> i32 {
        100
    }
}

fn main() {
    let v = 5u8;
    let r = &v;
    println!("{}", f(&r));
    let mut w = W(Inner(0));
    println!("{}", w.touch());
    let rw = &mut w;
    println!("{}", rw.touch());
}
