//! `std::ptr::read` from safe code (E0133): a bitwise copy that would double drop.
//@ pattern: E0133 context
//@ expect: reject
fn main() {
    let s = String::from("twice");
    let t: String = std::ptr::read(&s); //~ ERROR E0133
    println!("{s} {t}");
}
