// A struct's type parameters with defaults, written in the earlier ones: `W<u32>` is `W<u32, u8>`, `Lazyish<T>` is
// `Lazyish<T, fn() -> T>`; an impl and a function parameter that leave them out
struct W<T, U = u8> {
    a: T,
    b: U,
}
impl<T: Copy> W<T> {
    fn first(&self) -> T {
        self.a
    }
}
fn take(w: &W<u32>) -> u32 {
    w.a + w.b as u32
}
struct Lazyish<T, F = fn() -> T> {
    f: F,
    _t: Option<T>,
}
impl<T, F: Fn() -> T> Lazyish<T, F> {
    fn get(&self) -> T {
        (self.f)()
    }
}
fn seven() -> u32 {
    7
}
fn main() {
    let w: W<u32> = W { a: 1, b: 2 };
    let v: W<u32, u16> = W { a: 3, b: 4 };
    println!("{} {} {}", take(&w), w.first(), v.a as u16 + v.b);
    let l: Lazyish<u32> = Lazyish { f: seven, _t: None };
    println!("{}", l.get());
}
