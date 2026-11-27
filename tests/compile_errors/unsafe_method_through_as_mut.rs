trait U { unsafe fn u(&self) -> usize; }
impl U for str { unsafe fn u(&self) -> usize { self.len() } }
fn main() { let mut s = String::from("ab"); let n = s.as_mut().u(); let _ = n; }
