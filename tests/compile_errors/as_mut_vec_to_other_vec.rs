fn f<T: AsMut<Vec<u8>>>(_: &mut T) {}
fn main() { let mut v: Vec<u16> = vec![1]; f(&mut v); }
