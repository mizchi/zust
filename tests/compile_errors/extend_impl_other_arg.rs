// A program's `impl Extend<u8> for Bytes` is no `Extend<bool>`: the bound's argument is the impl's
struct Bytes(Vec<u8>);

impl Extend<u8> for Bytes {
    fn extend<I: IntoIterator<Item = u8>>(&mut self, iter: I) {
        for b in iter {
            self.0.push(b);
        }
    }
}

fn fill<E: Extend<bool>>(e: &mut E) {
    e.extend([true, false]);
}

fn main() {
    let mut b = Bytes(vec![]);
    fill(&mut b);
    println!("{}", b.0.len());
}
