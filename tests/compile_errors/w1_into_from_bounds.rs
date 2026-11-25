trait Marker {}
struct W<T>(T);
struct J;
impl<T: Marker> From<W<T>> for J { fn from(_: W<T>) -> J { J } }
fn need<T: Into<J>>(_: T) {}
fn main() { need(W(1u8)); }
