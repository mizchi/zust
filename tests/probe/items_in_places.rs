// Items where wasm-bindgen's code and its proc macro's output put them: an `extern` block in a function body (with
// attributes), `pub extern crate` in a module (a path through it), a type as a bound of a `where` clause in a module,
// and impls of a marker trait for `&T` and `&mut T` side by side (they do not overlap)
mod rt {
    pub extern crate alloc;
    pub extern crate core;
}

fn abs_of(x: i32) -> i32 {
    #[allow(dead_code)]
    unsafe extern "C" {
        fn abs(x: i32) -> i32;
    }
    unsafe { abs(x) }
}

fn labs_of(x: i64) -> i64 {
    unsafe extern "C" {
        fn labs(x: i64) -> i64;
    }
    unsafe { labs(x) }
}

mod conv {
    pub struct J(pub u32);
    impl From<u8> for J {
        fn from(x: u8) -> J {
            J(x as u32 + 1000)
        }
    }
    pub fn to_j<T>(t: T) -> J
    where
        J: From<T>,
    {
        J::from(t)
    }
}

trait Marker {}
impl Marker for u8 {}
impl<T: Marker + ?Sized> Marker for &T {}
impl<T: Marker + ?Sized> Marker for &mut T {}
fn marked<T: Marker>(_: T) -> u32 {
    1
}

fn main() {
    let v: rt::alloc::vec::Vec<u8> = rt::alloc::vec![1, 2, 3];
    println!("{} {}", v.len(), rt::core::cmp::max(4, 5));
    println!("{} {}", abs_of(-5), labs_of(-7));
    println!("{}", conv::to_j(5u8).0);
    let mut x = 3u8;
    println!("{} {} {}", marked(2u8), marked(&x), marked(&mut x));
}
