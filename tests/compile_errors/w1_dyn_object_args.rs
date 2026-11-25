trait Obj<T> {}
trait Marker { fn m() -> u8 { 1 } }
impl Marker for dyn Obj<u8> {}
fn need<T: Marker + ?Sized>() -> u8 { T::m() }
fn main() { println!("{}", need::<dyn Obj<u16>>()); }
