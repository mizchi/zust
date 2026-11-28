// A method of `impl<T: Size> Size for &T` whose turbofish is not its own parameter (`::<7>` for `N`): a call through
// the bound runs it (rustc prints 7, T's method would give 3)
trait Size {
    fn size<const N: usize>(&self) -> usize;
}
struct S;
impl Size for S {
    fn size<const N: usize>(&self) -> usize {
        N
    }
}
impl<'a, T: ?Sized + Size> Size for &'a T {
    fn size<const N: usize>(&self) -> usize {
        (**self).size::<7>()
    }
}
fn get<Z: for<'b> Size>(z: Z) -> usize {
    z.size::<3>()
}
fn main() {
    println!("{}", get(&S));
}
