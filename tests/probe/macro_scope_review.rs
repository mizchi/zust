// A macro call after `=` (or in an expression after it) in a module's items, a name two modules define beside a
// local of that name compared with `!=`, hygiene around a module a macro body defines, a definition inside
// `stringify!` (text, not a definition), a macro re-exported by `use`, a raw identifier, an
// associated type of `&T` beside `&mut T`'s, and impls for `&T` / `&mut T` / `Box<u8>` of one trait
#![allow(dead_code, unused_imports)]
macro_rules! value {
    () => {
        3
    };
}
mod m {
    pub const C: u8 = value!();
    pub const D: u8 = 1 + value!();
    pub const E: u8 = value!() * 2;
}
// (the same name in two modules, and a local of that name compared with `!=`)
mod p1 {
    macro_rules! k {
        () => {
            1
        };
    }
    pub fn f() -> u8 {
        k!()
    }
}
mod p2 {
    macro_rules! k {
        () => {
            2
        };
    }
    pub fn f() -> u8 {
        let k = 3;
        if k != 0 { k!() + k } else { 0 }
    }
}
macro_rules! h {
    ($e:expr) => {{
        mod helper {}
        let x = 1;
        let _ = x;
        $e
    }};
}
mod a {
    macro_rules! twice {
        ($e:expr) => {
            $e * 2
        };
    }
    pub(crate) use twice;
}
trait Mark {
    type Out;
}
impl<T> Mark for *const T {
    type Out = u32;
}
impl<'a, T> Mark for &'a T {
    type Out = u8;
}
impl<'a, T> Mark for &'a mut T {
    type Out = u16;
}
trait Plain {}
impl<'a, T> Plain for &'a T {}
impl<'a, T> Plain for &'a mut T {}
impl Plain for Box<u8> {}
fn size<M: Mark>(_m: M) -> usize {
    std::mem::size_of::<M::Out>()
}
fn main() {
    let x = 2;
    let _ = stringify!(macro_rules! value { () => { 9 } });
    println!("{} {} {}", m::C, h!(x), value!());
    println!("{}", a::twice!(5));
    println!("{} {} {} {}", m::D, m::E, p1::f(), p2::f());
    let r#let = 5;
    println!("{}", r#let);
    let mut y = 1i32;
    println!("{} {} {}", size(&y), size(&mut y), size(&y as *const i32));
}
