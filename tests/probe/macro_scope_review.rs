// A macro call after `=` in a module's items, hygiene around a module a macro body defines, a definition inside
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
    let r#let = 5;
    println!("{}", r#let);
    let mut y = 1i32;
    println!("{} {}", size(&y), size(&mut y));
}
