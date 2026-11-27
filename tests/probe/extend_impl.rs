// `impl Extend<T> for S` of a program's own (bit-vec's `impl Extend<bool> for BitVec`), and `E: Extend<T>` bounds
// met by those and by std's Vec / String
use std::iter::FromIterator;

#[derive(Debug, Default)]
struct Bits {
    words: Vec<u8>,
    len: usize,
}

impl Bits {
    fn push(&mut self, b: bool) {
        if self.len % 8 == 0 {
            self.words.push(0);
        }
        if b {
            let i = self.len / 8;
            self.words[i] |= 1 << (self.len % 8);
        }
        self.len += 1;
    }
}

impl Extend<bool> for Bits {
    fn extend<I: IntoIterator<Item = bool>>(&mut self, iterable: I) {
        for b in iterable {
            self.push(b);
        }
    }
}

impl FromIterator<bool> for Bits {
    fn from_iter<I: IntoIterator<Item = bool>>(iter: I) -> Self {
        let mut b = Bits::default();
        b.extend(iter);
        b
    }
}

// (an impl for references to the elements, as tinyvec's and smallvec's `Extend<&'a T>`)
#[derive(Debug, Default)]
struct Sum {
    total: u32,
    n: usize,
}

impl<'a> Extend<&'a u8> for Sum {
    fn extend<I: IntoIterator<Item = &'a u8>>(&mut self, iter: I) {
        for x in iter {
            self.total += *x as u32;
            self.n += 1;
        }
    }
}

fn fill<E: Extend<bool>>(e: &mut E, n: usize) {
    e.extend((0..n).map(|i| i % 3 == 0));
}

fn letters<E: Extend<char>>(e: &mut E) {
    e.extend("xyz".chars());
}

fn main() {
    let mut b = Bits::default();
    b.extend(vec![true, false, true]);
    b.extend([true, true]);
    println!("{:?}", b);
    let c = Bits::from_iter([false, true, true, false, true, false, false, false, true]);
    println!("{:?}", c);
    fill(&mut b, 4);
    println!("{} {:?}", b.len, b.words);
    let mut v: Vec<bool> = Vec::new();
    fill(&mut v, 3);
    println!("{:?}", v);
    let mut s = Sum::default();
    let data = [3u8, 4, 5];
    s.extend(data.iter());
    s.extend(&vec![10u8]);
    println!("{:?}", s);
    let mut text = String::from("ab");
    letters(&mut text);
    println!("{text}");
}
