//! CVE-2020-36317 (std, fixed in 1.49.0): `String::retain` was not panic safe; a panic in the predicate left the
//! String holding invalid UTF-8. The result of retain over multi-byte characters must be exactly the kept
//! characters (the panic path is cve_2020_36317_string_retain_panic).
//@ cve: CVE-2020-36317
//@ expect: run
fn main() {
    let mut s = String::from("aé€😀bé€😀c");
    s.retain(|c| c.len_utf8() != 2);
    println!("{s} {}", s.len());
    let mut t = String::from("日本語のテキスト");
    let mut i = 0;
    t.retain(|_| {
        i += 1;
        i % 2 == 0
    });
    println!("{t} {} {}", t.len(), t.chars().count());
    println!("{}", std::str::from_utf8(t.as_bytes()).is_ok());
}
