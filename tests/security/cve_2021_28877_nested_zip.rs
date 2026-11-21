//! CVE-2021-28877 (std, fixed in 1.51.0): nested `Zip`s composed their TrustedRandomAccess specializations wrongly
//! and fetched an index of the inner zip twice. Nested zips must give each triple once, in order.
//@ cve: CVE-2021-28877
//@ expect: run
fn main() {
    let a: Vec<String> = (0..5).map(|i| format!("a{i}")).collect();
    let b: Vec<u32> = (0..4).collect();
    let c = ["x", "y", "z"];
    let t: Vec<String> = a.iter().zip(b.iter()).zip(c.iter()).map(|((s, n), c)| format!("{s}{n}{c}")).collect();
    println!("{:?}", t);
    let mut z = a.iter().zip(b.iter().zip(c.iter()));
    while let Some((s, (n, c))) = z.next() {
        print!("{s}{n}{c} ");
    }
    println!("{:?}", z.next());
}
