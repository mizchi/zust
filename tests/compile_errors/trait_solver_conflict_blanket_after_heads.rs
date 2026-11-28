// E0119 between an impl for every type that meets a bound and an earlier impl of another head that meets it
trait Marker {}
impl Marker for [u8; 1] {}

trait Tr {}
impl Tr for u32 {}
impl Tr for [u8; 1] {}
impl Tr for Option<u8> {}
impl<T: Marker> Tr for T {}

fn main() {}
