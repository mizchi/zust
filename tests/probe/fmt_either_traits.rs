// either's `impl_fmt_traits!`: one generic impl per fmt trait, each calling `fmt::$trait::fmt(inner, f)` on numbers;
// Display's is the number's Display beside the prelude's `impl LowerHex for u32`
use std::fmt;
enum Either<L, R> { Left(L), Right(R) }
use Either::*;
macro_rules! impl_fmt_traits {
    ($($trait:ident),*) => { $(
        impl<L, R> fmt::$trait for Either<L, R> where L: fmt::$trait, R: fmt::$trait {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self { Left(inner) => fmt::$trait::fmt(inner, f), Right(inner) => fmt::$trait::fmt(inner, f) }
            }
        }
    )* }
}
impl_fmt_traits!(Binary, Display, LowerExp, LowerHex, Octal, UpperExp, UpperHex);
fn main() {
    let a: Either<u32, i64> = Left(49374);
    let b: Either<u32, i32> = Right(-3);
    println!("{:b} {} {:e} {:x} {:o} {:E} {:X}", a, a, a, a, a, a, a);
    println!("{} {:x} {:#x} {:b} {:e}", b, b, b, b, b);
}
