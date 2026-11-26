// Methods of impls for trait objects (#237's review): `E3::pick(x)` calls E3's default, not the inherent `pick` of
// `impl dyn O3`; an impl's own method for the object; a result type solved from the object's arguments
// (`impl<T> Get<T> for dyn O<T>`) and from the type expected (`let v: u8 = x.make()`)
trait O3 {}
impl dyn O3 {
    fn pick(&self) -> u8 {
        3
    }
    fn make<T: Default>(&self) -> T {
        T::default()
    }
}
trait E3 {
    fn pick(&self) -> u8 {
        2
    }
}
impl E3 for dyn O3 {}
trait O4 {}
trait E4 {
    fn pick(&self) -> u8 {
        2
    }
}
impl E4 for dyn O4 {
    fn pick(&self) -> u8 {
        5
    }
}
trait O<T> {}
trait Get<T> {
    fn get(&self) -> T;
}
impl<T: Default> Get<T> for dyn O<T> + '_ {
    fn get(&self) -> T {
        T::default()
    }
}
struct S;
impl O3 for S {}
impl O4 for S {}
impl O<u8> for S {}
fn shown(x: &dyn O<u8>) -> impl std::fmt::Display {
    x.get()
}
fn main() {
    let s = S;
    let x: &(dyn O3 + 'static) = &s;
    println!("{} {}", E3::pick(x), x.pick());
    let v: u8 = x.make();
    println!("{}", v);
    let y: &(dyn O4 + 'static) = &s;
    println!("{} {}", E4::pick(y), y.pick());
    let z: &dyn O<u8> = &s;
    let g = z.get();
    println!("{} {}", g + 1, shown(z));
}
