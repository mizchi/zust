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
}
