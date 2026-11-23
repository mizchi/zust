struct W<T>(T);
type Alias = W<u16>;
trait A { fn foo(&self) -> u8 { 1 } }
trait B { fn foo(&self) -> u8 { 2 } }
impl A for W<u8> {}
impl A for Alias {}
impl B for W<u16> {}
fn main() { println!("{}", W(0u16).foo()); }
