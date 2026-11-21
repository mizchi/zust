//! An out-of-range literal (overflowing_literals is deny by default): `256u8` is not 0.
//@ rule: integer literals in range (overflowing_literals)
//@ expect: reject
fn main() {
    let x: u8 = 256; //~ ERROR literal out of range
    println!("{x}");
}
