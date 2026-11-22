struct Pair<A, B> { a: A, b: B }
fn main() { let p = Pair::<u8, _> { a: 300u16, b: true }; let _ = (p.a, p.b); }
