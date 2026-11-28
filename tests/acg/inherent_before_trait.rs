// A method `[T]` / `str` has of its own is taken before a trait's of the name on a `&[T]` / `&str` receiver, also in
// that trait's impl for `[T]` (`self.iter()` there is the slice's, not a call of itself); through the trait's path,
// the trait's. Also with the trait's impl for `&[T]` (`s.iter()` the slice's, `Tr::iter(s)` the impl's); with impls
// for both, the one whose Self the argument is (`Both::iter(&s)` the impl for `&[u8]`'s). A `&mut self` method of the
// slice (`reverse`) comes after a trait's `&self` method of the name for a `&[T]` receiver, before it for `&mut [T]`
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
trait Tr {
    fn iter(self) -> usize;
    fn len(self) -> u8;
}
impl Tr for &[u8] {
    fn iter(self) -> usize {
        self.len() * 10
    }
    fn len(self) -> u8 {
        7
    }
}
trait Both {
    fn iter(&self) -> usize;
}
impl Both for [u8] {
    fn iter(&self) -> usize {
        1
    }
}
impl Both for &[u8] {
    fn iter(&self) -> usize {
        2
    }
}
trait Rev {
    fn reverse(&self) -> usize;
}
impl Rev for [u8] {
    fn reverse(&self) -> usize {
        self.len() * 7
    }
}
const BIG: [u8; 4096] = [7; 4096];
fn main() {
    let v = [1u8, 2, 3];
    let s: &[u8] = &v;
    println!("{} {} {}", s.iter().count(), It::iter(s), s.count_all());
    println!("{} {}", "  a b  ".trim(), Shout::trim("  a b  "));
    println!("{} {}", BIG.len(), BIG.is_empty());
    let n = Tr::iter(s) + 1;
    let m = <&[u8] as Tr>::iter(s);
    let c = s.iter().map(|x| *x as usize).sum::<usize>();
    println!("{n} {m} {} {} {c}", Tr::len(s), s.len());
    println!("{} {}", Both::iter(&s), Both::iter(s));
    let r: usize = s.reverse();
    let mut w = vec![1u8, 2, 3];
    let t: &mut [u8] = &mut w;
    t.reverse();
    let tt = &mut &mut *t;
    tt.reverse();
    println!("{r} {} {:?}", Rev::reverse(s), w);
}
