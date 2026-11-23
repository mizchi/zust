struct W<T>(T);
trait A { unsafe fn f<X>(&mut self) -> u8; }
trait B { fn f<X>(&mut self) -> u8 { 2 } }
impl A for W<u8> { unsafe fn f<X>(&mut self) -> u8 { 1 } }
impl B for W<u16> {}
fn main() {
    let mut w = W(0u8);
    println!("{}", w.f::<()>());
}
