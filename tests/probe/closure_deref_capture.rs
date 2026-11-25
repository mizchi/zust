// A closure that uses a shared reference it captures only through a deref of it holds a copy of the reference
// (edition 2021 captures `*h`): collected from a `filter_map`, boxed and called after the fn returns
struct H<'a>(&'a str);
impl<'a> H<'a> {
    fn mk(&self) -> Option<&str> { Some(self.0) }
    fn n(&self) -> usize { self.0.len() }
    fn all<'s>(&'s self, v: &[u8]) -> Vec<&'s str> { v.iter().filter_map(|_| self.mk()).collect() }
    fn boxed<'s>(&'s self) -> Box<dyn Fn() -> usize + 's> { Box::new(|| self.n()) }
}
fn f4<'r>(h: &'r H<'r>, v: Vec<u8>) -> Vec<&'r str> { v.iter().filter_map(|_| h.mk()).collect() }
fn f5<'r>(h: &'r H<'r>) -> Box<dyn Fn() -> usize + 'r> { Box::new(|| h.n()) }
fn main() { let h = H("ab"); println!("{:?} {} {:?} {}", h.all(&[1, 2]), h.boxed()(), f4(&h, vec![1]), f5(&h)()); }
