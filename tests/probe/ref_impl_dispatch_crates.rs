// The forms of the crates #144 kept out: an associated fn default over `&[Self]` (phf_shared), a `take(self)` default
// with Self = `&mut T` (bytes), `to_token_stream` beside T's override (quote), and `str` / `dyn Tr` as T
use std::fmt;
// phf_shared-like: an associated fn default taking &[Self]
trait PhfHash {
    fn phf_hash(&self, st: &mut Vec<u64>);
    fn phf_hash_slice(data: &[Self], st: &mut Vec<u64>) where Self: Sized {
        st.push(1000 + data.len() as u64);
        for p in data { p.phf_hash(st); }
    }
}
impl PhfHash for u8 {
    fn phf_hash(&self, st: &mut Vec<u64>) { st.push(*self as u64) }
    fn phf_hash_slice(data: &[Self], st: &mut Vec<u64>) { st.push(9000 + data.len() as u64) }
}
impl<'a, T: 'a + PhfHash + ?Sized> PhfHash for &'a T {
    fn phf_hash(&self, st: &mut Vec<u64>) { (*self).phf_hash(st) }
}
fn hash_all<T: PhfHash>(xs: &[T]) -> Vec<u64> { let mut v = vec![]; T::phf_hash_slice(xs, &mut v); v }
// bytes-like: take(self) default with Self = &mut T
trait Buf {
    fn remaining(&self) -> usize;
    fn advance(&mut self, n: usize);
    fn take(self, limit: usize) -> Take<Self> where Self: Sized { Take { inner: self, limit } }
}
struct Take<T> { inner: T, limit: usize }
impl<T: Buf> Take<T> { fn rem(&self) -> usize { self.inner.remaining().min(self.limit) } fn into_inner(self) -> T { self.inner } }
struct Bytes(Vec<u8>, usize);
impl Buf for Bytes {
    fn remaining(&self) -> usize { self.0.len() - self.1 }
    fn advance(&mut self, n: usize) { self.1 += n }
}
impl<T: Buf + ?Sized> Buf for &mut T {
    fn remaining(&self) -> usize { T::remaining(self) }
    fn advance(&mut self, n: usize) { T::advance(self, n) }
}
fn take2<B: Buf>(b: B) -> usize { let t = b.take(2); t.rem() }
// quote-like: to_token_stream default on &T, T overriding it
trait ToTokens {
    fn to_tokens(&self, out: &mut String);
    fn to_token_stream(&self) -> String { let mut s = String::new(); self.to_tokens(&mut s); s }
}
struct Id(&'static str);
impl ToTokens for Id {
    fn to_tokens(&self, out: &mut String) { out.push_str(self.0) }
    fn to_token_stream(&self) -> String { format!("<{}>", self.0) }
}
impl<'a, T: ?Sized + ToTokens> ToTokens for &'a T {
    fn to_tokens(&self, out: &mut String) { (**self).to_tokens(out) }
}
fn ts<T: ToTokens>(t: T) -> String { t.to_token_stream() }
// dyn and str as T
trait Show { fn show(&self) -> String; fn show2(&self) -> String { format!("[{}]", self.show()) } }
impl Show for str { fn show(&self) -> String { format!("str:{}", self) } fn show2(&self) -> String { "str-own".to_string() } }
struct D;
impl Show for D { fn show(&self) -> String { "D".to_string() } }
impl<T: Show + ?Sized> Show for &T { fn show(&self) -> String { format!("&{}", (**self).show()) } }
fn sh<T: Show>(t: T) -> String { t.show2() }
impl fmt::Debug for Bytes { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "B{}", self.1) } }
fn main() {
    println!("{:?}", hash_all(&[1u8, 2]));
    println!("{:?}", hash_all(&[&1u8, &2]));
    let mut b = Bytes(vec![1, 2, 3], 0);
    println!("{}", take2(&mut b));
    { let mut t = (&mut b).take(1); t.inner.advance(1); let _ = t.limit; let r = t.into_inner(); r.advance(1); }
    println!("{:?} {}", b, b.remaining());
    println!("{} {} {}", ts(Id("a")), ts(&Id("b")), (&Id("c")).to_token_stream());
    let d: Box<dyn Show> = Box::new(D);
    println!("{} {} {}", sh(&*d), sh("hi"), sh(&D));
}
