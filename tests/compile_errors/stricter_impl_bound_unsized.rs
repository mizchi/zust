// An impl's method that leaves its parameter Sized where the trait's is `?Sized` requires more than the trait (#264):
// rustc's "impl has extra requirement `W: Sized`", at the parameter
use std::fmt;

trait Writeable {
    fn write_to<W: fmt::Write + ?Sized>(&self, sink: &mut W) -> fmt::Result;
}

impl Writeable for u8 {
    fn write_to<W: core::fmt::Write>(&self, sink: &mut W) -> core::fmt::Result {
        write!(sink, "{self}")
    }
}

fn main() {}
