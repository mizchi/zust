//! CVE-2021-28879 / CVE-2021-28878 (`Zip` with iterators of unequal length could run past the
//! shorter one). Zipping unequal iterators yields exactly the shorter length, and the longer
//! iterator is left in a consistent state.
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
}
