//! Overlapping impls (E0119): with two impls of one trait for the same type, which method runs
//! would depend on the caller, and an associated type could be two types at once.
//@ rule: coherence (E0119)
//@ expect: reject
trait Size {
    fn size(&self) -> usize;
}
impl<T> Size for T {
    fn size(&self) -> usize {
        1
    }
}
impl Size for u8 { //~ ERROR E0119
    fn size(&self) -> usize {
        2
    }
}
fn main() {
    println!("{}", 5u8.size());
}
