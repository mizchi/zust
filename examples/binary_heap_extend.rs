// Sample of BinaryHeap's append / extend and Vec::extend: the same rebuild_tail decision as std
// (sift_up if few are added, a rebuild if many; including the decision beyond 2048 elements), so the order is also the same as std
use std::collections::BinaryHeap;

fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *s >> 40
}

fn checksum(h: &BinaryHeap<u64>) -> u64 {
    let mut c = 0u64;
    for (i, x) in h.iter().enumerate() {
        c = c.wrapping_mul(31).wrapping_add(*x ^ (i as u64));
    }
    c
}

fn main() {
    // extend: Vec / array / range / iterator / references
    let mut h = BinaryHeap::new();
    h.extend(vec![5, 1, 8]);
    println!("{:?}", h);
    h.extend([7, 2]);
    println!("{:?}", h);
    h.extend(10..13);
    println!("{:?}", h);
    let more = vec![4, 9, 0];
    h.extend(&more);
    h.extend(more.iter().map(|x| x * 3));
    println!("{:?} {}", h, h.len());
    // Add just a few (the sift_up path)
    let mut big = BinaryHeap::new();
    let mut seed = 7u64;
    for _ in 0..100 {
        big.push(lcg(&mut seed));
    }
    big.extend(vec![lcg(&mut seed), lcg(&mut seed)]);
    println!("{} {}", big.len(), checksum(&big));
    // Add many (the rebuild path)
    let v: Vec<u64> = (0..300).map(|_| lcg(&mut seed)).collect();
    big.extend(v);
    println!("{} {}", big.len(), checksum(&big));
    // Beyond 2048 elements
    let mut huge = BinaryHeap::new();
    for _ in 0..3000 {
        huge.push(lcg(&mut seed));
    }
    let w: Vec<u64> = (0..200).map(|_| lcg(&mut seed)).collect();
    huge.extend(w);
    println!("{} {}", huge.len(), checksum(&huge));
    let w2: Vec<u64> = (0..400).map(|_| lcg(&mut seed)).collect();
    huge.extend(w2);
    println!("{} {}", huge.len(), checksum(&huge));
    // append (adds to the larger one; other becomes empty)
    let mut a = BinaryHeap::from(vec![1, 5, 3]);
    let mut b = BinaryHeap::from(vec![9, 2, 8, 7, 6]);
    a.append(&mut b);
    println!("{:?} {} {}", a, a.len(), b.is_empty());
    let mut c = BinaryHeap::from(vec![String::from("m"), String::from("x")]);
    let mut d = BinaryHeap::new();
    d.push(String::from("a"));
    c.append(&mut d);
    println!("{:?} {:?}", c.into_sorted_vec(), d.len());
    let mut e: BinaryHeap<u64> = (0..50).map(|_| lcg(&mut seed)).collect();
    let mut f: BinaryHeap<u64> = (0..3).map(|_| lcg(&mut seed)).collect();
    e.append(&mut f);
    println!("{}", checksum(&e));
    let mut g: BinaryHeap<u64> = (0..3).map(|_| lcg(&mut seed)).collect();
    let mut k: BinaryHeap<u64> = (0..50).map(|_| lcg(&mut seed)).collect();
    g.append(&mut k);
    println!("{} {}", checksum(&g), k.len());
    // Vec::extend
    let mut xs = vec![1, 2];
    xs.extend(vec![3, 4]);
    xs.extend(&[5, 6]);
    xs.extend(7..9);
    xs.extend(xs.clone().iter().filter(|x| **x % 2 == 0));
    println!("{:?}", xs);
    let mut ws: Vec<String> = vec![String::from("a")];
    ws.extend(vec![String::from("b"), String::from("c")]);
    ws.extend(["d", "e"].iter().map(|s| s.to_string()));
    println!("{:?}", ws);
}
