//! A user ExactSizeIterator whose `len` lies: std may use it for allocation hints only, never to decide how much
//! it writes.
//@ pattern: trusting a user impl
//@ expect: run
struct Liar(u32);
impl Iterator for Liar {
    type Item = u64;
    fn next(&mut self) -> Option<u64> {
        if self.0 == 0 {
            None
        } else {
            self.0 -= 1;
            Some(self.0 as u64)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (1, Some(1))
    }
}
impl ExactSizeIterator for Liar {
    fn len(&self) -> usize {
        1
    }
}
fn main() {
    let v: Vec<u64> = Liar(64).collect();
    println!("{} {}", v.len(), v.iter().sum::<u64>());
    let v: Vec<u64> = Liar(64).map(|x| x * 2).collect();
    println!("{} {}", v.len(), v.iter().sum::<u64>());
}
