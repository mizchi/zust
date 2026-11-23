trait A { fn foo(&self) -> u8 { 1 } }
trait B { fn foo(&self) -> u8 { 2 } }
impl<T> A for T {}
struct S;
impl B for S {}
fn main() {
    println!("{}", S.foo());
}
