// A vector temporary as a `&self` receiver: its borrowed result used within the statement, and a receiver that is
// a reference (`id(&v).loan()`), which frees nothing at the statement's end. `loan` is two traits' (the second not
// in scope), so a `match` receiver's type is left open by inference
use std::arch::x86_64::*;
trait Loan {
    fn loan(&self) -> &__m128i;
}
impl Loan for __m128i {
    fn loan(&self) -> &__m128i {
        self
    }
}
mod other {
    use std::arch::x86_64::*;
    pub trait Loan2 {
        fn loan(&self) -> u8;
    }
    impl Loan2 for __m128i {
        fn loan(&self) -> u8 {
            0
        }
    }
}
fn id(v: &__m128i) -> &__m128i {
    v
}
fn pick(flag: bool, a: __m128i, b: __m128i) -> i32 {
    unsafe { _mm_movemask_epi8(*(match flag { true => a, false => b }).loan()) }
}
fn main() {
    let a = unsafe { _mm_set1_epi8(-1) };
    let b = unsafe { _mm_set1_epi8(0) };
    let r = id(&a).loan();
    // (the `&self` receiver is the value itself, not a copy: `loan` returns its address)
    let s = a.loan();
    println!("{} {} {}", pick(true, a, b), pick(false, a, b), unsafe { _mm_movemask_epi8(*r) });
    println!("{} {}", std::ptr::eq(r, &a), std::ptr::eq(s, &a));
}
