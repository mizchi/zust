// `x.fmt(f)` of a number with LowerHex the one fmt trait imported: LowerHex's (`ff`), inside an impl of it too; a
// Box through Pointer's path stays its owner's; the trait's path written with spaces
use std::fmt::{self, LowerHex};
struct W(u8);
impl fmt::LowerHex for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { self.0.fmt(f) }
}
struct P(u32);
impl fmt::Display for P {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "<")?; fmt::Display::fmt(&self.0, f)?; write!(f, ">") }
}
struct B(Box<u8>);
impl fmt::Pointer for B {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::Pointer::fmt(&self.0, f) }
}
struct Sp(u8);
impl fmt::LowerHex for Sp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { core :: fmt :: LowerHex :: fmt(&self.0, f) }
}
fn main() {
    println!("{:x} {:#06x} {}", W(255), W(10), P(3));
    let b = Box::new(5u8);
    let s = format!("{:p}", B(b));
    let b2 = Box::new(6u8);
    let mut t = String::new();
    { use std::fmt::Write; let _ = write!(t, "{:p}", b2); }
    println!("{} {} {}", s.starts_with("0x"), t.starts_with("0x"), *b2);
    println!("{:x}", Sp(171));
}
