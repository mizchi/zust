//! CVE-2020-36323 (std, fixed in 1.52.0): `[S]::join` for `S: Borrow<str>` measured the parts and then borrowed
//! them again to copy; a change in between exposed uninitialized bytes. join and concat of ordinary strings must
//! give exactly the joined text (a Borrow impl that changes its answer is cve_2020_36323_join_borrow).
//@ cve: CVE-2020-36323
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
