//! CVE-2020-36323 (`[S]::join` with a `Borrow<str>` impl that answers differently on two calls
//! exposed uninitialized memory). join / concat must produce exactly the joined text.
//@ expect: run
fn main() {
    let parts = vec![String::from("ab"), String::new(), String::from("ñ€"), String::from("z")];
    let j = parts.join("--");
    println!("{j} {}", j.len());
    let c = parts.concat();
    println!("{c} {}", c.len());
    let refs: Vec<&str> = parts.iter().map(|s| s.as_str()).collect();
    println!("{}", refs.join(""));
    let vv = vec![vec![1u8, 2], vec![], vec![3]];
    println!("{:?}", vv.concat());
}
