//! Calling a foreign function from safe code (E0133): `extern` functions are unsafe to call.
//@ rule: the unsafe boundary (E0133 / E0200 / E0512)
//@ expect: reject
unsafe extern "C" {
    fn free(p: *mut u8);
}
fn main() {
    free(16 as *mut u8); //~ ERROR E0133
}
