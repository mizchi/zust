// (a program's own trait named Pointer: a reference does not have it, as std's Pointer of references is not it)
trait Pointer {}
trait Flag { fn flag(&self) -> bool { true } }
struct W<T>(T);
impl<T: Pointer> Flag for W<T> {}
fn main() { let x = 1u8; println!("{}", W(&x).flag()); }
