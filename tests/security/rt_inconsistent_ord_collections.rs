//! Same pattern as CVE-2015-20001 (BinaryHeap left inconsistent by its comparisons): an Ord that is not a total
//! order must leave BinaryHeap and sort memory safe; every element comes out once.
//@ pattern: trusting a user impl
//@ expect: safe
use std::cmp::Ordering;
use std::collections::BinaryHeap;
#[derive(Debug)]
struct W(String, u32);
impl PartialEq for W {
    fn eq(&self, o: &W) -> bool {
        self.1 == o.1
    }
}
impl Eq for W {}
impl PartialOrd for W {
    fn partial_cmp(&self, o: &W) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for W {
    fn cmp(&self, o: &W) -> Ordering {
        if (self.1 * 7 + o.1 * 3) % 5 < 2 { Ordering::Less } else { Ordering::Greater }
    }
}
fn main() {
    let mut h = BinaryHeap::new();
    for i in 0..100 {
        h.push(W(format!("w{i}"), i));
    }
    let mut seen = vec![false; 100];
    while let Some(w) = h.pop() {
        seen[w.1 as usize] = true;
    }
    println!("ok heap {}", seen.iter().all(|&s| s));
    let mut v: Vec<W> = (0..100).map(|i| W(format!("w{i}"), i)).collect();
    v.sort();
    let mut seen = vec![false; 100];
    for w in &v {
        seen[w.1 as usize] = true;
    }
    println!("ok sort {}", seen.iter().all(|&s| s));
}
