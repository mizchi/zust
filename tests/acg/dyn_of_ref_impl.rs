// A trait implemented for a reference type (`impl<'a> Foo for &'a isize`), made a trait object of a `&isize`: the
// impl's method, as for a call through the reference
trait Foo {
    fn get(&self) -> isize;
}

impl<'a> Foo for &'a isize {
    fn get(&self) -> isize {
        **self
    }
}

struct S(u8);

trait Name {
    fn name(&self) -> String;
}

impl<'a> Name for &'a S {
    fn name(&self) -> String {
        format!("S({})", self.0)
    }
}

fn main() {
    let x: isize = 1;
    let r = &x;
    println!("{}", r.get());
    let b: Box<dyn Foo> = Box::new(&x);
    println!("{}", b.get());
    let s = S(7);
    let n: Box<dyn Name> = Box::new(&s);
    println!("{}", n.name());
}
