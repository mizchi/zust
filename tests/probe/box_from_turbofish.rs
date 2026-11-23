// A boxed slice from a `&[u8]`, its path spelled with a turbofish (alone in the program)
fn main() {
    let a: &[u8] = &[4, 5];
    let b: Box<[u8]> = Box::<[u8]>::from(a);
    println!("{:?}", &b[..]);
}
