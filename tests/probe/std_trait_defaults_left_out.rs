// std traits whose impls leave out the methods std gives defaults (#298): DoubleEndedIterator's nth_back / rfold /
// rfind, Iterator's size_hint, Error's source / description, Read's read_exact, Write's write_all, Hasher's
// write_u8 and the rest. Each is left out by one impl and defined by another, which keeps its own
use std::error::Error;
use std::fmt;
use std::hash::Hasher;
use std::io::{Read, Write};

// (next_back only, and no size_hint)
struct Span {
    lo: u32,
    hi: u32,
}
impl Iterator for Span {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.lo < self.hi {
            self.lo += 1;
            Some(self.lo - 1)
        } else {
            None
        }
    }
}
impl DoubleEndedIterator for Span {
    fn next_back(&mut self) -> Option<u32> {
        if self.lo < self.hi {
            self.hi -= 1;
            Some(self.hi)
        } else {
            None
        }
    }
}

// (the defaults defined)
struct All(u8);
impl Iterator for All {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}
impl DoubleEndedIterator for All {
    fn next_back(&mut self) -> Option<u8> {
        None
    }
    fn nth_back(&mut self, n: usize) -> Option<u8> {
        Some(n as u8 + self.0)
    }
    fn rfold<B, F: FnMut(B, u8) -> B>(self, init: B, mut f: F) -> B {
        f(init, self.0)
    }
    fn rfind<P: FnMut(&u8) -> bool>(&mut self, mut p: P) -> Option<u8> {
        if p(&self.0) { Some(self.0) } else { None }
    }
}

#[derive(Debug)]
struct Leaf;
impl fmt::Display for Leaf {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "leaf")
    }
}
impl Error for Leaf {}

#[derive(Debug)]
struct Wrap(Leaf);
impl fmt::Display for Wrap {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "wrap")
    }
}
impl Error for Wrap {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
    #[allow(deprecated)]
    fn description(&self) -> &str {
        "a wrap"
    }
}

// (one byte per read, two per write: read_exact / write_all loop)
struct Src(Vec<u8>, usize);
impl Read for Src {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.1 >= self.0.len() || buf.is_empty() {
            return Ok(0);
        }
        buf[0] = self.0[self.1];
        self.1 += 1;
        Ok(1)
    }
}

struct Sink(Vec<u8>);
impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = buf.len().min(2);
        self.0.extend_from_slice(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Sum(u64);
impl Hasher for Sum {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = self.0.wrapping_mul(31).wrapping_add(*b as u64);
        }
    }
}

fn main() {
    let v: Vec<u32> = Span { lo: 0, hi: 4 }.rev().collect();
    println!("{:?}", v);
    let mut s = Span { lo: 0, hi: 5 };
    println!("{:?} {:?}", s.next_back(), s.next());
    let mut a = All(5);
    println!("{:?} {:?}", a.nth_back(2), a.rfind(|x| *x > 1));
    println!("{}", All(3).rfold(10, |acc, x| acc + x));

    let w = Wrap(Leaf);
    println!("{} {}", w, w.source().is_some());
    let b: Box<dyn Error> = Box::new(Leaf);
    println!("{}", b);

    let mut r = Src(b"hello".to_vec(), 0);
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).unwrap();
    println!("{:?}", buf);
    println!("{}", r.read_exact(&mut buf).is_err());
    let mut k = Sink(vec![]);
    k.write_all(b"abcde").unwrap();
    println!("{:?}", k.0);
    let mut h = Sum(0);
    h.write_u8(3);
    h.write_u32(0x01020304);
    h.write_i8(-1);
    h.write_usize(9);
    println!("{}", h.finish());
}
