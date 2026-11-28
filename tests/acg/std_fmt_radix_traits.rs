// fmt::Binary / Octal / LowerHex / UpperHex / LowerExp / UpperExp as traits a program implements (either, tinyvec and
// uuid write `fmt::LowerHex::fmt(inner, f)`), std's impls for the numbers, `{:x}` / `{:b}` / `{:e}` .. of a program's
// type, negative numbers in a radix (two's complement), and a program's write!(f, "{}", ..) inside an impl, which
// formats with a spec of its own as std's does (not the outer `{:>6}` / `{:x}`)
use std::fmt;

struct W(u32);
impl fmt::LowerHex for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}
impl fmt::UpperHex for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "W({:X})", self.0)
    }
}
impl fmt::Binary for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // (a `{}` in a radix impl: decimal, as std's write! formats with a Formatter of its own)
        write!(f, "{} = ", self.0)?;
        fmt::Binary::fmt(&self.0, f)
    }
}
impl fmt::Octal for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Octal::fmt(&self.0, f)
    }
}
impl fmt::LowerExp for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::LowerExp::fmt(&(self.0 as f64), f)
    }
}
impl fmt::UpperExp for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::UpperExp::fmt(&self.0, f)
    }
}
impl fmt::Display for W {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

enum Either<L, R> {
    Left(L),
    Right(R),
}
impl<L: fmt::LowerHex, R: fmt::LowerHex> fmt::LowerHex for Either<L, R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Either::Left(l) => fmt::LowerHex::fmt(l, f),
            Either::Right(r) => fmt::LowerHex::fmt(r, f),
        }
    }
}

fn hex<T: fmt::LowerHex>(x: T) -> String {
    format!("{:#06x}", x)
}

fn main() {
    let w = W(255);
    println!("{:x} {:#x} {:08x} {:X} {:b} {:o} {:e} {:E}", w, w, w, w, w, w, w, w);
    println!("[{:>6}] [{:<6}] [{:x}]", w, w, &w);
    let l: Either<u8, W> = Either::Left(10);
    let r: Either<u8, W> = Either::Right(W(4096));
    println!("{:x} {:x}", l, r);
    println!("{} {} {}", hex(7u16), hex(W(171)), hex(-1i8));
    println!("{:e} {:E} {:e} {:e}", 1234.5f64, 0.00012f32, 1500u32, -7i64);
    let mut s = String::new();
    fmt::Write::write_fmt(&mut s, format_args!("{:b}|{:o}", 5u8, 64usize)).unwrap();
    println!("{}", s);

    // (negative numbers: their bits at the type's width)
    println!("{:x} {:#06x} {:b} {:o} {:X}", -1i8, -1i8, -2i16, -8i32, -255i64);
    println!("{:x} {:b} {:o} {:X} {:#x}", i128::MIN, -1i128, -9i128, -255i128, -2i128);
    println!("{} {} {:x} {:?} {:x?}", -5i32, i64::MIN, i64::MIN, -3i8, -3i8);
    // (a Display impl's own write!: not padded by the outer spec)
    println!("[{:>5}] [{:>5}] [{:<4}]", S(3), 3, S(42));
    println!("{:x}", M(255));
    nonzero_and_zero_pad();
}

// (std's NonZero types in a radix or an exponent, and `{:08e}`'s zeros after the sign, also
// through a LowerExp impl (#296's review))
fn nonzero_and_zero_pad() {
    use std::num::{NonZeroI8, NonZeroU32, NonZeroU8};
    println!("[{:08e}] [{:08e}] [{:08e}] [{:+09.2e}] [{:8e}] [{:<8e}]", -7i32, 1.5f64, E(-7), -1234.5f64, 42u8, 3u8);
    let n = NonZeroU8::new(255).unwrap();
    let m = NonZeroI8::new(-1).unwrap();
    let k = NonZeroU32::new(1500).unwrap();
    println!("{:x} {:#b} {:o} {:X} {} {:x?} {:?}", n, n, n, n, n, n, n);
    println!("{:x} {:b} {} {:e} {:E}", m, m, m, k, k);
    println!("{} {}", hex(255u8), hex(-1i8));
    // (NaN and infinity are padded with zeros too, the sign first; NaN takes no `+`)
    for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        println!("[{:08e}] [{:+08e}] [{:8e}] [{:08e}]", v, v, v, F(v));
    }
}

struct E(i32);
impl fmt::LowerExp for E {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::LowerExp::fmt(&self.0, f)
    }
}

struct S(u8);
impl fmt::Display for S {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// (the method call, with LowerHex imported and Display not)
mod m {
    use std::fmt::{self, LowerHex};
    pub struct M(pub u8);
    impl LowerHex for M {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            self.0.fmt(f)
        }
    }
}
use m::M;

struct F(f64);
impl fmt::LowerExp for F {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::LowerExp::fmt(&self.0, f)
    }
}
