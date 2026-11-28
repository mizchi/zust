// A receiver that is a value of a type of its own beside `impl<T: ?Sized> Tr for &T`'s `self` methods (#144): the
// probe's autoref at the first step makes Self = `&a` before a slice's method at the unsized step (`a.len()` of an
// array is the impl's), a number's name is not a str's (`"abc".abs()` is the impl's), and a struct's own method
// comes first where its receiver is the try's (`self` by value, `&self` at the `&` autoref), not one taking
// `&mut self`
trait Tr {
    fn abs(self) -> u32;
}
impl<T: ?Sized> Tr for &T {
    fn abs(self) -> u32 {
        99
    }
}
trait Ln {
    fn len(self) -> usize;
}
impl<T: ?Sized> Ln for &T {
    fn len(self) -> usize {
        77
    }
}
trait Val {
    fn val(self) -> u32;
}
impl<T: ?Sized> Val for &T {
    fn val(self) -> u32 {
        5
    }
}
struct Own;
impl Own {
    fn val(&self) -> u32 {
        1
    }
}
struct Bare;
struct MutOwn;
impl MutOwn {
    #[allow(dead_code)]
    fn val(&mut self) -> u32 {
        2
    }
}
fn main() {
    println!("{}", "abc".abs());
    let a = [1, 2, 3];
    println!("{} {}", (&a).len(), a.len());
    let s: &[i32] = &a;
    println!("{}", s.len());
    println!("{} {} {}", Own.val(), Bare.val(), MutOwn.val());
}
