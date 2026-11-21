//! An `unsafe trait` of the program's own needs an `unsafe impl` too (E0200): unsafe code may rely
//! on every implementation keeping the trait's contract.
//@ rule: the unsafe boundary (E0133 / E0200 / E0512)
//@ expect: reject
/// Implementors promise that `len` is at most the length of `bytes`.
unsafe trait TrustedLen {
    fn len(&self) -> usize;
    fn bytes(&self) -> &[u8];
}
struct Liar;
impl TrustedLen for Liar { //~ ERROR E0200
    fn len(&self) -> usize {
        100
    }
    fn bytes(&self) -> &[u8] {
        b"ab"
    }
}
fn last<T: TrustedLen>(t: &T) -> u8 {
    unsafe { *t.bytes().as_ptr().add(t.len() - 1) }
}
fn main() {
    println!("{}", last(&Liar));
}
