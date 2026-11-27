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
}
