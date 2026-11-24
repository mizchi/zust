// C strings: a literal `c"..."` (getrandom 0.4, libc's users), `CStr::as_ptr` / `from_ptr` across C's strlen, and
// CString's bytes with and without the 0
use std::ffi::{CStr, CString};

unsafe extern "C" {
    fn strlen(s: *const i8) -> usize;
}

const PATH: &CStr = c"/dev/urandom";

fn main() {
    let lit = c"hello libc";
    let n = unsafe { strlen(lit.as_ptr()) };
    println!("{} {:?} {}", n, lit.to_bytes(), lit.to_str().unwrap());
    println!("{} {}", PATH.to_bytes().len(), PATH.to_bytes_with_nul().len());
    let owned = CString::new("abc").unwrap();
    println!("{:?} {:?} {}", owned.as_bytes(), owned.as_bytes_with_nul(), unsafe { strlen(owned.as_ptr()) });
    let back = unsafe { CStr::from_ptr(owned.as_ptr()) };
    println!("{} {}", back.to_str().unwrap(), back.to_bytes().len());
    println!("{:?}", CString::new("a\0b").is_err());
    let v = CString::new(vec![b'x', b'y']).unwrap().into_bytes();
    println!("{:?} {}", v, CString::default().as_bytes().len());
}
