// macro_rules! calls inside an item's expression or type (an operand after `+` / `*`, an array length, a return type),
// a tt-muncher's `@closure` marker in a body that binds `let closure`, and doc comments on cfg'd-out items
macro_rules! one {
    ($ty:ty) => {
        1
    };
}
macro_rules! two {
    () => {
        1 + 1
    };
}
macro_rules! d {
    ($($ty:ty),*) => { const N: u32 = 0 $(+ one!($ty))*; };
}
macro_rules! ty {
    () => {
        u32
    };
}
const A: u32 = 0 + one!(u8) + one!(u16);
const B: u32 = 3 * two!();
static ARR: [u8; 1 + two!()] = [0; 1 + two!()];
d!(u8, u16, u32);
fn f() -> ty!() {
    4
}
macro_rules! item {
    ($n:ident, $v:expr) => {
        const $n: u32 = $v;
    };
}
pub(crate) use {item, one};
crate::item!(P, 7);
self::item!(Q, 0 + crate::one!(u8));

macro_rules! m {
    (@closure $x:expr) => { $x * 2 };
    (closure $x:expr) => { $x + 100 };
    (@go $v:expr) => {{ let closure = $v; m!(@closure closure) + m!(closure closure) }};
}

const _: () = {
    /// in a const block
    /// two lines
    #[cfg(any())]
    #[allow(dead_code)]
    static GONE: u8 = 1;
};
mod md {
    /// before a cfg'd-out fn at the end of a module
    #[cfg(any())]
    pub fn gone() {}
}
struct R {
    a: u8,
    /// a field removed
    #[cfg(any())]
    b: u8,
}

fn main() {
    println!("{} {} {} {} {} {} {}", A, B, ARR.len(), N, f(), P, Q);
    println!("{}", m!(@go 5));
    let r = R { a: 1 };
    println!("ok {}", r.a);
}
