// (an associated function registered before inherent methods of the name for two instances: the `W<u8>` one is
// unsafe, and calling it outside `unsafe` is E0133, while `W<u16>`'s safe one is not)
struct W<T>(T);
trait Mk {
    fn f() -> i32;
}
impl Mk for W<bool> {
    fn f() -> i32 { 1 }
}
impl W<u8> {
    unsafe fn f(&self) -> i32 { self.0 as i32 + 200 }
}
impl W<u16> {
    fn f(&self) -> i32 { self.0 as i32 + 300 }
}
fn main() {
    let a = W(5u16);
    let b = W(6u8);
    println!("{}", a.f());
    println!("{}", b.f());
}
