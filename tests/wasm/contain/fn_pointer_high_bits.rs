//! A function pointer with bits past 32 set: narrowed it would be a valid table index of the same signature. The
//! call checks the full value and traps.
fn hello() {
    println!("called");
}
fn main() {
    let f: fn() = hello;
    let bad = std::hint::black_box(f as usize + (1usize << 32));
    let g: fn() = unsafe { std::mem::transmute(bad) };
    g();
    println!("after");
}
