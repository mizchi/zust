//! CVE-2020-36317 (std, fixed in 1.49.0): a panic inside the `String::retain` predicate left the String holding
//! invalid UTF-8, observable after `catch_unwind`. zustc does not unwind: a panic ends the program, so the String is
//! never seen again. The panic must still end the run cleanly (exit 101, valgrind clean).
//@ cve: CVE-2020-36317
//@ expect: panic
fn main() {
    let mut s = String::from("aé€😀bé€😀c");
    let mut n = 0;
    s.retain(|c| {
        n += 1;
        if n == 5 {
            panic!("predicate panics");
        }
        c.len_utf8() != 2
    });
    println!("{s}");
}
