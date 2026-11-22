// `&a == &a` with PartialEq impls for `&TX<u8>` and `&TX<u16>` (and another trait's `eq` first): the receiver's impl
struct TX<T>(T);
trait Other2 {
    fn eq(&self, o: &Self) -> bool;
}
impl Other2 for &TX<u8> {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}
impl PartialEq for &TX<u8> {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}
impl PartialEq for &TX<u16> {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0 + 1
    }
}
fn main() {
    let a = TX(1u16);
    let b = TX(3u8);
    println!("{} {}", &a == &a, &b == &b);
}
