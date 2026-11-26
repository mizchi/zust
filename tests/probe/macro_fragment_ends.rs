// What the top crates' macro_rules! calls need of the matcher (#258): a `ty` / `path` fragment ends where rustc's
// grammar ends it, not at the next token of the matcher (time-core's `$($t:ty)|+ = $e:expr;`, futures-util's
// `|$p:ident: $t:ty| $e:expr`, pin-project-lite's generics and where clauses); a joint `=>` after a fragment is not
// the `=` of `..=` (httparse); `#[doc = $s:literal]` in a matcher takes a doc comment of the call (syn, zerocopy); a
// `$(,)?` before a repetition takes its `,` (pin-project-lite, zerocopy); statement calls in a block that expand to
// items only, or to one more call, leave nothing between the items (pin-project-lite's `const _: () = { .. };`)
#![allow(dead_code)]
use std::fmt::Debug;
use std::mem::size_of;

// time-core 0.1.9's impl_per!: each `ty` stops at `|` and at `=`
macro_rules! per {
    ($($t:ident per {$(
        $larger:ident : [$default_output:ty]
        $($int_output:ty)|+ = $int_value:expr;
        $($float_output:ty)|+ = $float_value:expr;
    )+})*) => {$($(
        fn $larger() -> ($default_output, $($int_output,)+ $($float_output,)+) {
            (1, $($int_value as $int_output,)+ $($float_value as $float_output,)+)
        }
    )+)*};
}
per! {
    Nanosecond per {
        micro: [u16] u16|u32|u64 = 1_000; f32|f64 = 1_000.;
        milli:
            [u32] u32|i64 = 1_000_000; f64 = 1_000_000.;
    }
}

// futures-util 0.3.34's delegate_all!: the closure's parameter type stops at `|`
macro_rules! make_new {
    (New[|$($param:ident: $paramt:ty),*| $cons:expr] $name:ident) => {
        fn $name($($param: $paramt),*) -> Vec<u8> {
            $cons
        }
    };
}
make_new!(New[|x: u8, y: Option<&'static str>| vec![x, y.map_or(0u8, |s| s.len() as u8)]] build);

trait Store<L> {
    type Item;
}
impl<T> Store<Vec<T>> for Vec<T> {
    type Item = T;
}
impl Store<Vec<u8>> for u8 {
    type Item = u16;
}

// pin-project-lite 0.2.18's generics and where clauses: a bound is a path (`Store<Vec<T>>` before the generics'
// `>`), a lifetime is not one (`T: 'static`), a where clause's type stops at its `:`
macro_rules! generics {
    (
        struct $ident:ident $(<
            $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            $( $generics:ident
                $(: $generics_bound:path)?
                $(: ?$generics_unsized_bound:path)?
                $(: $generics_lifetime_bound:lifetime)?
            ),* $(,)?
        >)?
        $(where
            $( $where_clause_ty:ty
                $(: $where_clause_bound:path)?
                $(: ?$where_clause_unsized_bound:path)?
                $(: $where_clause_lifetime_bound:lifetime)?
            ),* $(,)?
        )?
        { $($field:ident: $field_ty:ty),* $(,)? }
    ) => {
        struct $ident $(<
            $( $lifetime $(: $lifetime_bound)? ,)*
            $( $generics $(: $generics_bound)? $(: ?$generics_unsized_bound)? $(: $generics_lifetime_bound)? ),*
        >)?
        $(where $( $where_clause_ty $(: $where_clause_bound)? $(: ?$where_clause_unsized_bound)? $(: $where_clause_lifetime_bound)? ),* )?
        { $($field: $field_ty),* }

        impl $(< $( $lifetime ,)* $( $generics $(: $generics_bound)? $(: ?$generics_unsized_bound)? $(: $generics_lifetime_bound)? ),* >)?
            $ident $(< $( $lifetime ,)* $( $generics ),* >)?
        $(where $( $where_clause_ty $(: $where_clause_bound)? $(: ?$where_clause_unsized_bound)? $(: $where_clause_lifetime_bound)? ),* )?
        {
            fn names() -> &'static str {
                concat!($(stringify!($field), " "),*)
            }
        }
    };
}
generics! {
    struct Holder<T, B: Store<Vec<T>>>
    where
        B: Unpin,
    {
        list: B,
        extra: Option<T>,
    }
}
generics! {
    struct Local<T: 'static, F> {
        slot: Option<T>,
        future: Option<F>,
    }
}
generics! {
    struct Collect<T>
    where
        T: Clone,
        T: ?Sized,
    {
        collected: Option<Vec<u8>>,
        body: Box<T>,
    }
}
generics! {
    struct Peek<'a, St: Clone> {
        inner: Option<&'a mut Vec<St>>,
    }
}

// zerocopy 0.8.66's unsafe_impl!: `$(,)?` before `$($tyvar:ident ..),*`
macro_rules! sized_of {
    (const $constname:ident : $constty:ident $(,)? $($tyvar:ident $(: $bound:ident)?),* => $ty:ty) => {
        fn sized_of<const $constname: $constty, $($tyvar $(: $bound)?),*>() -> usize {
            size_of::<$ty>()
        }
    };
}
sized_of!(const N: usize, T: Copy => [T; N]);

// a type of each kind, each ending where it ends
macro_rules! sizes {
    ($($t:ty),* $(,)?) => {
        vec![$(size_of::<$t>()),*]
    };
}
macro_rules! sizes_spaced {
    ($($t:ty)*) => {
        vec![$(size_of::<$t>()),*]
    };
}

// httparse 1.10.1's expect!: the joint `=>` after `$pat` is not the `=` of `..=`
macro_rules! expect {
    ($bytes:ident.next() == $pat:pat_param => $ret:expr) => {
        expect!($bytes.next() => $pat |? $ret)
    };
    ($e:expr => $pat:pat_param |? $ret:expr) => {
        match $e {
            Some(v @ $pat) => v,
            _ => return $ret,
        }
    };
}
fn digit(s: &[u8]) -> Result<u8, i32> {
    let mut bytes = s.iter().copied();
    let d = expect!(bytes.next() == b'0'..=b'9' => Err(2));
    Ok(d - b'0')
}

// syn 3.0.10's define_punctuation_structs!: `#[doc = $usage:literal]` takes the call's doc comment
macro_rules! punct {
    ($($token:literal pub struct $name:ident/$len:tt #[doc = $usage:literal])*) => {
        $(
            #[doc = $usage]
            pub struct $name {
                pub spans: [u8; $len],
            }
            impl $name {
                fn show() -> String {
                    format!("{} {}{}", $token, stringify!($name), $usage)
                }
            }
        )*
    };
}
punct! {
    "_" pub struct Underscore/1 /// wildcard patterns
    "+=" pub struct PlusEq/2 /// compound assignment
}

// zerocopy 0.8.66's codegen_section!: `$(#[doc = $content:expr])*` of doc comments in the call
macro_rules! section {
    (header = $level:expr, bench = $bench:expr,) => {
        "bench"
    };
    (header = $level:expr, name = $name:expr, $(#[doc = $content:expr])*) => {
        concat!($level, " ", $name, $($content),*)
    };
}

// pin-project-lite's `const _: () = { a! { .. } b! { .. } impl .. }`: one call expands to nothing, one to items
// only, one to another call
#[macro_export]
macro_rules! proj {
    ([] $($t:tt)*) => {};
    ([$proj:ident $default:ident] $($t:tt)*) => {};
    ([$proj:ident] $($t:tt)*) => {
        #[doc(hidden)]
        #[allow(dead_code)]
        struct $proj<'__pin> {
            x: &'__pin u8,
        }
    };
}
macro_rules! forward {
    ($($t:tt)*) => {
        crate::proj! { $($t)* }
    };
}
struct S;
const _: () = {
    crate::proj! { [Given Projection] { a: u8 } }
    crate::proj! { [ProjectionRef] { a: u8 } }
    forward! { [Other] { a: u8 } }
    impl S {
        fn h() -> u8 {
            7
        }
    }
};

fn main() {
    println!("{:?} {:?}", micro(), milli());
    println!("{:?}", build(3, Some("four")));
    println!("{}|{}|{}|{}", Holder::<u8, Vec<u8>>::names(), Local::<u8, u8>::names(), Collect::<Vec<u8>>::names(), Peek::<u8>::names());
    println!("{}", sized_of::<4, u16>());
    println!(
        "{:?}",
        sizes![
            u8,
            &'static mut [u8],
            fn(u8) -> u8,
            Box<dyn Fn(u8) -> u8 + Send + 'static>,
            <u8 as Store<Vec<u8>>>::Item,
            (u8, char),
            [u32; 3],
            *const u8,
            &'static str,
            std::vec::Vec<u8>,
            &dyn Debug,
            unsafe extern "C" fn() -> i32,
            for<'a> fn(&'a u8) -> &'a u8,
            std::collections::HashMap<u8, Vec<Option<u8>>>,
        ]
    );
    println!("{:?}", sizes_spaced![u8 u16 String Vec<u8> (bool, char) &u8 [u8; 2]]);
    println!("{:?} {:?} {:?}", digit(b"7x"), digit(b"x7"), digit(b""));
    println!("{}|{}", Underscore::show(), PlusEq::show());
    let _ = (Underscore { spans: [0] }.spans, PlusEq { spans: [0; 2] }.spans);
    println!("{}", section!(header = "h5", name = "try_mut_from_bytes", /// See the other one.
    ));
    println!("{}", section!(header = "h5", bench = "b",));
    println!("{}", S::h());
}
