//! A non-exhaustive match (E0004): the unmatched case would run off the end of the match.
//@ expect: reject
enum E {
    A,
    B,
    C,
}
fn f(e: E) -> u32 {
    match e { //~ ERROR E0004
        E::A => 1,
        E::B => 2,
    }
}
fn main() {
    println!("{}", f(E::C));
}
