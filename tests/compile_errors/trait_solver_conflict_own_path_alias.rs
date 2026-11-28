// A program's own `type Path = str;` is str, not std's Path (#261): an impl for it conflicts with one for str (E0119)
type Path = str;
trait Tr {}
impl Tr for Path {}
impl Tr for str {}
fn main() {}
