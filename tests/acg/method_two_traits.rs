// A method two traits give a type is not ambiguous beside an inherent one (which wins), through a qualified path,
// for a type only one of the traits is implemented for, or in a generic fn, where the bound picks the trait
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
trait S0 {
    fn scaled(&self) -> u8 {
        10
    }
}
trait S1 {
    fn scaled(&self) -> u8 {
        11
    }
}
impl S0 for u16 {}
impl S1 for u16 {}
fn by0<S: S0>(s: &S) -> u8 {
    s.scaled()
}
fn main() {
    println!("{}", by0(&5u16));
    let ab = AB;
    println!("{} {} {} {}", ab.foo(), A::foo(&ab), <AB as B>::foo(&ab), OnlyA.foo());
}
