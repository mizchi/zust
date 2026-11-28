// A `*mut T` where a `*const T` is expected coerces: at a call's argument (ryu's `ptr::copy(p.offset(1), p, n)` with
// p: *mut u8), a method's, a generic's, a fn pointer's, a foreign fn's, a return value and a struct field
use std::ptr;

struct P {
    p: *const u8,
}

struct S;

impl S {
    fn m(&self, p: *const u8) -> u8 {
        unsafe { *p }
    }
}

unsafe fn shift(p: *mut u8, n: usize) {
    unsafe { ptr::copy(p.offset(1), p, n) };
}

fn takes(p: *const u8) -> u8 {
    unsafe { *p }
}

fn read_any<T: Copy>(p: *const T) -> T {
    unsafe { *p }
}

fn ret(p: *mut u8) -> *const u8 {
    if p.is_null() {
        return p;
    }
    p
}

unsafe extern "C" {
    fn strlen(s: *const std::ffi::c_char) -> usize;
}

fn main() {
    let mut a = [1u8, 2, 3, 4, 5];
    unsafe { shift(a.as_mut_ptr(), 4) };
    println!("{:?}", a);
    let m: *mut u8 = a.as_mut_ptr();
    let f: fn(*const u8) -> u8 = |p| unsafe { *p };
    println!("{} {} {} {}", takes(m), S.m(m), read_any(m), read_any::<u8>(m));
    let s = P { p: m };
    println!("{} {} {}", f(m), unsafe { *ret(m) }, unsafe { *s.p });
    let mut c = [b'h', b'i', 0];
    let cp: *mut std::ffi::c_char = c.as_mut_ptr() as *mut std::ffi::c_char;
    println!("{}", unsafe { strlen(cp) });
}
