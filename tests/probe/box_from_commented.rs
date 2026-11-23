// A boxed slice from a `&[u8]`, a block comment and a line comment between its path tokens (alone in the program)
fn main() {
    let a: &[u8] = &[4, 5];
    let b: Box<[u8]> = Box /* c */ :: // c
        from(a);
    println!("{:?}", &b[..]);
}
