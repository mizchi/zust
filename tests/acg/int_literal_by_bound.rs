// An integer literal passed as a type parameter bounded by a trait of the program's takes the one integer type that
// implements it (`foo2(42)` with `impl Foo for isize` only), as rustc selects `{integer}: Foo`; with two such types,
// the i32 default stays
trait Foo {
    type A;
    fn boo(&self) -> Self::A;
}
impl Foo for isize {
    type A = usize;
    fn boo(&self) -> usize {
        42
    }
}
impl Foo for char {
    type A = u8;
    fn boo(&self) -> u8 {
        7
    }
}
fn foo2<I: Foo>(x: I) -> <I as Foo>::A {
    x.boo()
}
trait Two {
    fn two(&self) -> &'static str;
}
impl Two for i32 {
    fn two(&self) -> &'static str {
        "i32"
    }
}
impl Two for u8 {
    fn two(&self) -> &'static str {
        "u8"
    }
}
fn which<T: Two>(t: T) -> &'static str {
    t.two()
}
fn size<T>(_: T) -> usize {
    std::mem::size_of::<T>()
}
fn main() {
    let a = 42;
    println!("{} {}", foo2(a), size(a));
    println!("{} {}", foo2('c'), which(5));
}
