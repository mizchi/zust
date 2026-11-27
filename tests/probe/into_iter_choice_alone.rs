// fastrand's `choice` (#313) in a program with no other iterator in it: `I: IntoIterator` alone has the prelude's
// iterators selected, so `iter.into_iter()` of a `&[u8]` is the slice's Iter, and `I::Item` is `&u8`
struct Rng(u64);

impl Rng {
    fn usize(&mut self, r: std::ops::Range<usize>) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        r.start + (self.0 >> 33) as usize % (r.end - r.start)
    }

    fn choice<I>(&mut self, iter: I) -> Option<I::Item>
    where
        I: IntoIterator,
        I::IntoIter: ExactSizeIterator,
    {
        let mut iter = iter.into_iter();
        let len = iter.len();
        if len == 0 {
            return None;
        }
        let index = self.usize(0..len);
        iter.nth(index)
    }

    fn alphabetic(&mut self) -> char {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        *self.choice(CHARS).unwrap() as char
    }
}

fn main() {
    let mut r = Rng(42);
    let mut s = String::new();
    let mut k = 0;
    while k < 16 {
        s.push(r.alphabetic());
        k += 1;
    }
    println!("{}", s);
    let mut v = vec![3i32, 4, 5];
    if let Some(x) = r.choice(&mut v) {
        *x = -*x;
    }
    println!("{:?} {:?}", v, r.choice(&v));
}
