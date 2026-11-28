struct W<T>(T);
impl<T> W<T> {
    const C: u8 = 1;
}
impl W<u16> {
    const C: u8 = 2;
}
fn main() {
    println!("{}", W::<u16>::C);
}
