//! CVE-2021-28879 (std, fixed in 1.52.0): Zip computed its size with an integer overflow, and a consumed Zip used
//! again overflowed a buffer. Zipping iterators of unequal length yields the shorter length, and a finished zip
//! stays finished.
//@ cve: CVE-2021-28879
//@ expect: run
fn main() {
    let a = vec![String::from("a"), String::from("b"), String::from("c")];
    let b = vec![1, 2];
    let z: Vec<(&String, &i32)> = a.iter().zip(b.iter()).collect();
    println!("{:?}", z);
    let n = a.iter().zip(b.iter()).count();
    println!("{} {}", n, b.iter().zip(a.iter()).count());
    let z: Vec<(String, i32)> = a.clone().into_iter().zip(b.clone().into_iter()).collect();
    println!("{:?}", z);
    let z: Vec<_> = b.iter().zip(a.iter()).map(|(x, s)| format!("{s}{x}")).collect();
    println!("{:?} {:?}", z, a.iter().zip(0..).nth(2));
    // (a finished zip, used again)
    let mut z = a.iter().zip(b.iter());
    while z.next().is_some() {}
    println!("{:?} {:?} {:?}", z.next(), z.next(), z.next());
}
