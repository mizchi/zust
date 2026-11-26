// Iterator::size_hint's default, and ExactSizeIterator's len from it: an `impl ExactSizeIterator for X {}` (#250: smallvec,
// bit-vec)
struct Count { n: usize, end: usize }
impl Iterator for Count {
    type Item = usize;
    fn next(&mut self) -> Option<usize> { if self.n < self.end { self.n += 1; Some(self.n) } else { None } }
    fn size_hint(&self) -> (usize, Option<usize>) { let r = self.end - self.n; (r, Some(r)) }
}
impl ExactSizeIterator for Count {}
struct Plain(u8);
impl Iterator for Plain {
    type Item = u8;
    fn next(&mut self) -> Option<u8> { if self.0 > 0 { self.0 -= 1; Some(self.0) } else { None } }
}
fn total<I: ExactSizeIterator>(it: I) -> usize { it.len() }
fn main() {
    let mut c = Count { n: 0, end: 5 };
    c.next();
    println!("{} {:?}", c.len(), c.size_hint());
    println!("{}", total(Count { n: 2, end: 3 }));
    println!("{:?} {}", Plain(3).size_hint(), Plain(3).count());
}
