// Calls through a path that names an associated type before the fn: `T::U::m()`, `<T as Foo>::U::m()` (with the
// fn's own type arguments), `<T::U as Bar>::m()`; `T::Unsigned::from(x)` reaching a number's builtin `from`; and
// `<u16 as From<u8>>::from(x)` / `<u32 as TryFrom<u64>>::try_from(x)`
trait Foo {
    type U;
}
trait Bar {
    fn method() -> u32;
    fn gm<T>() -> usize;
}
struct MyFoo;
struct MyBar;
impl Foo for MyFoo {
    type U = MyBar;
}
impl Bar for MyBar {
    fn method() -> u32 {
        2
    }
    fn gm<T>() -> usize {
        std::mem::size_of::<T>()
    }
}
fn a<T: Foo>() -> u32 where T::U: Bar {
    T::U::method()
}
fn b<T: Foo>() -> usize where T::U: Bar {
    <T as Foo>::U::method() as usize + <T as Foo>::U::gm::<f64>()
}
fn c<T: Foo>() -> u32 where T::U: Bar {
    <T::U as Bar>::method()
}
trait UInt: Copy + From<u8> {}
impl UInt for u16 {}
trait Int: Copy {
    type Unsigned: UInt;
    fn as_unsigned(self) -> Self::Unsigned;
}
impl Int for i16 {
    type Unsigned = u16;
    fn as_unsigned(self) -> u16 {
        self as _
    }
}
fn widen<T: Int>(x: u8, y: T) -> (T::Unsigned, T::Unsigned) {
    (T::Unsigned::from(x), y.as_unsigned())
}
fn main() {
    println!("{} {} {}", a::<MyFoo>(), b::<MyFoo>(), c::<MyFoo>());
    let (p, q) = widen(7, -1i16);
    println!("{p} {q}");
    let r = <u16 as From<u8>>::from(200u8) + <u16 as From<u8>>::from(6);
    let s = <u32 as TryFrom<u64>>::try_from(5u64).unwrap();
    let t = <u32 as TryFrom<u64>>::try_from(1u64 << 40).is_err();
    println!("{r} {s} {t}");
    let (a, b, c) = (<f64 as From<f32>>::from(1.5), <u32 as TryFrom<i64>>::try_from(-5).is_err(), <i64 as From<i8>>::from(-3));
    println!("{a} {b} {c}");
    // (suffixed literals keep their own suffix; `1E3` is a float)
    println!("{} {} {}", <f64 as From<f32>>::from(1f32), <f64 as From<f32>>::from(1E3), <u16 as From<u8>>::from(5u8));
}
