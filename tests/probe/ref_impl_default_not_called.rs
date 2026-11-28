// An impl for `&T` that leaves out a default method of its trait (rustc would run it with Self = `&T`), which the
// program never calls: the impl is accepted, and its required method forwards to T's (#144)
trait Hash {
    fn hash(&self, s: &mut u64);
    fn hash_slice(data: &[Self], s: &mut u64)
    where
        Self: Sized,
    {
        for d in data {
            d.hash(s);
        }
    }
}
impl Hash for u8 {
    fn hash(&self, s: &mut u64) {
        *s = *s * 31 + *self as u64;
    }
}
impl<'a, T: 'a + Hash + ?Sized> Hash for &'a T {
    fn hash(&self, s: &mut u64) {
        (*self).hash(s)
    }
}
fn digest<X: Hash>(x: X) -> u64 {
    let mut s = 7;
    x.hash(&mut s);
    s
}
fn main() {
    println!("{} {}", digest(&5u8), digest(&&9u8));
}
