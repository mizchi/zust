// A blanket impl's method for a reference made a trait object, which overrides the trait's default: the impl's.
// A default it does not override is the trait's
trait A {
    fn get(&self) -> i32 {
        0
    }
    fn other(&self) -> i32 {
        1
    }
}

impl<T> A for T {
    fn get(&self) -> i32 {
        7
    }
}

fn main() {
    let v = 5u8;
    let b: Box<dyn A> = Box::new(&v);
    println!("{} {}", b.get(), b.other());
}
