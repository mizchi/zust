// A string that opens a turbofish and never closes it, and a boxed slice from a `&[u8]` (alone in the program)
fn main() {
    let s = "Box::<";
    let a: &[u8] = &[4, 5];
    let b: Box<[u8]> = Box ::from(a);
    println!("{} {:?}", s, &b[..]);
}
