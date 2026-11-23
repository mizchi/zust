// Two traits with blanket impls that have a method of the same name: each trait's own is called (`A::get(&x)`,
// `B::get(&x)`, and B's through a `dyn B` made of a reference)
trait A { fn get(&self) -> i32; }
trait B { fn get(&self) -> i32; }
impl<T> A for T { fn get(&self) -> i32 { 1 } }
impl<T> B for T { fn get(&self) -> i32 { 2 } }
fn call(b: &dyn B) -> i32 { b.get() }
fn main() {
    let x = 5u8;
    println!("{} {}", A::get(&x), B::get(&x));
    let d: Box<dyn B> = Box::new(&x);
    println!("{}", call(&*d));
}
