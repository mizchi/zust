// `w.conv::<u16>()` / `W::<u8>::make::<3>()`: a method's turbofish names its own parameters, after its impl's
struct W<T>(T);
impl<T: Copy + std::fmt::Debug> W<T> {
    fn conv<U: Default + std::fmt::Debug>(&self) -> (T, U) {
        (self.0, U::default())
    }
    fn make<const N: usize>(x: T) -> [T; N] {
        [x; N]
    }
}
trait Tr {
    fn pick<const B: bool>(&self) -> u8;
}
impl<T> Tr for W<T> {
    fn pick<const B: bool>(&self) -> u8 {
        if B { 1 } else { 0 }
    }
}
fn main() {
    let w = W(7u8);
    let (a, b) = w.conv::<u16>();
    let arr = W::<u8>::make::<3>(5);
    println!("{} {:?} {:?} {} {}", a, b, arr, w.pick::<true>(), w.pick::<false>());
}
