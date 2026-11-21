//! `transmute` between types of different sizes is rejected at compile time (E0512), even in an
//! `unsafe` block: otherwise it reads past the end of the source value.
//@ expect: reject
fn main() {
    let x: u32 = 5;
    let y: u64 = unsafe { std::mem::transmute(x) }; //~ ERROR E0512
    println!("{y}");
}
