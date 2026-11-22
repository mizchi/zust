#[repr(u8)]
enum E { Z = 0, A = !0, B }
fn main() { println!("{}", E::Z as u8); }
