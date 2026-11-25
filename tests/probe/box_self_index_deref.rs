// A `self: Box<Self>` method called on a temporary Box, and a field read through a type's own Index and then its
// Deref (`v[0].f`)
use std::ops::{Deref, Index};
struct S;
impl S {
    fn f(self: Box<Self>) -> i32 {
        1
    }
    fn g(self: Box<Self>, k: i32) -> i32 {
        k
    }
}
struct D<'a>(&'a i32);
impl D<'_> {
    fn get(self: Box<Self>) -> i32 {
        *self.0
    }
}
fn mk() -> Box<S> {
    Box::new(S)
}
struct MyVec<T> {
    x: T,
}
impl<T> Index<usize> for MyVec<T> {
    type Output = T;
    fn index(&self, _: usize) -> &T {
        &self.x
    }
}
struct MyPtr<T> {
    x: T,
}
impl<T> Deref for MyPtr<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.x
    }
}
struct Foo {
    f: usize,
}
fn main() {
    let x = 5;
    println!("{} {} {} {}", Box::new(S).f(), mk().f(), Box::new(S).g(2), Box::new(D(&x)).get());
    let v = MyVec { x: MyPtr { x: Foo { f: 22 } } };
    let i = &v[0].f;
    println!("{} {}", i, v[0].f + 1);
}
