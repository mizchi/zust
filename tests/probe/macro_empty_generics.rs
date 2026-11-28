// `impl< >`: the empty generics a macro gives for no repetition (rand's `impl< $($tyvar,)* > .. for ($($tyvar,)*)`,
// called by `tuple_impls!` for every prefix of `A B C`, the empty one too), and a `ty` fragment a `:` follows
// (bytemuck's `$($nonzero:ty: $primitive:ty),*`)
trait Count {
    fn n() -> usize;
}

impl Count for u8 {
    fn n() -> usize {
        1
    }
}

macro_rules! tuple_impl {
    ($($tyvar:ident)*) => {
        impl< $($tyvar,)* > Count for ($($tyvar,)*)
        where $(
            $tyvar: Count,
        )*
        {
            fn n() -> usize {
                0 $(+ $tyvar::n())*
            }
        }
    }
}

macro_rules! tuple_impls {
    ($($tyvar:ident)*) => { tuple_impls!{[] $($tyvar)*} };
    ([$($prefix:ident)*] $head:ident $($tail:ident)*) => {
        tuple_impl!{$($prefix)*}
        tuple_impls!{[$($prefix)* $head] $($tail)*}
    };
    ([$($prefix:ident)*]) => {
        tuple_impl!{$($prefix)*}
    };
}

tuple_impls! {A B C}

trait Bits {
    type B;
    fn ok(b: &Self::B) -> bool;
}

macro_rules! impl_nonzero {
    ($($nonzero:ty: $primitive:ty),* $(,)?) => {
        $(
            impl Bits for $nonzero {
                type B = $primitive;
                fn ok(b: &Self::B) -> bool {
                    *b != 0
                }
            }
        )*
    };
}

impl_nonzero! {
    core::num::NonZeroU8: u8,
    core::num::NonZeroU16: u16,
}

struct G< >(u8);

impl< > G< > {
    fn g(&self) -> u8 {
        self.0
    }
}

fn h< >() -> u8 {
    2
}

fn main() {
    println!("{} {} {} {}", <() as Count>::n(), <(u8,) as Count>::n(), <(u8, u8) as Count>::n(), <(u8, u8, u8) as Count>::n());
    println!("{} {}", <core::num::NonZeroU8 as Bits>::ok(&3), <core::num::NonZeroU16 as Bits>::ok(&0));
    println!("{} {}", G::< >(1).g(), h::< >());
}
