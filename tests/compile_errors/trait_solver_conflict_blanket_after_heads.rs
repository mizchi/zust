// E0119 between an impl for every type and one of many impls of other heads that comes before it
trait Tr {}
impl Tr for [u8; 1] {}
impl Tr for u32 {}
impl Tr for [u8; 2] {}
impl Tr for Option<u8> {}
impl<T> Tr for T {}

fn main() {}
