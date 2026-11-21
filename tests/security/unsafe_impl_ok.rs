//! The other side of unsafe_impl_send: `unsafe impl` of `Send` / `Sync` / an `unsafe trait`, and
//! safe impls whose headers mention those traits in bounds, must still build.
//@ expect: run
use std::fmt;
struct P(*mut u8);
unsafe impl Send for P {}
unsafe impl Sync for P {}
/// doc comment before the trait
pub unsafe trait Zeroable {
    fn zero() -> Self;
}
#[allow(unused)]
unsafe impl Zeroable for u32 {
    fn zero() -> u32 {
        0
    }
}
struct W<T>(T);
impl<T: Send + Sync> fmt::Display for W<T>
where
    T: Copy,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "W")
    }
}
fn needs_send<T: Send>(_: &T) -> bool {
    true
}
fn main() {
    let p = P(std::ptr::null_mut());
    println!("{} {} {}", needs_send(&p), u32::zero(), W(1u8));
}
