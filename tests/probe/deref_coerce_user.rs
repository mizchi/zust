// Deref coercion through a program's Deref: `&W` where a `&Target` is expected (an argument, a typed let, a
// return value), `&mut` through DerefMut, nested wrappers, and on to a String's str / a Vec's slice
use std::ops::{Deref, DerefMut};
struct Foo(u32);
struct W<T>(T);
impl<T> Deref for W<T> { type Target = T; fn deref(&self) -> &T { &self.0 } }
impl<T> DerefMut for W<T> { fn deref_mut(&mut self) -> &mut T { &mut self.0 } }
struct V(Vec<u8>);
impl Deref for V { type Target = Vec<u8>; fn deref(&self) -> &Vec<u8> { &self.0 } }
fn f(x: &Foo) -> u32 { x.0 }
fn g(x: &mut Foo) { x.0 += 1; }
fn s(x: &[u8]) -> usize { x.len() }
fn st(x: &str) -> usize { x.len() }
fn ret(w: &W<Foo>) -> &Foo { w }
fn main() {
    let mut w = W(Foo(7));
    println!("{}", f(&w));
    g(&mut w);
    println!("{}", f(&w));
    let r: &Foo = &w;
    println!("{}", r.0);
    let ww = W(W(Foo(9)));
    println!("{}", f(&ww));
    let v = V(vec![1, 2, 3]);
    println!("{}", s(&v));
    let ws = W(String::from("hello"));
    println!("{}", st(&ws));
    println!("{}", ret(&w).0);
    let wv = W(vec![4u8, 5]);
    println!("{} {}", s(&wv), wv.len());
}
