// A program's LowerHex / UpperHex / Octal / Binary impls with std's numbers under one imported fmt trait: `self.0.fmt(f)`
// under `use std::fmt::LowerHex;` is LowerHex's, an impl's nested `write!(f, "{}", ..)` starts from no spec, and
// radix specs with a named width, a custom or `:` fill; exponents zero-padded, infinity too
use std::fmt::{self, LowerHex};
struct W(u8);
impl fmt::LowerHex for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { self.0.fmt(f) }
}
struct H(u32);
impl fmt::UpperHex for H {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "H{:04X}", self.0) }
}
struct N(u32);
impl fmt::Octal for N {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "{}", self.0) }
}
struct P(i32);
impl fmt::Binary for P {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::Binary::fmt(&self.0, f) }
}
fn main() {
    println!("{:x} {:#06x}", W(255), W(10));
    println!("[{:X}] [{:X}{:>5}] {:X}-{}", H(10), H(11), 1, H(3), 9);
    println!("[{:o}] [{:>6o}]", N(255), N(9));
    println!("[{:b}] [{:#010b}]", P(-3), P(5));
    let x = 255u8; let r = &x;
    let mut s = String::new();
    { use std::fmt::Write; let _ = write!(s, "{:x}", r); }
    println!("{} {:x} {:X} {:o} {:b} {:e} {:E}", s, -1i8, 255u16, 8u8, 5u8, 1234.5f64, 7u32);
    println!("[{:012e}] [{:+012e}] [{:012e}] [{:+010e}] [{:08e}]", 1234.5f64, 1234.5f64, -1234.5f64, -42i32, f64::INFINITY);
    let wid = 6;
    println!("[{:>wid$x}] [{:_>8b}] [{: >6o}] [{::>8x}] [{:w$x}]", 255, 5, 8, -3i8, -3i8, w = 4);
}
