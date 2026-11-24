// impls of a trait for `&mut A` / `&T` of a type parameter that forward each method to the referent, as serde's
// SeqAccess / Serialize impls do: with the trait's default methods defined, an associated type from A, a method's own
// type parameter, the UFCS form `A::m(&mut **self, ..)`, and calls through a generic parameter. And modules that
// import `fmt::Write` and `io::Write` each for itself (a crate's `mod lib` re-exports io's)
use std::fmt;

trait Access {
    type Error;
    fn next_seed<T: From<u8>>(&mut self, seed: u8) -> Result<Option<T>, Self::Error>;
    fn skip(&mut self, n: usize) -> usize;
    fn size_hint(&self) -> Option<usize> {
        None
    }
}

struct Counter(u8);
impl Access for Counter {
    type Error = String;
    fn next_seed<T: From<u8>>(&mut self, seed: u8) -> Result<Option<T>, String> {
        if self.0 == 3 {
            return Err(format!("done at {seed}"));
        }
        self.0 += 1;
        Ok(Some(T::from(self.0 + seed)))
    }
    fn skip(&mut self, n: usize) -> usize {
        self.0 += n as u8;
        self.0 as usize
    }
    fn size_hint(&self) -> Option<usize> {
        Some(3 - self.0 as usize)
    }
}

impl<'a, A: ?Sized + Access> Access for &'a mut A {
    type Error = A::Error;
    #[inline]
    fn next_seed<T: From<u8>>(&mut self, seed: u8) -> Result<Option<T>, Self::Error> {
        (**self).next_seed(seed)
    }
    fn skip(&mut self, n: usize) -> usize {
        A::skip(&mut **self, n)
    }
    fn size_hint(&self) -> Option<usize> {
        (**self).size_hint()
    }
}

fn drain<A: Access>(mut a: A) -> (Vec<u32>, Option<usize>, Result<Option<u32>, A::Error>) {
    let mut v = vec![];
    v.push(a.next_seed::<u32>(10).ok().unwrap().unwrap());
    let hint = a.size_hint();
    a.skip(1);
    (v, hint, a.next_seed(20))
}

trait Show {
    fn show(&self, depth: u8) -> String;
}
struct Leaf;
impl Show for Leaf {
    fn show(&self, depth: u8) -> String {
        format!("leaf@{depth}")
    }
}
impl<'a, T: ?Sized + Show> Show for &'a T {
    fn show(&self, depth: u8) -> String {
        (**self).show(depth)
    }
}
fn twice<S: Show>(s: S) -> String {
    format!("{} {}", s.show(1), s.show(2))
}

mod lib {
    pub use std::fmt::{self, Write as FmtWrite};
    pub use std::io::Write;
}
mod format {
    use crate::lib::fmt::{self, Write};
    pub struct Buf {
        pub s: String,
    }
    impl Write for Buf {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.s.push_str(s);
            Ok(())
        }
    }
}
mod bytes {
    use crate::lib::*;
    pub fn put(out: &mut Vec<u8>) {
        out.write_all(b"io").unwrap();
    }
}

fn main() {
    let mut c = Counter(0);
    let (v, hint, last) = drain(&mut c);
    println!("{:?} {:?} {:?} {}", v, hint, last, c.0);
    let mut c2 = Counter(0);
    let mut r = &mut c2;
    println!("{:?} {}", drain(&mut r).2, c2.0);
    println!("{}", twice(&Leaf));
    let mut b = format::Buf { s: String::new() };
    fmt::Write::write_fmt(&mut b, format_args!("{}-{}", 1, 2)).unwrap();
    let mut out = vec![];
    bytes::put(&mut out);
    println!("{} {:?}", b.s, out);
}
