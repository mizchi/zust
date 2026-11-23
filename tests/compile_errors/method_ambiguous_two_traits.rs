trait A { fn foo(&self) -> u8; }
trait B { fn foo(&self) -> u8 { 2 } }
struct AB;
impl A for AB { fn foo(&self) -> u8 { 1 } }
impl B for AB {}
fn main() {
    let ab = AB;
    println!("{}", ab.foo());
}
