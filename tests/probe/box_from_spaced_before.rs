// A boxed slice from a `&[u8]`, its path spelled with a space before `::` (alone in the program)
fn main() {
    let a: &[u8] = &[4, 5];
    let b: Box<[u8]> = Box :: from(a);
    println!("{:?}", &b[..]);
}
