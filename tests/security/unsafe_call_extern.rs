//! Calling a foreign function from safe code (E0133): `extern` functions are unsafe to call.
//@ expect: reject
unsafe extern "C" {
    fn free(p: *mut u8);
}
fn main() {
    free(16 as *mut u8); //~ ERROR E0133
}
