//! CVE-2018-1000810 (std, fixed in 1.29.1): `str::repeat` computed `len * n` with an overflow that wrapped, so the
//! String it allocated was too small and the copies wrote past its end. Before this change zustc's prelude had the
//! same bug (its build of this case looped writing past the buffer). Each argument is one size that overflows; all
//! must panic ("capacity overflow").
//@ cve: CVE-2018-1000810
//@ expect: panic
//@ args: 0 1 2
fn main() {
    let k: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    let n = usize::MAX / 2 + 1 + (k & 0);
    match k {
        0 => println!("{}", "ab".repeat(n).len()),
        1 => println!("{}", String::from("abc").repeat(n / 2).len()),
        _ => println!("{}", "x".repeat(usize::MAX - (k & 0)).len()),
    }
    println!("not reached");
}
