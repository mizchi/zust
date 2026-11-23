trait A { fn foo(self) -> u8 where Self: Sized { 1 } }
trait B { fn foo(&self) -> u8; }
impl<T> A for T {}
struct S;
impl B for S { fn foo(&self) -> u8 { 2 } }
fn main() {
    println!("{}", (&S).foo());
}
