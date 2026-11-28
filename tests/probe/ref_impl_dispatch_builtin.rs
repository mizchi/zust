// Codex's review cases (#144): a trait method of the name of a str's own method (`len(self)` beside
// `impl<T: ?Sized> Tr for &T`), where the str's own comes first (`"abc".len()` is 3) and `(&"abc").len()` takes the
// impl at `&&str`; and an impl for `&str` beside `impl<T: Nm> Nm for &T` (whose T is Sized), which a call through a
// bound with `T = &str` takes; bounds of the impl written in a where clause; a number's and a
// slice's own methods
trait Tr {
    fn len(self) -> usize;
}
impl<T: ?Sized> Tr for &T {
    fn len(self) -> usize {
        99
    }
}
trait Nm {
    fn name(&self) -> u32;
}
impl Nm for &str {
    fn name(&self) -> u32 {
        1
    }
}
impl Nm for u8 {
    fn name(&self) -> u32 {
        5
    }
}
impl<T: Nm> Nm for &T {
    fn name(&self) -> u32 {
        2 + (**self).name()
    }
}
// bounds written in a where clause: `?Sized` (for `&str`) and another trait's (`Mk`, which D is not)
trait Show { fn show(&self) -> String; fn show2(&self) -> String { format!("[{}]", self.show()) } }
impl Show for str { fn show(&self) -> String { format!("str:{}", self) } }
struct D;
impl Show for D { fn show(&self) -> String { "D".to_string() } }
impl<T> Show for &T where T: Show + ?Sized { fn show(&self) -> String { format!("&{}", (**self).show()) } }
trait Mk {}
trait Tw { fn tw(&self) -> u32; }
impl Tw for D { fn tw(&self) -> u32 { 1 } }
impl<T> Tw for &T where T: Tw + Mk { fn tw(&self) -> u32 { 10 } }
fn sh<T: Show>(t: T) -> String { t.show2() }
// a number's own `is_ascii(&self)` and a slice's `as_mut_ptr(&mut self)` beside impls for references with methods
// of those names: the number's / slice's
trait Asc { fn is_ascii(self) -> bool; }
impl<T> Asc for &T { fn is_ascii(self) -> bool { false } }
trait Mp { fn as_mut_ptr(self) -> *mut u8; }
impl<T: ?Sized> Mp for &mut T { fn as_mut_ptr(self) -> *mut u8 { std::ptr::null_mut() } }
fn run<T: Nm>(t: T) -> u32 {
    t.name()
}
fn main() {
    let s = "abc";
    println!("{} {}", s.len(), (&s).len());
    println!("{} {} {}", run("hello"), run(&3u8), run(&&3u8));
    println!("{} {} {} {}", sh("hi"), sh(&D), (&&D).show(), (&&D).tw());
    let x = 65u8;
    let mut buf = [1u8, 2, 3];
    let sl: &mut [u8] = &mut buf;
    println!("{} {}", (&x).is_ascii(), sl.as_mut_ptr().is_null());
}
