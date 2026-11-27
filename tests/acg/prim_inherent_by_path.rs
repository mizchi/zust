// A number type's inherent method by path inside a trait impl of the same names: `Self::is_nan(self)`,
// `f64::sqrt(x)`, `<u32>::wrapping_add(*self, *v)` are std's, not the impl's own method (num-traits' forward!,
// WrappingAdd). A trait method of no inherent name still goes to the trait
trait Fl: Copy {
    fn is_nan(self) -> bool;
    fn sqrt(self) -> Self;
    fn twice(self) -> Self;
}
impl Fl for f64 {
    fn is_nan(self) -> bool {
        Self::is_nan(self)
    }
    fn sqrt(self) -> f64 {
        f64::sqrt(self) + 0.5
    }
    fn twice(self) -> f64 {
        self * 2.0
    }
}
trait WrappingAdd: Sized {
    fn wrapping_add(&self, v: &Self) -> Self;
    fn count_ones(self) -> u32;
}
macro_rules! wrapping_impl {
    ($t:ty) => {
        impl WrappingAdd for $t {
            fn wrapping_add(&self, v: &Self) -> Self {
                <$t>::wrapping_add(*self, *v)
            }
            fn count_ones(self) -> u32 {
                Self::count_ones(self) + 100
            }
        }
    };
}
wrapping_impl!(u32);
wrapping_impl!(i8);
fn nan<T: Fl>(x: T) -> bool {
    x.is_nan()
}
fn wadd<T: WrappingAdd>(a: T, b: T) -> T {
    a.wrapping_add(&b)
}
fn main() {
    println!("{} {} {}", nan(f64::NAN), nan(1.0f64), Fl::sqrt(4.0f64));
    println!("{} {}", Fl::twice(1.5f64), <f64 as Fl>::is_nan(0.0));
    println!("{} {}", wadd(u32::MAX, 2), wadd(100i8, 100));
    println!("{} {}", WrappingAdd::count_ones(7u32), 7u32.count_ones());
}
