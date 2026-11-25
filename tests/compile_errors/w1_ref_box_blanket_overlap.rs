trait Marker {}
trait Flag {}
struct S;
impl Marker for &Box<S> {}
impl<T: Marker> Flag for T {}
impl Flag for &Box<S> {}
fn main() {}
