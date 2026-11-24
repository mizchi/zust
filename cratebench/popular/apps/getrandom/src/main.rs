fn main() {
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    getrandom::fill(&mut a).expect("fill a");
    getrandom::fill(&mut b).expect("fill b");
    println!("filled {} and {} bytes", a.len(), b.len());
    println!("a is not all zero: {}", a.iter().any(|&x| x != 0));
    println!("a and b differ: {}", a != b);
    let x = getrandom::u64().expect("u64");
    let y = getrandom::u64().expect("u64");
    println!("two u64 differ: {}", x != y);
    let mut empty = [0u8; 0];
    println!("empty fill ok: {}", getrandom::fill(&mut empty).is_ok());
    println!("done");
}
