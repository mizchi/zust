// chacha20 0.10's ChaChaCore::new: a `&mut [T]` passed for a generic `&T` (`size_of_val(iv_dst)` with iv_dst from
// split_at_mut, the slice's size), and `zip(dst)` over a `&mut [T]`, whose items are `&mut T`
use core::mem::size_of_val;
fn sizes() {
    let mut s = [0u32; 6];
    let (a, b) = s.split_at_mut(2);
    let iv = [1u8; 16];
    assert_eq!(size_of_val(b), size_of_val(&iv[..]));
    println!("{} {} {}", size_of_val(a), size_of_val(b), core::mem::size_of_val(&mut s));
}
fn f<T: ?Sized>(_x: &T) -> u8 { 1 }
fn g<T>(_x: &T) -> u8 { 2 }
fn generic() {
    let mut s = [0u32; 6];
    let m: &mut [u32] = &mut s[..];
    println!("{}", f(m));
    let mut x = 5u8;
    let r = &mut x;
    println!("{}", g(r));
}
fn zips() {
    let key = [1u8, 0, 0, 0, 2, 0, 0, 0];
    let mut state = [0u32; 4];
    let (dst2, rest) = state.split_at_mut(2);
    for (src, dst) in key.chunks_exact(4).zip(dst2) {
        *dst = u32::from_le_bytes(src.try_into().unwrap());
    }
    for (d, s) in rest.iter_mut().zip([7u32, 8]) {
        *d = s;
    }
    let mut v = vec![0u8; 2];
    for (i, x) in (0u8..).zip(&mut v) {
        *x = i + 3;
    }
    println!("{:?} {:?}", state, v);
}

fn main() {
    sizes();
    generic();
    zips();
}
