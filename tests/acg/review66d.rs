// Fourth review of #66: `(&s).foo()` of a blanket impl's default `foo(self)` takes the reference itself by value
// (Self = &S) before B's `foo(&mut self)`; a number's method follows the trait the first receiver candidate picks
// (A's default `foo(self)` / `&self` before B's impl taking `&self` / `&mut self`)
trait A {
    fn foo(self) -> u8
    where
        Self: Sized,
    {
        1
    }
}
impl<T> A for T {}
trait B {
    fn foo(&mut self) -> u8;
}
struct S;
impl B for S {
    fn foo(&mut self) -> u8 {
        2
    }
}

trait C {
    fn bar(self) -> u8
    where
        Self: Sized,
    {
        10
    }
}
impl C for i32 {}
trait D {
    fn bar(&self) -> u8;
}
impl D for i32 {
    fn bar(&self) -> u8 {
        20
    }
}

trait E {
    fn baz(&self) -> u8 {
        30
    }
}
impl E for u16 {}
trait F {
    fn baz(&mut self) -> u8;
}
impl F for u16 {
    fn baz(&mut self) -> u8 {
        40
    }
}

fn main() {
    let s = S;
    println!("{}", (&s).foo());
    let x = 5i32;
    let mut y = 5u16;
    println!("{} {} {}", x.bar(), y.baz(), F::baz(&mut y));
}
