//! CVE-2021-28876 and CVE-2021-28878 (std, fixed in 1.52.0): Zip's TrustedRandomAccess specialization fetched the
//! same index twice (after a panic in the inner iterator, or when `next` and `next_back` were mixed), handing out
//! two copies of one element. Mixing both ends of a zip must give each pair once.
//! Not applicable yet: zustc's Zip has no `next_back` (it is not a DoubleEndedIterator), so the build is rejected; when it builds, this case fails
//! and must become a test of the behavior above.
//@ cve: CVE-2021-28876, CVE-2021-28878
//@ expect: unsupported
fn main() {
    let a: Vec<String> = (0..6).map(|i| format!("a{i}")).collect();
    let b: Vec<String> = (0..4).map(|i| format!("b{i}")).collect();
    let mut z = a.iter().zip(b.iter());
    println!("{:?}", z.next());
    println!("{:?}", z.next_back());
    println!("{:?}", z.next());
    println!("{:?}", z.next_back());
    println!("{:?}", z.next());
    let v: Vec<(String, String)> = a.clone().into_iter().zip(b.clone()).rev().collect();
    println!("{:?}", v);
}
