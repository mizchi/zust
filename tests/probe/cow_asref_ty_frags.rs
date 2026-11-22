// Cow<str> without annotations, alloc:: paths, as_ref through bounds, `$($t:ty)*`, module aliases
extern crate alloc;

use alloc::borrow::Cow;
use std::fmt::Debug;

fn label(n: u32) -> Cow<'static, str> {
    if n == 0 { alloc::borrow::Cow::Borrowed("zero") } else { Cow::Owned(format!("n{n}")) }
}

fn shout(c: &Cow<'_, str>) -> String {
    match c {
        Cow::Borrowed(s) => s.to_uppercase(),
        Cow::Owned(s) => format!("{s}!"),
    }
}

fn first_len(s: &str) -> usize {
    s.split(',').next().map_or(0, |x| x.len())
}

fn lens<T: AsRef<str>>(xs: &[T]) -> Vec<usize> {
    xs.iter().map(|s| s.as_ref().len()).collect()
}

fn firsts<T: AsRef<str>>(xs: &[T]) -> Vec<char> {
    xs.iter().map(|s| s.as_ref().chars().next().unwrap_or('-')).collect()
}

fn hex<T: AsRef<[u8]>>(d: T) -> String {
    d.as_ref().chunks(2).map(|c| format!("{:02x}", c[0])).collect::<Vec<_>>().join(":")
}

macro_rules! id_ty {
    ($t:ty) => { $t };
}

macro_rules! count {
    ($($t:ty)*) => { 0 $(+ { let _x: Option<$t> = None; 1 })* };
}

macro_rules! defaults {
    ($($t:ty)*) => { ($(<$t as Default>::default(),)*) };
}

mod a {
    pub fn f() -> u32 { 1 }
    pub mod deep { pub const K: u32 = 10; }
}
mod b {
    pub fn f() -> u32 { 2 }
}
use a as x;
use b as y;

fn show<T: Debug>(t: T) -> String {
    format!("{t:?}")
}

fn main() {
    let c = label(0);
    let d = label(7);
    println!("{c} {d} {:?} {:?}", c, d);
    println!("{} {}", shout(&c), shout(&d));
    println!("{} {}", first_len(&c), c.len() + d.len());
    let e = Cow::Borrowed("a,bc");
    println!("{} {}", first_len(&e), matches!(e, Cow::Borrowed(_)));
    let words = ["one", "three"];
    println!("{:?} {:?}", lens(&words), firsts(&words));
    let refs: Vec<&&str> = words.iter().collect();
    println!("{:?}", lens(&refs));
    println!("{:?}", lens(&[String::from("four")]));
    println!("{} {}", hex("AB"), hex(vec![1u8, 255]));
    println!("{}", count!((u8, u16) u8 [u8; 2] &str Vec<Option<u8>> fn(u8) -> u8 id_ty!(u8) id_ty![u16] Box<dyn Fn(u8) -> u8>));
    println!("{}", show(defaults!(u8 String Vec<u8> (bool, char) Option<&'static str> id_ty!(u8) [u8; 2])));
    println!("{} {} {}", x::f(), y::f(), x::deep::K);
}
