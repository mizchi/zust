// #![allow(overflowing_literals)]
fn main() {
    let s = "#[allow(overflowing_literals)]";
    let x: i8 = -129;
    println!("{s} {x}");
}
