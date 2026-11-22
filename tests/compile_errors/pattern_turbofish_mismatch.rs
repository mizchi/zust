struct S<T>(T);
fn take(S::<u16>(x): S<u8>) -> u8 { x }
fn main() { println!("{}", take(S(3))); }
