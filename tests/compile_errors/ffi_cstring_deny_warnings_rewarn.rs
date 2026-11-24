// The crate attribute naming the lint last sets its level: `allow` then `warn` puts improper_ctypes_definitions back
// at warn, and `deny(warnings)` makes that an error
#![deny(warnings)]
#![allow(improper_ctypes_definitions)]
#![warn(improper_ctypes_definitions)]

pub extern "C" fn f(_s: std::ffi::CString) {}

fn main() {}
