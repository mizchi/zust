//! A key type whose Hash and Eq disagree (and change between calls): HashMap may give wrong answers, but must stay
//! memory safe (no lost or duplicated values: every value is dropped once).
//@ pattern: trusting a user impl
//@ expect: run
use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
thread_local! {
    static N: Cell<u64> = Cell::new(0);
}
#[derive(Debug)]
struct K(u32);
impl Hash for K {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let n = N.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });
        (n % 7).hash(h);
    }
}
impl PartialEq for K {
    fn eq(&self, o: &K) -> bool {
        (self.0 + o.0) % 3 == 0
    }
}
impl Eq for K {}
fn main() {
    let mut m: HashMap<K, String> = HashMap::new();
    for i in 0..200 {
        m.insert(K(i), format!("v{i}"));
    }
    let mut total = 0;
    for i in 0..200 {
        if m.remove(&K(i)).is_some() {
            total += 1;
        }
    }
    println!("ok {}", m.len() + total <= 200);
}
