// `impl ExactSizeIterator for X {}` (smallvec, bit-vec, thread_local, regex-automata): std's default `len` is
// size_hint's lower bound. An impl that writes its own len keeps it, and a type's inherent len comes first
struct Down {
    n: usize,
}
impl Iterator for Down {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        if self.n == 0 {
            None
        } else {
            self.n -= 1;
            Some(self.n)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.n, Some(self.n))
    }
}
impl ExactSizeIterator for Down {}

// (generic, as smallvec's `impl<A: Array> ExactSizeIterator for IntoIter<A> {}`)
struct Stack<T> {
    v: Vec<T>,
}
impl<T> Iterator for Stack<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.v.pop()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.v.len(), Some(self.v.len()))
    }
}
impl<T> ExactSizeIterator for Stack<T> {}

// (with a lifetime, as bit-vec's `impl<B: BitBlock> ExactSizeIterator for Iter<'_, B> {}`)
struct Slots<'a, T> {
    s: &'a [T],
}
impl<'a, T> Iterator for Slots<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        if self.s.is_empty() {
            None
        } else {
            let x = &self.s[0];
            self.s = &self.s[1..];
            Some(x)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.s.len(), Some(self.s.len()))
    }
}
impl<T> ExactSizeIterator for Slots<'_, T> {}

// (its own len, not size_hint's)
struct Own(u8);
impl Iterator for Own {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        if self.0 == 0 {
            None
        } else {
            self.0 -= 1;
            Some(self.0)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.0 as usize, Some(self.0 as usize))
    }
}
impl ExactSizeIterator for Own {
    fn len(&self) -> usize {
        100 + self.0 as usize
    }
}

// (an inherent len before the trait's default)
struct Inherent;
impl Inherent {
    fn len(&self) -> usize {
        7
    }
}
impl Iterator for Inherent {
    type Item = u8;
    fn next(&mut self) -> Option<u8> {
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}
impl ExactSizeIterator for Inherent {}

fn by_bound<I: ExactSizeIterator>(i: &I) -> usize {
    i.len()
}

fn drain<I: ExactSizeIterator<Item = usize>>(mut i: I) -> Vec<(usize, usize)> {
    let mut out = vec![];
    while let Some(x) = i.next() {
        out.push((x, i.len()));
    }
    out
}

fn main() {
    let mut d = Down { n: 3 };
    println!("{}", d.len());
    d.next();
    println!("{} {:?}", d.len(), d.size_hint());
    println!("{}", by_bound(&d));
    println!("{:?}", drain(Down { n: 4 }));

    let mut s = Stack { v: vec!['a', 'b', 'c'] };
    println!("{}", s.len());
    s.next();
    println!("{} {}", s.len(), by_bound(&s));
    let words = Stack { v: vec![String::from("x"), String::from("yz")] };
    println!("{}", words.len());

    let arr = [10, 20, 30, 40];
    let mut sl = Slots { s: &arr };
    sl.next();
    println!("{} {} {:?}", sl.len(), by_bound(&sl), sl.next());

    let o = Own(2);
    println!("{} {}", o.len(), by_bound(&o));

    let i = Inherent;
    println!("{}", i.len());
}
