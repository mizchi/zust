// `improper_ctypes` denied for the crate: an allow takes effect from any entry of its list, on the block it is
// written on or on one declaration in a block; a sibling declaration without one is still rejected
#![deny(improper_ctypes)]
use std::ffi::CString;

#[allow(dead_code, improper_ctypes)]
unsafe extern "C" {
    fn consume(s: CString);
}

unsafe extern "C" {
    #[allow(unused, improper_ctypes)]
    fn allowed(s: CString);
    fn denied(s: CString);
}

fn main() {}
