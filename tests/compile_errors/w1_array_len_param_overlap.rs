struct W<T>(T);
impl<const N: usize> W<[u8; N]> { fn f(&self) -> usize { N } }
impl W<[u8; 3]> { fn f(&self) -> usize { 99 } }
fn main() { println!("{}", W([0u8; 3]).f()); }
