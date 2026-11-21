//! A method declared `unsafe fn` in a trait is unsafe to call through the trait (E0133).
//@ pattern: E0133 context
//@ expect: reject
trait Raw {
    unsafe fn peek(&self) -> u8;
}
struct P(*const u8);
impl Raw for P {
    unsafe fn peek(&self) -> u8 {
        unsafe { *self.0 }
    }
}
fn main() {
    let p = P(16 as *const u8);
    println!("{}", p.peek()); //~ ERROR E0133
}
