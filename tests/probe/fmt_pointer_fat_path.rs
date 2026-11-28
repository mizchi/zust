// `fmt::Pointer::fmt(&p, f)` of fat pointers: a raw slice / str pointer's `Pointer { addr, metadata: n }`, a dyn
// reference's / Box's / raw pointer's `metadata: DynMetadata(..)`, as `{:p}` of p writes them
use std::fmt;
struct S(*const [u8]);
impl fmt::Display for S {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::Pointer::fmt(&self.0, f) }
}
struct T(*const str);
impl fmt::Display for T {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::Pointer::fmt(&self.0, f) }
}
struct D<'a>(&'a dyn fmt::Debug);
impl fmt::Display for D<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::Pointer::fmt(&self.0, f) }
}
struct B(Box<dyn fmt::Debug>);
impl fmt::Display for B {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::Pointer::fmt(&self.0, f) }
}
struct R(*const dyn fmt::Debug);
impl fmt::Display for R {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { fmt::Pointer::fmt(&self.0, f) }
}
fn main() {
    let a = [1u8, 2, 3];
    let p = &a[..] as *const [u8];
    println!("{}", format!("{}", S(p)) == format!("{:p}", p));
    println!("{}", format!("{:#}", S(p)) == format!("{:#p}", p));
    let st = "hey";
    let q = st as *const str;
    println!("{}", format!("{}", T(q)) == format!("{:p}", q));
    let x = 5;
    let d: &dyn fmt::Debug = &x;
    println!("{}", format!("{}", D(d)) == format!("{:p}", d));
    println!("{}", format!("{:20}", D(d)) == format!("{:20p}", d));
    let b: Box<dyn fmt::Debug> = Box::new(6);
    let bs = format!("{:p}", b);
    println!("{}", format!("{}", B(b)) == bs);
    let r = d as *const dyn fmt::Debug;
    println!("{}", format!("{}", R(r)) == format!("{:p}", r));
    println!("{}", format!("{}", S(p)).ends_with("metadata: 3 }"));
}
