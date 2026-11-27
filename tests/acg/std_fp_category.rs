// std::num::FpCategory and f32 / f64's classify / is_subnormal (the crate survey's #250 `FpCategory`: num-traits,
// num-integer, num-bigint), through a trait that forwards to the inherent methods as num-traits' does
use std::num::FpCategory;

trait FloatCore: Copy {
    fn classify(self) -> FpCategory;
    fn is_subnormal(self) -> bool;
    fn is_normal(self) -> bool {
        self.classify() == FpCategory::Normal
    }
}

macro_rules! forward {
    ($( Self :: $method:ident ( self ) -> $ret:ty ; )*) => {$(
        fn $method(self) -> $ret {
            Self::$method(self)
        }
    )*};
}

impl FloatCore for f32 {
    forward! {
        Self::classify(self) -> FpCategory;
        Self::is_subnormal(self) -> bool;
    }
}

impl FloatCore for f64 {
    forward! {
        Self::classify(self) -> FpCategory;
        Self::is_subnormal(self) -> bool;
    }
}

fn check<T: FloatCore + std::fmt::Debug>(x: T, c: FpCategory) {
    println!("{:?}: {:?} {} {} {}", x, x.classify(), x.classify() == c, x.is_subnormal(), x.is_normal());
}

fn main() {
    check(f32::INFINITY, FpCategory::Infinite);
    check(f32::MAX, FpCategory::Normal);
    check(f32::MIN_POSITIVE / 2.0, FpCategory::Subnormal);
    check(-0.0f32, FpCategory::Zero);
    check(f32::NAN, FpCategory::Nan);
    check(f64::NAN, FpCategory::Nan);
    check(f64::MIN_POSITIVE, FpCategory::Normal);
    check(f64::MIN_POSITIVE / 2.0, FpCategory::Subnormal);
    check(0.0f64, FpCategory::Zero);
    check(f64::NEG_INFINITY, FpCategory::Infinite);
    let v: Vec<FpCategory> = [1.5f64, 1e-310, 0.0].iter().map(|x| x.classify()).collect();
    println!("{:?} {}", v, 1e-310f64.is_subnormal());
    match (2.0f32).classify() {
        FpCategory::Normal => println!("normal"),
        c => println!("{:?}", c),
    }
}
