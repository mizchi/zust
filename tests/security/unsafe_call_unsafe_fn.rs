//! Calling an `unsafe fn` from safe code (E0133).
//@ rule: the unsafe boundary (E0133 / E0200 / E0512)
//@ expect: reject
unsafe fn read(p: *const u8) -> u8 {
    unsafe { *p }
}
fn main() {
    let v = read(8 as *const u8); //~ ERROR E0133
    println!("{v}");
}
