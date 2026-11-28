// The review's cases (#144): `Tr::me(&r)` infers Self = `&S` (its result `&S` flows into a closure), an inherent
// method only where its receiver is the probe's try (`r.value()` of `S::value(self)` takes the impl for `&T`), an
// impl for `&mut T` whose other bound (`Marker`) S lacks, `Tr::m(&mut *m)` / `Tr::m(m)` with Self the referent, and
// `(*s).len()` through a `&mut [T]` beside a shared loan (a shared reborrow), `(*s).sort()` a mutable one
trait Tr {
    fn name(&self) -> String;
    fn me(&self) -> Self where Self: Sized + Copy { *self }
    fn value(self) -> u32 where Self: Sized { 1 }
}
trait Marker {}
#[derive(Clone, Copy)]
struct S;
impl S { fn value(self) -> u32 { 10 } }
impl Tr for S { fn name(&self) -> String { "S".to_string() } fn value(self) -> u32 { 2 } }
impl<T: Tr + ?Sized> Tr for &T {
    fn name(&self) -> String { format!("&{}", (**self).name()) }
    fn value(self) -> u32 { 3 }
}
// inherent `&self` beside the impl's: the inherent one at `&U`
#[derive(Clone, Copy)]
struct U;
impl U { fn name(&self) -> String { "U-inherent".to_string() } }
impl Tr for U { fn name(&self) -> String { "U".to_string() } }
// an impl for `&mut T` whose bound is not only the trait
trait Tm { fn get(&self) -> u32; }
impl Tm for S { fn get(&self) -> u32 { 5 } }
impl<T: Tm + Marker> Tm for &mut T { fn get(&self) -> u32 { 50 } }
impl<T: Tr + ?Sized> Tr for &mut T { fn name(&self) -> String { format!("&mut {}", (**self).name()) } }
fn consume<T, F: FnOnce(T) -> String>(t: T, f: F) -> String { f(t) }
fn main() {
    let s = S;
    let r = &s;
    println!("{}", consume(Tr::me(&r), |x| (*x).name()));
    println!("{} {}", r.value(), s.value());
    let u = U;
    println!("{}", (&u).name());
    let mut s2 = S;
    {
        let m = &mut s2;
        println!("{}", (&m).get());
        println!("{} {}", Tr::name(&mut *m), Tr::name(m));
    }
    let mut v = vec![3, 1, 2];
    let sl: &mut [i32] = &mut v;
    let a = &*sl;
    let n = (*sl).len();
    println!("{} {}", n, a[0]);
    (*sl).sort();
    println!("{:?}", sl);
}
