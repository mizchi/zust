// A receiver that is a value of a type of its own beside `impl<T: ?Sized> Tr for &T`'s `self` methods (#144): the
// probe's autoref at the first step makes Self = `&a` before a slice's method at the unsized step (`a.len()` of an
// array is the impl's), a number's name is not a str's (`"abc".abs()` is the impl's), and a struct's own method
// comes first where its receiver is the try's (`self` by value, `&self` at the `&` autoref), not one taking
// `&mut self`; a scalar's builtins are its type's own (`is_ascii` a u8's, not an i32's; `abs` not a u32's; a
// float's `sqrt` and a char's `is_alphabetic` at the value, the impl's at a reference)
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
trait Asc {
    fn is_ascii(self) -> bool;
}
impl<T> Asc for &T {
    fn is_ascii(self) -> bool {
        false
    }
}
trait Fl {
    fn sqrt(self) -> f64;
}
impl<T> Fl for &T {
    fn sqrt(self) -> f64 {
        99.0
    }
}
trait Ch {
    fn is_alphabetic(self) -> bool;
}
impl<T> Ch for &T {
    fn is_alphabetic(self) -> bool {
        false
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
    let (x, c, u) = (65i32, 65u8, 5u32);
    println!("{} {} {} {}", (&x).is_ascii(), (&c).is_ascii(), (&u).abs(), u.abs());
    let (f, ch) = (4.0f64, 'a');
    println!("{} {} {} {}", f.sqrt(), (&f).sqrt(), ch.is_alphabetic(), (&ch).is_alphabetic());
}
