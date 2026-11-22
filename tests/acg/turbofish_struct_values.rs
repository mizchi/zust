// `U::<u8>(7)` / `Foo::<3>([1, 2, 3])` / `Generic::<5>`: a struct's constructor or a unit struct with a turbofish
struct Foo<const N: usize>([u8; N]);
struct Generic<const N: usize>;
struct S<T>(Option<T>);
struct U<T>(T);
impl<const N: usize> Generic<N> {
    fn n(&self) -> usize {
        N
    }
}
fn main() {
    let a = Foo::<3>([1, 2, 3]);
    let g = Generic::<5>;
    let s = S::<()>(None);
    let u = U::<u8>(7);
    println!("{} {} {} {}", a.0.len(), g.n(), s.0.is_none(), u.0);
}
