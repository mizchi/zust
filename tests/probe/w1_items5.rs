// W1, the cases of PR #139's review: rustc's output (also with -O)
use std::cell::Cell;
use std::ops::Shl;
use std::ptr::NonNull;

// (a macro's closure parameter of a `let`'s name: the parameter and its use are renamed alike)
macro_rules! closure_param {
    () => {{ let x = 1; let f = |_: i32, x: i32| x; f(0, 2) + x * 0 }};
}

// (an inherent `pick(&self)` before a blanket impl's `pick(self)` for `&S`; an inherent `run(&self, u8)` before a
// trait's `run(&self, u16)` for an untyped literal)
struct Pk;
impl Pk {
    fn pick(&self) -> u8 { 1 }
    fn run(&self, x: u8) -> u32 { x as u32 }
}
trait PickBy { fn pick(self) -> u8; }
trait PkMarker {}
impl PkMarker for &Pk {}
impl<T: PkMarker> PickBy for T { fn pick(self) -> u8 { 2 } }
trait RunBy { fn run(&self, x: u16) -> u32; }
impl RunBy for Pk { fn run(&self, x: u16) -> u32 { x as u32 + 1000 } }

// (`From<u8>` and `From<u16>` of one self type: the argument picks)
struct Wo<T>(T);
impl From<u8> for Wo<u32> { fn from(x: u8) -> Self { Wo(x as u32 + 100) } }
impl From<u16> for Wo<u32> { fn from(x: u16) -> Self { Wo(x as u32 + 200) } }

// (an impl's own `ne`, through a qualified path)
struct Ne(Cell<u32>);
struct NeR;
impl PartialEq<NeR> for Ne {
    fn eq(&self, _: &NeR) -> bool { true }
    fn ne(&self, _: &NeR) -> bool { self.0.set(self.0.get() + 1); false }
}

// (a shift's type is its impl's Output)
struct Sh(u32);
impl Shl<u8> for &Sh { type Output = u32; fn shl(self, r: u8) -> u32 { self.0 << r } }
struct Sh64(u64);
impl Shl<u8> for Sh64 { type Output = u64; fn shl(self, r: u8) -> u64 { self.0 << r } }

// (a name through two glob re-exports, imported under another name, beside two other `f`s)
mod ga { pub fn f() -> u8 { 1 } }
mod gb { pub use crate::ga::*; }
mod gc { pub use crate::gb::*; }
mod gz { pub fn f() -> u8 { 9 } }
use gc::f as picked;
fn f() -> u8 { 5 }

// (a macro's anonymous const with a generic impl for its own type: the impl stays whole)
macro_rules! local_generic {
    () => { const _: () = { struct Local<T>(T); impl<T> Local<T> { fn value(&self) -> u8 { 1 } } }; };
}
local_generic!();
// (one with an impl for another type: it goes out)
trait FromAnon { fn from_anon(&self) -> u8; }
macro_rules! anon_impl {
    () => { const _: () = { impl FromAnon for u8 { fn from_anon(&self) -> u8 { *self + 1 } } }; };
}
anon_impl!();

// (impls for two instances of one trait object)
trait Obj<T> {}
trait ObjMarker { fn m() -> u8 { 1 } }
impl ObjMarker for dyn Obj<u8> {}
impl ObjMarker for dyn Obj<u16> + Send {}
fn obj_m<T: ObjMarker + ?Sized>() -> u8 { T::m() }

// (an alias of a type the blanket impl's bound does not hold of: no overlap)
struct As;
struct Au;
type AuAlias = Au;
trait AsMarker {}
impl AsMarker for As {}
struct Aw<T>(T);
trait AwTr { fn t(&self) -> u8; }
impl<T: AsMarker> AwTr for Aw<T> { fn t(&self) -> u8 { 1 } }
impl AwTr for Aw<AuAlias> { fn t(&self) -> u8 { 2 } }

// (an impl for `&Box<S>` the blanket impl's bound does not hold of: no overlap)
trait BxMarker {}
trait BxFlag { fn flag(&self) -> u8; }
struct Bx;
impl<T: BxMarker> BxFlag for T { fn flag(&self) -> u8 { 1 } }
impl BxFlag for &Box<Bx> { fn flag(&self) -> u8 { 2 } }

// (a qualified call of a reference impl's method: its override, the trait's default where it has none)
struct Rw<T>(T);
struct Rv(u8);
trait RTr { fn m(&self) -> u8 { 1 } fn n() -> u8 { 10 } }
impl<T> RTr for &Rw<T> { fn m(&self) -> u8 { 2 } }
impl RTr for &Rv { fn n() -> u8 { 30 } }

// (a macro's enum whose variant has the name of a `let` of the macro: the variant keeps it)
macro_rules! enum_and_let {
    () => { #[allow(non_camel_case_types, dead_code)] enum Ev { value, other } fn ev_f() -> u8 { let value = 3u8; value } };
}
enum_and_let!();

// (a hand-written one-line anonymous const whose fn names a type of its own: read as before, not moved out)
const _: () = { type Local = u32; fn helper(_: Local) {} };

// (`Vec::from_raw_parts` of a dangling pointer of a type aligned to 4096: the empty buffer, not freed)
#[repr(align(4096))]
struct Big(u8);

// (an impl for a reference to a Box, called on an element of a Vec of them: the index is evaluated once)
trait BoxAbi { fn abi(self) -> u8; }
impl BoxAbi for &Box<u8> { fn abi(self) -> u8 { **self + 10 } }

fn main() {
    println!("{}", closure_param!());

    let ba = Box::new(1u8);
    let bb = Box::new(2u8);
    let brefs = vec![&ba, &bb];
    let bn = Cell::new(0usize);
    let bnext = || { bn.set(bn.get() + 1); bn.get() - 1 };
    let bi = 1;
    println!("{} {} {}", brefs[bi].abi(), brefs[bnext()].abi(), bn.get());

    let rw = Rw(5u8);
    let rr = &rw;
    let rv = Rv(1);
    let rvr = &rv;
    println!("{} {} {} {}", <&Rw<u8> as RTr>::m(&rr), <&Rw<u8> as RTr>::n(), <&Rv as RTr>::m(&rvr), <&Rv as RTr>::n());

    let s = Pk;
    let r = &s;
    let v = vec![&s];
    println!("{} {} {}", (&s).pick(), r.pick(), v[0].pick());
    println!("{} {}", s.run(7), RunBy::run(&s, 7));

    println!("{} {}", Wo::<u32>::from(1u16).0, Wo::<u32>::from(1u8).0);

    let n = Ne(Cell::new(0));
    println!("{} {}", <Ne as PartialEq<NeR>>::ne(&n, &NeR), n.0.get());

    let h = Sh(3);
    let k = &h << 1u8;
    println!("{} {}", k.count_ones(), k.pow(2));
    println!("{}", (Sh64(5) << 2u8).leading_zeros());

    let mut b = vec![1u8, 2, 3];
    let (p, l, c) = (b.as_mut_ptr(), b.len(), b.capacity());
    std::mem::forget(b);
    let w = unsafe { std::vec::Vec::from_raw_parts(p, l, c) };
    let st = unsafe { std::string::String::from_utf8_unchecked(vec![104, 105]) };
    println!("{:?} {}", w, st);
    let dp: *mut u32 = NonNull::dangling().as_ptr();
    let mut e = unsafe { Vec::<u32>::from_raw_parts(dp, 0, 0) };
    e.push(3);
    let zp: *mut () = NonNull::dangling().as_ptr();
    let z = unsafe { Vec::<()>::from_raw_parts(zp, 4, 4) };
    println!("{:?} {}", e, z.len());

    println!("{} {} {}", picked(), gz::f(), f());

    println!("{}", 5u8.from_anon());

    println!("{} {}", obj_m::<dyn Obj<u8>>(), obj_m::<dyn Obj<u16> + Send>());

    println!("{} {}", Aw(As).t(), Aw(Au).t());

    let bx = Box::new(Bx);
    println!("{}", <&Box<Bx> as BxFlag>::flag(&&bx));

    println!("{}", ev_f());
    let bp: *mut Big = NonNull::dangling().as_ptr();
    let bv = unsafe { Vec::<Big>::from_raw_parts(bp, 0, 0) };
    println!("{} {}", bv.len(), std::mem::align_of::<Big>());
    drop(bv);

    unsafe fn nested() -> u8 { 3 }
    fn inner() -> u8 { 4 }
    println!("{} {}", unsafe { nested() }, inner());
}
