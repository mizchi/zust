//! Drop check (E0597): a value whose `Drop` reads a reference must not outlive what it borrows.
//! `p` is declared first, so it is dropped after `s`, and its `drop` would read a freed String.
//@ rule: region checking (a reference never outlives what it borrows)
//@ todo: no drop check: a value with Drop may outlive what it borrows
//@ expect: reject
struct Printer<'a>(&'a String);
impl Drop for Printer<'_> {
    fn drop(&mut self) {
        println!("{}", self.0);
    }
}
fn main() {
    let first = String::from("first");
    let mut p = Printer(&first);
    let s = String::from("dropped before p");
    p = Printer(&s); //~ ERROR E0597
    let _ = &p;
}
