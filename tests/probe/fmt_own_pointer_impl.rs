// a program's own trait named Pointer with an impl for a raw pointer: `Pointer::fmt(&p, f)` calls that impl, not std's
// address formatting
use std::fmt;
trait Pointer { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result; }
impl Pointer for *const u8 { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { write!(f, "mine") } }
struct S(*const u8);
impl fmt::Display for S { fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { Pointer::fmt(&self.0, f) } }
fn main() { let x = 1u8; println!("{}", S(&x as *const u8)); }
