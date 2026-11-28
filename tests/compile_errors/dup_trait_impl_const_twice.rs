trait Tr<T> {
    const C: u8;
}
struct S;
impl Tr<u8> for S {
    const C: u8 = 1;
    const C: u8 = 2;
}
impl Tr<u16> for S {
    const C: u8 = 3;
}
fn main() {}
