// Impls of a trait for `&T` / `&mut T` that forward to T's (#144), in the forms crates write them: the qualified path
// (futures-core, quote), a path call passing `self` (embedded-io, futures-task), an `unsafe` block (allocator-api2), a
// receiver with a lifetime (bytes), `Pin` receivers (futures, tokio, http-body), and a unit struct for a `_`
// parameter (tokio). Each is called through a bound, whose call runs the impl's method, which reaches T's
use std::pin::Pin;

trait Fused {
    fn is_terminated(&self) -> bool;
}
struct F(bool);
impl Fused for F {
    fn is_terminated(&self) -> bool {
        self.0
    }
}
impl<T: Fused + ?Sized> Fused for &mut T {
    fn is_terminated(&self) -> bool {
        <T as Fused>::is_terminated(&**self)
    }
}

trait Frag {
    fn span(&self) -> Option<u32>;
    fn name(&self) -> String;
}
impl Frag for str {
    fn span(&self) -> Option<u32> {
        None
    }
    fn name(&self) -> String {
        self.to_string()
    }
}
impl<T: Frag + ?Sized> Frag for &T {
    fn span(&self) -> Option<u32> {
        <T as Frag>::span(*self)
    }
    fn name(&self) -> String {
        Frag::name(*self)
    }
}

trait Read {
    fn read(&mut self, buf: &mut [u8]) -> usize;
}
struct R;
impl Read for R {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        buf[0] = 7;
        1
    }
}
impl<T: Read + ?Sized> Read for &mut T {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        T::read(self, buf)
    }
}

trait Alloc {
    unsafe fn dealloc(&self, n: usize) -> usize;
    fn chunks<'b>(&'b self, d: &mut [u8]) -> usize;
}
struct A;
impl Alloc for A {
    unsafe fn dealloc(&self, n: usize) -> usize {
        n + 1
    }
    fn chunks<'b>(&'b self, d: &mut [u8]) -> usize {
        d.len()
    }
}
impl<T: Alloc + ?Sized> Alloc for &T {
    unsafe fn dealloc(&self, n: usize) -> usize {
        unsafe { (**self).dealloc(n) }
    }
    fn chunks<'b>(&'b self, d: &mut [u8]) -> usize {
        (**self).chunks(d)
    }
}

trait Body {
    fn is_end(&self) -> bool;
    fn poll(self: Pin<&mut Self>, n: u32) -> u32;
    fn poll_fill(self: Pin<&mut Self>) -> u32;
}
struct B(bool);
impl Body for B {
    fn is_end(&self) -> bool {
        self.0
    }
    fn poll(self: Pin<&mut Self>, n: u32) -> u32 {
        n
    }
    fn poll_fill(self: Pin<&mut Self>) -> u32 {
        3
    }
}
impl<T: Body + Unpin + ?Sized> Body for &mut T {
    fn is_end(&self) -> bool {
        Pin::new(&**self).is_end()
    }
    fn poll(mut self: Pin<&mut Self>, n: u32) -> u32 {
        T::poll(Pin::new(&mut **self), n)
    }
    fn poll_fill(self: Pin<&mut Self>) -> u32 {
        Pin::new(&mut **self.get_mut()).poll_fill()
    }
}

mod sealed {
    pub struct Internal;
}
trait Addrs {
    fn to_addrs(&self, _: sealed::Internal) -> u32;
}
struct H(u32);
impl Addrs for H {
    fn to_addrs(&self, _: sealed::Internal) -> u32 {
        self.0
    }
}
impl<T: Addrs + ?Sized> Addrs for &T {
    fn to_addrs(&self, _: sealed::Internal) -> u32 {
        (**self).to_addrs(sealed::Internal)
    }
}

fn term<X: Fused>(x: X) -> bool {
    x.is_terminated()
}
fn frag<X: Frag>(x: X) -> String {
    format!("{} {:?}", x.name(), x.span())
}
fn rd<X: Read>(mut x: X) -> usize {
    let mut b = [0u8; 2];
    x.read(&mut b) + b[0] as usize
}
fn dl<X: Alloc>(x: X) -> usize {
    let mut d = [0u8; 3];
    let a = unsafe { x.dealloc(4) };
    a + x.chunks(&mut d)
}
fn end<X: Body>(x: X) -> bool {
    x.is_end()
}
fn addrs<X: Addrs>(x: X) -> u32 {
    x.to_addrs(sealed::Internal)
}

fn main() {
    let mut f = F(true);
    let mut r = R;
    let mut b = B(false);
    println!("{} [{}] {} {}", term(&mut f), frag("abc"), rd(&mut r), dl(&A));
    println!("{} {}", end(&mut b), addrs(&H(4)));
}
