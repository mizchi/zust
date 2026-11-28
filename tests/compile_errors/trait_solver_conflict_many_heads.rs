// E0119 among many impls of one trait with different self types: the pairs compared are those of one head, and those
// with an impl whose self type is a parameter
trait Tr {}
impl Tr for [u8; 1] {}
impl Tr for [u8; 2] {}
impl Tr for u32 {}
impl Tr for [u8; 3] {}
impl Tr for Option<u8> {}
impl Tr for [u8; 2] {}
impl Tr for &u8 {}

fn main() {}
