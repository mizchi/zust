use rand::rngs::StdRng;
use rand::seq::{IndexedRandom, SliceRandom};
use rand::{RngExt, SeedableRng};

fn main() {
    let mut r = StdRng::seed_from_u64(2026);
    let a: Vec<u32> = (0..5).map(|_| r.random_range(0..100)).collect();
    println!("ints {:?}", a);
    let f: f64 = r.random();
    println!("float in [0,1): {} {:.6}", (0.0..1.0).contains(&f), f);
    let b: bool = r.random_bool(0.5);
    println!("bool {}", b);
    let mut v: Vec<u8> = (1..=10).collect();
    v.shuffle(&mut r);
    println!("shuffled {:?}", v);
    println!("choose {:?}", v.choose(&mut r));
    let mut t = rand::rng();
    let x: u64 = t.random();
    let y: u64 = t.random();
    println!("thread rng differs: {}", x != y);
    println!("done");
}
