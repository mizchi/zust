// A supertrait of std's that the impl's type lacks (#261): String has std's Deref, Name its own, Plain none (E0277)
use std::ops::Deref;
pub unsafe trait StableDeref: Deref {}
struct Name(String);
impl Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}
struct Plain(u8);
unsafe impl StableDeref for String {}
unsafe impl StableDeref for Name {}
unsafe impl StableDeref for Plain {}
fn main() {
    let p = Plain(1);
    println!("{} {}", p.0, Name(String::new()).len());
}
