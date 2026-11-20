//! Kani harnesses for zustc's prelude `Vec<T>` (the allocator side of the runtime).
//!
//! `build.rs` copies the prelude's code; this file gives it what zustc gives it: libc `realloc` / `free` (here a
//! model of them: a block knows its size, `realloc` copies `min(old, new)` bytes and returns null when the size
//! is too large, or when `ALLOC_MAY_FAIL` is set and the solver chooses so), `abort` (a panic) and `__exr_drop_in_place` (zustc lowers it to the drop of T).
//!
//! `cargo kani` runs every harness (all are expected to pass).

#![allow(dead_code, clippy::all)]

pub mod alloc_model {
    use std::alloc::{Layout, alloc, dealloc};

    const HDR: usize = 16;
    pub static mut LIVE: usize = 0;
    pub static mut ALLOC_MAY_FAIL: bool = false;

    unsafe fn layout(size: usize) -> Layout {
        Layout::from_size_align(size + HDR, HDR).unwrap()
    }

    /// libc `realloc`: null gives a new block; size 0 is a valid (non-null) block
    pub unsafe fn realloc(p: *mut u8, size: usize) -> *mut u8 {
        unsafe {
            #[cfg(kani)]
            if ALLOC_MAY_FAIL && kani::any() {
                return core::ptr::null_mut();
            }
            if size > isize::MAX as usize - 2 * HDR {
                return core::ptr::null_mut();
            }
            let q = alloc(layout(size));
            assert!(!q.is_null());
            *(q as *mut usize) = size;
            let q = q.add(HDR);
            LIVE += 1;
            if !p.is_null() {
                let old = *(p.sub(HDR) as *const usize);
                core::ptr::copy_nonoverlapping(p, q, old.min(size));
                free(p);
            }
            q
        }
    }

    pub unsafe fn free(p: *mut u8) {
        unsafe {
            if p.is_null() {
                return;
            }
            let b = p.sub(HDR);
            let size = *(b as *const usize);
            dealloc(b, layout(size));
            LIVE -= 1;
        }
    }
}

unsafe fn __exr_realloc(p: *mut u8, size: usize) -> *mut u8 {
    unsafe { alloc_model::realloc(p, size) }
}
unsafe fn __exr_free(p: *mut u8) {
    unsafe { alloc_model::free(p) }
}
/// (abort: the program stops. A panic here, so that `#[kani::should_panic]` tells it from undefined behaviour)
unsafe fn __exr_abort() {
    panic!("abort");
}
fn __exr_err_str<G>(_s: &str) {}
fn __exr_drop_in_place<T>(p: *mut T) {
    unsafe { core::ptr::drop_in_place(p) }
}

include!(concat!(env!("OUT_DIR"), "/vec.rs"));

#[cfg(kani)]
mod harnesses;
