// Review cases of #49: an inner use over an outer one, a qualified cast in a discriminant, a bool's `!` in a constant,
// a skipped use's type, Box::from of a Box, overflowing_shl's u32 amount
mod a { pub mod m { pub fn value() -> u8 { 1 } } }
mod b { pub mod m { pub fn value() -> u8 { 2 } } }
use crate::a::m;
fn first() -> u8 { use crate::b::m; m::value() }
fn second() -> u8 { m::value() }
#[repr(u16)]
#[derive(Clone, Copy)]
enum Q { A = (255u8 as core::primitive::u16) << 1 }
const NB: u8 = (!true) as u8;
const NB2: u8 = (!(1 < 2)) as u8 + 3;
const SK: i64 = {
    let x = 1 / ((!0 >> 7) + 1);
    let _y: u8 = if false { x } else { 0u8 };
    0
};
fn main() {
    println!("{} {}", first(), second());
    println!("{}", Q::A as u16);
    println!("{} {} {}", NB, NB2, SK);
    let b = Box::new(1u8);
    let c: Box<u8> = Box::from(b);
    let (r, o) = 7u8.overflowing_shl({ let s = 1; s });
    println!("{} {} {}", c, r, o);
}
