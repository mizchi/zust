// a parameter of a type parameter with a fmt bound, `x: T` with `T: UpperHex`: `x.fmt(f)` is the bound's, though the
// program imports LowerHex too
use std::fmt::{self, LowerHex};
struct E<T>(T);
fn emit<T: fmt::UpperHex>(x: T, f: &mut fmt::Formatter<'_>) -> fmt::Result { x.fmt(f) }
impl<T: fmt::UpperHex + Copy> fmt::Display for E<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { emit(self.0, f) }
}
struct W(u8);
impl LowerHex for W { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { self.0.fmt(f) } }
fn main() { println!("{} {:x}", E(255u8), W(171)); }
