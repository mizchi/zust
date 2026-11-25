struct S;
type Alias = S;
trait Marker {}
impl Marker for S {}
struct W<T>(T);
trait Tr {}
impl<T: Marker> Tr for W<T> {}
impl Tr for W<Alias> {}
fn main() {}
