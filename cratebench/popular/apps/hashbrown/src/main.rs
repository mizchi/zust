use hashbrown::{HashMap, HashSet};

fn main() {
    let mut m: HashMap<String, u32> = HashMap::new();
    for w in "the quick brown fox jumps over the lazy dog the end".split(' ') {
        *m.entry(w.to_string()).or_insert(0) += 1;
    }
    m.retain(|_, n| *n > 0);
    let mut v: Vec<_> = m.iter().map(|(k, n)| (k.clone(), *n)).collect();
    v.sort();
    println!("{} words: {:?}", m.len(), v);
    println!("the = {:?}, cat = {:?}", m.get("the"), m.get("cat"));
    let removed = m.remove("fox");
    println!("removed fox = {:?}, len = {}", removed, m.len());
    let a: HashSet<i32> = (0..10).collect();
    let b: HashSet<i32> = (5..15).collect();
    let mut i: Vec<_> = a.intersection(&b).copied().collect();
    i.sort();
    let mut d: Vec<_> = a.symmetric_difference(&b).copied().collect();
    d.sort();
    println!("intersection {:?}", i);
    println!("symmetric difference {:?}", d);
    let mut big: HashMap<u64, u64> = HashMap::with_capacity(4);
    for k in 0..10_000u64 {
        big.insert(k * 7919 % 10_007, k);
    }
    let sum: u64 = big.values().sum();
    println!("big: len {} sum {} cap>=len {}", big.len(), sum, big.capacity() >= big.len());
    println!("done");
}
