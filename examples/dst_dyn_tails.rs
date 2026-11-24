// Structs whose last field is a trait object (`Foo<dyn Tr>`): `&Foo<dyn Tr>` is a (data, vtable) pair; the tail is
// placed by the concrete type's alignment, which the vtable holds (with its size, for `size_of_val`).

use std::fmt::Debug;
trait Get { fn get(&self) -> u64; fn bump(&mut self); }
impl Get for u8 { fn get(&self) -> u64 { *self as u64 } fn bump(&mut self) { *self += 1; } }
impl Get for u64 { fn get(&self) -> u64 { *self } fn bump(&mut self) { *self += 10; } }
impl Get for (u16, u32) { fn get(&self) -> u64 { self.0 as u64 + self.1 as u64 } fn bump(&mut self) { self.1 += 100; } }
struct Foo<T: ?Sized> { tag: u8, inner: T }
struct Outer<T: ?Sized> { a: u16, f: Foo<T> }
fn show(x: &Foo<dyn Get>) -> u64 { x.tag as u64 * 1000 + x.inner.get() }
fn deep(x: &Outer<dyn Get>) -> u64 { x.a as u64 + x.f.tag as u64 + x.f.inner.get() }
fn dbg(x: &Foo<dyn Debug>) { println!("{} {:?}", x.tag, &x.inner); }
fn sizes() {
    use std::mem::{align_of_val, size_of_val};
    let s: &[u32] = &[1, 2, 3];
    println!("{} {}", size_of_val(s), align_of_val(s));
    println!("{}", size_of_val("hello"));
    let f: &Foo<[u16]> = &Foo { tag: 1, inner: [1, 2, 3] };
    println!("{} {}", size_of_val(f), align_of_val(f));
    let d: &dyn Get = &5u64;
    println!("{} {}", size_of_val(d), align_of_val(d));
    let g: &Foo<dyn Get> = &Foo { tag: 1, inner: 5u64 };
    println!("{} {}", size_of_val(g), align_of_val(g));
    let h: &Foo<dyn Get> = &Foo { tag: 1, inner: 5u8 };
    println!("{} {}", size_of_val(h), align_of_val(h));
}

// a Box<Foo<dyn Tr>> drops its tail through the vtable, and a Box<Foo<[T]>> each element
struct Loud(&'static str);
impl Drop for Loud {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
trait Named {
    fn name(&self) -> &'static str;
}
impl Named for Loud {
    fn name(&self) -> &'static str {
        self.0
    }
}

fn drops() {
    let b: Box<Foo<dyn Named>> = Box::new(Foo { tag: 1, inner: Loud("dyn tail") });
    println!("{} {}", b.tag, b.inner.name());
    let v: Box<Foo<[Loud]>> = Box::new(Foo { tag: 2, inner: [Loud("a"), Loud("b")] });
    println!("{} {}", v.tag, v.inner.len());
    drop(b);
    println!("after the dyn box");
}

fn main() {
    let a = Foo { tag: 1, inner: 7u8 };
    let b = Foo { tag: 2, inner: 1u64 << 40 };
    let c = Foo { tag: 3, inner: (5u16, 6u32) };
    let rs: [&Foo<dyn Get>; 3] = [&a, &b, &c];
    for r in rs.iter() { println!("{}", show(r)); }
    let mut m = Foo { tag: 4, inner: 9u64 };
    {
        let mm: &mut Foo<dyn Get> = &mut m;
        mm.inner.bump();
        mm.tag += 1;
    }
    println!("{} {}", m.tag, m.inner);
    let o = Outer { a: 100, f: Foo { tag: 9, inner: 3u64 } };
    println!("{}", deep(&o));
    let bx: Box<Foo<dyn Get>> = Box::new(Foo { tag: 7, inner: (1u16, 2u32) });
    println!("{}", show(&bx));
    dbg(&Foo { tag: 8, inner: "hi" });
    dbg(&Foo { tag: 9, inner: vec![1, 2] });
    match *rs[1] { Foo { tag, ref inner } => println!("{} {}", tag, inner.get()) }
    sizes();
    drops();
}
