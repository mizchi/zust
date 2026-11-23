const A: u8 = 255;
const N: usize = (A + 1) as usize;
fn main() { println!("{}", std::mem::size_of::<[u8; N]>()); }
