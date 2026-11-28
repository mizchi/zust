// A blanket impl conflicts with an impl for a std type the prelude makes an alias (`type OsString = String`): the
// blanket covers OsString through `impl Marker for OsString` (E0119, rustc: "for type `OsString`"). The alias exempts
// only two impls written for two such types (`.. for OsString` beside `.. for String`), not this (#261)
use std::ffi::OsString;
trait Marker {}
trait Tr {}
impl Marker for OsString {}
impl<T: Marker> Tr for T {}
impl Tr for OsString {}
fn main() {}
