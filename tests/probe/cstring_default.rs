// `CString::default()` alone (its body may not use vec!: the prelude selects vec!'s helpers by the program's text)
use std::ffi::CString;

fn main() {
    let c = CString::default();
    println!("{} {:?}", c.as_bytes().len(), c.as_bytes_with_nul());
}
