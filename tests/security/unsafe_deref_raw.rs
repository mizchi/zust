//! Safe code must not dereference a raw pointer: the `unsafe` boundary (E0133) is what keeps
//! memory errors out of safe Rust. A compiler that accepts this lets safe code read any address.
//@ rule: the unsafe boundary (E0133 / E0200 / E0512)
//@ expect: reject
fn main() {
    let p = 0x10 as *const i32;
    let y = *p; //~ ERROR E0133
    println!("{y}");
}
