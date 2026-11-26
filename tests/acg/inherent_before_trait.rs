// A method `[T]` / `str` has of its own is taken before a trait's of the name on a `&[T]` / `&str` receiver, also in
// that trait's impl for `[T]` (`self.iter()` there is the slice's, not a call of itself); through the trait's path,
// the trait's
trait It {
    fn iter(&self) -> usize;
    fn count_all(&self) -> usize;
}
impl It for [u8] {
    fn iter(&self) -> usize {
        100 + self.iter().count()
    }
    fn count_all(&self) -> usize {
        self.len()
    }
}
trait Shout {
    fn trim(&self) -> String;
}
impl Shout for str {
    fn trim(&self) -> String {
        format!("<{}>", self.trim())
    }
}
const BIG: [u8; 4096] = [7; 4096];
fn main() {
    let v = [1u8, 2, 3];
    let s: &[u8] = &v;
    println!("{} {} {}", s.iter().count(), It::iter(s), s.count_all());
    println!("{} {}", "  a b  ".trim(), Shout::trim("  a b  "));
    println!("{} {}", BIG.len(), BIG.is_empty());
}
