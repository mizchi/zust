struct Pair<A, B> { a: A, b: B }
fn main() { let t = true; let base = Pair { a: 1u16, b: &t }; let p = Pair::<u8, &'_ _> { ..base }; let _ = (p.a, p.b); }
