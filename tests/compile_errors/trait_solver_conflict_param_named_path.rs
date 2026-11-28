// An impl's parameter named `Path` is the parameter, not std's `Path` (the prelude's alias of str): the blanket impl
// covers S through `impl Marker for S` (E0119, #261)
use std::path::Path;
trait Marker {}
trait Tr {}
struct S;
impl Marker for S {}
impl<Path: Marker> Tr for Path {}
impl Tr for S {}
fn main() {
    let _ = Path::new("x");
}
