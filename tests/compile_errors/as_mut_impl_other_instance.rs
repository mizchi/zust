struct W<T>(T, String);
impl AsMut<str> for W<u8> { fn as_mut(&mut self) -> &mut str { self.1.as_mut_str() } }
fn f<T: AsMut<str>>(_: &mut T) {}
fn main() { let mut w = W(1i32, String::new()); f(&mut w); }
