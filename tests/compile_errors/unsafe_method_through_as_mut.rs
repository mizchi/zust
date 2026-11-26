trait U { unsafe fn u(&self) -> usize; }
impl U for [u8] { unsafe fn u(&self) -> usize { self.len() } }
fn main() { let mut v = vec![1u8, 2]; let n = v.as_mut().u(); let _ = n; }
