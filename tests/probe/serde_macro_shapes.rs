// What serde_core's macros and items need: a `::` separator, a lifetime as one tt, the same macro name in two
// modules, a module a macro body defines (no hygiene renaming in it), a where clause with `Item = T` in a macro's
// items, `type Ok = Ok;`, a unit struct matched in a module, impls for `&T` / `&mut T` beside one for a struct, and
// NonZero / Wrapping / Saturating
#![allow(dead_code)]
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::{NonZeroI8, NonZeroU32, NonZeroUsize, Saturating, Wrapping};

macro_rules! declare_error_trait {
    (Error: Sized $(+ $($supertrait:ident)::+)*) => {
        pub trait Error: Sized $(+ $($supertrait)::+)* {
            fn code(&self) -> u8 {
                1
            }
        }
    };
}
declare_error_trait!(Error: Sized + Debug + std::fmt::Display);

#[derive(Debug)]
struct E;
impl std::fmt::Display for E {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "E")
    }
}
impl Error for E {}

macro_rules! impl_copy_clone {
    ($ty:ident $(<$lifetime:tt>)*) => {
        impl<$($lifetime,)* X> Copy for $ty<$($lifetime,)* X> {}
        impl<$($lifetime,)* X> Clone for $ty<$($lifetime,)* X> {
            fn clone(&self) -> Self {
                *self
            }
        }
    };
}
struct U<X> {
    m: PhantomData<X>,
}
struct Borrowed<'de, X> {
    v: &'de str,
    m: PhantomData<X>,
}
impl_copy_clone!(U);
impl_copy_clone!(Borrowed<'de>);

mod de {
    macro_rules! seq_impl {
        ($ty:ident) => {
            pub fn $ty() -> &'static str {
                "de"
            }
        };
    }
    seq_impl!(vec);
    pub mod ignored_any {
        #[derive(Clone, Copy)]
        pub struct IgnoredAny;
        pub fn count(v: Vec<Option<(IgnoredAny, IgnoredAny)>>) -> usize {
            let mut n = 0;
            let mut it = v.into_iter();
            while let Some(Some((IgnoredAny, IgnoredAny))) = it.next() {
                n += 1;
            }
            n
        }
    }
}
mod ser {
    macro_rules! seq_impl {
        ($ty:ident, $n:expr) => {
            pub fn $ty() -> usize {
                $n
            }
        };
    }
    seq_impl!(vec, 2);
}

macro_rules! with_module {
    () => {
        mod made {
            pub fn seq(seq: u8) -> u8 {
                seq + 1
            }
        }
    };
}
with_module!();

macro_rules! iter_impl {
    ($name:ident) => {
        fn $name<I, T>(it: I) -> usize
        where
            I: Iterator<Item = T>,
        {
            it.count()
        }
    };
}
iter_impl!(how_many);

// (serde's `Impossible<Ok, Error>`: a type parameter named `Ok`)
struct Impossible<Ok> {
    v: Ok,
}
trait Ser {
    type Ok;
    fn done(self) -> Self::Ok;
}
impl<Ok> Ser for Impossible<Ok> {
    type Ok = Ok;
    fn done(self) -> Ok {
        self.v
    }
}

trait Name {
    fn name(&self) -> String;
}
struct S;
impl Name for S {
    fn name(&self) -> String {
        "S".to_string()
    }
}
impl<'a, T: ?Sized + Name> Name for &'a T {
    fn name(&self) -> String {
        (**self).name()
    }
}
impl<'a, T: ?Sized + Name> Name for &'a mut T {
    fn name(&self) -> String {
        (**self).name()
    }
}

// (serde reaches the reference impls through a generic parameter: `T: Serialize` with T = `&mut S`; they forward
// to the referent, as serde's do)
fn show<N: Name>(n: N) -> String {
    n.name()
}

fn main() {
    println!("{} {}", E.code(), E);
    let b: Borrowed<u8> = Borrowed { v: "lt", m: PhantomData };
    let c = b.clone();
    let u: U<u8> = U { m: PhantomData };
    let _a = u;
    let _b = u;
    println!("{} {}", b.v, c.v);
    println!("{} {}", de::vec(), ser::vec());
    use de::ignored_any::IgnoredAny;
    println!("{}", de::ignored_any::count(vec![Some((IgnoredAny, IgnoredAny)), None]));
    println!("{} {}", made::seq(4), how_many([1, 2, 3].iter()));
    let seven: u8 = Impossible { v: 7u8 }.done();
    println!("{seven}");
    let mut s = S;
    println!("{} {} {}", S.name(), show(&S), show(&mut s));
    let n = NonZeroU32::new(5).unwrap();
    println!("{} {:?} {:?} {}", n, NonZeroI8::new(-7), NonZeroUsize::new(0), n.get() + 1);
    let w = Wrapping(250u8);
    let t = Saturating(250u8);
    println!("{} {}", w.0, t.0);
}
