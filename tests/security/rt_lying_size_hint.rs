//! Same pattern as CVE-2021-28879 / CVE-2021-28875 (std trusting a number another implementation reports): a
//! user iterator whose size_hint is wrong (too small, too large, and an upper bound below the lower one) must not
//! make collect / extend write past what they allocated.
//@ pattern: trusting a user impl
//@ expect: run
struct Liar {
    left: u32,
    hint: (usize, Option<usize>),
}
impl Iterator for Liar {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        Some(format!("s{}", self.left))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.hint
    }
}
fn main() {
    let v: Vec<String> = Liar { left: 50, hint: (0, Some(0)) }.collect();
    println!("{} {:?}", v.len(), v.last());
    let v: Vec<String> = Liar { left: 3, hint: (1000, Some(1000)) }.collect();
    println!("{} {:?}", v.len(), v.last());
    let v: Vec<String> = Liar { left: 20, hint: (10, Some(2)) }.collect();
    println!("{}", v.len());
    let mut w = vec![String::from("x")];
    w.extend(Liar { left: 40, hint: (1, Some(1)) });
    println!("{} {:?}", w.len(), w.last());
    let s: String = Liar { left: 30, hint: (0, Some(0)) }.collect();
    println!("{}", s.len());
}
