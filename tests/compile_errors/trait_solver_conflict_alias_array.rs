// Two impls for one array of OsString, written with and without its path (#261): one type to rustc, so E0119, the
// type shown as Rust writes it (`[OsString; 2]`)
use std::ffi::OsString;
trait Tr {}
impl Tr for [OsString; 2] {}
impl Tr for [std::ffi::OsString; 2] {}
fn main() {}
