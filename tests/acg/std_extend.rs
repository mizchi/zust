// std::iter::Extend as a trait a program implements and bounds by (smallvec's `impl Extend<A::Item> for SmallVec<A>`), beside a
// Vec's own extend (#250)
struct Bag { items: Vec<u32>, n: usize }
impl Extend<u32> for Bag {
    fn extend<I: IntoIterator<Item = u32>>(&mut self, iter: I) {
        for x in iter {
            self.items.push(x * 2);
            self.n += 1;
        }
    }
}
fn fill<C: Extend<u32>>(c: &mut C) {
    c.extend(vec![7u32, 8]);
}
fn main() {
    let mut b = Bag { items: Vec::new(), n: 0 };
    b.extend(1u32..4);
    fill(&mut b);
    let mut v = vec![1u8];
    v.extend([2u8, 3]);
    println!("{:?} {} {:?}", b.items, b.n, v);
}
