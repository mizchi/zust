// A macro body that passes `*self => $i` to another macro (num-traits' `float_to_int_unchecked!(*self => $i)`): the
// `self` before `=>` is no match arm's binding, so hygiene does not rename it (#254). A real arm's binding still is
// the body's own
trait ToInt {
    fn to_i8(&self) -> Option<i8>;
    fn to_u16(&self) -> Option<u16>;
}

macro_rules! unchecked {
    ($float:expr => $int:ty) => {
        $float as $int
    };
}

macro_rules! impl_to_int {
    ($f:ident : $(fn $method:ident -> $i:ident;)*) => {
        impl ToInt for $f {
            $(
                fn $method(&self) -> Option<$i> {
                    if *self >= $i::MIN as $f && *self <= $i::MAX as $f {
                        Some(unchecked!(*self => $i))
                    } else {
                        None
                    }
                }
            )*
        }
    };
}

impl_to_int!(f32: fn to_i8 -> i8; fn to_u16 -> u16;);
impl_to_int!(f64: fn to_i8 -> i8; fn to_u16 -> u16;);

macro_rules! pick {
    ($e:expr) => {
        match $e {
            0 => "zero",
            n if n < 0 => "negative",
            _ => "positive",
        }
    };
}

fn main() {
    let n = 7;
    println!("{:?} {:?} {:?}", 3.7f32.to_i8(), 300.0f32.to_i8(), (-1.0f64).to_u16());
    println!("{:?} {} {}", 65535.0f64.to_u16(), pick!(n - 9), pick!(n));
}
