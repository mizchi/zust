// A value path no reading of zustc's answers is named in the error, as rustc's E0425 / E0599 name theirs (#265): it
// was "cannot find value (unsupported path)". rustc 1.97: "no associated function or constant named `NOPE` found for
// type `u32` in the current scope" (E0599)
fn main() {
    let x: u32 = u32::NOPE;
    println!("{x}");
}
