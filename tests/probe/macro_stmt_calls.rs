// Macro calls among a block's statements whose expansions are items and more calls (bitflags' `bitflags!`, as tracing's
// and tower-http's use it: `const _: () = { a! { .. } b! { .. } impl S { .. } };`, `__impl_internal_bitflags!` giving
// items, three `$crate::m! { .. }` and an item, `__impl_external_bitflags!` giving nothing), and a `ty` fragment
// before `<` (rand's `(1 as $u_scalar << precision)`)
macro_rules! inner_a {
    ($t:ident) => {
        impl $t {
            #[inline]
            fn a(&self) -> u8 {
                1
            }
        }
    };
}

macro_rules! inner_b {
    ($t:ident) => {
        impl $t {
            #[inline]
            fn b(&self) -> u8 {
                2
            }
        }
    };
}

macro_rules! nothing {
    ($t:ident) => {};
}

macro_rules! outer {
    ($t:ident) => {
        impl $t {
            #[inline]
            fn c(&self) -> u8 {
                3
            }
        }
        inner_a! { $t }
        inner_b! { $t }
        impl $t {
            #[inline]
            fn d(&self) -> u8 {
                4
            }
        }
    };
}

macro_rules! flags {
    ($t:ident) => {
        struct $t;
        const _: () = {
            outer! { $t }
            nothing! { $t }
            impl $t {
                #[inline]
                fn e(&self) -> u8 {
                    5
                }
            }
        };
        flags! {}
    };
    () => {};
}

flags! { S }

macro_rules! shifted {
    ($u:ty, $f:ty, $n:expr) => {
        fn scale(p: u32) -> $f {
            let a = (1 as $u << p) as $f;
            let b = 1 as $u < 2;
            let c: Vec<$u> = vec![2];
            a + if b { c[0] as $f } else { 0.0 } + $n as $f
        }
    };
}

shifted!(u32, f64, 3);

fn main() {
    let s = S;
    println!("{}", s.a() + s.b() + s.c() + s.d() + s.e());
    let v = {
        struct L;
        nothing! { L }
        inner_a! { L }
        L.a() + 6
    };
    println!("{} {}", v, scale(4));
}
