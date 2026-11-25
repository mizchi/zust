// No E0434 for what a fn item does not capture:
// - serde's tuple_impls: a macro writes the impls on one line, and each one's fn (through a second macro) has a nested
//   impl whose method binds `let T0` (a capitalized local, `#[allow(non_snake_case)]`); the other impls' `T0` locals
//   are on that line too
// - `stringify!(self)` in a fn nested in a method: text, not a use of `self`
use std::marker::PhantomData;
trait Make: Sized {
    fn make(v: &[u8]) -> Self;
}
trait Visit {
    type Value;
    fn visit(self, v: &[u8]) -> Self::Value;
}
#[derive(Debug)]
struct W1<T0>(T0);
#[derive(Debug)]
struct W2<T0, T1>(T0, T1);
macro_rules! tuple_impls {
    ($($w:ident => ($($n:tt $name:ident)+))+) => {
        $(
            impl<$($name: From<u8>),+> Make for $w<$($name),+> {
                tuple_impl_body!($w => ($($n $name)+));
            }
        )+
    };
}
macro_rules! tuple_impl_body {
    ($w:ident => ($($n:tt $name:ident)+)) => {
        fn make(v: &[u8]) -> Self {
            struct V<$($name,)+> {
                marker: PhantomData<($($name,)+)>,
            }
            impl<$($name: From<u8>),+> Visit for V<$($name,)+> {
                type Value = $w<$($name),+>;
                #[allow(non_snake_case)]
                fn visit(self, v: &[u8]) -> Self::Value {
                    $(
                        let $name = $name::from(v[$n]);
                    )+
                    $w($($name),+)
                }
            }
            V { marker: PhantomData }.visit(v)
        }
    };
}
tuple_impls! {
    W1 => (0 T0)
    W2 => (0 T0 1 T1)
}
struct S(u8);
impl S {
    fn get(&self) -> (u8, &'static str) {
        fn helper(x: u8) -> (u8, &'static str) {
            const WORD: &str = stringify!(self);
            (x + 1, WORD)
        }
        helper(self.0)
    }
}
fn main() {
    let a: W1<u16> = Make::make(&[1]);
    let b: W2<u16, u32> = Make::make(&[2, 3]);
    println!("{:?} {:?}", a, b);
    println!("{:?}", S(4).get());
}
