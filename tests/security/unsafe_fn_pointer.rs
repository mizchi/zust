//! A pointer to an `unsafe fn` is still unsafe to call (E0133).
//@ rule: the unsafe boundary (E0133 / E0200 / E0512)
//@ todo: fn pointer types do not record `unsafe`, so calling one needs no unsafe block
//@ expect: reject
unsafe fn danger() -> i32 {
    7
}
fn main() {
    let f: unsafe fn() -> i32 = danger;
    let x = f(); //~ ERROR E0133
    println!("{x}");
}
