// Codex's review cases (#144): a trait method of the name of a str's own method (`len(self)` beside
// `impl<T: ?Sized> Tr for &T`), where the str's own comes first (`"abc".len()` is 3) and `(&"abc").len()` takes the
// impl at `&&str`; and an impl for `&str` beside `impl<T: Nm> Nm for &T` (whose T is Sized), which a call through a
// bound with `T = &str` takes; bounds of the impl written in a where clause; a number's and a
// slice's own methods; two-phase borrows of a `&mut` autoref (an index
// receiver's index first, a local one too); another trait's method at an earlier try; deep chains of
// impls whose bounds name each other; an extra bound with impls for references
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
// a `&mut self` default with Self = `&S` whose argument reads the receiver (`r.write(r.read())`, two-phase), and
// the same through a bound (`b.advance(b.remaining())` of `B = &mut Bf`)
trait Rw { fn read(&self) -> u32; fn write(&mut self, v: u32) -> u32 { v + 100 } fn put(&mut self, p: &mut u32, v: u32) { *p += v } }
struct Sr;
impl Rw for Sr { fn read(&self) -> u32 { 7 } }
impl<T: Rw> Rw for &T { fn read(&self) -> u32 { 70 } }
trait Bf { fn remaining(&self) -> usize; fn advance(&mut self, n: usize); }
struct Cur(usize, usize);
impl Bf for Cur { fn remaining(&self) -> usize { self.1 - self.0 } fn advance(&mut self, n: usize) { self.0 += n } }
impl<T: Bf + ?Sized> Bf for &mut T { fn remaining(&self) -> usize { (**self).remaining() } fn advance(&mut self, n: usize) { (**self).advance(n) } }
fn idx() -> usize { println!("idx"); 1 }
// impls for references whose bounds name each other, through 16 reference layers (each goal once)
trait Ma { fn ma(&self) -> u32; }
trait Mb { fn mb(&self) -> u32; }
trait Mc { fn mc(&self) -> u32; }
impl Ma for S3 { fn ma(&self) -> u32 { 1 } }
impl Mb for S3 { fn mb(&self) -> u32 { 1 } }
impl Mc for S3 { fn mc(&self) -> u32 { 1 } }
impl<T: Ma + Mb + Mc> Ma for &T { fn ma(&self) -> u32 { 1 + (**self).ma() } }
impl<T: Ma + Mb + Mc> Mb for &T { fn mb(&self) -> u32 { 1 + (**self).mb() } }
impl<T: Ma + Mb + Mc> Mc for &T { fn mc(&self) -> u32 { 1 + (**self).mc() } }
// another trait's method of the name at an earlier try: `r.nm3()` of `r: &S3` is A3's (S3's, by value at `&S3`),
// not B3's impl for `&T` (an autoref)
trait A3 { fn nm3(&self) -> u32; }
trait B3 { fn nm3(&self) -> u32; }
struct S3;
impl A3 for S3 { fn nm3(&self) -> u32 { 1 } }
impl<T> B3 for &T { fn nm3(&self) -> u32 { 2 } }
fn f3<U>(r: &S3) -> u32 { r.nm3() }
fn drain<B: Bf>(mut b: B) -> usize { let n = b.remaining(); b.advance(b.remaining()); n + b.remaining() }
// an extra bound of a trait with an impl for references of its own: `S2` is not Marker, so `&S2` is not covered
trait Marker {}
impl<T> Marker for &T {}
trait Nm2 { fn name2(&self) -> u32; }
struct S2;
impl Nm2 for S2 { fn name2(&self) -> u32 { 1 } }
impl<T: Nm2 + Marker> Nm2 for &T { fn name2(&self) -> u32 { 2 } }
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
    let sr = Sr;
    let mut r = &sr;
    let mut c = Cur(1, 5);
    println!("{} {} {}", r.write(r.read()), drain(&mut c), c.0);
    let mut a = [&sr, &sr];
    println!("{}", a[idx()].write(a[0].read()));
    let mut acc = 0u32;
    let p = &mut acc;
    r.put(p, r.read());
    *p += 1;
    println!("{}", acc);
    let mut i = 0;
    println!("{}", a[i].write({ i = 1; a[i].read() }));
    println!("{}", f3::<u8>(&S3));
    let s3 = S3;
    let deep = &&&&&&&&&&&&&&&s3;
    println!("{} {} {}", (&deep).ma(), deep.mb(), (&&deep).mc());
    let s2 = S2;
    println!("{}", (&&s2).name2());
}
