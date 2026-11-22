// A method two traits give a type is not ambiguous beside an inherent one (which wins), through a qualified path,
// or for a type only one of the traits is implemented for
trait A {
    fn foo(&self) -> u8;
}
trait B {
    fn foo(&self) -> u8;
}
struct AB;
impl AB {
    fn foo(&self) -> u8 {
        0
    }
}
impl A for AB {
    fn foo(&self) -> u8 {
        1
    }
}
impl B for AB {
    fn foo(&self) -> u8 {
        2
    }
}
struct OnlyA;
impl A for OnlyA {
    fn foo(&self) -> u8 {
        3
    }
}
fn main() {
    let ab = AB;
    println!("{} {} {} {}", ab.foo(), A::foo(&ab), <AB as B>::foo(&ab), OnlyA.foo());
}
