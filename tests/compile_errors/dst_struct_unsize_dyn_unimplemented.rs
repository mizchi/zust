trait Get {
    fn get(&self) -> u64;
}
impl Get for u64 {
    fn get(&self) -> u64 { *self }
}
struct Foo<T: ?Sized> { tag: u8, inner: T }
fn main() {
    let f = Foo { tag: 1, inner: 3u8 };
    let r: &Foo<dyn Get> = &f;
    println!("{}", r.tag);
}
