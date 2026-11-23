trait Tr {}
struct S<T: Tr>(T);
struct Hoge<K> {
    s: S<K>,
    a: u32,
}
fn main() {}
