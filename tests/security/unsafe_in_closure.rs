//! A closure inside a safe function is safe code too: it may not dereference a raw pointer (E0133).
//@ expect: reject
fn main() {
    let p = 0x20 as *const u64;
    let f = || *p; //~ ERROR E0133
    println!("{}", f());
}
