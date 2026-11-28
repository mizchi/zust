// The review's cases (#144): `Tr::me(&r)` infers Self = `&S` (its result `&S` flows into a closure), an inherent
// method only where its receiver is the probe's try (`r.value()` of `S::value(self)` takes the impl for `&T`), an
// impl for `&mut T` whose other bound (`Marker`) S lacks, `Tr::m(&mut *m)` / `Tr::m(m)` with Self the referent, and
// `(*s).len()` through a `&mut [T]` beside a shared loan (a shared reborrow, also beside another trait's `len(&mut self)`), and
// `(*s).sort()` / `(*s).halve()` of an `impl Halve for [i32]` a mutable one; a parameter `r: &S` beside `T = &S` is S's; of two
// traits with the method and impls for references, the bound's (or the path call's); `impl<T: Sz> Sz for &T`
// not for `&str`
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
// two traits with `val`, each with an impl for references: `t.val()` of `T: Bv` is Bv's
trait Av { fn val(&self) -> u32; }
trait Bv { fn val(&self) -> u32; }
impl Av for S { fn val(&self) -> u32 { 1 } }
impl Bv for S { fn val(&self) -> u32 { 2 } }
impl<X: Av + ?Sized> Av for &X { fn val(&self) -> u32 { 10 } }
impl<Y: Bv + ?Sized> Bv for &Y { fn val(&self) -> u32 { 20 } }
fn run_b<T: Bv>(t: T) -> u32 { t.val() }
fn run_a<T: Av>(t: T) -> u32 { t.val() }
// an impl for `&T` without `?Sized` is not for `&str`: `(&&s).sz()` of `s: &str` is str's
trait Sz { fn sz(&self) -> u32; }
impl Sz for str { fn sz(&self) -> u32 { 1 } }
impl<T: Sz> Sz for &T { fn sz(&self) -> u32 { 2 } }
// a path call naming its trait among two with impls for references: `Ga::gv(&&s)` is Ga's (u32, not bool)
trait Ga { fn gv(&self) -> u32; }
trait Gb { fn gv(&self) -> bool; }
impl Ga for S { fn gv(&self) -> u32 { 4 } }
impl Gb for S { fn gv(&self) -> bool { true } }
impl<X: Ga + ?Sized> Ga for &X { fn gv(&self) -> u32 { 40 } }
impl<Y: Gb + ?Sized> Gb for &Y { fn gv(&self) -> bool { false } }
fn use_it<A, F: FnOnce(A) -> u32>(a: A, f: F) -> u32 { f(a) }
// a parameter declared with a concrete type beside `T = &S`: `r.name()` is S's, `t.name()` the impl's for `&T`
fn conc<T: Tr>(t: T, r: &S) -> String { format!("{} {}", t.name(), r.name()) }
// a trait whose `len` takes `&mut self`, which slices do not implement, and one they do
#[allow(dead_code)]
trait Other { fn len(&mut self) -> usize; }
trait Halve { fn halve(&mut self); }
impl Halve for [i32] { fn halve(&mut self) { for x in self.iter_mut() { *x /= 2; } } }
fn main() {
    let s = S;
    let r = &s;
    println!("{}", consume(Tr::me(&r), |x| (*x).name()));
    println!("{}", conc(&s, &s));
    println!("{} {}", run_b(&&s), run_a(&s));
    let st: &str = "x";
    println!("{} {} {}", (&&st).sz(), (&st).sz(), st.sz());
    println!("{} {}", use_it(Ga::gv(&&s), |x| x + 1), use_it(Gb::gv(&&s), |b| b as u32));
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
    (*sl).halve();
    println!("{:?}", sl);
}
