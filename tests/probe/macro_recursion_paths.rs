// Macros that rustc expands within its recursion limit and zustc once expanded without end (#252)

// writeable's assert_writeable_eq and sharded-slab's debug_assert_eq_in_drop: an `@internal` rule after rules that
// start with `$x:expr`. No expression starts with `@`, so those rules do not match `@internal, ..`
macro_rules! check_eq {
    ($actual:expr, $expected:expr $(,)?) => {
        check_eq!($actual, $expected, "")
    };
    ($actual:expr, $expected:expr, $($arg:tt)+) => {{
        check_eq!(@internal, $actual, $expected, $($arg)*)
    }};
    (@internal, $actual:expr, $expected:expr, $($arg:tt)+) => {{
        assert_eq!($actual, $expected, $($arg)*);
        $actual
    }};
}

// which tokens an expression starts with (rustc's `can_begin_expr`)
macro_rules! kind {
    ($e:expr) => {
        "expr"
    };
    ($($t:tt)*) => {
        "other"
    };
}

// embassy's fmt.rs: macros named as core's and std's, which call those by their paths
macro_rules! assert {
    ($($x:tt)*) => {{
        #[cfg(not(feature = "defmt"))]
        ::core::assert!($($x)*);
        #[cfg(feature = "defmt")]
        ::defmt::assert!($($x)*);
    }};
}

macro_rules! println {
    ($($x:tt)*) => {
        std::println!("[log] {}", format_args!($($x)*))
    };
}

// aho-corasick's macros.rs: `debug!` gives a block under a cfg that is off, here in the block of an `if !..`
macro_rules! log {
    ($($tt:tt)*) => {
        #[cfg(feature = "logging")]
        {
            $($tt)*
        }
    };
}

macro_rules! debug {
    ($($tt:tt)*) => {
        log!(log::debug!($($tt)*))
    };
}

// a `macro_rules! stringify` takes the bare name only: `core::stringify!` / `std::stringify!` are core's, whose
// arguments are text, also a call of a macro that would call itself without end
#[allow(unused_macros)]
macro_rules! again {
    () => {
        again!()
    };
}

macro_rules! stringify {
    ($($t:tt)*) => {
        "local"
    };
}

fn pick(n: usize) -> Option<usize> {
    if !cfg!(target_endian = "little") {
        debug!("skipping: big endian");
        return None;
    }
    if !(n > 0) {
        debug!("skipping: zero");
        return None;
    }
    let mut k = 0;
    while !(k > 2) {
        debug!("round {}", k);
        k += 1;
    }
    Some(n * 2 + k)
}

fn main() {
    let a = check_eq!(1 + 1, 2);
    let b = check_eq!(3 * 3, 9, "nine {}", 9);
    println!("check_eq: {} {}", a, b);
    assert!(a == 2, "a is {}", a);
    assert!(b == 9);
    println!("pick: {:?} {:?}", pick(3), pick(0));
    println!("starts: {} {} {} {} {} {}", kind!(@x), kind!(-1), kind!(!true), kind!(..), kind!(::core::primitive::u8::MAX), kind!(_));
    println!("no expr: {} {} {} {} {} {}", kind!(pub x), kind!(mut x), kind!(.5), kind!(= 1), kind!(-> u8), kind!(+ 1));
    println!("more: {} {} {} {}", kind!(|x: u8| x), kind!(&&1), kind!(<u8 as Default>::default()), kind!(#[allow(unused)] 1));
    // (a punctuation glued to the next: `!=`, `*=` .. are one token, which starts no expression; `!!b`, `**&&x` are two)
    let (x, t) = (1, true);
    let _ = (x, t);
    println!("glued: {} {} {} {} {} {} {} {} {}", kind!(!= x), kind!(*= x), kind!(|= x), kind!(&= x), kind!(-= x), kind!(<= x), kind!(<- x), kind!(<<= x), kind!(!=));
    println!("not glued: {} {} {} {} {}", kind!(!!t), kind!(**&&x), kind!(|| 1), kind!(<<u8 as Default>::default as Fn<()>>::call_once), kind!(- x));
    // (std's println!, not the local one: tokens a `macro_rules!` passes on are respaced, `again ! ()`, a difference of
    // its own)
    std::println!("stringify: {} | {} | {} | {}", core::stringify!(again!()), std::stringify!(again!(), x), ::core::stringify!(again![]), stringify!(again!()));
}
