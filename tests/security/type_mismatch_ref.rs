//! A `&u8` passed where `&u64` is expected (E0308): reading 8 bytes from a 1-byte place.
//@ rule: type checking
//@ expect: reject
fn read(x: &u64) -> u64 {
    *x
}
fn main() {
    let b: u8 = 1;
    println!("{}", read(&b)); //~ ERROR E0308
}
