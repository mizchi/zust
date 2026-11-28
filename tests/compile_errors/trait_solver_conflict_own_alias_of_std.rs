// A program's own alias of OsString is OsString (#261): an impl for it conflicts with one for OsString (E0119), though
// an impl for OsString and one for String (one type in zustc's prelude) are kept apart
use std::ffi::OsString;
type MyOs = OsString;
trait Tr {}
impl Tr for MyOs {}
impl Tr for OsString {}
fn main() {}
