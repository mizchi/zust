// std's other fmt traits: Binary / LowerHex / UpperHex / Octal / LowerExp / UpperExp / Pointer, implemented by a
// type of the program's (either's `impl_fmt_traits!`), called through their path on numbers and pointers, and the
// format specs `{:b}` `{:x}` `{:X}` `{:o}` `{:e}` `{:E}` `{:p}` that reach them
use std::fmt;

enum Either<L, R> {
    Left(L),
    Right(R),
}
use Either::*;

macro_rules! impl_fmt_traits {
    ($($trait:ident),*) => {
        $(
            impl<L, R> fmt::$trait for Either<L, R>
            where
                L: fmt::$trait,
                R: fmt::$trait,
            {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    match self {
                        Left(inner) => fmt::$trait::fmt(inner, f),
                        Right(inner) => fmt::$trait::fmt(inner, f),
                    }
                }
            }
        )*
    }
}

impl_fmt_traits!(Binary, Display, LowerExp, LowerHex, Octal, Pointer, UpperExp, UpperHex);

// (a type whose Display and LowerHex differ: `{:x}` is LowerHex's)
struct Two(u32);
impl fmt::Display for Two {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "two")
    }
}
impl fmt::LowerHex for Two {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "hex:")?;
        fmt::LowerHex::fmt(&self.0, f)
    }
}

// (the traits' fns on numbers, with the formatter's width and flags)
struct Nums(u32, f64, i64);
impl fmt::Display for Nums {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)?;
        write!(f, " ")?;
        fmt::Binary::fmt(&self.0, f)?;
        write!(f, " ")?;
        fmt::Octal::fmt(&self.0, f)?;
        write!(f, " ")?;
        fmt::UpperHex::fmt(&self.0, f)?;
        write!(f, " ")?;
        fmt::LowerExp::fmt(&self.1, f)?;
        write!(f, " ")?;
        fmt::UpperExp::fmt(&self.2, f)
    }
}

fn main() {
    let value: Either<u32, i32> = Left(0xC0DE);
    println!("{value:b} {value} {value:e} {value:x} {value:o} {value:E} {value:X}");
    let neg: Either<u32, i32> = Right(-3);
    println!("{neg} {neg:x} {neg:#x} {neg:08b} {neg:e}");
    let fl: Either<f64, f32> = Left(1500.0);
    println!("{fl:e} {fl:E}");
    let pointer: Either<*const u32, &mut i32> = Left(std::ptr::null());
    println!("{pointer:p}");
    let x = 7u8;
    let r = &x;
    let p: Either<&u8, &u8> = Right(r);
    let b = Box::new(5i32);
    println!(
        "{} {} {}",
        format!("{p:p}") == format!("{:p}", r),
        format!("{:p}", r) == format!("{:?}", r as *const u8),
        format!("{:p}", b).starts_with("0x")
    );
    println!("{} {:x} [{:>8x}]", Two(255), Two(255), 255u32);
    println!("[{}]", Nums(255, 1234.5, -42));
    println!("[{:>4}]", Nums(10, 1.0, 7).to_string().split(' ').next().unwrap());
    more();
}

// (a `&&u8` through Pointer's path keeps its reference; a slice's / str's `{:p}` is `Pointer { addr, metadata }`;
// `{:0w}` / `{:#p}` zero padding after `0x` and the sign; an impl's own spec does not stay for the next piece)
struct H(u32);
impl fmt::LowerHex for H {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "H{:04x}", self.0)
    }
}
struct R<'a>(&'a &'a u8);
impl fmt::Pointer for R<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.0, f)
    }
}
struct Q<'a>(&'a &'a u8);
impl fmt::Pointer for Q<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(self.0, f)
    }
}
struct S<'a>(&'a str);
impl fmt::Pointer for S<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&self.0, f)
    }
}

fn more() {
    let x = 1u8;
    let r = &x;
    let rr = &r;
    let pointer = r;
    println!("named {}", format!("{pointer:p}") == format!("{:p}", r));
    println!("rr {} {}", format!("{:p}", R(rr)) == format!("{:p}", rr), format!("{:p}", Q(rr)) == format!("{:p}", r));
    let st = "hello";
    let sl: &[u8] = &[1, 2, 3];
    let bx: Box<[u8]> = vec![1u8, 2].into_boxed_slice();
    let m = |s: String, p: *const u8, n: usize| s == format!("Pointer {{ addr: {:p}, metadata: {} }}", p, n);
    println!("fat {} {} {}", m(format!("{:p}", st), st.as_ptr(), 5), m(format!("{:p}", sl), sl.as_ptr(), 3), m(format!("{:p}", bx), bx.as_ptr(), 2));
    println!("fat alt {}", format!("{:#p}", sl) == format!("Pointer {{\n    addr: {:#p},\n    metadata: 3,\n}}", sl.as_ptr()));
    println!("fat path {}", format!("{:p}", S(st)) == format!("{:p}", st));
    let z = format!("{:020p}", r);
    let al = format!("{:#p}", r);
    println!("zero {} {} alt {} {}", z.len(), z.starts_with("0x00"), al.len(), al.starts_with("0x0"));
    println!("[{:#24p}] [{:>12p}] [{:<12p}]", 0usize as *const u8, 0x10usize as *const u8, 0x20usize as *const u8);
    println!("[{:012e}] [{:+012e}] [{:012e}] [{:012E}] [{:+010e}]", 1234.5f64, 1234.5f64, -1234.5f64, 7u32, -42i32);
    println!("[{:>12e}] [{:*^12e}]", 1.5f64, 2.5f64);
    println!("[{:x}] [{:x}{:>5}] {:x}-{}", H(10), H(11), 1, H(3), 9);
    let wid = 6;
    println!("[{:>wid$x}] [{:_>8b}] [{: >6o}]", 255, 5, 8);
}
