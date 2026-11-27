// `p as *mut _` / `p as *const _` takes its pointee from the pointer expected there (rustc unifies the cast's type
// with the expected one), not from the source: errno's `strerror_r(e, buf.as_mut_ptr() as *mut _, n)` with a
// `*mut c_char` parameter and a `[u8]` buffer
use std::ffi::{c_char, c_void};

unsafe extern "C" {
    fn strlen(s: *const c_char) -> usize;
}

fn fill(p: *mut c_char, n: usize) -> usize {
    for i in 0..n - 1 {
        unsafe { *p.add(i) = b'a' as c_char + i as c_char };
    }
    unsafe { *p.add(n - 1) = 0 };
    n - 1
}

fn first(p: *const c_void) -> u8 {
    unsafe { *(p as *const u8) }
}

fn main() {
    let mut buf = [0u8; 8];
    let n = fill(buf.as_mut_ptr() as *mut _, buf.len());
    let len = unsafe { strlen(buf.as_ptr() as *const _) };
    println!("{} {} {:?}", n, len, &buf[..len]);
    // (a `*mut _` where a `*const T` is expected: the `*mut T` that coerces to it)
    let len2 = unsafe { strlen(buf.as_mut_ptr() as *mut _) };
    println!("{} {}", len2, first(buf.as_ptr() as *const _));
    // (no pointer expected: the source's pointee, as before)
    let q = &buf[1] as *const _;
    let r: *const u8 = q;
    println!("{}", unsafe { *r });
}
