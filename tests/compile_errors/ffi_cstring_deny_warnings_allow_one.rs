// `deny(warnings)` makes improper_ctypes an error; an allow on one declaration in a block covers that one only (an
// item's allow is not a crate-level one)
#![deny(warnings)]
use std::ffi::CString;

unsafe extern "C" {
    #[allow(improper_ctypes)]
    fn allowed(s: CString);
    fn denied(s: CString);
}

fn main() {}
