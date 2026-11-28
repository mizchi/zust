// (std's Debug for `dyn Any` meets a generic `T: Debug` bound, but not for a program's own trait named Any (E0277))
trait Any {}
impl Any for u8 {}
fn dbg<T: std::fmt::Debug + ?Sized>(x: &T) -> String {
    format!("{:?}", x)
}
fn main() {
    let x: &dyn Any = &1u8;
    println!("{}", dbg(x));
}
