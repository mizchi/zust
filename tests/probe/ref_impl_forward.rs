// impls of a trait for `&mut A` / `&T` of a type parameter that forward each method to the referent, as serde's
// SeqAccess / Serialize impls do: with the trait's default methods defined, an associated type from A, a method's own
// type parameter, the UFCS form `A::m(&mut **self, ..)`, and calls through a generic parameter; an impl whose method
// does more than forward, with direct calls only (they reach the type's own method, as in rustc). And modules that
// import `fmt::Write` and `io::Write` each for itself (a crate's `mod lib` re-exports io's): a fn's own `use`, a use
// tree that imports io's `Write` beside `fmt`, a comment in a `use`, `Write::{self}`, and an alias to io's `Write`
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

// (direct calls only: `Plain.label()` is Plain's own method; the impl for `&T` would run only through a bound)
trait Label {
    fn label(&self) -> String;
}
struct Plain;
impl Label for Plain {
    fn label(&self) -> String {
        "plain".to_string()
    }
}
impl<'a, T: ?Sized + Label> Label for &'a T {
    fn label(&self) -> String {
        format!("ref {}", (**self).label())
    }
}
struct Unrelated;
impl Unrelated {
    fn label(&self) -> &'static str {
        "unrelated"
    }
}

mod net {
    use std::io::*;
    pub fn render() -> String {
        use std::fmt::Write;
        let mut s = String::new();
        write!(s, "fmt").unwrap();
        s
    }
    pub fn send<W: Write>(w: &mut W) {
        let _ = w.write(b"io");
    }
}
// (a fn's `use` after a statement: still that fn's)
mod late {
    use std::io::*;
    pub fn render() -> String {
        let mut s = String::new();
        use std::fmt::Write;
        write!(s, "late").unwrap();
        s
    }
    pub fn send<W: Write>(w: &mut W) {
        let _ = w.write(b"!");
    }
}
mod tree {
    use std::{fmt, io::Write};
    pub fn send<W: Write>(w: &mut W) -> fmt::Result {
        let _ = w.write(b"tree");
        Ok(())
    }
}

// (a comment inside a `use`, `Write::{self}`, and an alias that binds the name to io's trait)
mod commented {
    use std::fmt::Write /* formatter */;
    pub struct Buf(pub String);
    impl Write for Buf {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.push_str(s);
            Ok(())
        }
    }
    pub fn render<W: Write>(w: &mut W) {
        w.write_str("commented").unwrap();
    }
}
mod selfed {
    use std::fmt::Write::{self};
    pub struct Buf(pub String);
    impl Write for Buf {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.push_str(s);
            Ok(())
        }
    }
    pub fn render<W: Write>(w: &mut W) {
        w.write_str("selfed").unwrap();
    }
}
mod aliased {
    #![allow(unused_imports)]
    use std::fmt::*;
    pub use std::io::Write as IoWrite;
    pub use self::IoWrite as Write;
    pub fn send<W: self::Write>(w: &mut W) {
        let _ = w.write(b"x");
    }
}

// (a child module that takes its parent's `fmt::Write` through `use super::*`, and a string that holds the name)
mod forwarded {
    use std::fmt::Write;
    pub const LABEL: &str = "Write";
    pub mod child {
        use super::*;
        pub struct Buf(pub String);
        impl Write for Buf {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                self.0.push_str(s);
                Ok(())
            }
        }
    }
}
// (a turbofish of the method's own parameter forwards it)
trait Size {
    fn size<const N: usize>(&self) -> usize;
}
struct Sized3;
impl Size for Sized3 {
    fn size<const N: usize>(&self) -> usize {
        N
    }
}
impl<'a, T: ?Sized + Size> Size for &'a T {
    fn size<const N: usize>(&self) -> usize {
        (**self).size::<N>()
    }
}
fn size_of_3<Z: Size>(z: Z) -> usize {
    z.size::<3>()
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
    println!("{} {}", Plain.label(), Unrelated.label());
    let mut v: Vec<u8> = vec![];
    net::send(&mut v);
    tree::send(&mut v).unwrap();
    println!("{} {:?}", net::render(), String::from_utf8(v).unwrap());
    let mut c = commented::Buf(String::new());
    commented::render(&mut c);
    let mut d = selfed::Buf(String::new());
    selfed::render(&mut d);
    let mut x: Vec<u8> = vec![];
    aliased::send(&mut x);
    println!("{} {} {:?}", c.0, d.0, x);
    let mut y: Vec<u8> = vec![];
    late::send(&mut y);
    println!("{} {:?}", late::render(), y);
    let mut f = forwarded::child::Buf(String::new());
    fmt::Write::write_str(&mut f, "sup").unwrap();
    println!("{} {} {}", f.0, forwarded::LABEL, size_of_3(&Sized3));
}
