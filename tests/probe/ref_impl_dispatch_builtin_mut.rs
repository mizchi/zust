// A slice's own `as_mut_ptr(&mut self)` beside `impl<T: ?Sized> Tr for &T`'s `as_mut_ptr(self)` (#144): at a shared
// `&[u8]` the slice's method does not apply (it takes `&mut [u8]`), so the impl's runs; at a `&mut [u8]` the slice's
// comes first; `(*m).as_mut_ptr()` of the place `*m` takes the impl's at the `&` autoref, before the slice's at the
// `&mut` one. A str's `len(&self)` at a `&mut str` does not apply (the impl for `&mut T`'s)
trait Tr {
    fn as_mut_ptr(self) -> *mut u8;
}
impl<T: ?Sized> Tr for &T {
    fn as_mut_ptr(self) -> *mut u8 {
        std::ptr::null_mut()
    }
}
trait Ln {
    fn len(self) -> usize;
}
impl<T: ?Sized> Ln for &mut T {
    fn len(self) -> usize {
        99
    }
}
fn main() {
    let v = [1u8, 2];
    let s: &[u8] = &v;
    println!("{}", s.as_mut_ptr().is_null());
    let mut w = [1u8, 2];
    let m: &mut [u8] = &mut w;
    println!("{}", m.as_mut_ptr().is_null());
    println!("{}", (*m).as_mut_ptr().is_null());
    let mut t = String::from("abc");
    let ms: &mut str = t.as_mut_str();
    println!("{}", ms.len());
}
