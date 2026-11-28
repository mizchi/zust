struct W<T>(T);
impl<T> W<T> {
    fn new() -> u8 {
        1
    }
}
impl W<u8> {
    fn new() -> u8 {
        2
    }
}
fn main() {
    println!("{}", W::<u8>::new());
}
