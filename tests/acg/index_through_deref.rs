// Indexing a struct whose `Deref` is to a slice (`type Target = [i32]`): `c[i]` is `c.deref()[i]`, and
// `deref_mut` for a write or a `&mut c[i]`, as rustc does. A struct's own `Index` comes first
use std::ops::{Deref, DerefMut, Index};
struct C { v: Vec<i32>, s: String }
impl Deref for C { type Target = [i32]; fn deref(&self) -> &[i32] { &self.v } }
impl DerefMut for C { fn deref_mut(&mut self) -> &mut [i32] { &mut self.v } }
struct D { v: Vec<i32> }
impl Deref for D { type Target = [i32]; fn deref(&self) -> &[i32] { &self.v } }
impl Index<usize> for D { type Output = i32; fn index(&self, i: usize) -> &i32 { &self.v[self.v.len() - 1 - i] } }
fn main() {
    let mut c = C { v: vec![1, 2, 3], s: String::new() };
    let a = &c[0];
    let b = &c[1];
    println!("{} {} {}", a, b, c[2]);
    c[1] = 20;
    let m = &mut c[2];
    *m += 10;
    println!("{:?} {} {}", &c[..], c.len(), c.s.len());
    let r = &mut c;
    r[0] = 7;
    println!("{}", r[0] + r[1]);
    let d = D { v: vec![1, 2, 3] };
    let first = &d[0];
    println!("{} {} {}", d[0], d.len(), first);
}
