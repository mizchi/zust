// Two traits implemented for the same reference type (`impl A for &S`, `impl B for &S`), each made a trait object:
// each call takes its own trait's impl
trait A {
    fn get(&self) -> i32;
}

trait B {
    fn get(&self) -> i32;
}

struct S;

impl<'a> A for &'a S {
    fn get(&self) -> i32 {
        1
    }
}

impl<'a> B for &'a S {
    fn get(&self) -> i32 {
        2
    }
}

fn main() {
    let s = S;
    let a: Box<dyn A> = Box::new(&s);
    let b: Box<dyn B> = Box::new(&s);
    println!("{} {}", a.get(), b.get());
}
