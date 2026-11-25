// core::ffi's C types (libc 0.2 re-exports them): `size_of::<c_int>()` in a constant (glibc's
// `__PTHREAD_MUTEX_INITIALIZER`) and their associated constants, `c_int::MAX` (libc's types.rs)
use core::ffi::{c_int, c_long, c_uint, c_ulong};

const K: usize = 4 * core::mem::size_of::<c_int>();
const L: usize = core::mem::size_of::<c_long>() + core::mem::align_of::<c_uint>();

const fn ulong_cast_int(x: c_ulong) -> c_int {
    assert!(x <= (c_int::MAX as c_ulong));
    x as c_int
}

fn main() {
    let a = [0u8; K];
    println!("{} {} {}", a.len(), L, ulong_cast_int(7));
    println!("{} {} {}", c_int::MAX, c_uint::MAX, c_ulong::MIN);
}
