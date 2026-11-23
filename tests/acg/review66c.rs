// Third review of #66: an impl whose parameter's bound fails (`impl<T: Marker> A for W<T>`, no `Marker` for i32) is
// not W<i32>'s; `W<&mut i32>` is not `W<&i32>`; the trait a call names for its lookup keeps the method call's own
// borrow of the receiver (`w.f::<()>(w.0)`) and its unsafe check (inside `unsafe` here)
struct W<T>(T);
trait Marker {}
trait A {
    fn foo(&self) -> u8 {
        1
    }
}
trait B {
    fn foo(&self) -> u8 {
        2
    }
}
impl<T: Marker> A for W<T> {}
impl B for W<i32> {}

struct R<T>(T);
impl A for R<&'static mut i32> {}
impl B for R<&'static i32> {}
static X: i32 = 5;

trait C {
    fn f<X>(&mut self, v: u8) -> u8 {
        v
    }
}
trait D {
    fn f<X>(&self, v: u8) -> u8 {
        v + 100
    }
}
struct V(u8);
impl C for V {}
impl D for V {}

trait U {
    unsafe fn g<X>(&mut self) -> u8;
}
trait G {
    fn g<X>(&mut self) -> u8 {
        20
    }
}
impl U for W<u8> {
    unsafe fn g<X>(&mut self) -> u8 {
        10
    }
}
impl G for W<u16> {}

fn main() {
    println!("{} {}", W(0i32).foo(), R(&X).foo());
    let mut v = V(3);
    println!("{} {}", C::f::<()>(&mut v, 1), v.f::<()>(v.0));
    let mut w = W(0u8);
    println!("{}", unsafe { w.g::<()>() });
}
